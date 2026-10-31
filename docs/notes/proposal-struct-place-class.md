# Proposal (v3): a struct field's declaration is an annotation; the value is the argument's

> Status: **proposal — for review, not applied.** Supersedes v2 (`e6abccb2`). Addresses
> the review's four requirements for a v3: a role-based declaration, a real merge-free
> check, the complete path, and three semantic examples. No new syntax; the annotation
> (§Annotation) is the check the language already has. The spec file is untouched.

## 1. Declaration, by role

Proposed for §Type, after the sentence "expression with unit variant lowlevel value has
type `Type`":

> A **declaration** is an expression in a **declaration position**:
>
> - the annotation of an annotated expression (`e : d`);
> - a struct template's field expression (`s := {.n1 d1 …}`);
> - the codomain argument of a kernel launch (a library position:
>   `compute.parallel f "gpu" d`).
>
> A declaration states a type; it is not a value. An expression in a value position —
> an argument, a parameter binding, the left of `=` — is never a declaration, even when
> its type is `Type`: `id = (T : Type) => T; id Int` binds `Int` to `T`.

This is positional, so it covers `Buf x` and `@any : Int` (struct field declarations) and
`k.O` (the annotation `r : type_of (k.O)`), and it excludes a type parameter, whose
binding is a value unify.

## 2. The check: the annotation is already merge-free for values

Proposed clarification of §Annotation:

> The annotation unifies and replaces the **type attributes**; the expression's **value**
> attribute is not unified with the annotation. A declaration in an annotation position
> therefore checks `e`'s type and leaves `e`'s value alone. When `e`'s value is
> undecided, the type check is deferred to its bind.

No new primitive and no post-unify suppression: the annotation's own rule is the check,
and the class merge happens only among type attributes. The `a = b` => `a <: b` rule is
untouched, so no recursion through `<:`.

## 3. §Struct: the declaration becomes an annotation

Proposed reduction, replacing today's:

```sytax
`s{.n2 e3, .n1 e4}` 
:= 
`{
  let {.n1 d1, .n2 d2} = s;
  v := raw[name_table, nominal_id, @raw None, @raw None];
  raw v[name_table["n1"]+2] : d1;
  raw v[name_table["n2"]+2] : d2;
  e4 = raw v[name_table["n1"]+2];
  e3 = raw v[name_table["n2"]+2];
  raw[raw v, Struct, ...]
}`

`s.<name>` := `{type_of s = Struct; @raw s.value[s.value.name_table[<name>]+2]}`
```

> `raw v[...] : d` is the annotation: it checks the place's type against the declaration
> and carries no value. `e = raw v[...]` binds the supplied expression's value. The
> field's value is `e`'s; the declaration's is nobody's.

## 4. The library

`compute.lichen`'s `plrun` ends with `r : type_of (k.O); r = k.O`. The annotation is the
requirement; the `r = k.O` **value bind is removed**. The codomain still reaches
`$plrun` as its third operand — the recording's roles and the launch's declared layout
read it there, not through `r`'s value.

## 5. The four paths

| path | change |
|---|---|
| general unify (`ExprKind::Unify` → `check_unify_expr`, `checker.rs`) | **none** — a declaration is not unified through `=`; the requirement goes through the annotation (`check_ann`) |
| struct instantiation (`check_instantiate`, `checker/structs.rs`) | both branches check the field against its declaration as a **type** (the decided branch's `check_unify(vals[argument], field)` becomes the annotation check); the undecided branch (`structs.rs:826-877`) already checks type and attributes only and now does so by rule |
| runtime apply (`apply.rs` parameter slots) | **none** — a parameter is a value position; no declaration is a parameter |
| `plrun`'s `r = k.O` (`compute.lichen`) | the value bind is removed; the annotation stays |

## 6. Three semantic examples

**Type parameter binding.** `id = (T : Type) => T; id Int`. The apply unifies the
argument with the cloned parameter (`=`), both value positions; `T` binds `Int` and the
call answers `Int`. A declaration rule keyed on a position does not fire.

**`@any : Int` config.** `Par := {.n @any : Int, .scale @any : Float}`. Instantiating
`Par{.scale s.scale}` annotates the instance's `.scale` place with `Float`: the type is
checked, and the place's value is `s.scale`'s — a runtime float the kernel reads. The
declaration `Float` carries no value into the place.

**`plrun` codomain field read.** `r := $plrun(k.native, a, k.O); r : type_of (k.O)`.
`r` keeps the launcher's result as its value; the codomain is the requirement. A body's
`first.r` reads the result's field (the recorded wrapper) and the codomain's declaration
is nowhere in that class.

## Compatibility

- A decided instance answers as today: the place already holds the supplied value.
- An open field answers its later bind or undecided — the `plrun` result's `.r` read the
  22 tests ask for.
- Removing `r = k.O` was measured inert on the current suite (215/25), so no test relies
  on that bind today; the codomain's type still reaches `r` through the annotation.
- The SPEC's §Annotation gains one clarifying sentence; §Struct changes the two `= d`
  lines to `: d`. No new syntax, no change to `=`, clones, or attributes.

## What this proposal does not claim

- It does not touch `a = b` or the subtype direction.
- It does not rely on "the declaration's type is `Type`".
- It does not assume the checker's post-unify error truncation is a check.
