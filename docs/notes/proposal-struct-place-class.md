# Proposal (v4): the template match fills fresh cells; it never installs the template's value

> Status: **proposal — for review, not applied.** Supersedes v3 (`f1ebd0ae`). It keeps
> the confirmed rulings of [value-templates](value-templates.md) rather than overturning
> them: `=` remains the only template match, `:` stays a non-match, a direct template is
> instantiated per use, and a call-result template keeps `r : type_of (k.O); r = k.O`.
> The rule below states what that match does with the template's **expression children**
> — the four aspects the review asked to separate. The spec file is untouched.

## The rule: text proposed for §Function, after the apply reduction

> A **template match** is `a = t` where `t` is a template (a direct template value, a
> parameter pattern, or a call-result template). The match reads `t`'s expression
> children, and for each position:
>
> - the **type** is `t`'s type child's denotation; it is shared and checked (`a`'s type
>   is a subtype of it);
> - the **attributes** are `t`'s attribute children; they are shared and unified
>   directionally (`a`'s attribute is a subtype of `t`'s);
> - the **value** child: a **closed** value is a **pin** — `a`'s value must equal it
>   (`expected 5, found 6`); an **open** value (`@any`, an empty cell) is a fresh empty
>   cell for this use, filled by `a`; a **type expression** states the type requirement
>   and carries no value; `t` is never specialized;
> - a **group** child (a struct value, e.g. a buffer's parameter group) is matched
>   position by position by the same rule.
>
> The match's answer is `a`. A read of a matched place answers `a`'s value for that
> part; before `a` runs, an open part is undecided — never the template's value.

This is the confirmed per-use instantiation ruling ("its open value slots become fresh
empty cells, while its types, attributes, name table and nominal id are shared. The
actual fills the fresh slots, so the template itself is never specialized") stated as the
match's rule; it adds no syntax and changes no other form.

## How the four review aspects map

1. **type** — from `t`'s **type child**, not the whole field expression: the field
   construction already splits the children (`checker/tuples.rs`, the field's value and
   type children) and calls `type_denotation` on the type child. `check_ann`'s
   whole-RHS `type_denotation` is not used for the field; that path is why v3's example
   was wrong.
2. **attributes** — the existing directional requirement (subtype via the attribute's
   `is_subtype`), unchanged.
3. **value pin** — only a **closed** value child pins. `S := {.a 5 : Int}; S{.a 6}`
   keeps its refusal (`expected 5, found 6`, pinned by
   `struct_element_attributes`); `(Buf (BufParam{.length 8}))`'s length is closed and
   pins; its native `@any` is open and is filled.
4. **compound template content** — a group child is matched position by position, so the
   buffer group's internal pins and opens are preserved instead of comparing the shared
   `Struct` type.

## §Struct: the reduction text is unchanged

Today's lines are exactly this match — `raw v[...] = e1` matches the fresh place against
the declaration, and `e = raw v[...]` fills it. No spec text changes in §Struct; the new
paragraph states what the match does with the value.

## Paths

| path | what the rule does |
|---|---|
| `check_instantiate` (`checker/structs.rs`) | per field, match the supplied term against the declaration: type from the type child, attributes directional, a closed value child pins, an open one is fresh, a group recurses. The **decided** branch's whole-term `check_unify(vals[argument], field)` is replaced by this match; the **undecided** branch's existing type/attribute checks are the match's type/attribute aspects and stay |
| runtime apply (`apply.rs`'s parameter slots) | the parameter pattern's slots are the same match: a closed slot pins, an open slot is filled by the argument, a group recurses. This covers the expected parameter templates (`(k = Par)`) that the review named |
| library wrappers (`launch`, `call`, `write`, `write_atomic`, `mma`, `plrun`) | their `r = declaration` is a match under the rule, so **no library edit** — the confirmed ruling ("a direct template is instantiated per use; `=` is the only template match") is kept |
| reads (`value_slot_read`, `slot_value_of`, lowlevel `Index`) | unchanged: a place answers the matched value's part; an open part before its value runs is undecided |

## Four examples

1. **Type parameter.** `id = (T : Type) => T; id Int`. `T` is a parameter place (an open
   cell), not a template's value child; the match fills it with `Int` and the call
   answers `Int`.
2. **`@any : Int` config.** `Par := {.n @any : Int, .scale @any : Float}`. `.scale`'s
   type child `Float` is checked, its value child `@any` is a fresh cell, and `s.scale`
   fills it; the kernel reads the supplied float and the declaration adds no value.
3. **`plrun` codomain.** `r := $plrun(k.native, a, k.O); r : type_of (k.O); r = k.O`.
   The codomain's open buffer fields are fresh cells, `r`'s recorded buffer fills them,
   its closed `.length` pins check; `first.r` answers `r`'s buffer (the recorded
   wrapper).
4. **Value pin.** `S := {.a 5 : Int}; S{.a 6}` refuses with `expected 5, found 6` — the
   closed value child is a pin.

## Compatibility and the rulings it keeps

- `=` is the only template match; `:` is not a match; a direct template is instantiated
  per use; the call-result template stays `r : type_of (k.O); r = k.O`.
- §Struct's reduction text is unchanged; only the match's rule is stated where the spec
  already defines the apply reduction.
- A decided field answers as today; an open field answers its fill or undecided.
- The pin semantics and the compound content are preserved (aspects 3 and 4).

## What this proposal does not claim

- It does not touch `a = b`'s direction, the annotation rule, or any syntax.
- It does not rely on post-unify error truncation: a pin is a check, and the fresh-cell
  fill is the template's own per-use instantiation.
