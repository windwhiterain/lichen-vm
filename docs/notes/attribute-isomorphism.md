# The attribute isomorphism — handoff

> Status: **in progress**, handed over. Branch `spec-literals` (base `dev`), HEAD
> `02d31d8f`, working tree clean, not merged. The rule is landed in the language
> spec and the attribute-specific record machinery is gone. Three reds remain: the
> kernel codomain (mechanism 2b, with the verdict that governs it), and two
> language-server tests that read the same at this session's start commit.
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

## Readings (the tool's own `test result:` lines)

| suite | result |
|---|---|
| `lichen-highlevel --test highlevel` | **136 passed; 0 failed** |
| `lichen-language --test language` | **275 passed; 0 failed** |
| `lichen-compute --test lichen_compute` | **235 passed; 2 failed; 1 ignored** |
| `lichen-lowlevel` | **164 passed; 0 failed; 1 ignored** |

Red:

- `compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches` and
  `compute::a_jit_kernel_crosses_the_two_classes_both_ways` — the kernel codomain;
  mechanism 2b carries the measurements and the verdict that governs the next attempt.
- `language_server::statement_values::a_lazy_binding_reports_type_but_no_value` and
  `language_server::statement_values::compute_wrapper_functions_hover_with_named_type_variables`
  — both read the same at `359b18c4`, the commit this session started from, so neither
  is this work's regression (measured in mechanism 2b).

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

## Mechanism 2b — a kernel's codomain stays open (the verdict that governs it)

`a_float_domain_...` fails on `type_is_int` after `p : tuple{Int, Float} => p(0)`;
`a_jit_kernel_crosses...` fails on `type_is_float`/`type_is_int` over its five cases.
Both read the launch's result type, which is `k.O` — the kernel struct's codomain
field.

Measured (`type_head` on the root type node): the result type is a five-position type
expression whose slots 1–4 are decided and whose slot 0 — the denotation — is a free
cell; a fill log over the whole program never named it, so nothing ever unified it.
Both tests pass at `aeac9efe`, so this is the second casualty of `5d585047`: the
non-reporting binding now unifies the type position, and the frozen signature's
return expression it meets names a **template hole** in its value position. With
`619a2606` that skeleton no longer fills anything, but the pair is never made either
(the node-less arm returns before descending), so the codomain stays open.

### The verdict: a static node is read directly

A frozen position is **read in its own module**, exactly as a dynamic one, and never
copied into a dynamic node in order to be read. The rule now sits at both readers
(`Module::node_value`, `shape::node_value`) as their `# Invariant`. A copy gives the
position a fresh class, so one value reads as several.

This was learned by trying the opposite (`a524e578`, `f0f8f668`): a copy walk that
materialized every open frozen position into a fresh cell. It made the codomain read
(`compute` 237/0, the two pinned renderings back to `(raw Kernel, Int, Int)`), and it
was still wrong — the copy is what has to go, not the read. Both commits are reverted
by `484e9fa2`, and `02d31d8f` restores the two pins to `raw none`, which is what a
direct read sees while the codomain is open.

So the codomain closes **where the type side installs its cells**, not where a reader
reads: the return type a call binds has to be this call's clone — a fresh cell from
the apply's own remap — so a plain direct read of it is decided. The measurements
that bound that work: `report=false` for every `apply_function_type` in these
programs, so `materialize_static_return_apply` is the only static type apply there is
(candidate D has no second owner to defer to), and cloning every `undecided()` ref in
`clone_static_signature_node` changes nothing (candidate F).

### The two language-server reds are older than this session

`lichen-language-server --test language_server` carries two failures, and **both read
the same at `359b18c4`**, the commit this session started from:

- `statement_values::a_lazy_binding_reports_type_but_no_value` aborts with
  `memory allocation of 38654705664 bytes failed`: the paradox program's type walks a
  chain the printer never stops building. Measured with the copy walk present and
  removed alike, so no clone-at-read is involved.
- `statement_values::compute_wrapper_functions_hover_with_named_type_variables` counts
  **198** type variables where it pins **24** — also 198 at `359b18c4`, so this is not
  the copy walk either: it is the attribute-widened expression the checker builds, 
  whose every position is a cell of its own.

Both need their own investigation, and neither re-opens the verdict above.

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
  (`a524e578`, `f0f8f668`) is reverted. The codomain fix has to happen in the
  **type-side apply**, where a per-call clone is the natural product, not in a reader.
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
