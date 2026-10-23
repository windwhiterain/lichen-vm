# The array attribute view: sparse storage, full-width reading

Status: **complete** — stages 1–5 landed (`df24812c`, `58ca01c2`, this round's stage-4
commit, and the full-width generation): the storage and registry, the checker's
layout-carrying builders and readers, the default array, the unify wiring by canonical
index, the `type_of`-shaped `perspective_of` spelling that replaced the call-site
references, and the annotated expr generated full width. The measured results are at the
end of this note.

## The problem, in the superior's words

> 本质上是AST期间确定的稀疏unify映射无法递归进类型，或者说无法递归进入任何attribute。
> 重构方案：让lowlevel的array有一个稀疏表指针，将该array映射到全宽attribute array，
> lowlevel global再有一个default attribute array，稀疏中的空洞就去default找。
> 这样高层就把所有东西当全宽attributes

The check-time attribute mapping is *sparse*: each expression's schema tail says
which attributes it carries, and the expr stores them densely
(`[value, type, attrs…]`, `attr_slot(i) = 2 + i`). Two consequences measured this
session:

- An expr cannot be made full width. An expression expr is also a *type value*
  when the expression is a type (`[shape, kind]`, two items), so a five-wide expr
  breaks every type-level function (`expected raw[raw[…]], TypeStruct, …`), and
  the expr readers that assumed 2..=3 elements silently returned nothing
  (`f = x => x; f 1` answered `?a: ?a`).
- The *runtime* expr unify deliberately excludes the attribute positions
  (`apply_parameter_check` unifies slots 0..2), so a caller's attribute values
  never reach a callee's cells — the asymmetry with the type slot that makes
  `type_of` work and a `perspective_of` written the same way fail (it answers an
  undecided `?a`; `x # p` binds only when the operand has a provider, and a
  parameter has none).

## The design

**Every lowlevel array carries a mandatory attribute-layout id** naming one entry
of a lowering-time index-array registry; the entry maps the array's slots to
canonical attribute indices. A module-global *default attribute array* supplies
the holes. The highlevel then reads every expr and every element slot as a
full-width attribute array: positions it does not carry read the default.

The underlying items stay as they are, so a type value remains a two-item
`[shape, kind]` array while its *attribute view* is full width. That is what
removes the dual-use conflict that defeated the wide-expr attempt.

## The encoding (the superior's, second round)

The sparse table is **not a lichen object**: it is a set of **index arrays
predefined at lowering time**. Each lowlevel array **chooses one** of them, and
lowlevel unification **wires the attribute positions according to that choice**.
The choice is **mandatory, not optional** — every array states which attribute
layout it carries.

Concretely:

1. **The table.** A module-level registry of index arrays (the layouts), filled by
   the highlevel at lowering: `Vec<Box<[u32]>>`, one entry per distinct schema
   tail. Entry `0` is the empty layout (no attributes — a plain value array, and
   what every order-0 construct picks). An entry lists, in slot order, the
   canonical attribute index each attribute slot of the value carries.
2. **The array.** `LowValue::Array(ArrayValue)` — a payload handle plus a
   mandatory `layout: u32` (`ArrayValue::new` = layout `0`, `with_layout` = a
   registered id; `Deref<Target = AnyHandle<[ArrayItem]>>` keeps every
   `unsafe { array.items() }` read unchanged). The id is a *small integer*, not a
   heap array per value, so the storage stays O(1) and the table is shared.
   `Module::register_attribute_layout` de-duplicates by equality (O(1)) and
   requires strictly increasing canonical indices; `Module::array_attribute`
   binary-searches them, so a schema tail reaches the registry in **canonical
   order**, never merely slot order.
3. **The default.** A module-global **default attribute array**: the full-width
   slots an array's layout does not cover. Every canonical attribute the layout
   omits is read from the default, so the highlevel can treat every value as
   full-width.
4. **Unify.** Lowlevel unification of two arrays wires the attributes **by
   canonical index**: attribute `i` of one array meets attribute `i` of the other
   (each found through its own layout), and a side whose layout omits `i` meets
   the default's slot for `i`. That is the recursion into attributes the current
   design lacks; the plain value and type positions keep unifying as they do.
5. **The highlevel.** The checker's schema tails pick the layout: an expr with
   tail `[Perspective]` registers (or reuses) the layout `[0]`, and its attribute
   readers resolve a canonical attribute index to the slot the layout gives.
   `type_of`-style reading (`x # p` binding to the parameter's attribute cell) and
   the per-call flow then follow from the lowlevel wiring, not from checker-side
   reference tables.
6. **Persistence.** The artifact carries the layout registry and each array's
   layout id (`ARTIFACT_FORMAT_VERSION` bumps); freeze and the static module copy
   both.

## Stages

1. `lichen-lowlevel`: the payload field, the module default, the accessor, and the
   mechanical 241-site update; no behaviour change (all suites green).
2. `lichen-highlevel`: the attribute readers rerouted through the accessor, still
   with the dense tails; no behaviour change.
3. The lowlevel array unify (`crates/lichen-lowlevel/src/equality.rs`, the two
   `LowValue::Array` arms) wires attributes by canonical index through each
   array's layout, a side that omits an index meeting the default. Behaviour
   change: a caller's attributes flow into the callee's cells, so
   `perspective_of = x => {p = _; x # p; p}` (`type_of`'s shape) starts to work.
4. The artifact carries the registry and the default — **landed with stage 1**
   (`ARTIFACT_FORMAT_VERSION` 15; `StaticModule` copies both, and the default's
   items count as roots, so a freeze cannot produce an artifact whose default
   names a node it lacks). What remains: the compute vocabulary's
   `perspective_of` spelling replacing the call-site references (`# @any`/
   `# x.to`), which are then deleted.

## The superior's verdicts (recorded)

- **The default attribute array holds wildcard slots.** A hole means "no claim",
  and unification skips it, so a hole never couples two unrelated values.
- **No runtime assert for a flowed attribute.** The design's point is that the
  attribute slot is isomorphic to the type slot, so **the checker that catches a
  type error catches an attribute error**: the same unification that decides a
  type mismatch decides an attribute mismatch, statically. The checking model is
  therefore unchanged — what changes is that the attribute slots are *wired* like
  type slots, so the checker sees the values. Stage 3 measures whether this holds
  (`type_of`'s shape and `perspective_of`'s must both work; the existing
  requirement tests must keep refusing what they refuse).

## Measured with stages 1–3

- `lichen-lowlevel` 164 passed / 0 failed (1 ignored), `lichen-highlevel` 136 / 0,
  `lichen-language --test language` 275 / 0, and the artifact subset 16 / 0; the
  compute perspective filter 4 / 0. Comments and format gates clean.
- `cargo test -p lichen-lowlevel` did **not compile on the base** `dev`: eight
  `Function { … }` literals in `crates/lichen-lowlevel/tests/basic/{assert,gc_safety,recursion,main}.rs`
  named none of the five function-attribute fields. Stage 1 repaired them
  (`None`); that repair is independent of the attribute view.
- Stages 2–3 changed behaviour only where they were meant to: the two lane-varying
  refusals (`a_lane_varying_buffer_is_refused`, `a_lane_varying_operand_is_refused`)
  had *regressed* when the copy paths (`function.rs`'s clone, the static re-homes)
  rebuilt an array with `.into()` and dropped its layout — every hole then read as a
  wildcard and the refusal vanished. Preserving the layout in those copies restored
  them; the compute suite is 252 passed (up from 250) plus the pre-existing
  `pack_placement` failure, and the artifact version moved to 16 because a stage-1
  artifact's slots carry no layout.
- The accessor had to learn the static side: a loaded artifact's layout ids index the
  *frozen* module's registry, not the live one's.

## Stage 4: the explicit spelling

The `type_of`-shaped function is the spelling:

```lichen
perspective_of = x => {p = _; x # p; p}
read  = (x : (Read _)) => ($read(x.from, x.at) # (perspective_of x))
write = (x : (Write _)) => ($write(x.to, x.at, x.value) # (perspective_of x.to))
```

Two fixes made it work, neither a name match:

- **An annotation's computed value is not validated in the body.** A call's answer is not
  decidable where the body is checked, so the provider unify is skipped for a computed
  attribute value; the check belongs to the call site the certificate reaches.
- **A call's declared slot is its class's representative.** An apply node holds no value
  of its own — the merge wrote the `[value, type]` expr onto the class's representative —
  so `Checker::annotation_value_slot` forces the walk and names the representative. Before
  that, an annotation whose value was a call compared the call's answer array against the
  number (`expected raw[?a, ?b], found 2`).

The reference is then recognized by **shape** (`Checker::attribute_reference`): a call
whose argument is the enclosing parameter is a reference to the whole argument, and one
whose argument is a named field of it is a reference to that field. The slot is a fresh
cell and the call site resolves it — the whole-parameter case through the certificate
hole's fallback (the argument's provided slot), the field case through
the applied clone's return expr.

`# @any` reverted to the wildcard's plain meaning (a stated wildcard certificate is
copied like any decided one); the `@any`-on-a-return overload is gone.

Measured on the four probes and the suites: a `# warp` buffer read at a uniform index
answers `?a # 32`, at a lane-varying index it is refused (`expected 32, found 1`); a write
from a `# 0` buffer answers `?a # 0` and from a `# warp` buffer it is refused
(`expected 0, found 32`). The artifact format stays 16 — no persisted shape changed.

**Later: the whole-argument spelling was retired.** `read`'s line was
`# (perspective_of x)` because the annotation gate combined its value's direct children
with `attr_or_missing`, so the reference was what made `divisor(from, at)` visible. The
gate now reads the provider through `provided_slot` — the apply's reading, made recursive
through a call's argument — so `read` states nothing and a caller's `combine` derives the
meet from the argument expression it wrote. The **field** case is not a meet: `write`'s
`# (perspective_of x.to)` states that the result *is* the input segment, and stays
(`docs/notes/attributes.md` §"The whole-argument default"). Measured: removing `read`'s
line alone refuses nothing new and leaves `a_read_takes_its_perspective_from_the_buffer_and_the_index`
red (the provider reads `0`); with the provider change it is green, and language 275,
highlevel 136 and compute 237/0/1 are green.

## Stage 5: the annotated expr is generated full width

An annotation's expr carries **every** canonical attribute slot — the settled ones at
their canonical positions and the wildcard elsewhere — with the full-width layout
(`Checker::check_ann`). One shape now describes every expr that carries attributes, so no
reader has to know a tail's dense order; the renderer's `value_label` and
`render_attributes` resolve through `Module::array_attribute` instead of a dense index,
which is what made a doc label vanish when the expr went wide.

The width stops at the exprs that carry attributes. `Checker::expr_of` is used for both a
**value expr** (`[value, type]`) and a **type value** (`[shape, kind]`), and the type
readers still take the two-slot form, so making every expr wide is a separate change to
the type-value encoding rather than an attribute change.

Measured: language 275, highlevel 136, lowlevel 164 (1 ignored), compute 252 plus the
pre-existing `pack_placement` failure; the four perspective probes answer as before.

## Landed: the requirement is replayed at the apply

The read that states a requirement consumes it against the parameter's hole, and a hole
unifies with anything, so a caller's attribute was never compared with the body's own
requirement. A requirement on an expression that **is a Parameter** is recorded against the
function being checked and **replayed at every apply** of it, where the caller's provided slot
exists (`Checker::deferred_requirements`; measured by
`perspective_requirements::a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it`,
which fails without the change and refuses with `expected 32, found 1`).

## The isomorphism: every expression carries the default, the order is the checker's

**No slot is ever missing.** Every expression's expr is full width and every attribute position
carries a value: a literal `1` is `[1, Int, 0, 0, 0]`, the perspective default `0` being the
lattice identity. A parameter that states nothing, and a caller that provides nothing, both carry
that default rather than a hole.

**The order is not the runtime's to judge.** `apply_parameter_check` compares the **value** and the
**type** only. Comparing the attribute positions there would compare `32` (a requirement) against
`0` (the default), and `32 | 0 = 32` — the fact that admits the program — is an order fact the
lowlevel does not have; widening that loop is measured to refuse six perspective programs with
`expected Int, found Int`. Carrying the parameter's requirement in the apply's expr instead (to make
the structural compare exact) makes the runtime compare vacuous and is measured to lose the check
for a runtime-valued attribute.

The attribute comparison therefore lives in the checker: `unify_declared_value` at each requirement
site, plus the **deferred requirement** recorded where a body's read consumes it and replayed at the
apply. The order is the extension's: `check_unify_relaxed` tries the unify, and on failure consults
`AttrExt::is_subtype` — "consulted after a failed unify" — and suppresses the recorded error when
the relation holds (`checker/diagnostics.rs`). That is why `0` against `32` is admitted and `1`
against `32` is refused without the lowlevel ever knowing the lattice. Measured: compute 219,
highlevel 136, lowlevel 164 (1 ignored), language 275 — all green.

**And one language test's count moved with it.** A failed apply no longer narrows the checked set, so
the body is checked under the mismatched argument and records its own refusal:
`pipeline::an_apply_argument_mismatch_reports_expected_and_found_at_the_argument` now sees **two**
diagnostics — the apply's (span `(2, 4)`, `Runtime`, `expected tuple{Int, Int}, found Int`) and the
body's (`RuntimeIndexTarget` at `(1, 6)`, `this value is not a container — it has no element to
read`) — and asserts both. Its old `d.len() == 1` was the one red left at the isomorphism's tip,
because the change that made the body check is the change that added the second line.

## Open questions

Nothing blocks the design. The runtime attribute *flow* — a caller's provided slot
reaching a callee's attribute cell through the expr unify — is wired and exercised only
where a certificate is a hole; no user-facing program reads a flowed cell yet.
