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

## Proposed encoding — awaiting confirmation

1. **Payload.** `LowValue::Array(AnyHandle<[ArrayItem]>)` becomes
   `LowValue::Array { items: AnyHandle<[ArrayItem]>, attributes: Option<AnyHandle<[ArrayItem]>> }`,
   where `attributes` is the full-width attribute array (one slot per canonical
   position, `slot_width::<P>()` entries). 241 `LowValue::Array` sites across the
   workspace are mechanical to update. The pointer travels with the value, so
   clone, freeze and the codec carry it without a side table.
2. **Default.** `Module::default_attributes: Option<AnyHandle<[ArrayItem]>>`, set
   once by the highlevel to a full-width array of *wildcard* slots. A reading of
   position `p` on an array whose `attributes` is `None`, or whose entry is the
   wildcard, returns the default entry for `p`.
   Open question 1: is the default's content the wildcard ("no claim") or
   something else?
3. **Read.** `Module::array_attribute(value, position) -> Option<AnyNodeId>`
   (dynamic and static sides); the highlevel's attribute readers
   (`pair_attr_slot`, `element_attr_slots`, the shape readers in
   `crates/lichen-highlevel/src/shape.rs`) go through it.
4. **Unify.** The pair unify recurses into the two arrays' attribute views
   position-wise, the wildcard/default skipped — the "recursion into attributes"
   this design exists for.
5. **Codec and freeze.** The artifact persists each array's attribute view and
   the module default; `ARTIFACT_FORMAT_VERSION` is bumped again.

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

## Open questions

- **1. The default array's content**: wildcard slots, or undecided cells?
  Wildcards say "no claim" and keep the unify skipping; cells would make every
  hole a unification variable shared across arrays, which couples unrelated
  pairs.
- **2. Where the highlevel attaches the view**: when the checker builds a pair
  with a non-empty tail (only then), or always? Always costs an allocation per
  pair; only-with-a-tail keeps the default path free.
- **3. The checking model for a flowed attribute**: once a caller's attribute
  value reaches the callee's cell at runtime, a static `# n` requirement on the
  *result* can no longer be decided at check time. The options are the runtime
  assert (the existing `assert` worklist) or keeping the check static only where
  the value is decided. This decides how much static refusal the perspective
  keeps.
