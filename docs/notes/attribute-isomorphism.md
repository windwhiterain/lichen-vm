# The attribute isomorphism — handoff

> Status: **the codomain is closed**. Branch `spec-literals` (base `dev`), working
> tree clean, not merged. The rule is landed in the language spec, the attribute-specific
> record machinery is gone, and mechanism 2b is closed: `unify` consumes a frozen
> signature by its **own refs** instead of a materialized copy. Compute is 237/0, and the
> two language-server reds the hunt started with now pass as a consequence. One red
> remains and it is **pre-existing**: `lichen-language`'s lib test
> `sort_op_sorts_a_usize_array` fails at the unmodified tree too (measured by reverting
> the slice and re-running it).
>
> What this note is: the state a successor resumes from — what the rule is, what
> landed, the exact readings, the two mechanisms with the probes that found them,
> and the traps that cost this session its time.
>
> Points at: `docs/language-spec.md` §1 and §3, `lichen-highlevel`
> (`checker`, `checker/annotations`, `checker/lambda`, `shape`), `lichen-lowlevel`
> (`apply`, `static_module`, `static_module/apply`), `lichen-compute`
> (`compute.rs` `compile_fragment`), and the notes
> [array-attribute-view](array-attribute-view.md),
> [kernel-parameter-class](kernel-parameter-class.md),
> [apply-type-side-check](apply-type-side-check.md).

## The rule

Every position of an expression is an expression: the value, the type, and one
per attribute (`#`, `?`, `!`). The **type position is the model** for every
attribute: it holds an expression, it is unified, it rides the function's
parameter and return expression so an applied clone re-materializes it per call,
and an absent position is an undecided cell. A value/type unification pairs the
attribute positions **one-to-one in canonical order**, and the pairing is
recursive into a compound's element slots. There is **no attribute-specific
record, table or reference**.

A literal decides its own attributes: a constant states `# 0` and the wildcard
for `?`/`!`; `_` is undecided in every attribute, one hole per position. An
annotation is syntax — what it spells is replaced and unified; its own order
decides relaxation. `@any` states nothing, at any position.

## What landed

| commit | change |
|---|---|
| `1d8e6d5f` | the rule in the spec; `Function`/`StaticFunction.return_marker`/`return_field`, `attribute_reference`/`attr_refs`, the static mirrors and their persisted bytes deleted (**artifact format 21**); a value literal decides its attributes, `_` is undecided in every position |
| `1911f8c8` | `pair` → `expr` everywhere: the record is an expression |
| `5dc79926` | `unify_parameter_slots`: the non-reporting static apply binds the frozen clone to the argument |
| `7b93eb1a` | a reference in an attribute names the field's own slot |
| `6521974a` | one materialization per call: the signature remap is shared by the parameter and the return copy (measured: no change alone) |
| `2721a1ab` | a reference names the field record's attribute position, not a result cell |
| `592b0854` | the non-reporting static apply keeps the signature's record (reverted the earlier bind; see the trap below) |
| `5d585047` | the unify covers **every** position, the type included (the `slot == 1` skip was a masking and is gone) |
| `cdb1d638` | `annotation_value_slot` uses the expression's own node, no class representative |
| `3577398e` | the return copy rides the **dynamic clone rule** (`static_node_apply`), per `kernel-parameter-class.md` |
| `017f3db5` | a per-call cell enters the signature remap, so the mirror rule resolves a reference to this call's clone — **and `a_write_returns...` passes** |
| `0ae17d02` | `shape`'s low-type readers cross the frozen boundary (`static_read`) |
| `9a5406c0` | a struct instance's provided perspective is the meet of its **fields**; its type expression states a requirement, not a perspective — **and `a_read_takes...` passes** |
| `619a2606` | `unify`'s fill refuses an answer that still holds an **open frozen position**, so a template's skeleton binds nothing — **and both `jit_inline` tests pass** |
| `fc834b67`, `c0802cab`, `bdde4d8d` | the comment budget: every section the branch added fits its 120-character preamble limit |
| `484e9fa2` | a static node is **read directly**, never copied into a dynamic node to be read — the invariant now sits at `Module::node_value` and `shape::node_value`; the copy walk of `a524e578`/`f0f8f668` is gone |
| `02d31d8f` | the two pinned renderings follow: a direct read of an open codomain is `raw none` |
| `4c0f1b50` | an attribute holding its own **default** (`AttrExt::holds_default`, the value an absent occurrence reads) is left out of a render — **and the literal reads `1: Int` again** |
| `64af5927` | `unify` consumes a frozen signature by its **own refs** (`function_cells` answers `AnyNodeId`s, `materialize_static_signature` deleted) — **and the kernel codomain closes: compute 237/0, both language-server reds pass** |

## Readings (the tool's own `test result:` lines)

| suite | result |
|---|---|
| `lichen-highlevel --test highlevel` | **136 passed; 0 failed** |
| `lichen-language --test language` | **275 passed; 0 failed** |
| `lichen-language --lib` | 69 passed; **1 failed** — pre-existing (`sort_op`) |
| `lichen-compute --test lichen_compute` | **237 passed; 0 failed; 1 ignored** |
| `lichen-lowlevel` | **164 passed; 0 failed; 1 ignored** |
| `lichen-language-server` | **32 passed; 0 failed** |

The kernel codomain and both language-server reds are closed by mechanism 2b's fix; the
only red left is the pre-existing `sort_op_sorts_a_usize_array`, which fails identically at
the base tree (measured) and sits outside this branch's files.

## Closed mechanism 1 — a read's lane-varying index stays warp-uniform

The read wrapper has no `#` annotation; its result must be the meet of the
buffer's and the index's perspectives, so a lane-varying index must be refused
with "expected 32, found 1".

Measured with a temporary print in `check_app`'s certificate branch: the read
call's applied return expr states **no** attribute position at all (its `#` slot
is absent), so the call states nothing, and the demand's provider is the combine
over the call's children. That combine was **undecided**, because
`persp_combine_children` gave an `Instantiate` `[type_expr, value]`: the type
expression `(Read _)` contributes the placeholder's *hole* cell, and an undecided
contribution makes the whole `Gcd` undecided, so the demand's unify bound the
hole and the lane-varying index was never checked.

`9a5406c0`: an `Instantiate` contributes **its fields**, like a `Record`; the type
expression is the value's requirement, never a perspective the value provides.
Measured: the test passes, compute 232 → 233, nothing else moves.

Tried and rejected while finding it:

- `stated_attributes` skipping a wildcard-valued slot (the previous session): no
  change — the read's return expr has no `#` position at all.
- `attribute_value` returning `None` for a wildcard: it turns a wildcard
  certificate into a hole, which the spec says it is, but no test exercises it and
  it closed nothing here. Reverted.
- A probe that deep-evaluates a certificate **changes what the real
  `stated_attributes` then sees** (the wildcard becomes visible and the
  certificate disappears). While probing, print before the real read, never on the
  same node as it.

## Closed mechanism 2a — a jitted kernel's class reads undecided

`compute.jit` refuses with `UNDECIDED_DOMAIN` for
`k = compute.jit (x => helper x + 1)`, which must check.

**Bisected: `5d585047` is the commit that opens it** (green at `aeac9efe`, red at
`5d585047`, on `jit_inline_lichen_function`). The commit removed the `slot == 1`
skip from `unify_parameter_slots` — the *non-reporting* static apply.

Measured, in order:

- `compile_fragment` (`crates/lichen-compute/src/compute.rs`): the kernel's
  `param_type` is a dynamic node (`NodeId(166)`) whose value is a **five-position
  expression array whose items are all static refs** into the frozen module; its
  slot 0 is `Static(1248)`, whose frozen state is
  `value=false operation=false runned=false evaluated_deep=undecided, root value=false`.
  So the type's denotation is a frozen hole: `low_type_of_slot` and
  `struct_fields_of_slot` both read nothing, `DomainStatement::Absent`, and
  `kernel_domain` refuses.
- The write that installs that skeleton is a `unify_inner` fill
  (`equality.rs`'s node-less arm), with a **static** side carrying the frozen
  expression as its value: `PROBE write node=166 root=166`. The class root is
  `207` by the time `compile_fragment` asks.
- `unify_parameter_slots`'s type position pairs (measured, `slot=1`):
  `(78, 42)`, `(335, 299)`, `(406, 170)`, `(706, 398)`; at the last pair
  `706`'s value is a five-position array `[dyn(decided), dyn(decided), dyn(open),
  dyn(open), dyn(open)]` and `398` has none — the signature clone's type
  expression against the argument's.
- The checkpoint is the *frozen signature's* type expression: unifying the type
  position copies it into the caller's type cell, and its open interior is frozen
  cells that nothing can bind. Before `5d585047` the caller's type cell stayed
  free and the body's own inference (`helper x`) bound it to `Int`.

**What landed: B, at the fill.** `619a2606` gives `unify`'s node-less fill a
`frozen_answer_is_open` guard: an answer that still holds a static ref whose
frozen node holds nothing is not an answer, so it fills nothing. Measured: both
`jit_inline` tests pass, compute 233/4 → 235/2, highlevel 136/0 and language 274/1
unchanged. Candidate A (a skip in the non-reporting binding) was not needed: the
rule is position-agnostic, so the type is not special-cased.

Measured and **rejected**: broadening the guard to *dynamic* open items as well
(the same 235/2, so the two remaining failures are not a fill). Candidate C (the
signature clone refusing an open value) was not tried in that form: the open
positions are the *attribute* ones while the denotation is decided, so dropping
the whole value would drop the signature's answer. Candidate D (the eval-time
materialization reusing the check-time one) was tried with
`!self.applied_exprs.contains_key(&node)` and changed nothing — measured,
**every** `apply_function_type` in both remaining tests is `report=false`, so
`materialize_static_return_apply` is the only static type apply there is; there is
no second owner to defer to.

## Mechanism 2b — a kernel's codomain stays open (the state of the hunt)

`a_float_domain_...` fails on `type_is_int` after `p : tuple{Int, Float} => p(0)`;
`a_jit_kernel_crosses...` fails on `type_is_float`/`type_is_int` over its five cases.
Both tests pass at `aeac9efe`, so `5d585047` opened them: it made the type position ride
the same rule as every other (the isomorphism), and the clone that rule installs is one
nothing binds.

### The verdict: a static node is read directly

A frozen position is **read in its own module**, exactly as a dynamic one, and never
copied into a dynamic node in order to be read. The rule sits at both readers
(`Module::node_value`, `shape::node_value`) as their `# Invariant`; a copy gives the
position a fresh class, so one value reads as several.

Tried the opposite first (`a524e578`, `f0f8f668`: a copy walk that materialized every
open frozen position). It made the codomain read (compute 237/0, the two pinned
renderings back to `(raw Kernel, Int, Int)`), and it was still wrong — the copy is what
has to go, not the read. `484e9fa2` reverts it; `02d31d8f` restores the two pins to
`raw none`, which is what a direct read sees while the codomain is open.

### What the open cell is

- A probe in `type_head` prints, for the failing program, `node=NodeId(487) items=5
  item0=Dynamic(NodeId(975)) value=false`: the read node is a five-position
  **expression** whose value position is an empty cell. Nothing is frozen there.
- A probe in `Module::add_node` catches 975's creation: `Module::node_apply` →
  `function::value_apply`. And a clone probe prints its instantiation as
  `value=false asserts=0` — a **type-side** instantiation of an assert-free function.
- A probe in `check_app` lists the main program's calls binding cells `66` and `368`, so
  `487` is **not a call's cell**. The static value side does bind (measured:
  `cell=368 codomain=708 decided=[true, true, true, true, true]`), so the open read is
  the type `k.O` carries **at check time**, before any value apply runs.
- The binding itself is one node: `ExprKind::TypeFunction` holds the two binding
  `ExprId`s and a name is one `ExprId`, so the annotation and the value use share
  `state[e].term`. And the assert is the binder, at the right level:
  `check_type_element` → `type_denotation` returns the type expression's **term**, so
  `I -> O` descends into `O`'s **value** position.

### Measured and rejected (each on its own build)

- A **shared remap** between the two sides of one call (`call_clones`, keyed by the
  value-side node, consumed by the other side): **nine tests break** — a call *site* is
  instantiated many times, and the second evaluation must not reuse the first's cells.
- Deferring the dynamic type-side bind to the value side: two tests break and the
  codomain stays open.
- Running the type side's asserts on its own clones (`ParameterCheck::Type` instead of
  `Skip`): nothing moves (235/2, language 275/0, highlevel 136/0).
- Reusing the check-time instantiation at eval (`apply_value_applies` reverse pairing):
  two tests break; every evaluation needs its own cells.
- Binding the call's cell inside `materialize_static_return_apply`: **175 compute tests
  fail** — the type side would bind before the value side. The static path's rule ("the
  value side owns the bind when nothing reports") stands.
- Disabling the `619a2606` fill guard: no change; the two `jit_inline` tests stay green.
- A probe on `apply_type_with` prints the cells the type side binds (`286`, `554`, `786`,
  `919`) — none of them is the failing node, so no codomain bind touches it.

### The read, measured (this is the last link)

The failing read is the root's **type expression**, and instrumentation printed its whole
neighbourhood without evaluating anything before the read (every print used the raw
readers: `node_value`, `node_operation`, `node_origin`, `equality_representative`):

- The root type is a five-position expression `[value, type, attribute…]` built by
  `Checker::check_app`'s `fresh_cell` — the stack reaches `run_at` through
  `build_in_attr_native`, so it is the program's own cell.
- Its **value position is a lazy `Index(target, 0)`**, where `target` is the `.O` slot of
  the read `k.O`. The target holds five positions: `[value, type, attribute…]`.
- On the failing attempt, that target's position 0 is **`AnyNodeId::Static`** — a ref into
  the prelude module (`ModuleKey(1)`, `LocalNodeId(1264)`). On a later attempt the same
  position reports a *dynamic* marker, which the class machinery then binds.
- `StaticModule::read(1264)` answers `value = false, operation = false`: the frozen node is
  a **bare empty cell** — not a residual operation, not a decided marker, and no operation
  for anything to run. It can never be filled: a frozen module is immutable.
- The reader is therefore right to answer undecided, and undecided for ever. Evaluating the
  Index by hand (stamp cleared, target and subscript both decided) still answers nothing,
  because the class short circuit reads the representative's slot, which the frozen ref
  never fills.

So the codomain is not left open by a *reader*: the prelude's `(K _)` annotation froze the
`.O` position as an empty cell, and the type-side instantiation **referenced that frozen ref
in place** — [`Instantiation::node_of`]'s documented rule — so no per-call binding can
reach it. The value side has dynamic clones to bind; the type side has a static ref.

This is a different act from the copy walk that was rejected: the walk copied a frozen
position *in order to read it*; the gap here is that the instantiation must materialize a
ref *in order to bind it* per call. Both must be measured before either is kept — the
rejected one made the two codomain tests pass and was still wrong.

### The seam: no clone site ever sees the ref

Three probes asked every clone site that could keep a ref, by index (`1264`, which also
prints `same = <module equals the applied module>`):

- `function::value_apply`'s array arm, the keep branch;
- `static_module::apply::static_remap_value`'s `node => node` arm;
- `clone_static_signature_node`'s residual keep.

**None of them fires.** The open ref never passes through a clone or a remap at all: it
reaches the read **inside a value `materialize_leaf` copied**. `checker::slot_read`'s
`resolved` arm calls `shape::field_type` → `as_dynamic` → `materialize_leaf`, which is
`add_node(block, None, static_read(sref))` — a verbatim copy of the frozen `.O` slot whose
item 0 *is* `Static(1264)`. A ref in a copied array is never remapped, so the open cell
travels into the read and stays dead.

So the codomain sits on a **seam between two rules this branch keeps**:

- `Module::node_value` (and `shape::node_value`): *a static node is read directly, never
  copied into a dynamic node to be read*;
- `checker::slot_read`: *a frozen field type is materialized as a leaf, so it is decided
  the same way*.

`materialize_leaf` is the one copy the branch licenses, and it copies **verbatim**, so an
open position inside it is a dead ref. The two candidate fixes, both needing measurement:

1. **The leaf's open positions become this call's cells.** Smallest change, but the leaf is
   built once per read *site* at check time, so the cells would be shared by every
   evaluation of that site — the aliasing the shared-remap attempt was rejected for.
2. **The frozen artifact refuses an open exported position.** The freeze invariant says a
   solved module holds an answer or a residual; a cell with *neither* an answer nor an
   operation is a third state nothing can resolve. If the prelude's `(K _)` wildcard cell
   must not be exported, the freeze (or the export) has to close it.

Measured and neutral this round (each build reverted, tree clean):

- an open frozen position inside a **cloned value** becomes this call's cell
  (`function::value_apply`): no change — the site is not on this path;
- a **dependency's** open ref becomes this call's cell (`static_remap_value`): no change.

### The verdict: undecided cells live only inside functions

The maintainer's verdict, which supersedes both candidates above: a frozen module **may**
hold undecided cells, but only **inside functions** — because the only way another module
reaches them is to instantiate the function, and the clone unfreezes every undecided node
into that call's own cells, so unify proceeds normally.

That is the freeze-side half of one boundary theorem; the consumer-side half is its dual:

- **Freeze side**: an undecided cell is legal only inside a function; the module-level
  export surface is decided.
- **Crossing side**: the frozen→dynamic crossing *is* instantiation and nothing else: every
  undecided position unfreezes into a cell of the crossing context (per call, in the
  remap); decided positions cross as refs — read directly, immutable, shared. A verbatim
  payload copy is legal **only** for a payload that is decided everywhere.

Why this is the SPEC-faithful reading:

- The spec has no frozen modules; freezing is an implementation artefact whose one demand is
  that importing a library means the same as inlining it. Inline, every call owns its own
  cell for every undecided position of the template — that is what an apply *is*. An open
  position crossed as a dead ref is a position no caller can ever write: not undecided but
  undecidable, which changes the semantics.
- The theorem turns `static_remap_value`'s existing comment ("a dependency's ref — concrete
  by construction") from an assumption into a consequence: open cells live only inside
  functions, functions are only crossed by clones, so a raw cross-module ref is decided and
  keeping it is safe.
- It composes with every standing verdict: *read directly* governs **reads** (`node_value`
  of a static ref reads in its own module, no copy); this theorem governs **crossings** (a
  bindable handle the dynamic world must own). The rejected copy-walk turned a read into a
  copy; this never touches reads.
- It is the attribute isomorphism at the boundary: value, type and every attribute position
  cross by one rule. The codomain bug is one position (`.O`'s value) crossing by another —
  a verbatim copy.

Consequences for the two candidates: fix 2 was this theorem stated too strongly — open
cells inside bodies and signatures are the everyday `_` and parameter cells, and refusing
to freeze them refuses the language; what is illegal is an open cell *outside* a function.
Fix 1 is a patch on the messenger: under the theorem, a leaf that can meet an open position
proves an upstream instantiation leaked (copied where it had to clone); fix the leaking
door and the patch is dead code. The theorem also names the smell: four crossing doors with
slightly different item rules (`node_apply`/`value_apply`, `static_node_apply`/
`static_remap_value`, `clone_static_signature_node`, `materialize_leaf`) — one rule, one
helper, no drift.

### Closed: `unify` consumes the frozen signature directly

The maintainer's ruling: **every node consumer (unify, evaluation) supports static nodes
and never clones.** The smuggle probe, re-run once the executor came back, named the door
in one stack: `unify_inner`'s function arm (the `[parameter, return]` descent) →
`function_cells` → `materialize_static_signature` → `materialize_leaf`. So it was unify
itself that copied a frozen signature into a leaf, and the copy carried the open `.O` cell
as a dead `Static` ref.

The fix is the ruling applied at that door:

- `function_cells` answers `(AnyNodeId, AnyNodeId)` and, for a frozen function, hands back
  the signature's **own refs** — no copy;
- the function arm descends with `Side::of(ref, node_value(ref))`, which compares a frozen
  side by value and never writes it, and `UnifyStep` takes `node_or_default(ref)`;
- `materialize_static_signature` is **deleted**, so that door no longer exists and nothing
  can drift back to it.

Readings (the tools' own lines): compute **237 passed; 0 failed; 1 ignored**, language
**275/0** (integration), highlevel **136/0**, lowlevel **164/0; 1 ignored**,
language-server **32/0** — including the two reds the hunt started with
(`a_lazy_binding_reports_type_but_no_value`,
`compute_wrapper_functions_hover_with_named_type_variables`), which the decided codomain
fixed as a consequence. The two pinned renderings follow the closure: the kernel type
renders `(raw Kernel, Int, Int)` / `(raw Kernel, tuple{Int, Int}, Int)`, not `raw none`.

One red remains and it is **pre-existing**: `lichen-language`'s lib test
`sort_op_sorts_a_usize_array` fails too when the slice is reverted to the base tree
(measured: patch saved, the two files checked out, the test re-run, the patch applied
back). It sits on `SortOp::run`, untouched by this branch.

### Next

The codomain hunt is closed. What remains is the landing: squash the slice, check
`git diff dev...HEAD --name-only` names only this slice's files, then merge to `dev` and
delete the branch. The `sort_op` red is outside this branch's files and predates it; it
belongs to its own slice.

Instruments that answered the question, for the next session: a per-module log
(`Module::probe_log`) rather than a thread-local one — node ids repeat in every module, and
a thread-local log silently mixed the prelude's and the program's entries for three builds;
the raw readers above, so a print never answers the question it is asked; and a keep-site
probe keyed on the ref's index, which is what proved the ref is *not* cloned.

## Traps (each cost time this session)

- **Do not special-case an attribute against the type.** A `slot == 1` skip in
  `unify_parameter_slots` made five pinned renderings pass and was rejected: it
  binds the value and the attributes but not the type. The same applies to a
  "combine ignores an undecided contribution" rule.
- **The class invariant is the reading.** Every member of a class *is* the same
  expression, so a node-level read is the class read; `annotation_value_slot`
  must not select a representative.
- **A static apply mirrors the dynamic one.** `kernel-parameter-class.md`'s
  load-bearing statement; the signature clone carries every per-call cell into
  the remap, and residual operations keep their frozen reference.
- **A frozen position is read, not copied.** A static node is read in its own module
  exactly as a dynamic one; copying it into a dynamic node first gives every position
  a fresh class, so one value reads as several. The rule sits at
  `Module::node_value` and `shape::node_value`, and the copy walk that broke it
  (`a524e578`, `f0f8f668`) is reverted.
- **One call, one clone — and a call is instantiated three times.** The check-time type
  apply, the eval-time type apply (`apply_callee_type`) and the value apply each build
  their own graph, so the codomain's denotation cell exists three times and the assert
  fills one. Reusing a graph across those applies is measured wrong (every evaluation
  needs its own cells), so the sharing to look for sits inside the **binding**: the
  checker should give one node for `O = _`'s two uses.
- **The doc/code pair moves together.** This branch is **not mergeable** while any
  test is red, because the spec on this branch states the rule the code keeps.
- **Bisect before theorising.** Mechanism 2 cost this session hours of reading
  `unify`, `static_node_apply` and the clone walk; `git checkout <commit>` plus
  one filtered test named the culprit (`5d585047`) in four builds. The commits are
  self-contained and the failing test is cheap, so the bisect is the cheapest
  instrument in this tree.
- **A probe's own evaluation is a write.** `evaluate_node_deep` and
  `attribute_value` materialize a wildcard / an answer, so a print placed *before*
  the code under test changes what that code reads. Both wrong turns above came
  from a probe that ran first.
- **`--exact` in this crate needs the crate prefix**: the suite prints
  `lichen_compute::<module>::<test>`, so a bare `<module>::<test>` filters to **0
  tests** and reads as `ok`.

## Verify

```pwsh
$env:CARGO_TARGET_DIR='C:\resource\lichen-vm\.worktrees\spec-literals\target'
cargo check --manifest-path C:\resource\lichen-vm\.worktrees\spec-literals\Cargo.toml --workspace --all-targets
cargo test --manifest-path C:\resource\lichen-vm\.worktrees\spec-literals\Cargo.toml -p lichen-highlevel --test highlevel
cargo test --manifest-path C:\resource\lichen-vm\.worktrees\spec-literals\Cargo.toml -p lichen-language --test language
cargo test --manifest-path C:\resource\lichen-vm\.worktrees\spec-literals\Cargo.toml -p lichen-compute --test lichen_compute
```
