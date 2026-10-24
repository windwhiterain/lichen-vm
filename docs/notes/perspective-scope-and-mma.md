# The register perspective MMA needs, and the scope question behind it

> Status: in progress — the decision record between the landed `Perspective` work and
> the MMA item on the roadmap: what is enforced today, what the GPU path reads, and
> the register-side shape the author has ruled is the right home. §6 holds the
> verdicts.
> Points at: `crates/lichen-highlevel/src/attr.rs` (the perspective vocabulary),
> `crates/lichen-compute-gpu/src/spirv.rs` (the GPU path).
> Companion: [gpu-mma](gpu-mma.md) (the product this note's perspective serves).

This note is the decision record between the `Perspective` work that is landed and the
MMA item on the roadmap. It states what is enforced today, what the GPU path reads, why
the kernel-config annotation fails (a separate diagnostic-quality finding, not the MMA
path), and the register-side shape the author has ruled is the right home. The verdicts
are recorded in §6. Measurements are on dev at `c3eefe04`, the slot isomorphism's tip;
the first round was taken at `3231542a`, and §4 keeps both readings because the
isomorphism is what moved it.

## 1. What is enforced today

`Perspective` is a divisibility lattice on a value's lane uniformity: `# p` states "the
same across every `p`-aligned lane group", `0` is the absent top, `1` is lane-varying.
The acceptance table is [attributes](attributes.md) §"Behaviour"; the enforcements are:

- a parameter's declared `# p` is checked at each apply (`declared | provider`), and an
  absent parameter annotation states nothing (the isomorphic reading);
- a function's return certificate rides to the call site, including through a frozen
  (package) callee — the compute wrapper's `# 1` seeds on `range`/`row`/`col` make a
  false `# 64` claim a check-time refusal, and a read's is its operands' meet
  (`docs/notes/attributes.md` §"The whole-argument default");
- a lane-group requirement is spellable in the shipping vocabulary:
  `f = (x : Int # compute.warp) => x` answers `expected 32, found 1` for a lane value,
  and accepts a uniform value or a `# 64` claim (`compute.warp = 32`, the backend's
  `SUBGROUP_WIDTH`).

## 2. What the GPU path reads

**Nothing.** The kernel IR carries no attribute field, the emitters read no perspective,
and `compile_parallel_fragment` hardcodes its lane width. [Axis
C](gpu-algorithm-roadmap.md#42-axis-c-lane-width-and-the-consumer-perspective-has-been-waiting-for)
is the intended first consumer (hoist a uniform subexpression, and emit it as one where
the target has a vector width); it is a codegen feature, not a check. The checks above
are the checker's; they constrain programs, but no GPU code is generated *from* them yet.

### What "the emitter consumes it" would take

Two mechanics, both larger than they look:

- **A hoist.** The SPIR-V walk emits every expression per invocation. A value proven
  uniform could be computed once per *workgroup* and shared instead — either on the host
  (the config leaves already ride push constants, so the machinery exists for leaves) or
  by one lane writing a workgroup variable behind a barrier. The first needs a host-side
  evaluator for the body expression; the second needs shared memory and a barrier, which
  the backend does not have yet ([shared-memory-and-barriers](shared-memory-and-barriers.md)).
- **A vector form.** "Emitted as one where the target has a vector width" means the
  value is uniform over the vector's lane group and the operation is vectorizable; the
  emitter would have to build `vec4` values from scalars, which changes the whole value
  model of the shader (loads, stores, arithmetic and the buffer layout).

Neither changes the *answer*. A body that ignores the perspective computes the same
result, one element per lane — which is why the author's reading (§6) holds: the emitter
consumes a perspective only as a performance factor; MMA consumes it because the lane
layout *is* the semantics.

## 3. The reference's model, and what lichen covers

The reference document types an expression as `expr: type(DP) @ UP` over a *scope* form
`level(n)` (`grid`, `block`, `thread`, with `warp(n) ≔ thread(32n)`), and it lists four
CUDA contracts: uniform parameter, disjoint mutation, synchronization, and convergence (a
cooperative function such as `wmma` needs a contiguous run of threads). The pieces:

| piece | reads | lichen today |
|---|---|---|
| `UP`, uniform perspective | the value is identical within the group | **this** `Perspective` — divisibility; the paper's `divisor` is lichen's `gcd`, and `lower` (a coarser value serves a finer requirement) is `declared \| provider` |
| `DP`, disjoint perspective | two thread groups' mutable regions do not overlap | absent — `Buf` carries no disjointness; the ladder's silent scatter is the cost |
| scope and `@requires` | the launch aligns `grid`/`block`/`thread`; `lower` moves a value to an aligned group | implicit: the kernel is dispatched with a fixed lane width (64, with a 32-lane subgroup), so `thread[32]` exists for every body |
| convergence | a cooperative operation needs the contiguous group | the dispatch supplies it, as the author ruled (§6) |

So the *register* side the author named — "寄存器的 perspective" — is the `UP` column, and
the value lattice is exactly that: the paper's `int @ thread[32]` is lichen's `# 32`, and
the `lower` relation is the acceptance table's `declared | provider`. `DP` is a *memory*
axis carried by the type (the reference encodes it in `view`, `ptr`, `shared`), not a
value annotation; it is the check that would catch the ladder's silent write, and it is a
separate item.

**MMA's checks, then.** The tile descriptor must agree across the group — `# 32` on the
`UP` axis, landed; a fragment's lane layout is the value's own perspective — the default,
with no requirement form (see the test in
[perspective.rs](../../crates/lichen-language/tests/language/perspective.rs): a warp-level
operation keeps the fragment's finer width, and the result claim is what a checker
refuses); and the convergence is the dispatch's. Encoding the convergence as `# 32` on
the operands would be wrong — a fragment is lane-varying — which is why
[attributes](attributes.md) §"A lane-group requirement" calls the landed check the
*uniform-load* shape.

## 4. The kernel-config annotation, measured

`even_raw = (k : ParA # 4) => …`, where `ParA = compute.P (compute.KT Float)(…)`.
**Re-measured at `c3eefe04`**, on the same shape the first round used:

| program | reading at `c3eefe04` |
|---|---|
| `(k : ParA)` | checks |
| `(k : ParA # compute.warp)` | checks |
| `(k : ParA # 4)` | **refused at check**: `expected 32, found 4`, once per `compute.read`/`compute.write` in the body |
| `(k : ParA # 1)` | refused at check: `expected 32, found 1`, the same two sites |

So the annotation is a **value-uniformity fact** and the attribute check reads it: it
narrows what the body's operators may assume, and the body's `Read`/`Write` field
requirements (`# warp`) then refuse a config declared less uniform than a warp. The
config is uniform by construction, so every `# p` is true *of the config*; what the
annotation changes is what the body is allowed to rely on. Nothing was added to make this
work — the slot isomorphism (§ of [array-attribute-view](array-attribute-view.md)) is what
made the expr the annotation builds merge like any other.

**The earlier reading, kept because the isomorphism is what removed it.** At `3231542a`
the same program failed with a **runtime** diagnostic: the generic apply-time
parameter-check rendering (`diagnostic.rs::mismatch`'s apply arm), located at the
argument's expression with descent path `Attr(0)`, whose two sides were function types —
`raw[?a, raw[?b, ?c]] -> raw[?d, ?e]` against `struct{…} -> ?j`. What was ruled out by
instrumentation then: the annotation provider path, the element-expr and type-attribute
reconciliations, and the frozen-parameter tail check did not fire; disabling the frozen
return-attribute propagation did not change it; `check_unify` and `check_unify_relaxed`
never saw the failing expr. A computed type with a perspective worked in the bare language
(`A = T 5; f = (s : A # 4) => s.n`), so "computed type + perspective" was not the trigger
— the apply's expr unify was, and the isomorphism removed that arm
(`apply_parameter_check` now compares the value and the type only, and the order is the
checker's).

## 5. Options

**Superseded: the isomorphism settled this without any of the three.** What landed is
option 3's reading — the annotation is a value uniformity, checked where every other
attribute is — and it needed none of option 2's merge tolerance, because the expr the
annotation builds is the same expr every expression carries. Option 1 (a named refusal)
has nothing left to refuse: the check already answers by name, in the lattice's own
words. The list below is the shape the question had before the measurement moved, kept so
the answer can be read against it.

1. **Refuse by name at check time.** Recognize the kernel-config parameter (the
   `compute.P`-shaped type is plugin knowledge, or is recognized by the parameter's type
   value being a type-level function) and answer with a diagnostic instead of the runtime
   raw unify: "a perspective on a kernel config parameter is not supported: the config is
   uniform by construction and a scope perspective is not built." No semantic change; the
   author's pending question answered on the conservative side.
2. **Support as a scope declaration.** `(k : ParA # p)` declares the kernel's lane-group
   width: the launch asserts `config.n % p == 0` (the deferred-assert channel already
   re-checks per call), and a warp-level requirement references `p`. Needs an
   attribute-tolerant kernel-type merge (the measured failure), the scope semantics, and a
   deferral story under "the editor evaluates nothing outside check mode".
3. **Support as a value uniformity.** Accept the annotation and treat the config as
   `# p`-uniform (true by construction, since a launch parameter is uniform). Cheap, but
   the claim is vacuous and it still needs option 2's merge tolerance.

**Recommendation.** None of the three is needed for MMA (§6): the config annotation is a
diagnostic-quality question, and the diagnostic it gets now is the honest one. Option 2's
scope reading stays parked with the kernel-type merge, and no operator needs it.

## 6. The author's verdicts

- **Registers, not the config.** "为什么mma需要 config 的 perspective，寄存器的
  perspective 就够了吧" — a fragment's lane layout is a value fact, and the group is the
  dispatch's; the config annotation is not on the MMA path. The register-side check the
  value lattice already states is the uniform-operand requirement (`# compute.warp`), and
  the lane-varying fragment is the default.
- **Disjointness is uniformity on the buffer.** Because the language has no mutation,
  a buffer's *disjoint* perspective is expressed by the *uniform* one: a segment of
  shared storage allocated per warp is written `buffer # warp`, and its reads and writes
  carry the warp-uniform requirement — the segment's base is what the warp agrees on, and
  every lane addresses it through its own index and value. The certificate follows the
  segment: a product over `# warp` tiles is a `# warp` buffer — warp-uniform, no wider —
  and its readers require that. The requirement is stated
  where the buffer is named: `Read.from (Buf T) # warp`, `Write.to (Buf T) # warp`,
  `WriteAtomic.to (Buf T) # warp`. A compound element's slot is `[value, type, attrs…]`
  with the wildcard for an unspelled attribute, so a type function's field requirement
  travels in its frozen type value, and the instantiate check reads it when the type the
  call names is a value rather than a written struct
  (`Checker::unify_value_field_attributes`; the argument's provided slot is the meet of
  its components). A lane-varying buffer is refused (`expected 32, found 1`), a plain
  buffer reads. A write **certifies its result** `# warp` — the pending write is a
  description of a warp-uniform segment — so chaining `write` keeps the target's
  uniformity and a read of a write's result still reaches the op's own named refusal.
  `Mma` states the same requirement on its three fields (`Mma = T => struct{.s string,
  .a (Buf T) # warp, …}`), and the vocabulary's types are all type functions for that
  reason. Measured on the way there: a **direct** package struct type's field slots are
  rebuilt by the import with fresh cells, so their attribute positions freeze undecided —
  a type function's body does not. Pinning down where the import drops the value is an
  open question; the type-function spelling makes the requirement checkable today.
- **A body provides what its parameter requires.** The function is checked once, for
  every caller its requirement admits, so the parameter's declared slot is what the body
  may rely on — the paper's "a variable's perspective is the one of the code where it was
  declared". The unify hook's subtype direction then decides each use inside the body
  (`# 32` required of a `# 1` parameter fails, `# 32` of a `# 32` passes), and a compound
  argument's provided slot is the meet of its components. This is the flow the read/write
  rules need on the operand side; their *results* still wait on a derived return
  certificate (a function's return expr carrying the provided slot when the body states
  no annotation) — without it a wrapper's return can only be a fixed certificate, because
  the body cannot see which caller's values it will run on.
- **A read's perspective is its operands', a write's is its input's.** `compute.read`'s
  result is `divisor(from, at)` — a warp-uniform buffer read at a uniform index stays
  warp-uniform, a lane-varying index makes the result lane-varying — and the vocabulary
  **states nothing** for it: the caller's combine derives it from the argument expression
  (`provided_slot`, the attribute's own protocol). `compute.write` returns a new buffer
  whose perspective is the **input buffer's** (the segment is the same storage; the
  written index and value are content, not segment shape) — that is a *part* of the
  argument, not a meet, so its return names the `.to` field
  (`# (perspective_of x.to)`) and the call site takes that field's provided slot. Both
  are whole-program level statements; the first needs no line from the package at all.
  Measured: removing `read`'s line alone leaves the lane-varying-index refusal red; with
  the combine reading it is green (`crates/lichen-compute/tests/lichen_compute/perspective_requirements.rs`).
- **The op states its perspective, uncurried.** The vocabulary's `mma` takes one `Mma`
  struct argument — not four curried lambdas — with the three tiles required warp-uniform
  (`# warp` on the parameter) and the result certified `# 0`. A lane-varying operand is
  refused at the call (`expected 32, found 1`), which is the paper's descriptor
  requirement; the lane-varying *fragments* stay outside the host-buffer op, since a
  buffer is uniform over every lane.
- **Only MMA semantically consumes a perspective.** The emitter's hoist/vectorize (Axis
  C) is an *optimization* — [the roadmap](gpu-algorithm-roadmap.md) itself calls it "a
  constant factor, not a capability": a body that ignores the perspective still computes
  the right answer. The *semantic* consumers are the collectives (matrix multiply,
  shuffles, reductions), where the lane layout decides the result. So the emitter item
  should stay parked unless the performance factor is wanted, and the MMA item's next
  step is the *checking* shape, not the codegen.

## Open questions

- ~~Is the config annotation's named refusal worth a check-side hook now, or does the
  runtime raw message wait until the kernel-type merge is visited for its own sake?~~
  **Answered by the measurement**: no hook is needed and the runtime raw message is gone.
  The annotation is a value uniformity, the attribute check reads it, and the refusal
  names the body's own requirement (`expected 32, found 4`). See §4.
- Should Axis C stay parked as an optimization, or be scheduled as a performance item
  when a uniform-heavy shape exists to measure?
