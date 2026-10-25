# Shared memory, and where a barrier comes from

> Status: **in flux — part one landed as a token, is being reverted, and the
> replacement is restaged.**
> A design for the one capability the [roadmap](gpu-algorithm-roadmap.md) calls
> the largest single item in axis C — lanes that can see each other — and for the
> analysis that makes it sound. The three parts were written as inseparable; a
> code audit before implementation showed two of the three are **cheaper than
> this note first claimed**, and §9 records the staging that follows from it.
>
> Part one landed as a zero-width `State` token ([§6.2](#62-the-spine-what-landed)).
> **That choice is retracted and being reverted.** §3's argument for a witness over a
> buffer version rested on a runtime-value model that does not hold inside a kernel
> body, where a `Buf` is an undecided wrapper node and the JIT walks structure rather
> than tracing values — so all three of its objections fall, and the spine becomes a
> **buffer version**: `write` returns a new version of the buffer it wrote, and
> `read` consumes it through the `.from` field it already has. That is what
> *purifying* the write means, which a token never was, because a token is consumed
> and never read.
>
> A spike then measured the replacement and corrected this note twice more (§3). The
> read **does** grow an operand — a version needs it just as a token did — and reading
> back what a body wrote has **no data path in today's ABI**, so landing the
> annotation alone would turn a named refusal into a silent `0`. The spine therefore
> moves out of step 1 and lands **with** the staged landing (§9 step 4), where the
> elements are readable by construction because their type says `block[1]`; step 1 is
> now the reversion itself.
>
> **§4's "refuse a contended write" is superseded by the superior's ruling:** `compute.write`
> states disjointness as a contract and `compute.write_atomic` is the accumulate, so the
> def-cone/`UP` judgement this note designed for that refusal is no longer a requirement.
> What an atomic does not order is the program's own problem. The measurement and the new
> shapes are [gpu-algorithms-ladder](gpu-algorithms-ladder.md) §5.
>
> The spike also measured a case that looked like a silently wrong answer on `dev` —
> a write the codomain does not reach is never emitted and never diagnosed — and it
> is **not one**. Checking it against the type settled the design instead:
> [code-audit D17](code-audit.md), **a `compute.write` is not an effect, it is the
> value of an output position.** That is the sentence this retraction rests on, and it
> makes the token a category error rather than merely an expensive choice.
>
> The paper this project's attribute extension is inspired by supplies the `DP`
> half ([Typed Perspectives](../reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf),
> read in Chinese in
> [the walkthrough](../reference/类型系统验证底层%20GPU%20语言：Perspective.md)).
> [§6.1](#61-dp-is-the-types-and-up-is-the-annotation) records where the `DP`
> lives — on the buffer's type — and why the lattice needs no level to write one:
> a finite perspective is inside a block, and `0` is the `∞` that crosses blocks.
>
> Points at: `crates/lichen-compute/src/compute.lichen` (the algebra),
> `crates/lichen-compute/src/compute.rs` (`ComputeValue`, `ComputeOperator`,
> `Positions`, `write_value_node`, `parallel_output_nodes`,
> `run_parallel_range`), `crates/lichen-compute/src/compute/body.rs`
> (`Lower::buffer_write`), `crates/lichen-kernel-ir/src/lib.rs` (`ScalarClass`,
> `KernelInstr::BufferWriteCall`, `fragment_digest`),
> `crates/lichen-kernel-ir/src/body.rs` (the SSA body and `validate`),
> `crates/lichen-kernel-ir/src/barrier.rs` (the scan: §4's three facts, and the two
> refusals it owes),
> `crates/lichen-compute-gpu/src/spirv.rs` (`LOCAL_SIZE_X`),
> `crates/lichen-compute-gpu/src/dispatch.rs` (the padding that makes a barrier
> safe), `crates/lichen-perspective/src/perspective.rs` (the perspective lattice:
> `0` is the top and the `∞` fold, `1` the bottom).
>
> Companions: [compute-buffer-wrapper](compute-buffer-wrapper.md) (the ABI this
> extends, and the determinism theorem §3 scopes to `Buf`),
> [compute-kernel-struct](compute-kernel-struct.md) (the struct-wrapper model
> shared memory joins), [loop-conversion](loop-conversion.md) (the loop a
> shared-memory algorithm needs, and the `CONDITIONAL_WRITE` discipline a write
> inside one breaks), [lichen-compute-gpu](lichen-compute-gpu.md) (the tail-lane
> obligation, and why a write in a loop is refused today),
> [gpu-algorithms-ladder](gpu-algorithms-ladder.md) (the measurement that a
> contended write is silently wrong today — this design's sharpest evidence).

## 1. The claim, in one paragraph

lichen's kernel bodies are already pure, and the one impure thing in them is
`compute.write`. It is impure in the way that matters: **it produces no value a
later instruction can depend on, so the value graph has no edge from a write to
the read that must observe it** — and that edge is the one a barrier needs. So
the work is not "make the body pure"; it is **"give the effect a value"**, after
which the barrier question becomes a walk over a DAG. But a walk over the DAG
answers only *which exprs to consider*: whether an expr actually needs a barrier
is decided by whether the two lanes' addresses can overlap. This note first
called that judgement `DP`. **The audit corrected where it comes from**: the overlap
question is answered by the **`DP` of the buffer's type** — `block[1]` or `grid[1]`
([§6.1](#61-dp-is-the-types-and-up-is-the-annotation)) — plus a def-cone query over the
body, and that query is local to `lichen-kernel-ir`. The annotation `UP` (`# p`) is the
**precision upgrade** on top, not the enabler
([§4](#4-what-decides-a-barrier-and-it-is-three-facts)).

## 2. What the write is today

Four facts. The first two are corrections to what this note originally recorded;
each is cited so the next reader does not repeat the mistake.

- **At the language level the write is a lazy thunk, not "a computed write".**
  `ComputeOperator::Write`'s `run` arm returns `None` together with `Range`'s —
  the comment is "Kernel-only operators: `range`/`write` never reach the VM as a
  standalone apply — stay lazy" (`compute.rs:1487`). The node therefore has *no*
  value at all. What reads the written expression is `write_value_node`, which
  walks the **operand array** to `operand_items(…)[2]` — the `.value` field —
  rather than reading anything the write produced (`compute.rs:2282`). The
  consequence for the spine is the reason it is cheap at the surface and not
  cheap underneath: making `write` produce a value means making it **eager**,
  which is a change to `ComputeValue`, to the codec and to every match site, not
  a change to a signature.
- **At the IR level the write already has a node; what it lacks is a class.**
  `KernelBody::add_op` allocates a `ValueId` for every instruction
  (`lichen-kernel-ir/body.rs:114`), `BufferWriteCall` included. Its
  `ValueDef::Instr` carries `classes: vec![]` because `produces()` returns `0`
  (`lichen-kernel-ir/src/lib.rs:561`) and `validate` enforces the two agree
  (`body.rs:333`). So "it leaves no trace in the value graph" was too strong: the
  trace is a node with no result and no consumer.
- **Every ordinal is a compile-time constant, and it is the `.to` the author wrote.**
  `write`'s first operand is the `.to` **buffer**, not a length, and `body.rs`'s
  `buffer_write` resolves it to an output ordinal through any chain of writes
  (`write_chain_output_pos`) — the write-side counterpart of the `parallel_buffer_pos`
  a read resolves its `.from` with. **This was not true when this note was written:**
  item 0 was ignored *entirely* and the ordinal was `tally.writes`, an emission counter
  the emitter called the write's *emission ordinal* (`spirv.rs`). So a body naming its
  targets in another order than the codomain lists them silently exchanged two outputs,
  a write naming an *input* buffer compiled and wrote an output, and a chain of two
  writes to one output dropped the inner one as unreachable. All three measured and
  fixed; the rule they now obey is [code-audit D17](code-audit.md)'s.
- **The write set is a private tally.** `Positions { emissions, reads,
  write_classes, read_classes, element_class }` counts them
  for the fragment's `outputs`/`output_classes` — `write_classes` is placed **at its
  ordinal** rather than pushed, so a chain that writes one output twice leaves one
  entry — and the return point recovers
  which nodes they are by walking the codomain (`parallel_output_nodes`).
  Nothing outside the lowering can read it.

The body itself is SSA with one instruction per `ValueId`
([compute-kernel-struct](compute-kernel-struct.md), `lichen-kernel-ir/body.rs`),
so "a DAG" is not a rewrite the language needs — it is the shape the body is
already in.

## 3. The trap: a value graph does not order two effects on one buffer

A pure node with no result — a `write` spelled `$write(buf, i, v)` returning
nothing — has **no consumers and no producer**. A later `read buf i` shares only
the `buf` operand with it, and `buf` is a value: two reads of one value are not
ordered by the value graph, they are *unordered* by it. So the DAG's verdict on
the expr `(write buf i v, read buf i)` is "independent", which is exactly
wrong.

The ordinary case shows how wrong. Today the plane is a **buffer map**: one lane
per index, each writing its own slot, and the design's whole determinism theorem
is that the split is unobservable
([compute-buffer-wrapper](compute-buffer-wrapper.md#the-parallel-abi): "the
**result is bit-identical to the sequential loop's** as long as no two indices
write the same output slot: the partition depends only on the count and the
worker count, never on a schedule or a timing"). A value DAG says that correctly,
and its proviso is exactly the theorem's: the order is unobservable *because* no
two lanes touch one slot. It is only shared memory that makes order matter, and
it makes it matter **across lanes** — so the edge we need is not a value edge at
all. It has to be created.

**Conclusion, and it is the whole of part one: a write must return something a
read consumes.** With that spine in place the expr is in the graph and the graph
can be walked.

### The audit's correction, and why the spine was built anyway

`BasicBlock.instrs` is an **ordered** `Vec<ValueId>` and the blocks form a CFG
(`lichen-kernel-ir/body.rs:39`, `:81`), so program order over a body's effects is
*already recoverable* without a spine, and no pass in the tree today reorders a
block's `instrs`. A scan that walks `instrs` in order and the CFG across blocks
would find every `(write, read)` expr §4 needs. The trap above is therefore a
fact about the **value** graph, not about what an analysis can see today.

The spine is being built regardless, for two reasons that are about the future
rather than the present, and they are recorded so the cost is not paid blind:

- **A positional order does not survive restructuring; a value edge does.** Two
  restructurings are already planned or live: [loop-conversion](loop-conversion.md)
  turns an expanded body into blocks with carried parameters, and the host
  backend's segment execution ([§7](#7-the-host-backend-is-the-expensive-half))
  splits a body at its barriers into separately callable pieces. Both move
  instructions between blocks. A state token is carried through a `Br`'s `args`
  by the same machinery that carries every other value, so the order survives
  both; an `instrs` index does not.
- **A token is what makes the edge checkable.** `validate` can refuse a
  `BufferWriteCall` whose state operand is not the previous effect's result. A
  positional scan has nothing to refuse against — its input is the thing it would
  have to trust.

What the spine must **not** be is the reason the analysis is deferred: §4's three
facts need no perspective attribute, so the spine lands alone and the scan lands
when shared memory does.

### Why the witness was the wrong answer, and what replaces it

**Retracted, with the evidence.** The subsection this replaces argued that a
buffer-valued write costs three things a zero-width token does not. All three were
written against a **runtime-value** model of what a buffer inside a kernel body is,
and that model is wrong. Inside a parallel index function a `Buf` is an **undecided
wrapper node** — "two items, the second an element *type*", with a placeholder in
`.native` ([compute-buffer-wrapper](compute-buffer-wrapper.md#a-placeholder-under-a-buf-wrapper))
— and a parallel kernel "is compiled by `$parallel`'s own run, and at that moment
the parameter's annotated type may still be an unresolved cell", which "leaves the
operator **undecided**" (same note, §"The kernel may compile before its
parameter's annotation resolves"). The JIT is a **structural** walk over nodes and
index paths — `Lower::value`, `write_value_node` over `operand_items`,
`parallel_buffer_pos` over `param_path`/`named_path` — and not a trace over values.
Each objection, against that:

- **"One new buffer value per write is a new allocation per write."** There is no
  buffer *value* in a kernel body to allocate; a version costs a **node**. And even
  where a payload-bearing `ComputeValue::Buffer` does exist, the freeze collects
  handles by `(ptr, len)` and dedups them (`static_module/freeze.rs:200`), so N
  nodes sharing one payload cost one arena entry, not N.
- **Two versions would be collapsed into one.** They would be, if they had decided
  values: `ValueExt::value_eq` compares handle payloads by pointer **and by content**
  (`lichen-lowlevel/src/lib.rs:346`), and the equality walk merges two nodes into one
  class when their values are `value_eq` (`equality.rs:744`). But that path is gated —
  "Undecided is not a fact to propagate, so **only a decided value reaches here**"
  (`equality.rs:292`), and `value_expr_equal` asks "whether two **decided** leaf
  values are one" (`:757`). An undecided wrapper is never merged by it.
- **"It removes the buffer from first-class position."** The one that survives, and
  it survives as a *question* rather than a refutation: a closure capturing a buffer
  would have to name which version it captured. Whether that is a cost or the honest
  semantics is [§10](#10-what-is-open) item 2's, not this section's.

What remains of the first objection — "two adjacent values of a scatter/gather are
not ordered, and a buffer-valued write says they are" — is not an objection to
versions at all. **Versions are the cure for it**: `w1 = write(b, …)` and
`w2 = write(b, …)`, both taken from `b`, are *not* ordered, which is the true
partial order, where a token threaded in emission position imposes a total one
([§10](#10-what-is-open) item 5).

**So the spine is a buffer version, not a token — but two rounds of measurement put
the price considerably higher than this section first assumed.** `write` returns a new
version of the buffer it wrote and `read` consumes it through the `.from` field it
already has; that much holds, and it needs no `ComputeValue` at all —
`ComputeOperator::Write` stays **lazy**, because a `: T` annotation writes the type
cell and neither the checker nor the lowering reads the operator's answer (`check_ann`,
`lichen-highlevel/src/checker/annotations.rs:274`; `NativeOp::build` for `Write`
already answers `decided: false`). This is also what *purifying* the write means — the
body becomes a function of buffer values — and a token was never that, because a token
is consumed and never read.

**The annotation is not the unlock, and that was measured rather than assumed.**
`: x.to` type-checks, and it changes **nothing observable**: `read (.from <a write's
result>, .at i)` compiles and lowers identically with and without it, because
`write`'s result type is *undecided* today and an undecided cell unifies with `Read`'s
`.from (Buf T)`. So the wiring the JIT reads is already there — which is exactly the
point of `$read`/`$write` being no-op placeholder operators whose only job is to be
recognised, and it is correct — and what is missing is not a type but everything
downstream of the wire.

**What the spike refutes is "no extra operand on the read"; what the probe then found
was a bug, and it is now fixed.** The edge exists at the **lichen node** level — a
read's `.from` operand *is* the write's result node — but `parallel_buffer_pos`
neither refused it nor saw through it: it resolved it to an **input position**, and the
program silently read the wrong buffer. Measured on `dev@be3dcf8`, `"cpu"`, input seeded
`[0, 1, 2, 3]`: a body that writes `i + 7` to `k.out.z`, reads it back and writes
`r + 1` answered `[1, 2, 3, 4]` where it means `[8, 9, 10, 11]`, with no diagnostic at
all. The cause was not the write. It was a **second route** in that function, reading
the path off the `Read` wrapper's own projection chain — which is the same for every
read, whatever buffer the author named — and suffix-matching it against `roles.inputs`.
It answered input position 0 for **every** struct-form read, so a kernel reading its
second input silently read the first, and a read of an output silently read an input.
The route is deleted, `named_path` with it, and the measurement is recorded in
[compute-buffer-wrapper](compute-buffer-wrapper.md#the-chain-walk-what-it-measures-and-what-it-cannot-answer).
What the spine still needs is the part that was never there: a small structural walk,
exactly `write_value_node`'s shape, resolving a read of a write's result to that write's
`.to` position — and until the ABI can read an output at all, that spelling is refused
by name, which is what it now is.

The edge also does **not** exist in the lowered IR: `BufferReadCall`'s operands are
`vec![position, index]`, where `position` is a fresh `emit_const` ordinal
(`body.rs:1548`), so the write's `ValueId` has nowhere to travel. A version therefore
needs the same IR change a token needed — the read grows an operand — and step 3's
scan, which reads the *lowered* body, cannot see the edge without it. What a version
buys over a token is not the absence of that operand but its **content**: an operand
naming a buffer version lets the scan tell a read of `A` after a write to `B` from a
read of `A` after a write to `A`, which an opaque token
cannot.

**Two things the encoding settled, and both belong where the encoding is stated.** The
version is **appended**, not prepended: a slice read's operands are
`[slot, index, version]`, so §6.2's illustration of the retracted token — a staged read
written `(t1, ws, (i + 1) % 64)`, the state first — is the *token's* operand order and not
this one, and a reader implementing from that example would build the wrong list. And **the
version carries an element class it does not deserve**: `produces() == 1` obliges an
instruction to state one, the IR has exactly one class vocabulary, so `own_class` answers
the written element's class. The label is inert — the version is only ever read as operand
2 of a `SliceReadCall`, and no class types that position — but it is a lie the type
system tolerates. It is recorded rather than fixed because the alternative is a token
class, which is the design §6.2 reverted; what matters is that nothing later come to
depend on the label.

**And behind that is a wall, which is the real reason to restage.** Reading back
what a body wrote has **no data path in today's ABI**: the host `read` import reads
`state.inputs` only (`compute.rs:5779`, `.unwrap_or(0)`), outputs are write-only, and
the fragment's `inputs`/`outputs` split has no room for "this input is really an
output of the same body". So landing the annotation and the position walk *alone*
would turn today's named refusal —

```text
compute.parallel: read's buffer argument is not an input buffer of the parallel
parameter (the parameter's inputs are [])
```

— into a program that compiles and **silently reads `0`**. Strictly worse. A global
`Buf` version is therefore not a type-level change but an **ABI** change, and nothing
needs it until shared memory exists — where the slot space is readable *by
construction*, because that is what `Workgroup` storage is for.

**Which moves the spine out of step 1 and into step 4.** [§9](#9-the-staging) now
lands it with the `Workgroup` declaration as one landing: the buffer's **`DP`** says which
threads may see the elements, the plan says which storage implements that, and the lowering
emits the staged access the IR names — so the edge, its operand and its data path arrive
together and the scan has something real to walk. The token is still reverted
([§6.2](#62-the-spine-what-landed)), but nothing replaces it on global buffers, because on
global buffers the read-back the spine would order does not exist.

Two measurements outlive that decision and are recorded here rather than lost:

- **A write the codomain does not reach is not written, and that is correct.** The
  lowering is demand-driven from the codomain leaves, so a `write` bound to a
  statement the return value never reads is never emitted
  and never diagnosed: two writes to one output with only the second
  reachable answer with the second's value and no complaint. That looks exactly like
  the ladder's histogram until it is checked against the **type**: an index function
  is `A T.I -> T.O` over a parameter whose `.out` group *is* the outputs, so walking
  the return necessarily covers every declared output, the per-output
  `write_value_node` gate and the check that each position's write **names** that
  position already say the two sets are the same,
  and an unreturned `write` is a dead expression — dropping it is what a
  pure-function semantics does. **One narrowing:** a write reached through another
  write's `.to` is *not* dead and is now emitted, so "unreached" means reached neither
  by the codomain nor by a `.to` chain. [code-audit D17](code-audit.md) records the
  decision
  and its scope: **it holds for global buffers only**, because §9 step 4 makes a
  `block[1]` buffer readable by another lane, and then an unreturned staged write has a
  reader and reachability from the codomain stops being the test. The token would not
  have caught it either, and could not — both writes hang off the same
  `Const(State, 0)` root, so the first is a value nothing consumes. **D17's sentence
  is what this whole section rests on**: a `compute.write` is not an effect, it is the
  value of an output position. So threading a state token through it was a category
  error and not merely an expensive choice, and returning a new version of the buffer
  is not a clever encoding but the honest type for it.
- **A data-flow chain of two writes composes, and the gate said to refuse it is
  gone.** This bullet originally recorded that a chain trips the tally gate ("emitted
  2 write(s) but its codomain names 1 output(s)"), because the gate counted
  *emissions* and a chain emits twice for one output. **Two corrections, both
  measured.** The spelling that matters was never refused: a chain is `.to` closing
  over the first write's result, and because `buffer_write` ignored `.to` the inner
  write was simply *dropped* as unreachable — the program compiled and answered with
  the outer write's value, which looked right only because both writes hit the same
  slot. Given the inner one slots of its own, they come back **zero**. And the gate is
  retired rather than taught: the ordinal a write fills now comes from its `.to`, and
  `write_classes` is placed at that ordinal, so a chain leaves one entry per output and
  the emission count no longer decides anything. What replaced the gate is the check
  that each codomain position holds a write naming that position. Under versioning
  that program is ordinary — which is what D17's sentence already said, and is now what
  the lowering does.

**The enforcement question is answered for global buffers, and by two refusals rather
than by a check.** It used to be that nothing forbade reading the original `b` after
writing it — a silently unordered read — and `KernelBody::validate` could not forbid it,
because it cannot tell an input buffer from an output. It does not need to. A read's
`.from` resolves against `roles.inputs` and a write's `.to` against `roles.outputs`, and
those two path lists are **disjoint by construction**: `parallel_roles` gives each leaf
one role by which top-level field it sits under (`named("in")` and `named("out")`, two
different positions), and a `Buf`-shaped leaf under neither is refused outright ("a
buffer outside `.in`/`.out`"). So no global buffer is both read and written by one body,
and the unordered expr is not forbidden but **unspellable**. Both halves are measured: a
read of an output, or of a write's result, is refused
([compute-buffer-wrapper](compute-buffer-wrapper.md#the-chain-walk-what-it-measures-and-what-it-cannot-answer)),
and a write naming an input is refused by the same resolver from the other side. The
token never provided this either — `read` does not consume it. [§10](#10-what-is-open)
item 3.

## 4. What decides a barrier, and it is three facts

With the spine, the analysis is a scan. For every expr `(write j, read i)` in
program order, a barrier is needed iff **all three** hold:

1. **The addresses may overlap** — `slots(j) ∩ slots(i) ≠ ∅`.
2. **The writer and the reader are not provably the same lane.**
3. **No barrier already sits between them.**

Fact 2 is what makes the ordinary write-then-read of one element — `write buf i v`, then
`read buf i` — cheap: both `i`s are the same SSA value, hence the same lane, hence no
barrier — one lane always observes its own earlier write in program order. Fact 3 is a latch.
Both are free; the cost is fact 1.

**Fact 1 is not string equality, because an address is usually an interval.** A staged write
at index `i % 64` touches the element `i % 64`, which is one concrete element per lane but a
range `[0, 63]` at compile time; the staged read at `(i + 1) % 64` reads a range too. The
question is therefore:

> do there exist two lanes `a ≠ b` with `index_of(a, j) == index_of(b, i)`?

which is a `may-alias` over two intervals. It is decidable and it is local —
the index expressions are affine over `%`, `/`, `+` and the lane count is the
fragment's `LOCAL_SIZE_X` (64, `spirv.rs:150`) — so it is O(1) integer
arithmetic, not a prover.

### All three facts are a def-cone query in the IR, and the kind is the type's `DP`

This is the audit's second correction, and it is what makes the staging of §9
possible. The value graph alone cannot answer fact 1 — it knows `k` is an `Int`
and not whether a staged write at `k` touches one element per lane or the same
element for every lane. But the fact that decides it is **which memory it is**, and
that is a **type** fact lichen carries by putting the `DP` in the buffer's type: a
`grid[1]` buffer is thread-disjoint and a `block[1]` one is block-shared. Given
that, the three facts **split** — and the split
corrects what this subsection first claimed, which was that all of them are
computable inside `lichen-kernel-ir` from the body alone. Fact 1's *precision* is
not. It is stated below as "modulo `LOCAL_SIZE_X`", and `LOCAL_SIZE_X` is
`crates/lichen-compute-gpu/src/spirv.rs`'s own constant — the
`SpvExecutionModeLocalSize` that emitter declares — while `KernelFragment` carries no
lane count at all (its fields are `param_shape`, `roles`, `body`, `inputs`, `outputs`,
`input_classes`, `output_classes`, `result_classes`), and `lichen-kernel-ir`'s
zero-dependency rule is a stated invariant rather than an accident of the tree: "this
crate is the contract between a *compiler* stage … and one or more *backends* …
Anything it named would be named by both sides." So the body alone cannot answer it
and the crate is forbidden from learning it.

| the question | the query | cost | where it can live |
|---|---|---|---|
| fact 2, same lane | the two index operands are the same `ValueId`, **and nothing else** — the clause this row first carried, "or neither's def cone contains the lane parameter", is dead and would be unsound | one comparison | the IR, from the body |
| fact 1, **conservative** | the two index intervals intersect at all, a lane's own interval being the whole unsigned range — so an operation that may overflow answers *unbounded*, because a wrapped index is not inside the sum of the bounds | O(1) integers per expression | the IR, from the body |
| fact 1, **exact** | the same question **decided** rather than bounded: does a lane other than the writer reach this slot | not bounds — arithmetic over the per-lane function, a modulo/periodicity question | the IR, and the width is its **domain and not its answer**: `lane_width` alone does not close it |
| fact 3, already separated | a **position** latch and not a boolean: a recorded barrier separates an expr when its read sits strictly after the write and no later than the read, and that walk order stands for a path order only because `validate` makes such a barrier's read dominate every reachable `Return` | a comparison | the IR |
| the contended-write **refusal** | the write's index def cone excludes the lane parameter **and** the slice's buffer type says `block[1]` | the same cone walk | the IR, and it must stay there |

The refusal stays above the backends because it is a **program** property, not a target
limitation: on a host that runs invocations in order a contended write has a defined
meaning, so refusing it in one backend only would leave a differential test comparing a
race against an order. It needs no barrier instruction to compute — that was measured
separately and is §9 step 3's business, not this paragraph's.

**It is the same answer for the instruction: a barrier does not belong in the vocabulary
every backend shares.** This paragraph first recorded the cost as **11 edits across 7
files in 4 crates**, and a second measurement corrected it in both directions to **13**:
`produces` is a wildcard and needed no edit at all, so its row was double-counted, while
the SPIR-V side is four edits rather than one — the `OpControlBarrier` opcode, the
`ScopeWorkgroup` and `MemorySemanticsWorkgroupMemory` constants a greenfield backend does
not have either, and the emission arm. **A third measurement, taken while emitting it,
found the SPIR-V side's four still short**: it also needs `OpTypeArray`, the `Workgroup`
storage class, the length constant an `OpTypeArray` must have defined before it, and the
shared type chain and variables §9 records.

**The count is not the finding; the classification is.** Eight of the thirteen sites are
*mechanical* — a target with no barriers answering "no type", "no class", "nothing" — and
five are *semantic*, and all five sit in targets that will never have a barrier: the
fusion bridge must add a `BridgeRefusal` variant and refuse, the wasm emitter must refuse
because the host calls once per *index* and not per workgroup, and the differential test's
own CPU interpreter has to say what a barrier means to a sequential run. That last site is
the sharpest: the answer it invites is a no-op, which **silently compiles a barrier into a
per-index interpreter** — the exact thing §7 says must be refused. Nothing in the compiler
makes that site refuse; a person does, or nobody does.

**That is a cost argument about the instruction, not a location argument about the
width, and this note first drew the wrong conclusion from it.** The crate's invariant
forbids *depending on* a backend's constant; it does not forbid *receiving* a width as
data, and `KernelFragment` already does exactly that for a target fact: `int_width:
IntWidth` is "declared rather than assumed", is hashed in `fragment_digest`, and a backend
whose target cannot represent it "refuses the fragment **by name** rather than narrowing
silently" (`lichen-kernel-ir/src/lib.rs`; `SpirvRefusal::UnsupportedIntWidth`). The
lane-group width is the same kind of fact and wants the same treatment — declared by the
fragment, in the digest, and refused by name by a target that cannot schedule it. With the
width as data the whole scan is the IR's, which is §9's staging and not the backend's. It
landed at `c7223fc`; §9 step 2 records what did *not* land with it.

**The pattern to copy from `int_width` is *declared → hashed → refused by name*, and its
shape is the one part that cannot be copied.** `IntWidth` is a one-variant enum, so its own
refusal is unreachable and `fragment_identity.rs` records that the case cannot be
constructed. A fact modelled on it should therefore be a newtype carrying a number rather
than an enum carrying a choice, and it should be grouped with its siblings so a later fact
joins the digest by construction rather than by someone remembering a line — which is what
`TargetFacts` is, and why `fragment_digest` hashes the struct and not each field.

**And "declared by the fragment" is not "chosen by one side".** The two sides that must
agree on a width cannot see each other: `lichen-compute` is the lowering and deliberately
does not depend on `lichen-compute-gpu`, where `LOCAL_SIZE_X` lives — the wasm backend must
not need the GPU crate — while the one crate both may depend on is `lichen-kernel-ir`,
whose invariant forbids it from *choosing* a width. So each side states its own: the
lowering declares 64 as the policy it lowered for, the emitter keeps 64 as the capability
it schedules. **The named refusal is the only link between them**: a divergence makes the
emitter refuse the fragment rather than emit a `LocalSize` the body was not lowered for.
That is the honest resolution rather than a gap, but it does mean the agreement is enforced
at the one place that can still refuse, and not by the type.

The refusal is the interesting row, because it is the one this note originally
assigned to `DP`. "Excludes the lane parameter" is a **sound** test for
lane-invariance and therefore for a contended write; it is **incomplete** — it
misses `i % 1`, whose cone contains the lane and whose value is nonetheless
constant across lanes. `UP` is exactly what closes that gap: a value annotated
`# 0` is uniform over all threads, which is the precise form of "lane-invariant"
([attributes](attributes.md#perspective)). So the perspective attribute is a
**precision upgrade on a refusal that already works**, not the thing that makes
the refusal expressible. That is the whole of §6.1's deferral.

The table this note originally justified `DP` with, restated with what actually
decides each row:

| index | writes | needs a barrier before a read of another lane's slot? | decided by |
|---|---|---|---|
| `0` | one slot, all 64 lanes colliding | no barrier helps — this is the **error**, refused | the def cone (and `UP` for the incomplete cases) |
| `i % 64` | one slot, one per lane | depends on the reader's interval | fact 1's interval arithmetic |
| `i` (a global address) | one slot, one per lane | no — the disjointness the buffer map already has | the buffer type's `DP`: `grid[1]` |

The last row is also the **scope** of the analysis: a barrier is needed for
*shared* memory because one workgroup's lanes see each other, and a `grid[1]`
buffer's disjointness means a global write needs no barrier at all — cross-dispatch
ordering is the graph's job and the trailing `SHADER_WRITE → SHADER_READ |
TRANSFER_READ` barrier's ([lichen-compute-gpu](lichen-compute-gpu.md#why-these-graph-rules-are-silent-wrong-answer-rules)),
not a kernel's.

### What the scan settled

`crates/lichen-kernel-ir/src/barrier.rs` is §4's rule as code, in the conservative
form, and what it settled is recorded rather than left to the next reader.

**The lane parameter is not nameable from a `KernelBody`, and this note never said
where it would come from.** The lowering emits it as the entry block's **last**
parameter — `lower_index_function` adds the config's leaves and then one more, and
`ComputeOperator::Range` resolves to `self.parameter(...)` at the sum of the slots'
flat arities (`compute/body.rs`) — while the fragment states it as `param_shape`'s
last leaf, which is `KernelFragment::scalar_leaves()`. The identification is
therefore the **fragment's**: every entry parameter is a `BlockParam`, so nothing in
the body tells the invocation index from a config leaf. The scan takes `lane` as an
argument the caller states rather than guessing, and a `from_flat` body names its own
lane the same way.

**Fact 2's second clause is dead, and read alone it is wrong.** "Neither index's def
cone contains the lane parameter" cannot survive the contended-write refusal: that
refusal fires for *every* slice write whose index's cone excludes the lane, so by
the time an expr is examined the write's cone contains the lane and the conjunct is
false. It would not be sound if it did fire — two lane-invariant indices are one
address for every lane, so the reader is *not* the writing lane — and the refusal is
what makes that case an error rather than a skip. The scan implements fact 2 as the
same `ValueId` and nothing else.

**The conservative fact 1 is an interval, and its arithmetic must not wrap.** The row
that says "the two index intervals intersect at all" is O(1) integer arithmetic per
expression, and the lane's own interval is what makes it decidable **without** the
width: it is the whole unsigned range. A `+` that can overflow therefore answers
*unbounded* rather than the sum of the bounds, because a wrapped index is not inside
that sum — and a bound that excludes a reachable slot is how a barrier goes missing.
The **exact** row is not implemented: it needs the lane count *and* the modulo
structure (`i % 64` is one slot per lane only because the width is 64), which is
arithmetic over a per-lane function rather than over an interval, and `lane_width` is
a fragment fact the scan does not take.

**Fact 3 is a position, and the scan reads the body's own exprs.** A recorded barrier
separates an expr when its read sits strictly after the write and no later than the
read, in the **walk's** order; so a scan of a body that already holds the exprs it
would return answers none. The walk's order is exact for a straight-line body, and it
is the order a branching body's own recorded barriers are read in only because
[§9](#9-the-staging) has `validate` require such a barrier's read to dominate every
reachable `Return` — a barrier in one arm alone is refused rather than trusted here.

Two smaller things the code settled. A `from_flat` body must compute a read's slot
and index **before** the write it observes, because the stack hands operands over in
push order and the version is appended; the natural write-then-read spelling exists
only through `add_op`. And the **refused read** — one whose version operand names no
slice write — is the obligation §9 step 4 owes, refused by name by the scan's own
first pass rather than inherited from `validate`.

## 5. The division of labour

| | carrier | supplies |
|---|---|---|
| **order** | the witness spine in the body DAG | which `(write, read)` exprs to examine, and fact 3 |
| **kind** | the `DP` of the buffer's type (`block[1]` / `grid[1]`) | the scope: whether an expr is in the analysis at all |
| **address** | the def cone + interval arithmetic, in the IR | facts 1 and 2, and the contended-write refusal |
| **precision** | `UP` (`# p`) | closing the refusal's incomplete cases; `claim`/`partition` narrowing |

The first three are sufficient and are what §9 stages. The fourth is an upgrade on
top of a working analysis, and §6.1 records what §5's merge makes it cost:

- the spine without the scan is inert — correct and doing nothing, which is
  exactly what part one is meant to be on its own;
- the scan without the spine has a positional order to walk, but loses it the
  first time a pass restructures the body (§3);
- neither needs `UP` to be sound. Both get more precise with it.

**So: `DAG` gives the order, the type's `DP` gives the scope, the def cone gives the
judgement, and the barrier is a scan over the first three.** That is the sentence the
whole note is for; the original version of it named `DP` where the def cone now sits,
and the audit's correction was that the def cone is the judgement while the `DP` is
what puts an expr in the analysis at all.

### The three spaces, and how one is chosen

The facts decide *whether* an expr needs a barrier. They also decide **where the value
between the expr lives**, which is the same question one level up: a value that crosses
lanes needs somewhere to be between its write and its read. There are four spaces, and three of
them are a kernel's to choose — registers, shared, and the warp exchange; the fourth, global, is
the graph's.

| space | who reads it | mechanism | today |
|---|---|---|---|
| **registers** | the lane that wrote it | a **scalar** is an SSA value and needs nothing; a **slice** is a per-lane array of its declared count, which needs a `Private`-storage variable and an array type — the array type and the module-scope variable exist for `Shared`, the storage class does not | a scalar: the only one that exists; a slice: refused by name (`SpirvRefusal::StagingNotOnThisBackend`) |
| **shared** | another lane of the same workgroup | a `Workgroup` variable, and a **block-level** barrier | emitted for a `from_flat` body — `Workgroup` storage and `OpControlBarrier` (§9 step 5's GPU half); the surface that names one is unbuilt |
| **global** | another workgroup | **a graph edge**, so the region is not one kernel | the fusion gate's answer |
| **warp** | another lane of the same **subgroup** | **the shuffle network**: a write emits nothing (the value stays in the writer's register) and a read is one `OpGroupNonUniformShuffle` | landed at `de60dec0` — `Space::Warp` (`lichen-kernel-ir`), emitted by `spirv.rs`; a target without it keeps refusing by name |

**The space a slice states is a field, not a name.** The values a slice may carry are the
plan's own, `space = Register | Shared | Warp`, and they are `Slice { class, count, space }`'s third
field — `KernelFragment::slices`, hashed whole by `fragment_digest` as it was before. A target
that cannot honour one refuses it **by name** rather than placing the elements in the other
(`SpirvRefusal::StagingNotOnThisBackend`); `Shared` and `Warp` are emitted, `Register` is not.

**`Warp` is a placement and not an instruction, which is why it is a `Space` member** (`de60dec0`).
Its contract: **element `i` is staged by the lane whose subgroup-local id is `i`** — the lane a
read's index names. Putting the shuffle in an access instruction would move a storage decision
into the op, which the rule above forbids outright, and giving `Register` a conditional warp
meaning would make one spelling mean two programs depending on a gate — the exact trap this
section names ("naming the slice after one of them turns a placement into an identity, and then
nothing can move it"). The gate is one element per subgroup lane: a wider slice "names a lane the
reader cannot reach" and a narrower one a lane its siblings also staged, so `spirv.rs` refuses
both (`StagingNotOneElementPerLane`) and refuses a read of a slice this lane never staged into
(`WarpExchangeWithoutAWrite`) — **never placing them elsewhere**. The capability is the child
`GroupNonUniformShuffle` (65), **not** the parent `GroupNonUniform` (61): a module that declares
only the parent is refused by `spirv-as`. The companion barrier is `OpControlBarrier` at
**Subgroup** scope with `AcquireRelease|WorkgroupMemory` — a register-staged value has no shared
memory for `WorkgroupMemory` alone to order — so the warp path is also what "a barrier downgraded
to a warp sync" means.
**A landmine the module cannot close**: the subgroup width **cannot be pinned**.
`OpExecutionMode SubgroupSize` needs `SubgroupDispatch` (Vulkan 1.3 / SPIR-V 1.6) and this target
emits SPIR-V 1.3, so `spirv::SUBGROUP_WIDTH = 32` is an assumption about the device exactly as
`LOCAL_SIZE_X = 64` already is — and on a device whose subgroup is 16, a read at index 20 names a
lane outside the reader's subgroup and the answer is **undefined**, not merely wrong, in a way
`spirv-val` cannot see. Closing it needs Vulkan 1.3 emission or a `subgroupSize` query gating the
`Warp` path by name. **And the emitter cannot prove the body's half** of the contract: that lane
`i` staged element `i` is a statement about the body, and the IR does not hand the emitter the
lane parameter, so a body writing `slice[lane / 2]` would exchange against the wrong lane and
answer a plausible wrong number; enforcing it belongs to the walk.

**`global` is not a placement.** A fused body has no global staging: a value two
workgroups must exchange cannot be one kernel, so the answer is that the region does not
fuse and the graph keeps two nodes. That is the graph's decision and not a kernel's, which
is why the plan's staging has two values and not three — a plan asking for `global` is
asking the engine to place a value in the one space that costs a kernel boundary.
**`registers` is not a placement for a value, and it is one for a slice.** A value no other
lane reads is a scalar SSA value, so what the walk concludes at step 1 below is that nothing is
owed — the facts confirm that rather than believe it. A slice placed in registers is not that
statement: the author asked for an owned array and the placement answers *one array per lane*,
which is why the plan may state it, why `Space` has the member, and why `spirv.rs` refuses it
by name today. The two are asked of different objects — the walk asks whether a **value** is
exchanged, the space answers where a **slice's** elements live — and reading one as the other is
what the row above this paragraph corrects.

So the plan's real content is *which* values are staged, *where* (a space), and *which*
barriers — and the automatic plan is the walk that decides it:

**The walk's input is a value that already crosses lanes**, which is worth stating because
nothing in the automatic path creates one until §9 step 8 decides to tile a reduction. So
for a declared plan the walk is a *check* (§9 step 7), for a logic package it is the
*decision* (step 8), and §9 step 4's scan is neither: it consumes a body that already
states its space. The three steps are the same three in all three places.

1. **Is the reader the writing lane?** Fact 2's test, which is the two index operands being the
   same `ValueId`, **and nothing else** (§4). Then the value is a register and nothing is owed.
   The lane-invariance clause this step first carried — "or neither's def cone contains the lane
   parameter" — is dead and, in this direction, unsound: an index that excludes the lane names
   the **same** element in every lane, which is the most contended case there is rather than a
   private one. That case is §4's refusal, not this step's conclusion.
2. **Is the value certified uniform over the group?** A value at `# n` covering every
   reader's lane is one every lane computes, so nothing is exchanged and the space is a
   register again. **This is what `# p` buys**: it is the **sound** form of what the dead
   clause reached for — a value every lane computes — and it settles an expr fact 2 could not,
   in the direction of *needing nothing*. That is why the certificate is a precision upgrade
   on a rule that already works, and not a requirement for the rule to exist.
3. **Otherwise the value is exchanged**, and the readers pick the space: all of them
   inside the writing workgroup is `shared`, with a barrier placed by facts 1 and 3; a
   reader outside it is `global`, which is the fusion gate's refusal rather than a
   placement.

The width enters in steps 2 and 3 and nowhere else: "uniform over the group" needs to know
what the group is, and fact 1's *exact* judgement needs the lane count to know whether
`i % 64` is one slot per lane or one slot for every lane. Both are §4's table's third row,
and both read the width the fragment declares.

**What makes the choice legal is declared algebra, not a proof.** The paper has the author
state the sharing structure (`partition`, `claim`) and places the synchronisation at the
scope's end; lichen does not give the logic that surface, so the licence comes from the
mark channel instead. A `Reduction` declares its `combine` **associative with an
identity**, and that declaration is precisely the permission to re-associate the fold — it
is what makes a workgroup-tiled implementation of a reduction the *same answer* as the
per-lane loop, and therefore what lets the automatic plan be a floor rather than the only
choice. [graph-fusion](graph-fusion.md#the-memory-plan-the-second-thing-a-pack-hands-over)
§5 carries the interface this rule feeds and §7 the acceptance case that needs it.

### The type names the scope, the plan names the space, and no op names either

The walk decides a space; **the thing it decides the space *of* is a memory slice**, and the
two were one name for too long. A slice is an owned staging area with an element class and a
length, and register, shared and global are three places it may live. Naming the slice after
one of them — which is what `Shared` did — turns a placement into an identity, and then
nothing can move it.

**`UP` and `DP` are two facts and they do not collapse, which is what fixes where each
lives.** The paper's `DP` says the *memory a value names* is not overlapped by another group
of threads; its `UP` says the *value itself* is the same inside one group
([the walkthrough](../reference/类型系统验证底层%20GPU%20语言：Perspective.md), "Data Memory
Heirarchy"). One access can need both, at different scopes: copying a single value into
**every** block's shared memory requires that value to be `grid[1]`-uniform — every block must
receive the same one — while the regions being written are `block[1]`-disjoint. One annotation
cannot say that, so the split is not a redundancy to fold away: `UP` is the annotation (`# p`)
and **`DP` is carried by the buffer's type**. That is the paper's own table — `shared(T,N)` is
`block[1]`, `ptr(T,N)` is `grid[1]`, `view(T,N,DP)` is `DP`, a thread-local is `thread[1]` —
and lichen's wrapper is where it already lands
([§6.1](#61-dp-is-the-types-and-up-is-the-annotation) below).

A `DP` is a **scope** and not a storage space, and the plan picks the space inside it:

> **The buffer's type says which threads may see the elements; the plan says which storage
> implements that; an op says neither.** The logic states dataflow, the type names the `DP`,
> the plan names the space, the IR carries the space it was given, and a backend emits what
> that space means for its target or refuses by name.

| layer | what it states | names a `DP` | names a space |
|---|---|---|---|
| the logic DSL | dataflow, and each buffer's type, which carries its `DP` | in the type | **no** |
| a body — the engine's, or one built by hand (`from_flat`) | dataflow, and **where values are staged** — an element class, a length | in the types it declares | **no** |
| the plan (the shadow file) | the **space** of each staged value, and the synchronisation | no | **only here** |
| the IR | the decided space, on the slice | *post*-decision | *post*-decision |
| a backend | what that space means here, or a named refusal | yes | yes |

**Which is also why `Space` has no `Global` member.** A `grid[1]` scope is the buffer's type
and not a staging placement: a value two workgroups must exchange is a value the region did
not fuse around, so the graph keeps two nodes. A slice is a `block[1]` scope, whose space is
one of the two a *block* offers — `Register` (one array per lane) or `Shared` (one per
workgroup) — and both are refused by name by a target that cannot honour them.

**And the ops name neither.** `read` and `write` are the language layer's whole vocabulary: a
staged write is the same `write` on a buffer whose type says `block[1]`, and the version a
staged read must observe is emitted by the lowering rather than spelled at the surface.
`swrite`/`sread` were this note's own error — an op name answering a question the type already
answers, which is what put a space into the language layer while this same section forbade it.

**Three quantities keep being read as one, and they belong to three owners.**

| quantity | what it is | whose |
|---|---|---|
| a slice's **element count** | how many elements the staging area holds | the **algorithm's**: `i % n` and the escape refusal both read it |
| the staging **group width** | how many lanes take part in the exchange | the **plan's**, and what `StagingWidthNotAWidth` checks against a device |
| the device's **lane width** | how many lanes a workgroup runs | the **fragment's**, declared as a target fact (§4) |

They coincide for a one-element-per-lane exchange, which is the reduction's case, and that
coincidence is how a wrong derivation survived review: the count was read off `lane_width`, so
it was right for the acceptance case and wrong in general. A tiled matmul stages a 16×16 tile —
**256 elements over 64 lanes** — so the count is the algorithm's, and the lane width is only
what a *barrier* must synchronise. A plan's staging entry names the **space**, and the width
it may carry is the plan's own group width — a different quantity from the count, which the
body's `block[1]` type carries.

**A buffer's type carries `(element, length, DP)`, and its length is stated only where
nothing binds it.** `Buf` is *bound*: the host sizes it, so a body never names that. A slice
is *owned*: it has no binder, so the body states its length — and its `DP`, which is
`block[1]` where a host-bound buffer's is `grid[1]`. That is the whole of the asymmetry
[compute-buffer-wrapper](compute-buffer-wrapper.md#the-prelude-algebra) records, and it is why
one type with a `DP` argument covers what two nominals did: `Shared` was not a different kind
of thing, it was `Buf` at `block[1]`. The cost of one nominal is that `.native` reads as "the
storage handle, resolved at lowering" rather than "a descriptor binding" — a resolver's
business, not the type's.

## 6. The shape of the two halves

### 6.1 `DP` is the type's and `UP` is the annotation

The paper encodes the hierarchy in the data type and **derives** `DP` from it
([the walkthrough](../reference/类型系统验证底层%20GPU%20语言：Perspective.md), "Data Memory
Heirarchy"):

| hierarchy | type | `DP` |
|---|---|---|
| thread local | `int` | `thread[1]` |
| **shared** | **`shared(T, N)`** | **`block[1]`** |
| global | `ptr(T, N)` | `grid[1]` |
| view | `view(T, N, DP)` | `DP` |
| immutable | `const(T)` | `0` |

**`DP` being a *type* fact and not an annotation is the load-bearing choice, and it survives
§5's merge rather than being spent by it.** `UP` and `DP` answer different questions about one
access — is the **value** the same inside a group, is the **memory** unoverlapped between
groups — and a program can need the two at different scopes. Replicating one value into every
block's shared memory is the case that settles it: the value must be `grid[1]`-uniform,
because every block has to receive the same one, while the regions written are
`block[1]`-disjoint. **One perspective per expression cannot state that**, so the two do not
merge, and the `DP` has to live where a *type* can hold it.

What the merge changes is the carrier and not the choice: there is **one buffer type and the
`DP` is one of its arguments** — `(element, length, DP)` — so the paper's two named cases are
instances of it rather than a second and third nominal. Written as the paper spells them, the
one type looks like two nominals, and the difference between the two spellings is the length a
*slice* states and a host-bound buffer does not:

```lichen
-- the paper's cases, not lichen surface: `compute.lichen` spells one `Buf`, `Shared` and
-- `view` name no type there, and a written `DP` is surface the lowering does not have yet (§10)
Shared = T => struct{.native _, .element T, .n Int}   -- the paper's shared(T, N): Buf at block[1]
Buf    = T => struct{.native _, .element T}           -- the paper's ptr(T, N): Buf at grid[1]
```

`view(T, N, DP)` is that type with the argument written rather than implied, and `Shared`
being a *second nominal* is exactly what made a placement look like an identity — the level
was a function of *which* wrapper, so nothing could move a slice without changing its type.
**That is also why no layer but the plan names a space, and why the language spells no
`Shared`.** The logic's whole memory vocabulary is one buffer type plus the read, the write
and the ingress ([compute-buffer-wrapper](compute-buffer-wrapper.md#the-prelude-algebra)), so a
storage noun in the language would be a placement spelled as an identity — the thing the
paragraph above says cannot move.

**And the level component is not owed at all, which corrects what this subsection first said.**
A `DP` written in a type does have to say *how far* a value reaches, and the lattice already
answers it. `# p` is a non-negative integer whose order is divisibility, whose bottom is `1` —
one thread per group — and whose **top is `0`**, which `perspective.rs` states as "uniform over
all threads, the `∞` fold", with `gcd(n, 0) = n` and `divides(0, sup) ⟺ sup = 0`. So the two
cases a kernel needs are already distinct, and neither needs a tag:

- **a finite `p` is inside a block.** It is a group of `p` threads the block contains, and the
  only barrier this landing places is the block's (§9), so the warp is not a level here.
- **`0` is `∞` and it is the top**: it crosses blocks, and no finite `p` names it.

Three scopes collapsing into one number is the mismatch the roadmap records as "divisibility
admits a width no device has — `6` is a legal perspective, and no lane group is six wide"
([roadmap §4.2](gpu-algorithm-roadmap.md#42-axis-c-lane-width-and-the-consumer-perspective-has-been-waiting-for)),
and **with cross-block synchronisation out of scope it stops mattering**: there is no third
scope to tell apart, so nothing needs a level and `Perspective` does not change. What separates
block from grid is finiteness, not a tag.

**That is also `Space` having no `Global` member, stated in the lattice rather than as a
convention.** A slice is what a *block* can stage, so its `DP` is finite; the `∞` case is a
value that crosses blocks, and with no cross-block synchronisation primitive there is nothing
to stage it with — the region does not fuse and the graph keeps two nodes (§5).

**If a third scope ever arrives** — a cluster, or a barrier narrower than the block, which is
where a warp's width would become a question — folding a level
into `Perspective` is the option to prefer, and the price is then the one this subsection
measured while it was still an option: `AttrExt::combine`, which hard-codes `ctx.expr(gcd,
ctx.int_type())` (`perspective.rs:156`), the `GcdOp` codec *and* its `run`, `is_subtype`,
`render`, `missing_value`, the persist codec, and every row of
[attributes.md](attributes.md)'s acceptance table.

**What is deliberately not considered is `UP` against `DP`.** The design owes no comparison
between the two annotations: `UP` lowers implicitly to the perspective it aligns to (the
paper's `lower`), so a case where the annotation is already finer than the `DP` is absorbed by
that lowering and never asked about. A wrong `DP` is caught instead by §4's def cone, which
reads the index expressions and refutes the placement the `DP` implied — a test of the body
rather than of the two declarations against each other.

### 6.2 The spine: what landed

> **Reverted — read this as the record of what the token cost, not as the design.**
> [§3](#why-the-witness-was-the-wrong-answer-and-what-replaces-it) retracts the
> argument that chose it. Everything below stayed true of the landing while it was
> on `dev`, and the costs it records — a `ScalarClass` variant with `unreachable!`
> arms in four crates, a state carried through every loop nest, ~20 hand-written
> bodies reseeded, a refusal variant and a bridge arm that both exist only to reject
> the token — are exactly what the replacement does not pay.

Part one, and the only part built. **`compute.write` returns a zero-width
state**, and the lowered write threads it.

At the surface the change is two lines and one new type:

```lichen
State = struct{}
write = (x : (Write _)) => $write(x.to, x.at, x.value): State
```

`struct{}` is the zero-field struct form the parser already accepted, so the token
needed no new syntax. `Write` keeps its three fields and `$write` its three
operands — the state is threaded by the lowering, not spelled by the author (§3's
"the token threads through the *lowered* body, not through the codomain").
Underneath, `ComputeOperator::Write` stops being lazy (§2's first bullet) and
produces the token while `Range` stays lazy, so `ComputeValue` gains a `State`
variant and `ValueCodec::write_value` refuses to persist it the way it already
refuses the other runtime-only compute values.

In the IR the token is an operand and a result:
`KernelInstr::BufferWriteCall`'s `produces()` went `0 → 1` — now unconditional for
every instruction — and its `arity()` `3 → 4`, with the state **first** —
`(state, position, index, value)` — so the three existing operands keep their
relative order and a hand-written body is a four-operand push rather than a
reshuffle. `own_class` answers `State` for a write, not the element class in its
own form: that class is what was *written*, while what the instruction *leaves* is
the token, and declaring the element class would make `from_flat` state a result
class the write does not produce.

**`ScalarClass::State` is a third variant and deliberately not in
`ScalarClass::ALL`**, which stays the two *data* classes. That is what keeps the
change cheap: every fixed-size per-class table in the tree — an SPIR-V module's
element type and `ArrayStride` per class, the host's packing widths — is sized by
`ALL` and indexed by `ScalarClass::index`, and a state is no buffer's element, so a
table entry for it would be a chain nothing can bind. `byte_width` and `index`
answer `unreachable!` for it rather than a number, because a zero stride is a wrong
layout that compiles while a state reaching either is a bug in the compiler's own
construction. Where a state reaches a *data* position the emitter can see, it is a
named refusal: `SpirvRefusal::StateWhereAValueBelongs`.

The residual hazard is stated rather than fixed: **`KernelFragment` has no
`validate`**, so a fragment that *declares* `result_classes: [State]` reaches
`type_of`'s `unreachable!` and panics instead of being refused. The lowering cannot
produce one — a parallel fragment's result is the dummy constant in the first
write's value class — but a hand-written fixture can. §10 item 4 keeps the general
question open.

**The state is a carried value of every loop nest, and no source program can put a
write there yet.** `Lower` seeds the token with a `Const(State, 0)` in the entry
block, `buffer_write` consumes and replaces it, and `lower_loop` gives every nest
block one more parameter beside its slots, the backedge and the exit arm handing it
on. `spirv_validation`'s hand-built loops exercise the carry structurally; the
*source* path into it is closed, and naming the gates that close it matters because
[§8](#8-what-the-loop-rule-does-and-does-not-change) is exactly the relaxation that
opens the first.

A codomain position must reduce to a `compute.write`, and the one non-write leaf
that gate tolerates is a `Selection::Computed` conditional (`compute.rs:2316`) — a
`@loop` head is an `Apply`, so `@loop pass = s => if s(0) == 0 then write(…) else
pass(…)` *as* the codomain is refused before the lowering runs. With the loop inside
a codomain write's value instead, `buffer_write` takes its ordinal before it lowers
its operands (`body.rs:1574`), so the nest's own write bills a second ordinal and
the tally gate refuses it (`compute.rs:2335`). And there is no third route:
`lower_index_function` walks only the codomain leaves, so a `@loop` bound to a
statement the codomain never reads is not emitted at all, and `Lower::value` hoists
a node the active nest does not own to the nest's **entry** block rather than to an
arm (`body.rs:262`). A write reaches an arm's block only as a nest-owned node, and
both gates refuse one.

So the carry is built ahead of the relaxation that will need it, and §3's argument
for the spine rests on the restructurings that are *coming* — a converted loop's
nests, a slice write inside one, the host backend's segments — rather than on an
ordering a source program can observe today. **The end-to-end test of a write
inside a nest is therefore not writable yet either**; it becomes writable at §8,
which is the point to add it, and until then the carry's only coverage is the
structural one. One unit test did land with the spine, and it is not about the
carry: `static_function_compute_operator_skips_an_operand_less_operation`
(`lichen-lowlevel/tests/basic/static_module.rs`) pins the discriminator the
annotated `write` forced on `static_function_compute_operator` (§6.2's surface
change), which nothing else in the tree guards.

The two backends materialise the token for free, and neither changes an ABI:

| | the state's representation | what it costs |
|---|---|---|
| **wasm** | `i32`; the `call` is emitted over `[position, index, value]` with **no result**, and the token is a fresh `i32.const 0` | nothing — the host import stays `[i64, i64, value] -> []`, so `run_parallel_range` and the whole CPU path are untouched |
| **SPIR-V** | `Kind::State`, bound to the module-scope `OpConstant %uint 0` the emitter already declares unconditionally — for the write's result, for `Const(State, _)`, and for every incoming edge of a carried state's phi | no extra instruction, type or constant per write |

A hand-written `from_flat` body follows the same two rules: seed
`Const(State, 0)` as its **first** step, so it sits at the bottom of the value
stack while the other three operands are built above it, and end with a data
constant rather than with the write, because the return hands out the top of the
stack and a state is no fragment result. `FlatOp::Read(k)` is *not* how the seed is
named — that reads an entry-block parameter, so a state there would grow the ABI.

**The state carries nothing.** It is not a new buffer, and it is not a summary of
what has been written — it is only "the effects up to here have happened". The
slot and the index are read off the *instruction that consumes the state*, which
is where the index expression already lives:

```
t1 = swrite(t0, ws, i % 64, v)   -- state t1; the expr (64, i) is the instruction's
t2 = sread (t1, ws, (i + 1) % 64) -- consumes t1: ordered after it, and its own expr is (64, i + 1)
```

Encoding the touched slots *into* the state instead would make it a record of
index expressions rather than a token, would have to flow through every
intermediate value, and would duplicate the index that the instruction already
holds. So the split is clean: **the state orders, the instruction's operand
describes, and the def cone decides.**

The `swrite`/`sread` spelling above is the **reverted token's**, and no layer spells it:
the IR's two staged instructions are `SliceWriteCall` and `SliceReadCall`, and the language
layer has one `write` and one `read` (§5). Part one changed `write` only: `read` and `Read`
are untouched, because a
global buffer read has no ordering requirement — that is the buffer map's
determinism theorem, and the spine must not quietly revoke it. Explicit threading
at the surface (a `.after` field on `Write`, an initial state to thread from) is
deferred with shared memory; until then `Lower` threads the token in emission
order, which is the order the body already had, so **no false dependency was
introduced** and no existing program changed meaning — `Positions`, the
`tally.writes == outputs.len()` check, and `write_value_node`, which reads the
*lichen-level* `operand_items[2]`, are all untouched by it. *(Since reverted, and
since then overtaken: that check is retired — a write's ordinal now comes from its
`.to`, so an emission count decides nothing. See §2 and
[code-audit D17](code-audit.md).)*

Nothing about the buffer ABI changed either: the ordinal is still a compile-time
constant, the fragment still declares `outputs`, and the run still binds
inputs-then-outputs in one descriptor set. What changes later, when shared memory
lands, is that the write set becomes a **fragment field** rather than a private
tally, and therefore something `fragment_digest` must hash — two fragments that
differ only in their shared-memory layout are two modules, and a cache that
conflated them would serve one kernel's shader for another's. `fragment_digest`
hashes `format!("{:?}", fragment.body)` today (`lichen-kernel-ir/src/lib.rs:693`),
so the spine itself needed no new hashing; the slice field will.

## 7. The host backend is the expensive half

This section did not exist in the original note, and it is the largest cost the
design carries.

[`run_parallel_range`](../../crates/lichen-compute/src/compute.rs) is a
**per-index call loop**: one worker owns a contiguous span of indices, instantiates
the wasm module once, and calls the kernel body once per index, with `read`/`write`
resolved through host imports. That is sound precisely because the body *is* a
function of one index.

**A barrier makes it not one.** `barrier` means "every lane of this workgroup has
arrived", so the host must run all 64 lanes of a group to the barrier and then all
64 from it — the body is a function of a *group*, evaluated in segments. Two ways
to get there:

| | what it is | what it costs | what it loses |
|---|---|---|---|
| **refuse** | a **staged** body — either space, and every recorded barrier — is a named refusal on the wasm/CPU path | one refusal and one diagnostic | `gpu_matches_cpu` cannot cover shared-memory algorithms |
| **segment** | split the body at its barriers, export one wasm function per segment, put the slices in wasm linear memory at `[slot_base + lane × len]`, and make the host loop per-group × per-segment | a second emitter shape, a new host loop, and a shared-scratch layout that must agree with SPIR-V's | nothing — the CPU/GPU agreement survives |

**Decision: refuse, GPU only.** The segment path is the right eventual answer and
is recorded here so it is not rediscovered, but the first landing takes the
refusal. The cost is stated plainly because this project's correctness evidence is
*cross-backend agreement* — the [ladder](gpu-algorithms-ladder.md) measures the
two backends against each other, and `gpu_matches_cpu` is the regression that
keeps them equal — and shared-memory algorithms are exactly the class that loses
it. Their evidence will have to be the SPIR-V structural tests plus on-device
runs, until segmentation lands.

**How to read that deferral, and what it decides in the meantime.** It is not a property of
the host: `run_parallel_range` is a per-index call loop, and the segment path above is how a
host runs a body that is a function of a group instead. So "GPU only" means *until
segmentation lands*, and a reader who takes it for "a CPU cannot do this" has the wrong model —
which is a mistake this note's own summaries made, since the refusal is the cheapest line in
the table and the path that removes it sits one paragraph up. What it does decide is the
**order**, and that is settled the other way: **the acceptance case goes first and segmentation
waits.** The tiled reduction is judged by the closed form and the per-lane reference and
**not** by cross-backend agreement, which is this project's strongest oracle and the one
`gpu_matches_cpu` exists to keep — so for this landing the weaker evidence is accepted
deliberately, with the reason stated rather than with the test weakened: `gpu_matches_cpu` keeps
its refusal of a staged body, and segmentation is a later step that pays the second emitter
shape, the host loop and the shared-scratch layout on its own.

## 8. What the loop rule does and does not change

The `WriteInsideLoop` refusal exists for one reason: `dispatch` allocates output
buffers without initialising them, so a lane that never runs a loop body leaves
its output element as whatever the allocation held — the refusal is the premise
the skip rests on ([lichen-compute-gpu](lichen-compute-gpu.md#the-tail-lane-obligation),
[loop-conversion](loop-conversion.md) §6).

**A slice is not an ABI output**, so the premise does not transfer as stated —
but it does not vanish either. A surplus *output* element is written where no reader
takes it, while a shared element is read by whichever lane addresses it, so the two
differ in *who can observe* the write and not in whether the write is owed. The
relaxation still holds and still has to: every algorithm that wants shared memory (a
scan, a tiled matmul, a block reduction) writes it from inside a loop. What it does
**not** buy is the slot's content: a lane whose trip count is zero writes no element
of the slot, and "written before it is read by construction of the algorithm" is an
assumption about the trip count rather than something the compiler checks. The guard
this section first named — a barrier under divergent control flow, which is a hang —
does not cover it: the lane that skipped the body still reaches the barrier, so
nothing hangs and a reader can take an element no lane wrote. That is the same
obligation the tail lane leaves open below, and it is stated there.

**And the relaxation has three named sites, not one.** §6.2 records that no source
program can put a write inside a nest today, and the reason is *not*
`WriteInsideLoop` — that is the SPIR-V emitter's and it fires last. Two
`lichen-compute` front gates refuse first, both in `compile_parallel_fragment`:
the **codomain gate**, which requires every output position to reduce to a
`compute.write` and tolerates only a `Selection::Computed` conditional
(`compute.rs:2316`), and the **tally gate**, which requires exactly one write per
output and so bills a nest's own write as a second one (`compute.rs:2335`). A
shared slice in a loop has to be admissible at both, *and* at `WriteInsideLoop`
for the output case that stays refused. That is the concrete meaning of "a
genuine relaxation": three refusals have to learn to tell a slice from an
ABI output, and the fact that lets them is the buffer type's `DP` (§6.1), not a new
flag.

That is a genuine relaxation of a stated invariant and is recorded as one, not as
an oversight. The guard it needs is the barrier one, not the reachability one: a
loop whose trip count differs per lane can reach a barrier on some lanes and not
others, which is a hang rather than a wrong number, and fact 3's scan must
therefore treat a barrier inside divergent control flow as a refusal. **That
refusal belongs in `KernelBody::validate`**, where the CFG lives, so one
implementation covers both backends and hand-written bodies too — not in
`spirv.rs` beside `WriteInsideLoop`, which is where the loop rule it resembles
happens to sit.

### The tail-lane obligation: convergence is free, the slot's content is not

The good news the audit found, and it removes what looked like the sharpest new
hang risk. `LOCAL_SIZE_X` is 64 and "a run of `count` indices is
`ceil(count / LOCAL_SIZE_X)` workgroups, and the entry point's body branches on
`index < extent` before any instruction of it, so a surplus lane runs none"
(`spirv.rs`), with the buffers padded as well: "Round up so the last workgroup's
surplus lanes address padding rather than memory past the end" (`dispatch.rs`).

**A staged launch still runs the whole shader on every lane, and that is the case
this section is about**: `launch_workgroups` refuses a count that is not a whole
number of workgroups when the launch records a barrier — a slice's or a buffer's —
so no surplus lane exists, the guard is uniformly true
and the branch never diverges — so **every lane reaches every barrier**, and a
`count` that is not a multiple of 64 does not hang. The padding that exists for an
unrelated reason is exactly what a convergent barrier needs. What remains is the
divergence case above, which is a refusal and not a padding question. Without
staging the guard does mask a surplus lane, and that is what now keeps it from
writing into a real slot — see
[lichen-compute-gpu](lichen-compute-gpu.md#a-computed-write-address-is-not-protected-by-the-padding-and-the-clobber-was-a-race).

**The section this replaces claimed more than that, and the surplus lane's *write*
is what it missed.** Its second half read "a slice ... is written before it is
read by construction of the algorithm that uses it, and its unwritten state is never
a caller-visible element", which treats the surplus lane as a lane that writes
nothing. It writes: the padding is exactly what keeps it running, so it executes
every staged write, at the index its own global invocation id computes. Take the staged
exchange — write `slot[i % 64]`, barrier, read `slot[(i + 1) % 64]` — and a `count`
that is not a multiple of 64: the last real lane of the last workgroup reads an
element whose only writer *in that workgroup* is a surplus lane. With `count = 100`
and 64 lanes, workgroup 1's real lane 99 reads slot 36, and slot 36 is written by
lane 100, which the dispatch does not cover. `dispatch` zeroes the **input** padding,
so that write was computed from `0`, and whether the answer is visibly wrong depends
on whether the staged value depends on an input: `i + 1` hides it, `in[i] + 1` does
not. `graph_on_device` sits on the safe side of this deliberately — `count = 128`,
"a workgroup multiple, so no surplus lane writes a slot a real lane reads".

So the padding discharges convergence and nothing else, and **the launch now refuses
the count that would make the surplus lane observable**: a launch set with any
`slices` non-empty is dispatched over a whole number of the root's
`target.lane_width` workgroups only, refused by name with the count and the width
otherwise (`RunError::StagedLaunchNotAWholeWorkgroup`). With a whole workgroup count
there is no surplus lane at all, so the counterexample above is unspellable rather
than diagnosed after the fact — the obligation it left is closed.

**What that does not close is the slot's content**, a different question of the same
shape. A whole-workgroup launch makes every writing lane a declared lane; it says
nothing about *which* of them wrote the element a reader takes. The extent refusal
(§9) is indifferent to which lane computed the index — it bounds one by
`[0, count)` — and a slot's declared count is not the dispatch's `count`: the first
is the array's length, the second is how many indices the launch covers. So a write
no lane makes — a predicated staged write, a loop body whose trip count skipped it, an
element no lane addresses — still leaves a reader with a value nobody wrote, and a
slot is workgroup-private, so a reader that expects a *neighbouring* workgroup's
element takes its own group's instead. Boundary elements of that kind are the
author's to state: no barrier and no `spirv-val` run can see them.

## 9. The staging

The order the audit's corrections make possible. Each step is separately
mergeable and leaves the tree green.

| | step | crate(s) | note |
|---|---|---|---|
| **1** | **revert the token**: `ScalarClass::State`, `ComputeValue::State`, `Lower.state` and the loop-nest carry, `StateWhereAValueBelongs`, `BridgeRefusal::StateClass`, the ~20 `Const(State, 0)` seeds, `State = struct{}`; `BufferWriteCall` back to `produces() == 0`, `arity() == 3`, and `Write`'s `run` arm back to lazy | `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu`, `lichen-graph-ir`, `lichen-fusion-bridge`, `lichen-language` (tests) | §6.2 is the inventory. Coordinates with `356fc4a`, which adapted the fusion bridge to the variant — and fixes a latent defect there while it does (`read_position` reads `args.first()`, which the token made the state). **Nothing replaces it on global buffers**: §3's ABI wall |
| **2** | **the lane-group width as a fragment fact** — **landed** at `c7223fc` as `KernelFragment::target.lane_width`, declared by the lowering, hashed whole as `TargetFacts`, and refused by name by a target that cannot schedule it (`SpirvRefusal::UnsupportedLaneWidth`) | `lichen-kernel-ir`, `lichen-compute-gpu`, `lichen-compute` | Independent of 3–5, and it is what moves fact 1's *exact* judgement and the uniformity check out of a backend without the crate depending on any backend's constant (§4). It landed **one** refusal, not the two this cell first claimed: `StagingWidthNotAWidth` is a plan-level refusal that needs step 7, and `StagingNotOnThisBackend` is a backend's refusal of a space it cannot honour — step 6's for the host, which refuses any staged body (§7), and now `spirv.rs`'s for `Register` (§5). What landed is a *target*'s refusal of the fragment's own declared width, which is `UnsupportedIntWidth`'s mirror |
| **3** | staging vocabulary: `SliceReadCall` and `SliceWriteCall` as *additive* variants — `[slice, index, value]` and `[slice, index, version]`, each arity 3 and each producing one value — the slice set (`KernelFragment::slices`, one `Slice { class, count, space }` per ordinal with `space: Space = Register | Shared`) hashed in `fragment_digest`, and **the barrier as data** — the separated `(write, read)` exprs on `KernelBody::barriers`, recorded as `ValueId`s and never as positions — **landed** | `lichen-kernel-ir`, `lichen-compute`, `lichen-compute-gpu`, `lichen-fusion-bridge` | **`SliceReadCall` carries the version operand here** — §3's "the read grows an operand" is paid once, at the point where a consumer exists. Additive variants, so the ~50 existing `BufferWriteCall` sites do not move — but they are **not answerless**: every exhaustive `match` on `KernelInstr` must now say something, so the four paths that cannot stage refuse by name *here* rather than at steps 5–6. The fusion bridge refuses a projection that reaches a slice (`BridgeRefusal::SliceAccess`), the wasm emitter refuses the whole shape because the host calls once per index (§7), the differential test's own interpreter refuses rather than no-op, and `spirv.rs` refuses because it still has no `Workgroup` storage and no `OpControlBarrier` (`SpirvRefusal::StagingNotEmitted`, which was step 5's and is now deleted, not §5's `StagingNotOnThisBackend`). The divergent-placement refusal is `validate`'s and is dominance, below. A `Barrier` *instruction* is rejected by measurement, §4: thirteen sites against five, and the five semantic ones all sit in targets that will never have a barrier. Values and not positions because the one restructuring in the tree shifts positions silently, and because an expr is already the shape a plan's `sync` has. **Renamed in place, and the space became a field**: `SharedSlot`, `shared_slots` and the two `Shared*Call` variants are `Slice`, `slices` and `Slice*Call`, so the name no longer asserts a home; a slice states its space (`Space = Register | Shared`), and `spirv.rs` dispatches on it and refuses `Register` **by name** (`SpirvRefusal::StagingNotOnThisBackend`) rather than emitting `Workgroup` storage for it |
| **4** | **the barrier scan**: the walk over a body that **already states** slice accesses, recording the exprs that must be separated, and the contended-write and index-escape refusals — **landed** in `barrier.rs` | `lichen-kernel-ir` | In the IR crate so `from_flat` bodies get it and it is testable without the language. **The scan landed**, conservative fact 1 first, and it takes `lane` as an argument because a body cannot name its own invocation index (§4). **It is not the selection, which this row first said.** §5's three-step walk takes as its input *a value that already crosses lanes*, and in the automatic path nothing creates one until step 8 — so the walk is steps 7 and 8's rule, and this step consumes a body that states its space (a pack's, or the one step 8 generates). The divergent-placement refusal landed in step 3, where `validate` lives: it needs dominance over the CFG rather than a look at one terminator. **And this step owed an obligation it could not inherit**, which the scan now produces: step 3's `validate` checks an expr that is *recorded* and cannot require that a bare `SliceReadCall` names a real write, so a read whose version operand names no slice write is refused by name in the scan's own first pass. What stays open is §4's exact fact 1 |
| **5** | **the lowering, and the GPU half of it**: the plan's space carried from match to emit, a staged access emitted for a buffer whose type says `block[1]`, and the backend (`OpVariable` in `Workgroup`, `OpControlBarrier`, the slice layout) | `lichen-compute`, `lichen-compute-gpu` | **There is no new language surface, and this row claimed one for two revisions.** The language layer keeps one `write` and one `read`; the buffer's type carries the `DP` that says the elements are block-visible, and the *plan* says which space implements it (§5). §3's conclusion holds: the edge, its operand and its **data path** must arrive together, because a read-back with no data path compiles and silently answers `0` — and the version operand is the lowering's to introduce rather than a surface spelling (`.from`). The descriptor set and binding layout do not change — shared memory is not bound |
| **6** | the host backend: the named refusal | `lichen-compute` | §7. A barrier makes the body a function of a workgroup rather than of an index, so the CPU path refuses a **staged** body — either space, and every barrier — and loses `gpu_matches_cpu` coverage for this class of algorithm. Segmentation is a later, separate step |
| **7** | **the plan interface**: a pack's plan matched by schema, **checked against the three facts rather than believed**, carried from match to emit, with the refusals the interface owes | `lichen-compute` (match), `lichen-kernel-ir` (check) | **The placement half landed** at `77fafaf`: a plan is data (`lichen_kernel_ir::{Plan, Staging}`) and `KernelFragment::check_plan` refuses one the fragment refutes — a name no mark published (`UnknownStagedValue`), a name two values carry (`AmbiguousStagedValue`), a value that is not a staging (`NotAStagedValue`), a group width the target has not (`StagingWidthNotAWidth`), and a space the slice does not declare (`PlanContradicted`). `KernelBody::staged_slice` answers which slice a write staged into, `Tiled::staged`/`Tiled::marks` publish the staged value under the **logic's own mark**, and `crates/lichen-compute/tests/lichen_compute/plan_staging.rs` drives a declared plan on a device. **What remains**: the plan *matched by schema*, the two refusals that need §4's three facts (`PlanContradicted` as the facts refute a plan, and `UncertifiedCrossLaneRead`), and `sync`. The artifact that carries a fused kernel — and its plan, which is what gives a placement a route to the emit at all — **landed** at `fb0574c` in `crates/lichen-runtime/src/fused/` ([graph-fusion](graph-fusion.md) §5's artifact bullet states the format and the reader) and the landing **landed** (later commit on this branch: `land_fused_kernel` replaces a recorded node's fragment, refusing a shape that would mis-wire). The plan never enters the *schema* hash and its **effect** enters `fragment_digest` through the fragment — which is why step 3's slice field and step 2's width are the same kind of fact and land in the same place. **§5's three-step walk belongs to this step**: a declared plan is a proposal about which values cross lanes and where they are staged, and the walk is what decides whether the facts allow the proposal. **And a pack's two directives are installed as one pass**: the graph-pass seam stores a single pass and every installer replaces it, so `install_floor_with_packs` (`crates/lichen-compute/src/floor.rs`) chains the region merge and the tiling — which is what lets a pack's `plan` and its `fuse` both apply in one run, rather than one of them silently ([graph-fusion](graph-fusion.md) §5) |
| **8** | **the automatic plan's first consumer**: a `Reduction` tiled across the workgroup rather than folded per lane, licensed by the declared associativity | `lichen-compute`, `lichen-compute-gpu` | **Landed** at `d3f984f`: the consumer is a **graph pass** the host installs, consulted once per run immediately before the dispatcher, and the host that derives and installs it is `lichen-compute` (`automatic_tiling.rs`) — so the pipeline tiles a program rather than a test ([graph-fusion](graph-fusion.md) §8 records the seam). **This step is where a value comes to cross lanes at all**, and therefore where §5's walk is *run rather than obeyed*: the walk is not applied to a body that has the crossing, it decides to create one by tiling. That is why step 4 can be a scan over stated accesses while the selection lives here. [graph-fusion](graph-fusion.md) §7's register-resident rolling state is the floor this replaces where a device can do better. **A correction the acceptance case measured**: the body this row replaces is `Folding::per_lane`'s `[0, lane + 1)` schedule, and that is *not* what a program writing `fold (k.n, …)` runs — such a program folds the whole extent in every lane, so its own `out[0]` is already the closed form and a tiling of it would be unobservable at `out[0]`. What the acceptance case differences against is the per-lane body the tiling actually replaces. **What remains of this row is not the tiling but the plan**: a schema-matched pack plan is step 7's |
| **9** | nothing, and this row used to owe the level component on `Perspective` | — | **not owed**: a finite `# p` is inside a block and `0` is the `∞` that crosses blocks, so the divisibility lattice already separates the two scopes a kernel has. §6.1 keeps the fold as the option to prefer if a third scope ever arrives |

**What step 5's GPU half settled, and it landed on its own.** The row above names three arrivals;
the emission is separable from the surface, and it is the one that landed first, because a
`from_flat` body spells a staged exchange without the surface, the plan or a lowering. Two things
the emission settled, which this note had left to the target:

- **A slot is one `Workgroup` variable per slot**, each an array of the count that slot
  itself declares. The slot ordinal is a compile-time constant and selects a variable id
  directly, exactly as a buffer position selects a binding; one array per *class* with the
  slot as an index would need a second mapping from a global ordinal to a per-class index
  and a second array dimension, for no gain.
- **The descriptor set and binding layout did not change**, which is the row's invariant: a
  `Workgroup` variable carries no `DescriptorSet` and no `Binding`, and the module's annotations
  are the ones the buffers already had. Nothing else in the module moved.

**And one question §4 did not ask: how long is a slot? It is a fragment fact now.** A slot's
element count is `Slice::count`, paired with the element class rather than a second list
that has to stay the same length, and it is the length the emitter declares for that slot's
`Workgroup` array — one `OpTypeArray` per distinct class and count, one length constant per
distinct count, and the constant defined before the type that names it. The conservative interval
§4 specifies answers *may two lanes collide* and never *is this element in the array*, so the
answer to the second question is a second reading of the same interval rather than a second
computation: `KernelFragment::barrier_scan` refuses a slice access whose index interval may reach
`count`, which is well defined only because the bound and the declared array are one number. The
automatic path derives the count from `target.lane_width` — the only length a fragment can supply
while the surface that names a slice's own length is unbuilt, that surface being the `DP` on the
buffer's type, whereas a plan's field
([graph-fusion](graph-fusion.md#the-memory-plan-the-second-thing-a-pack-hands-over) §5) names the
**space** and carries the plan's own **group width** rather than this count — and it states the
count in the field rather than leaving the emitter to read the width, so a declared length is
checkable, two lengths are two digests, and the bound and the declared array cannot drift.
`i % 64` against a count of 64 stays accepted; the global invocation id against it does not.
**A length of zero is refused by the fragment's own gate now** —
`KernelFragment::validate`, the answer to §10 item 4 — because a zero-length `OpTypeArray` has no
type to declare and no index it could bound; `barrier_scan` runs that gate before it reads the
count as a bound. And the launch carries the other half of §8: a launch set that records a barrier — a slice's
or a buffer's — at a count that is not a whole multiple of `target.lane_width`, is refused by
name at the dispatch rather than run with a surplus lane in it.

**Where a recorded expr becomes a barrier, and the one place it cannot go.** The expr is
data; the barrier is a position in the emitted code, and that position is *immediately
before the read's definition* — the earliest point at which the reader could observe the
writer, and so the only point one barrier can serve. That is well defined and needs no
search. **And the barrier is the block's**: `OpControlBarrier` with `Workgroup` execution scope
and `WorkgroupMemory` semantics (`spirv.rs`'s `SCOPE_WORKGROUP` and
`MEMORY_SEMANTICS_WORKGROUP_MEMORY`), one per recorded expr. A narrower barrier — a warp or a
subgroup — is not in this landing, and inserting one is what would make the warp's width a
question at all. **What is not well defined is a read under divergent control flow**: SPIR-V
requires `OpControlBarrier` in uniform control flow, so a barrier placed with the read
inside a branch that not every lane takes is not imprecise, it is invalid. The honest
answers are to place it in the divergent region's immediate post-dominator, where every
lane passes, or to refuse — and which of those is right is **not settled here**. Note the
asymmetry this puts on the refusal: a block-local terminator check finds a barrier in a
block that branches and **misses one in the branch's target**, which was measured while
proving the refusal computable, and is why it needs dominance rather than a look at one
terminator.

**And the barrier has a second kind: the store a merged body keeps.** A slice is a *staging*
space, but a merged fragment can also keep a **producer's store** — the merged body writes the
producer's output buffer as well, and the consumer reads that buffer back, so the value reaches
the consumer without being re-derived ([graph-fusion](graph-fusion.md) §5's store-keeping path).
That pair is a **buffer write and an output read**, not a slice write and a slice read, and the
IR's `Barrier` now names both kinds: `KernelBody::validate` checks that a recorded expr's write
and read name **one buffer** (the two output ordinals are one number) and that the **write
precedes the read** — a buffer pair has no data edge, so the order is the barrier's own
obligation, where the slice pair's version operand already carried it. The GPU emits that pair
as a workgroup `OpControlBarrier` **and** a workgroup `OpMemoryBarrier` over `AcquireRelease`:
the store is a storage buffer's, and `WorkgroupMemory` semantics order the shared memory only.
**A pair of one lane is emitted as neither**: the write's and the read's index operands are the
same `ValueId` (§4's fact 2), so one lane observes its own store in program order and no barrier
reaches it — at an extent below one workgroup a barrier would sit in the `index < extent` branch
and be non-uniform.

**And the scope is the whole limit.** A barrier orders the lanes of **one** group, so a staged
read is only sound where one group holds every lane — the reading lane and the writing lane.
The merge pass refuses the wider case by name (`StagedReadIsNotOrdered`), and the measurement
behind it is in the fusion note: with the guard lifted, a merged `squares` → `scale` over 1 024
lanes changed the step's answer, because the consumer read another group's store. A staged read
that crosses a group needs a **grid** synchronisation, which no `OpControlBarrier` offers — its
execution scope is `Workgroup` or `Subgroup` — so the wider case waits for the mechanism that
tiles a reduction across the group (§9's staging, roadmap axis C).

**The fan-in's store-keeping path has the same two limits, and its own measurement.** The
composition is `compose_fan_in_staged` ([graph-fusion](graph-fusion.md) §5), and it is measured
over one lane group: two reductions and one consumer, the consumer reading at `i / 2` so that it
reads what another lane of the same group wrote. The region merges into one node of three
outputs, and the device's answer equals the unmerged program's and the host's own. The second
limit is the uniform-control-flow rule below: the qnorm fan-in's barrier would sit in the
reduction's own loop, so that region is refused (`MergedBodyIsRefused`) before the group width is
even asked.

**What step 3 landed of that, and what the check is worth.** `KernelBody::validate` refuses a
recorded expr whose read does not dominate **every reachable `Return`** — so every path that ends
passes through the barrier, which is the uniform-control-flow condition read statically. That
catches the branch arm and the dead read, and the expr must be well formed besides: the write is a
`SliceWriteCall`, the read is a `SliceReadCall` observing exactly that write, and both are
`ValueId`s of the body. Two honest limits, neither of them a proof: a read under a branch whose
condition is *uniform* is refused, because only `UP` ([§4](#4-what-decides-a-barrier-and-it-is-three-facts),
step 9) can see that it is uniform; and a lane that can loop forever without reaching the read is
**not** caught, because a path that never returns is not an exit — so a body with no reachable
`Return` passes the check vacuously. Whether a divergent read is refused or moved to its region's
immediate post-dominator is still **not settled here**; what landed is the refusal.

**Where the mechanism lives, and who owes what.**
[graph-fusion](graph-fusion.md#the-memory-plan-the-second-thing-a-pack-hands-over) §5
states the *interface* — what a pack may say about a plan — and delegates the
*mechanism* here: "the wrapper, the spine, the scan, the barrier instruction and the
staging table". This note is the receiving end of that delegation, and the pieces it
owes are steps 3–5. The four refusals §5 names split across them: `PlanContradicted`
and `UncertifiedCrossLaneRead` are step 4's (the facts refuting a declared plan; a
cross-lane value with neither a barrier nor a certificate) — **and measured at `9616085b`, both are
still unwritten**: `UncertifiedCrossLaneRead` is named only in these notes, and the code's
`PlanContradicted` means *the plan's `space` against the slice's declared `space`*
(`crates/lichen-kernel-ir/src/plan.rs`), which is step 2's question rather than the facts'. No shape in
the tree needs a staging yet, so nothing has made them bite — `StagingNotOnThisBackend`
is step 6's, and `StagingWidthNotAWidth` is step 2's — the step that did not exist
when §5 was written, and the reason §5's seam "a declared plan is checked against the
same facts the automatic one is" was owing a width with no named source.

Two facts about the starting point that the staging assumes, the first of them true only until
the GPU half above landed:

- **The GPU backend was greenfield, not extended.** There was no `Workgroup`
  storage class and no `OpControlBarrier` anywhere in `crates/lichen-compute-gpu`;
  the only barriers in the tree are Vulkan **pipeline** barriers between
  dispatches (`dispatch.rs:1068` and neighbours), which order *dispatches* and have
  nothing to do with a workgroup's lanes. `op::VARIABLE` was used at module scope
  only, for the storage buffers and the push-constant block, so there was not even a
  `Function`-storage local to extend — which is why the emission added a storage
  class, a fixed-size array type and a variable per slot rather than extending a
  local.
- **Steps 3–5 are a strict prerequisite chain, step 2 is not a prerequisite of
  anything but the walk, and step 1 is not a prerequisite at all** — it is a
  reversion. The spine was originally step 1 precisely so that it could be validated
  before anything consumed it; §3 records why that turned out to be backwards, and
  why the version that does land lands with its first consumer.

**What would show the staging works, and the number it is judged by.** This note owed an
acceptance case and did not have one: [loop-conversion](loop-conversion.md) §8.5 has its
own — a dynamic reduction against the hand-derived `length(length + 1)/2` —
[graph-fusion](graph-fusion.md) §7 has its own in Flash Attention v1, and the placement
work had only step 8's mechanism. **The case is that same triangle-number reduction, tiled
across the workgroup**: the algorithm the loop line already runs per lane, now computed by
lanes exchanging through a slot and a barrier. Three properties say why it and not
something else:

- **It already has a hand-derived closed form**, so it needs no new oracle, and this
  project's stated method — "hand-derived closed forms rather than a second run of the
  same code" ([graph-fusion](graph-fusion.md) §5) — is what it is checked against.
- **It already has a per-lane implementation, running on both backends**
  (`a_kernel_loop_reduces_a_runtime_buffer_length`), so the tiled version has a
  **reference to differ against**, which is §5's equivalence obligation and the floor
  "faster, never wrong" rests on.
- **Its declared algebra is the licence for the placement**, so it tests the design's
  load-bearing sentence and not only the code: `combine` associative with an identity is
  exactly what makes a re-associated fold the same answer. A reduction whose `combine` is
  **not** associative must not be tiled, and that case is worth pinning beside this one.

One consequence for how it can be verified, from §7: a barrier makes the body a function of
a workgroup rather than of an index, so the host path refuses any staged body and
`gpu_matches_cpu` cannot carry this class. Its verification is the closed form **plus** the
per-lane reference, and not CPU against device.

**The case it was landed with, and the two things driving it measured.** The tiling is in
`lichen-fusion-bridge/src/tiled.rs` and its acceptance case in
`crates/lichen-compute/tests/lichen_compute/tiled_reduction.rs`; the fold over a staged slice is also driven
directly on a device by `crates/lichen-compute-gpu/tests/compute_gpu/graph_on_device.rs`. Two facts came out
of that work and belong to the design rather than to the tests:

- **A staged launch's extent is the elements the fold reads, not the lanes it runs.**
  `GpuContext::run` uploads `extent × width` bytes of each host input, so a two-step tile
  dispatched over one workgroup uploads one tile and the strided read answers the staging's
  padding — which is silently `0`, not an error. The tiled body's domain is therefore `(extent,
  index)`, and its fold count is `ceil(extent / lane_width)` read off that extent parameter at run
  time, so the count a caller means and the whole tile the body covers are different numbers: the
  caller pads to the tile and launches the larger. Every workgroup past the first folds the whole
  tile and writes the same answer, so the surplus is redundant rather than wrong.
- **A `BufferReadCall`'s placement is free, and a `SliceReadCall`'s is not.** A buffer read in a
  loop header is emitted and carried correctly — measured directly on a device — because it carries
  no barrier. A slice read's does: the barrier that separates it from its write is emitted
  immediately before the read's definition, and `KernelBody::validate` refuses a recorded expr whose
  read sits in a block a lane can skip. A slice read therefore stays in the loop **header**, where
  every lane reaches it, and the read and the barrier move together.

One reading of the first wrong answer was **wrong and is recorded so it is not re-derived**: the
symptom — a two-step fold whose per-lane partial came out as the lane's own address — looks exactly
like a `BufferReadCall` hoisted out of the loop as invariant in its address. It is not. The
minimal body that appeared to show it answered its one-step fold, because its slot started where
the address did, and the real cause was the staging above.

The licence and the two oracles the case is judged by, as landed: `BinaryOp::associative` is the
declaration the re-association is read off (`+`, `*`, `max`, `min`), a non-associative `combine` is
refused **by name** beside the positive cases, and the tiled answer is checked at three lengths
against the hand-derived `count(count + 1) / 2` **and** against the per-lane body the same
declaration produces, with a second and third combine (`max`, `*`) driven through the same call at
both a one-step and a three-step tile.

**What the acceptance case cannot yet do, and it is the algebra's fault rather than the schedule's.**
The pack's `combine` and `identity` cannot be read back out of a lowered body: a loop conversion
cannot be bounded by an argument of the function that writes it, so the declared loop's bound is a
literal, the loop is compile-time expanded, and the marked accumulation folds into the constant its
base case returns — `projection(…, ReadRoles::DATUM_ONLY)` answers `0` where the marks name `+`.
The case therefore asserts that the `claim` and **both roles** reached the fragment and takes the
algebra from the pack's declared shape. Reading a declaration's algebra out of a **lowered** body
needs a reduction whose loop survives lowering; until then step 8's automatic path is driven by a
declaration the test states rather than one the bridge projected.

**A scan is the second case and the harder one**, because it needs *several* barriers whose
positions are not the read's own definition — which is where §9's unsettled
post-dominator question bites. It has no per-lane reference today:
[gpu-algorithms-ladder](gpu-algorithms-ladder.md) records it as "`i + 1` reads at index
`i` — quadratic in the source text, and unreachable in practice", so one has to be written
before it can be differenced against, and that is why the reduction goes first.

## 10. What is open

Settled by the audit and the staging above, and recorded so they are not
re-litigated:

- **Where the level lives** — **nowhere, because no level is needed.** §6.1: a finite `# p` is
  inside a block and `0` is the `∞` that crosses blocks, so the divisibility lattice already
  separates the two scopes a kernel has; §9 step 9 owes nothing, and the fold stays the option
  to prefer if a third scope arrives.
- **Automatic insertion, or an explicit barrier with a checked placement** —
  **automatic**. The original note preferred an explicit `compute.barrier` first,
  on the grounds that the paper's automatic insertion needs a **scope** perspective
  lichen does not have. That reasoning does not survive the audit: the scan's scope
  is the whole body, the one case a scope would have caught (divergent placement) is
  a `validate` refusal, and automatic insertion is *less* surface than an explicit
  operator plus a placement checker plus a sufficiency check on the author's
  placement.
- **What the spine is spelled as** — **a buffer version, not a token.** §3. The
  token landed first (`4a41a71`) and is reverted; §6.2 stays as the record of what
  it cost. `: x.to` is the annotation that expresses a version and it type-checks,
  but §3 measures that it changes **nothing observable**: `write`'s result type is
  undecided today and an undecided cell unifies with `Read`'s `.from (Buf T)`, so the
  wiring already exists and the annotation is not what unlocks it. It is worth having
  for honesty's sake, and it is not step 4's blocker: the thing downstream of the wire
  that mis-resolved is **fixed**
  ([compute-buffer-wrapper](compute-buffer-wrapper.md#the-chain-walk-what-it-measures-and-what-it-cannot-answer)),
  and a read of a write's result is now refused by name instead of being silently
  answered from an input.
- **Whether `write` must be eager** — **no.** A `: T` annotation writes the type
  cell, and neither the checker nor the lowering reads the operator's answer, so
  `ComputeOperator::Write` goes back to lazy and no `ComputeValue` variant is needed
  for a version. A token needed a *value*; a version needs only a *node*.
- **Where the spine lands** — **with the staged landing, not before it** (§9 step 4). §3's
  ABI wall: a read-back with no data path compiles and silently answers `0`, so the edge,
  its operand and its data path arrive together or not at all — and it is the staged
  landing that makes them readable, because there the buffer's type says `block[1]`.
- **Whether the read grows an operand** — **yes, either way.** §3 measured that a
  version needs it just as a token did, because `BufferReadCall`'s operands are
  `[position, index]` and the write's `ValueId` has nowhere to travel. What a version
  buys is the operand's *content*, not its absence.
- **`DP` on the buffer's type** — **decided, and the other way.** There is one nominal and
  the `DP` is one of its arguments: `Buf` is that type at `grid[1]`, and what was called
  `Shared` was never a second nominal but the same type at `block[1]` (§6.1). What remains
  is the *spelling* — writing a `DP` in a buffer type is surface the lowering does not have
  yet — and not the decision.

Still open:

1. **`claim` and `partition`.** The paper's expr for *narrowing* a `DP` — the only
   way a slice becomes private to a lane group — is a second piece of surface
   (`partition(data, P, offset)` in, `claim(data, thread[1])` for the degenerate
   case), and `offset`'s disjointness is the user's to prove in the paper as well.
   Whether lichen needs both, or only the degenerate one, is not settled. It is also the
   first thing that would want a third scope, which is what §6.1's fold would be for.
2. **Whether a closure must name the version it captures.** §3's one surviving
   objection to a buffer-valued write, and versioning makes it concrete rather than
   hypothetical: threading *is* the surface now (`read (.from w1, …)`), so a closure
   that captures a buffer captures some version, and the question is whether
   capturing a stale one is refused or silently unordered. Today a closure captures a
   handle whose writes are not versions, which is why this never arose.
3. **The enforcement question — closed for global buffers, open for a `block[1]` scope.**
   Reading a
   write's result, or an output buffer at all, is **refused by name** inside a body:
   `param_path` resolves either to a path that is not in `roles.inputs`, and
   `parallel_buffer_pos` answers nothing. That was not true until the fix recorded in
   [compute-buffer-wrapper](compute-buffer-wrapper.md#the-chain-walk-what-it-measures-and-what-it-cannot-answer);
   before it, both spellings silently answered from input position 0, and so did a read
   of the *second* of two inputs. **The write side now refuses from the other direction
   too**: a `.to` naming an input buffer, a scalar, or nothing at all is refused by
   `parallel_output_pos`, and since `roles.inputs` and `roles.outputs` are disjoint by
   construction (§3's closing paragraph) no global buffer is both read and written by one
   body — the unordered expr is *unspellable*, not merely checked. What is still open is
   the case step 4 creates: once a `block[1]` slice **is** readable by another lane, a read
   has to name the version it observes, and nothing today distinguishes "ordered after the
   write" from "not".
   `KernelBody::validate` cannot host that check — it cannot tell an input buffer from
   an output — so it belongs where the roles are known. The token never provided it
   either, because `read` did not consume it.
4. **Whether `KernelFragment` needs a `validate` at all — answered, and it does.** The
   token made this concrete and then went away: `KernelFragment` has no validate, so a
   fragment declaring `result_classes: [State]` reached `type_of`'s `unreachable!` and
   panicked instead of being refused. The revert removed that instance, and the
   question came back with a fact no body can see: a `Slice` of zero elements is
   a zero-length `OpTypeArray`, which has no type to declare and no index it could
   bound, so `KernelFragment::validate` refuses one and is kept to exactly that kind
   of fact. `barrier_scan` runs it, because the count it bounds an access by is the
   length that gate establishes. Item 3's check needs the same thing — fragment-level
   facts (`roles`, `inputs`, `outputs`) that `KernelBody::validate` cannot see — and
   joins this gate when it lands rather than starting a second one.
5. **The residual false edge.** Versioning makes the order partial and matches the
   program text, which cures the token's total order across unrelated buffers. It does
   not cure everything: a scatter writing *one* buffer at distinct indices still gets
   an edge between adjacent versions of that buffer. Breaking it needs the scan to
   prove two writes touch disjoint slots — fact 1's interval question on a
   write/write expr — and then to split the version chain, which means a body carries
   two versions at once and something must say which a later effect consumes. That is
   item 2 arriving by another route, so the two should be settled together, and
   neither is worth settling before a reordering pass exists.
6. **The refusal's diagnostic.** A contended write is today silently wrong on both
   backends (below). Making it a refusal is the fix, but the message has to name the
   index expression, the slot and the reason, and the reason differs between "the
   def cone excludes the lane" and "`UP` proves it uniform" — two causes that should
   not read as one.
7. **`KernelBody::validate` requires a used `ValueId` to be defined where the use sits —
   answered, and it does.** A use is in scope when the value is that block's parameter, an
   instruction earlier in the same block, or an instruction of a block that dominates it, and
   `validate` builds the dominator map **once** — the walk `unavoidable_blocks` already paid
   for — so the scope rule and the barrier rule read the same one. Three shapes are refused
   **by name**: a definition the use's block does not dominate, an operand placed after its
   use in one block, and a value no block lists. The acceptance case supplied the evidence
   that the rule was missing, and the rule found the class **live rather than historical**:
   `Folding::tiled`'s `Builder::constant` caches one `ValueId` per `(class, bits)` and
   ignores the block it was asked for, so the first fold's step-block constant is read by
   the second fold's step block —
   `Bin(Int, Add) reads ValueId(16), which block 2 defines, and block 2 does not dominate
   block 6`. That is the builder-state leak `55ecce4` fixed the accumulated half of, and it
   is why the tiled reduction's two-step bodies are red here. **The rule stands anyway**: a
   value's block membership is its scope in this IR
   ([lichen-compute](lichen-compute.md) §"`validate`'s four rules"), and the one emitter that
   tolerates the leak does so by accident — `spirv.rs` materialises a `Const` as a
   module-scope `OpConstant` deduped by `(class, value)`, which is why this body passed
   `spirv-val` and answered correctly rather than wrongly. The transform, not the rule, is
   what owes the fix.

## Recovered measurements

**Why this is not "just add a `__syncthreads`".** The ladder's histogram — 64
elements into 3 buckets, answered `(1, 1, 1)` on **both** backends with no
diagnostic and no refusal
([gpu-algorithms-ladder](gpu-algorithms-ladder.md#5-a-scatter-accumulate-is-silently-wrong))
— is a contended write that is *expressible today*: the same shape a naive
`SliceWriteCall` at index `0` has. That is the sharpest available evidence that the missing
half is a **judgement** and not an instruction: the program runs, both backends
agree, and the answer is wrong. A barrier would not have fixed it.

What notices it is the def-cone query of §4: that write's index has a cone
that excludes the lane parameter, so the write is the same slot in all 64 lanes,
so it is contended, so it is refused. **The original note credited `DP` with this,
and that was too strong** — `UP` would notice *more* of them (`i % 1`, and any
expression `UP` can prove uniform whose cone still contains the lane), which is
why §4 calls the perspective attribute a precision upgrade and §10 item 6 asks for
the two causes to be told apart in the diagnostic.
