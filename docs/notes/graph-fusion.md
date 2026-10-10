# Graph fusion: the logic below, the implementations below, a schema between

> Status: **proposal.** The graph JIT this sits on is current
> ([compute-graph-jit](compute-graph-jit.md)); the fusion mathematics is the
> worked example in [auto reduction fusion](../reference/reduction%20fusion.typ)
> (Neptune, PLDI'26); the kernel capabilities this depends on are the roadmap's
> ([gpu-algorithm-roadmap](gpu-algorithm-roadmap.md)) axes B and C, one landed,
> one not.
>
> What this note is: the design for **semantic/implementation separation** — the
> logical program is written once, the compiler derives a **schema** from it
> automatically, and any number of **implementations** (a derived fusion, a
> hand-written *shadow* file) bind to the schema rather than to the code. The
> acceptance case is Flash Attention v1: the online-softmax rolling update must
> be *derivable* by the engine, not written by hand.
>
> Companions: [compute-graph-jit](compute-graph-jit.md) (the recorded graph this
> rewrites), [lichen-compute](lichen-compute.md) (the operator vocabulary and the
> fragment IR), [loop-conversion](loop-conversion.md) (the dynamic loop a fused
> reduction body needs), [artifact-cache](artifact-cache.md) (the
> content-addressed store a schema artifact rides on).

## 1. The separation, stated as one rule

**Nothing the optimizer knows is inferred from arbitrary code. It is declared
in the vocabulary the logic is written in, and the schema is the automatic
summary of those declarations.**

That rule is the whole design, and it exists because the alternative has a
known failure mode: algebra is not recoverable from loops. A compiler that
meets `for` bodies and index arithmetic cannot tell a reduction from a
stencil from a histogram — Halide's answer to this was to separate *algorithm*
from *schedule*, TVM's was tensor expressions with declared reduce axes, and
both work precisely because the semantics were never allowed to dissolve into
a loop nest. Lichen's version of the same move:

- **Logic** (what is computed) is written in a small **declared operator
  vocabulary** — `map`, `reduce`, `broadcast`, `matmul` — where each operator
  states its algebraic class and carries its functions as analyzable values,
  not as opaque code.
- The **schema** (what is *true* about the logic) is derived, always, by the
  compiler — a pure function of the logical graph. Same logic, same schema,
  byte for byte. Nobody writes it and nobody maintains it.
- **Implementations** (how fast it runs) bind to the schema. There are two
  sources and they share one interface: the **fusion engine**, which derives
  an implementation, and **shadow files**, which are written by hand.

The property this buys, stated once because everything below is a consequence:
**the logical code is the reference implementation, and it is always runnable.**
Every optimization is a substitution justified against the schema, so the worst
case of any optimizer — engine or shadow — is that the logic itself runs,
correctly, unfused.

## 2. The three artifacts, and who writes each

| artifact | written by | content-addressed? | wrong answer possible? |
|---|---|---|---|
| logical code | the user, once | — (it *is* the source) | yes — the user's bug, reported as such |
| schema | the compiler, from the logic alone | yes — hash of the region DAG + declarations | no — a derivation, not a decision |
| implementation | the engine, or a shadow file | binds to the schema's hash | engine: no (by construction); shadow: caught by the differential obligation |

A schema that could drift from its logic would make every binding suspect, so
the schema is *derived, not written*: there is no schema syntax a user can
type, the same way the graph JIT's node list is a transcript rather than a
template. "The same code logic automatically generates one schema" is then not
a feature but a tautology — derivation is a function.

## 3. The logical vocabulary declares its algebra

The vocabulary is a native plugin (the shape
[lichen-compute](lichen-compute.md) already establishes: plain structs, `$`
operators, an embedded source wrapper), tentatively `ml.lichen`. Each operator
is a **declaration**, and the declaration is what the schema reads:

```lichen
-- the vocabulary's declarations, schematically
map      = f => xs          -- elementwise; f is a lichen lambda
reduce   = op => e0 => xs   -- op: associative combine, e0: its identity — DECLARED
broadcast = x => n
matmul   = a => b           -- sugar: map-reduce over the inner axis

-- safe softmax, written in the vocabulary — this is *logic*, not a primitive
softmax = xs => {
  m = reduce max (-inf) xs
  e = map (x => exp (x - m)) xs
  s = reduce add 0 e
  map (x => x / s) e
}

-- attention, likewise
attention = q => k => v => {
  s = matmul q (transpose k)
  p = softmax s
  matmul p v
}
```

Two things are doing all the work here:

- **`reduce` carries its algebra.** The combine function and its identity are
  *arguments with stated laws*, not a loop the compiler must reverse-engineer.
  "Associative" is a declared certificate on the operator (the vocabulary
  ships `add`, `mul`, `max`, `min` with their laws attached; a user-defined
  combine must declare its own, and the engine refuses what it cannot prove —
  §6).
- **The element functions are values.** `x => exp (x - m)` lowers to a
  `KernelFragment` — an expression tree the engine can read, invert, and
  distribute over. This is what the fusion mathematics needs `g` to *be*
  (§6), and it is available because the vocabulary takes functions, not
  strings, shapes, or enum tags.

`matmul` written as sugar over `map`-`reduce` rather than as a primitive is a
decision, not an economy: it makes the third reduction of attention *the same
kind of node* as softmax's two, so the engine sees three chained reductions
over one axis rather than two reductions and a black box.

## 4. What the schema is

A schema is computed per **fusible region** of the recorded graph: a maximal
connected run of vocabulary nodes. It is plain data, content-hashed, and
storable in the artifact store:

```rust
RegionSchema {
    nodes: Vec<SchemaNode>,        // in evaluation order, like the graph itself
    ports: Vec<Port>,              // region inputs and outputs: extent, class, axis roles
    hash: SchemaHash,              // over everything below
}

SchemaNode {
    class: Map | Reduce | Broadcast | Reshape,
    functions: Vec<KernelFragment>, // the symbolic g / combine op, as expression trees
    laws: LawSet,                   // associative, commutative, identity — declared, not proved here
    axis: AxisRole,                 // which port axis is parallel, which is reduced
}
```

Three properties make this the right contract surface:

- **It is the pattern language for both optimizer sources.** The engine
  matches structural classes (a `Reduce` feeding a `Map` whose `g` mentions
  the reduction's running value, feeding another `Reduce`); a shadow matches
  the whole schema by hash plus optional extra constraints. One contract, two
  consumers, and neither ever looks at source text.
- **It carries functions, not just shapes.** A shape-only pattern language
  (op names + tensor ranks) can say "this is a softmax"; it cannot derive a
  repair function for it. The schema keeps the fragments because the
  mathematics in §6 operates on them.
- **It is stable under everything except semantics.** Rename the function,
  reformat, reorder independent statements — same schema. Change the algebra —
  different hash, and every shadow bound to the old hash stops applying
  *loudly* (§5). That stability is what makes "bind to the schema" safe to
  reach for at all; it is the same bet `fragment_digest` already makes for
  kernels, one level up.

## 5. Shadow files: hand-written implementations, bound by schema

A shadow is the escape hatch that makes the system honest: whatever the engine
cannot derive — register-level tiling, shared-memory choreography, a
hand-tuned instruction schedule — a human writes, once, in a separate file,
against the schema rather than against anyone's source code.

```lichen
--- a shadow file, schematically ---
implements = schema "sha256:9f2c…"        -- the region schema's hash
requires   = { class = f32, axis = "last" } -- extra constraints, matched at bind time
reference  = logical.attention            -- the region it claims to equal
kernel     = cfg => { …the hand-written fused body… } "gpu"
```

The rules, each of which is a decision:

- **Binding is by hash, never by name.** Name-binding couples an optimization
  to source text, and a rename then breaks it *silently*. Schema-binding
  couples it to semantics: any code that computes the same algebra earns the
  same implementation, and code that no longer does loses it.
- **Losing a shadow is loud and safe.** On schema drift the shadow simply
  stops matching; the logical code runs instead, and the orchestrator reports
  `shadow "flash-attention" no longer matches (schema 9f2c… → 41b0…)`. So a
  shadow can make a program **faster, never wrong** — the fallback is the
  reference itself. This is the property that makes hand-written substitutions
  tolerable in a system that otherwise refuses everything by name.
- **Equivalence is an obligation with teeth, not a comment.** A shadow file
  ships with a differential harness — the shadow's output against the
  reference region's output over generated inputs — run where the shadow is
  *authored* (CI, the package's tests), exactly as the graph suite compares
  against hand-derived closed forms rather than a second run of the same code.
  At load time the orchestrator checks the hash and the `requires`
  constraints only; it does not re-run the harness, the same way a backend
  trusts a cached artifact. A shadow that cannot name its `reference` region
  is refused: an implementation of nothing is nothing.
- **Shadow beats engine.** When both match a region, the hand-written one
  applies and the report says so — a derived fusion is the floor, not the
  ceiling, and a human who bothered to write a shadow knew something the
  engine did not.

## 6. The engine: reduction fusion by repair function

The mathematics is worked through in
[auto reduction fusion](../reference/reduction%20fusion.typ); this
section is what that mathematics *is* inside the system — the engine's one
transform, stated as the obligations it checks and the kernel it emits.

**The shape it matches.** A producer reduction `r` (combine `op`), an
element-wise `g(d_i, r_i, r_N)` that depends on the producer's *final* value
`r_N`, and a consumer reduction `t` (associative combine `f`):

```
r   = reduce op xs
out = reduce f (map (i => g(d_i, r_i, r)) xs)
```

The `r_N` inside `g` is what blocks naive fusion: the consumer's step needs
the producer's answer before the producer has finished. The engine's move,
per the reference, is to index the consumer by the *growing* producer extent
`N` and ask for a **repair function** `h`:

```
h(g(d, r, r_N), r_N, r_{N+1}) = g(d, r, r_{N+1})        -- repairs one element
h(f(x, y), ·, ·) = f(h(x, ·, ·), h(y, ·, ·))            -- distributes over f
```

and then the fused rolling update `t'_{N+1} = f(h(t'_N, r_N, r_{N+1}),
g(d_{N+1}, r_{N+1}, r_{N+1}))` computes the consumer in the same pass as the
producer, with no intermediate ever stored.

**What the engine does, mechanically, on the schema's fragments:**

1. **Inline** every element-wise node between the two reductions into `g`
   (the reference notes this costs nothing and is always safe).
2. **Find `h` by the inverse-function method.** Rewrite `g` as
   `g'(r_N)(c)` with `c` the outermost `r_N`-free subexpression — the
   reference's rule, *"the outermost expression independent of the parameter
   is the one to abstract"*, which is exactly the choice that maximizes
   invertibility — then `h(x, r_N, r_{N+1}) = g'(r_{N+1})(g'(r_N)⁻¹(x))`.
   Inversion is symbolic rewriting on the fragment's expression tree: each
   operator either names its inverse (`exp`/`ln`, `+c`/`−c`, `*c`/`÷c` for
   `c ≠ 0`) or the rewrite fails.
3. **Check `h` distributes over `f`**, structurally — the two sides of the
   distributivity equation are small expression trees, and the check is a
   normalization and comparison, not a search.
4. **Emit the rolling update** as one kernel body: the state tuple `(r, t')`
   carried across the reduction loop, `op` and `h` and `g` inlined into it.
   With several producers, the states concatenate — the reference's
   multi-producer case is free here, because the state is already a tuple.
5. **Split-K** for parallelism: the fused reduction is cut into local
   partials plus a global combine, and the combine's update applies `h` to
   repair each partial into the running global — the reference's two-level
   equations, mapped onto two dispatches (which exist today) or one workgroup
   kernel (once roadmap axis C lands).

**Equivalence is by construction, and construction refuses.** Each step is a
check that either holds symbolically or stops the fusion with a named cause:

| refusal | when |
|---|---|
| `NotAReductionChain` | the region's classes are not producer→map→consumer |
| `ElementFunctionNotInvertible { at }` | no `c`-abstraction of `g` inverts (the reference's `relu²` case) |
| `NoRepairFunction { g }` | inversion succeeded but the composite is not closed over the vocabulary |
| `NotDistributive { h, f }` | `h` does not distribute over the consumer's combine |
| `UndeclaredLaw { op, law }` | a combine claims no associativity/identity the transform needs |

The last row is where the declared laws of §3 pay off: the engine never
*guesses* that `max` is associative — it reads the certificate, and a
user-defined combine without one fuses element-wise only. Refusal is the
correct outcome often enough that it must be cheap and named; a region the
engine refuses is precisely a region where a shadow file is the answer.

## 7. Flash Attention v1 is the acceptance case

Walk the logical attention of §3 through the engine:

- `softmax`'s `reduce max` is the producer; `x => exp (x - m)` is `g`;
  `reduce add` is the consumer. Inversion: `c = x`, `g'(m)(c) = exp(c − m)`,
  inverse `ln`, giving `h(x, m_old, m_new) = x · exp(m_old − m_new)` — which
  distributes over `+` trivially (`(a+b)·k = a·k + b·k`).
- The emitted rolling state is the triple `(m, s)` updated as
  `m' = max(m, x)`, `s' = s · exp(m − m') + exp(x − m')`.
- The second `matmul`'s sum-reduction chains the same way against the
  division, extending the state to `(m, s, acc)` with `acc` rescaled by the
  same `exp(m − m')` factor.

**That triple, with that rescaling, *is* Flash Attention v1's online softmax**
— the thing the field wrote by hand in 2022, here derived from a schema and
three laws. The split-K step is FA's tiling: local partials per tile, a global
repair-combine across tiles. What the engine deliberately does *not* derive is
the rest of the paper — SRAM budgeting, register blocking, the `exp2` trick —
and that remainder is exactly the shadow file's job. The division of labour
this design predicts, stated as the test the design is judged by: **the
correctness of fused attention comes from the engine; the speed beyond that
comes from a shadow; neither is written into the logic.**

The win here is also a different quantity from the one the graph JIT
measured. There, fusion removed *dispatch overhead* (65–81% of a 16-link
chain at ≤ 65 536 elements, ~10% at a million). Here it removes **stored
intermediates**: unfused attention materializes the `N×N` score matrix twice,
and fused attention materializes neither — the memory footprint drops from
O(N²) to O(N), which matters at every dispatch-overhead regime and grows
where the dispatch win shrinks.

## 8. Orchestration: the rewrite sits between recording and running

The pass ordering inside `compute.graph` / `compute.graphrun`:

```
record (today)  →  cut into fusible regions  →  per region: schema
                →  per schema: shadow registry, then engine
                →  rewrite the node list  →  run under the chosen policy
```

Four properties, each chosen to cost the existing design nothing:

- **The graph IR does not change.** A fused region is one node — still "a
  kernel fragment over an index range", the one kind
  [compute-graph-jit](compute-graph-jit.md) argues for. Fusion changes *which
  fragment a node holds and how many nodes there are*, not what a node is.
  Split-K in two-dispatch form is two nodes with one edge between them —
  nothing new either. The whole fusion feature is a *rewriter over* the IR,
  not a revision *of* it, which is the strongest sign the IR's "one kind of
  node" decision was right.
- **Rewriting merges; it never reorders.** The recorded order is the program's
  own evaluation order, and the graph note's rule stands: the engine consumes
  a connected region and emits one node at the region's earliest legal
  position — which, over a DAG in evaluation order with all edges pointing
  backward, is the region's last node. No edge crosses the merge boundary
  except through the new node's declared ports, so `EdgeBeforeItsProducer`
  remains the only cycle check, unamended.
- **The unoptimized transcript is kept.** A run under `--no-fusion` (or after
  any engine refusal) executes the recorded graph verbatim — the reference
  implementation, always one flag away. This is the debugging story and the
  correctness story in one: any suspected mis-fusion is bisected by naming
  the shadow or the region, and the differential harness runs region-local,
  not program-global.
- **The cost policy is boring on purpose.** The measured lesson from the
  chain experiments is *no minimum-count gate*: the answer is not a count.
  Reduction fusion has a simpler economics than chain fusion anyway — it
  removes a materialized intermediate, which is never a pessimization in this
  model (there is no register-pressure cliff to fall off, because what the
  engine emits is still the buffer-map ABI; pressure tuning is the shadow's
  department). So the policy is: **fuse every region the engine can prove,
  apply every shadow that matches, report both.** The report extends the
  graph note's one formula — submissions = closure calls + segments — with
  the fusion facts: regions found, engine firings, shadow firings, and every
  refusal by name. "Your attention fused via shadow `flash-attention`; your
  second softmax was refused (`ElementFunctionNotInvertible`)" is the
  actionable sentence this owes the user.

Liveness gets strictly better: every fused region is intermediates that never
reach the value table, so the fan-out memory inversion the graph note warns
about shrinks exactly where fusion fires.

## 9. What exists, what is missing, and the order to build in

| dependency | status | what this design needs from it |
|---|---|---|
| recorded graph, value table, policies | **in** ([compute-graph-jit](compute-graph-jit.md)) | the rewrite substrate, unchanged |
| dynamic loop in a kernel body | **in** ([loop-conversion](loop-conversion.md)) | the fused reduction's inner loop |
| fragment IR with expression trees | **in** ([lichen-compute](lichen-compute.md)) | what `g`, `op`, `f`, `h` are made of |
| a declared `reduce` operator | **missing** — roadmap §4.1 names it "a separate operator" | the vocabulary's core; without it there is no producer to match |
| float | **in flight** (ten worktrees, [floating-point](floating-point.md)) | softmax/attention are float algorithms; the engine is class-agnostic but its acceptance case is not |
| shared memory + workgroup | **missing** (roadmap axis C) | one-dispatch split-K; until then split-K runs as two dispatches, which is correct and merely slower |
| content-addressed artifact store | **in** ([artifact-cache](artifact-cache.md)) | where schema artifacts and shadow bindings live |

Staging, each stage independently useful:

1. **Vocabulary + schema derivation, no fusion.** `ml.lichen` runs unfused on
   today's machinery (a `reduce` lowers to the chain/loop forms the roadmap
   already writes). This alone is worth having: the schema artifact and the
   match report exist, and shadow files can bind immediately.
2. **The engine, split-K as two dispatches.** The full repair-function
   pipeline, emitting local+global node pairs. This is the stage that must
   derive online softmax from §3's code to be called done.
3. **One-dispatch split-K**, after axis C lands. An emitter change behind the
   same engine, invisible to logic and shadows alike.

## 10. What this does not decide

Four genuine forks, cheapest to settle before the vocabulary is written:

**A new `ml` plugin, or more of `compute.lichen`?** Recommended: a new
plugin. `compute` is the machine (buffers, launches, graphs); `ml` is a
*semantics* over it, and a user writing raw `plrun` chains should not pay for
— or trip over — the declared-algebra layer. The counter-argument is that one
more plugin is one more composition surface.

**Exact-hash shadow binding, or constraint-based matching?** Recommended:
exact hash plus *explicit* `requires` constraints, as sketched. A fuzzy
structural match (same shape, different element functions) would fire a
shadow on code that computes something else, and "faster, never wrong" dies
there. The cost is real: two spellings of one attention are two hashes and
need one shadow each. Schema normalization (commutative operand order,
common-subexpression canonicalization) narrows that gap without admitting
semantic distance.

**Fusion on by default, or opt-in per graph?** Recommended: on by default
with named refusals and the global `--no-fusion` kill switch — the logic is
the reference, so optimization that declines to fire is a report, not a bug.
Opt-in would re-couple the logic to the optimizer, which is the thing this
design exists to uncouple.

**Is `reduce` a primitive operator, or a shape `loop` takes?** Roadmap §4.1
already named it a separate operator and deferred the spelling; this design
*needs* the separate-operator answer, because a reduction's combine and
identity must be declared arguments for the schema to carry them. A `loop`
that happens to accumulate is a `T -> T` with no laws attached, and no repair
function was ever derived from that.

## 11. Non-goals

- **Arbitrary-code fusion.** The engine reads the declared vocabulary and
  nothing else. A hand-rolled `plrun` chain fuses exactly as much as it does
  today — the chain fusion the graph note measured — and is never pattern-
  matched into a reduction it did not declare.
- **Autotuning.** Tile sizes, workgroup widths, vectorization factors: the
  shadow author's and roadmap axis C's business. The engine emits one correct
  fused form; searching the space of faster ones is a later note.
- **A general rewrite engine.** There is one transform (repair-function
  fusion) plus substitution (shadows), not an equality-saturation framework.
  If a second transform ever earns its place, it enters the same way: a
  schema-level pattern, a proof obligation, a named refusal.
- **Reordering, parallelism invention, or any change to what a graph runs.**
  The graph remains a transcript; this design only changes how much of the
  transcript one node computes.

## Related

- [compute-graph-jit.md](compute-graph-jit.md) — the substrate: nodes, values,
  policies, and the measured fusion economics this extends from dispatch
  overhead to memory footprint.
- [gpu-algorithm-roadmap.md](gpu-algorithm-roadmap.md) — axes B and C, the
  capabilities the fused kernel body needs.
- [loop-conversion.md](loop-conversion.md) — the dynamic loop, landed, that a
  fused reduction body lowers through.
- [auto reduction fusion](../reference/reduction%20fusion.typ) — the
  mathematics §6 operationalizes.
- [artifact-cache.md](artifact-cache.md) — the content-addressed store a schema
  artifact rides on.
