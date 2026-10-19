# The array attribute view: sparse storage, full-width reading

Status: **proposed** — the design is the superior's, the encoding below is the
author's proposal and awaits confirmation. Nothing is implemented.

## The problem, in the superior's words

> 本质上是AST期间确定的稀疏unify映射无法递归进类型，或者说无法递归进入任何attribute。
> 重构方案：让lowlevel的array有一个稀疏表指针，将该array映射到全宽attribute array，
> lowlevel global再有一个default attribute array，稀疏中的空洞就去default找。
> 这样高层就把所有东西当全宽attributes

The check-time attribute mapping is *sparse*: each expression's schema tail says
which attributes it carries, and the pair stores them densely
(`[value, type, attrs…]`, `attr_slot(i) = 2 + i`). Two consequences measured this
session:

- A pair cannot be made full width. An expression pair is also a *type value*
  when the expression is a type (`[shape, kind]`, two items), so a five-wide pair
  breaks every type-level function (`expected raw[raw[…]], TypeStruct, …`), and
  the pair readers that assumed 2..=3 elements silently returned nothing
  (`f = x => x; f 1` answered `?a: ?a`).
- The *runtime* pair unify deliberately excludes the attribute positions
  (`apply_parameter_check` unifies slots 0..2), so a caller's attribute values
  never reach a callee's cells — the asymmetry with the type slot that makes
  `type_of` work and a `perspective_of` written the same way fail (it answers an
  undecided `?a`; `x # p` binds only when the operand has a provider, and a
  parameter has none).

## The design

**Every lowlevel array may carry a pointer to a full-width attribute array.**
A module-global *default attribute array* supplies the holes of the sparse
storage. The highlevel then reads every pair and every element slot as a
full-width attribute array: positions it does not carry read the default.

The underlying items stay as they are, so a type value remains a two-item
`[shape, kind]` array while its *attribute view* is full width. That is what
removes the dual-use conflict that defeated the wide-pair attempt.

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
2. **The array.** `LowValue::Array` gains a layout id:
   `LowValue::Array { items: AnyHandle<[ArrayItem]>, layout: u32 }`. The pointer
   is a *small integer*, not a heap array per value, so the storage stays O(1)
   and the table is shared. Constructors: `LowValue::array(items)` picks layout
   `0`; a builder that carries attributes passes the registered id.
3. **The default.** A module-global **default attribute array**: the full-width
   slots an array's layout does not cover. Every canonical attribute the layout
   omits is read from the default, so the highlevel can treat every value as
   full-width.
4. **Unify.** Lowlevel unification of two arrays wires the attributes **by
   canonical index**: attribute `i` of one array meets attribute `i` of the other
   (each found through its own layout), and a side whose layout omits `i` meets
   the default's slot for `i`. That is the recursion into attributes the current
   design lacks; the plain value and type positions keep unifying as they do.
5. **The highlevel.** The checker's schema tails pick the layout: a pair with
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
3. The checker builds the full-width attribute view (`slot_width` slots per
   pair), and the runtime pair unify descends into it. Behaviour change: a
   caller's attributes flow into the callee's cells, so
   `perspective_of = x => {p = _; x # p; p}` (`type_of`'s shape) starts to work.
4. The codec/freeze/static-module persistence (format bump) and the compute
   vocabulary's `perspective_of` spelling replacing the call-site references
   (`# @any`/`# x.to`), which are then deleted.

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

## Open questions

Nothing blocks stage 1. Stage 3 is where the verdict above is measured.
