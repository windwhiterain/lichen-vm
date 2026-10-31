# Proposal (v6): pending requirements are a mergeable set checked before any value is visible

> Status: **proposal — for review, not applied.** Supersedes v5 (`8c195252`). It answers
> the three remaining blockers: the pending requirement is a **set** with union, dynamic
> and static clone rules; a single **pre-write gate** covers every path that makes a
> value visible; and an open attribute has the same deferred path, checked through a
> relation the highlevel registers. The value-templates rulings and §Struct's reduction
> text are kept. The spec file is untouched.

## 1. The match rule: text proposed for §Function, after the apply reduction

> A **template match** is `a = t` where `t` is a template (a direct template value, a
> parameter pattern, or a call-result template). The match reads `t`'s expression
> children, and for each position:
>
> - the **type** is `t`'s type child's denotation; it is shared and checked;
> - the **attributes** are `t`'s attribute children; they are shared, and the match
>   applies **the attribute's own relation with the declaration on the left**
>   (`declared ⊑ value` for `Perspective`), never a plain `a <: t`;
> - the **value** child: a **closed** value is a **pin** — `a`'s value must equal it; an
>   **open** value (`@any`, an empty cell) is a fresh empty cell for this use, filled by
>   `a`; a **type expression** states the type requirement and carries no value;
> - a **group** child is matched position by position by the same rule.
>
> The match's answer is `a`; `t` is never specialized and is never a place's value. A
> read of a matched place answers `a`'s value for that part.

Wording fixed from v5: a **closed** value child with an **open actual** is what registers
a pending requirement; an **open** value child is fresh-filled and contributes no pin.

## 2. Pending requirements: text proposed after the match rule

> A class may carry **pending requirements**. A requirement is a **pin** (a closed value
> the bound value must equal) or an **attribute requirement** (the attribute's own
> relation, declaration on the left). A requirement is recorded on the **actual's class**
> when the actual is not yet decided; it is not installed as a value.
>
> - **Union.** A class union merges the members' requirement sets.
> - **Clone.** A dynamic clone copies its template's set through the clone walk's remap;
>   a static clone copies it through the static walk's remap.
> - **Pre-write gate.** A value becomes visible to a class only after every pending
>   requirement of the class is checked against it. A mismatch is refused **before** the
>   value is installed, with the same `expected <pin>, found <value>` the decided case
>   reports.
> - **Movement.** GC relocation keeps the set with the class and is not a new visibility:
>   an already-checked value is not re-checked.

## 3. The mechanisms (not spec text)

**A requirement is a record, not a single value.**

```text
PendingRequirement := Pin { expects: value } | Attribute { extension: id, expects: class-cell }
Module.requirements: HashMap<class-root, Vec<PendingRequirement>>
```

- The checker fills it: for each field child it resolves the type and attributes; a
  **closed pin + open actual** pushes `Pin`; an **open attribute + open actual** pushes
  `Attribute`. A decided actual checks immediately, as today.
- `add_equality` unions the two sets under the merged representative, so the review's
  `S1`/`S2` case keeps **both** pins (one accepted, one refused).
- The dynamic clone walk (`function.rs`) and the static clone walk
  (`static_module/apply.rs`) copy each requirement through their own remap; a
  requirement whose declared cell has a clone gets the clone's cell, so a pin mismatch
  that only appears after a per-call clone is still checked on that clone.

**One pre-write gate.**

```text
Module::install_value(class, value) -> Result<(), UnifyError>
  check every requirement of the class against `value`; on failure return before writing
```

Every path that makes a value visible goes through it, replacing the direct writes in
`write_node_value`, `write_node_answer`, `propagate_class_value`, `fill_class_holes` and
`add_equality`'s pre-union fill. No path writes `node.value` around it; GC's
`relocate_node_value` is the one licensed move (it keeps the class and the set).

**Open attributes need a registered relation.** The engine is generic and may not call
`AttrExt` (`deferred-instantiation-checks.md` §3), so the highlevel registers, per
extension id, the relation the gate calls:

```text
Module.attr_relations: HashMap<extension-id, fn(&Module, &declared, &actual) -> bool>
```

The highlevel installs each `AttrExt`'s `is_subtype` here when the module is created;
the lowlevel's gate looks the relation up by the id the checker recorded. No marker or
attribute semantics enters the engine; the engine only holds a function pointer keyed by
an opaque id.

## 4. §Struct: the reduction text is unchanged

`raw v[...] = e1` is the match; `e = raw v[...]` fills the place. The rule above states
what the match does with the template's value; no symbolic change is needed.

## 5. Paths

| path | what happens |
|---|---|
| checker (`check_instantiate`) | resolves each field's type/attributes; decided actual checks now; a closed pin with an open actual pushes a `Pin`; an open attribute with an open actual pushes an `Attribute`; an open value child stays a fresh cell |
| dynamic apply (`function.rs` clone walk) | copies requirements through the remap; a merged class's set is unioned |
| static apply (`static_module/apply.rs`) | the same copy through the static remap |
| every value-visibility site (`write_node_value`, `write_node_answer`, `propagate_class_value`, `fill_class_holes`, `add_equality`'s fill) | routed through `install_value`; failure before install |
| GC (`relocate_node_value`) | keeps the set; no re-check |
| reads | unchanged |

## 6. Acceptance cases

1. **Deferred pin.** `S := {.a 5 : Int}; x := _; s := S{.a x}; x = 5` accepted, `x = 6`
   refused `expected 5, found 6`, `x` never pre-filled from `5`.
2. **Two pins on one actual class.** `S1 := {.a 5 : Int}; S2 := {.a 6 : Int}; x := _;
   s1 := S1{.a x}; s2 := S2{.a x}; x = 5` — accepted by `S1`'s pin and refused by
   `S2`'s: both requirements are checked, neither is lost.
3. **A pin that only a per-call clone meets.** A body whose template pin is 5 is applied
   with an open argument that the call's clone binds to 6 — the clone's copied
   requirement refuses it; the template itself is untouched.
4. **Attribute direction.** Requirement `# 4` against `# 8` accepted (`declared ⊑ value`),
   against `# 2` refused; an open actual's attribute requirement is deferred and checked
   at the bind by the registered relation.
5. **Type parameter / `@any : Int` / plrun codomain** as v5 (§6 examples 3–5).

## 7. Rulings kept

`=` is the only template match; `:` is not a match; a direct template is instantiated per
use; the call-result template stays `r : type_of (k.O); r = k.O`; the annotation rule is
untouched; the attribute extensions keep their own relations and the engine calls them
through the registered fn pointer, not by knowing them.
