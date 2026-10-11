# The type accesses `X{n}` and `X::a`

> Status: current
> Points at: `crates/lichen-language-lex` (the glued `LBrace`/`DoubleColon`),
> `crates/lichen-language-parser` (`Expr::TypeAccess`, `Expr::NamedTypeAccess`),
> `crates/lichen-language/src/compile.rs` (`ExprKind::TypeAccess`,
> `ExprKind::NamedTypeAccess`), `crates/lichen-highlevel/src/ir.rs`
> (`ExprKind::TypeAccess`, `ExprKind::NamedTypeAccess`),
> `crates/lichen-highlevel/src/checker.rs` (`check_type_access`,
> `check_named_type_access`, `is_type_struct_kind_any`),
> `crates/lichen-render/src/render.rs`.  Syntax and semantics are the spec's
> business: [language-spec.md §2, §2.1, §3](../language-spec.md).

Two glued postfixes read a component of a **type-as-value**: `X{n}` by rank and `X::a` by
name.  Each lowers to the lowlevel `Index` and resolves the component structurally,
bypassing every *value* guard the typed forms apply — no array-type pinning (unlike `e[i]`),
no `IndexTarget` / shape-derived type (unlike `a(k)`), no bounds assert — and each checks the
container's kind with a unify instead.  This is the syntax the array type used to occupy; the
array type is `array{T, n}` (keyword-led), so the two cannot be confused.

## The positional read `X{n}`

`X{n}` reads component `n` of `X`'s value from a **tuple type value**.  The container's type
must be the tuple kind, stated once as a unify.  A decided container is refused where it
stands:

- a tuple *value* (`(1, 2){0}`): its type is the tuple shape `tuple{Int, Int}`, not the kind
  `TypeTuple` — `expected TypeTuple, found tuple{Int, Int}`;
- a struct type value: its kind is `TypeStruct` and its components read by name (`X::a`)
  — `expected TypeTuple, found TypeStruct`;
- an array or an atomic type: `expected TypeTuple, found array{Int, 2}` / `found Int`.

An undecided container (a parameter, a call result) is **pinned** to the tuple kind, so the
apply that binds it refuses a wrong-kind actual per call; without that pin, an array
container would reach the element read at run time.

So it reads a component of a *type-as-value*: `tuple{Int, string}{0}` is the `Int` type.  An
out-of-bounds subscript is still an evaluation error, recorded during the definition pass, so
the build is refused.

The result is the component's own pair — value slot element 0, type slot element 1.  The
**value** slot is read lazily; the **type** slot is computed when the read is checked, and it
is the component type value's *kind* (`Type` for `tuple{Int, string}{0}`, a `TypeStruct` kind
for a component that is itself a struct type), which is what lets a later check compare the
read's type instead of writing into it — the same rule, and the same measurement, as the
named sibling below.  A read the kind requirement refused is **not built**: it carries the
hole a refused definition carries, because over a container that is not a tuple type value
the two slot reads land on whatever the container's type holds (measured: an array's length).
Every component of a type-as-value is such a pair, which is what the form is for; a runtime
array's element is read with `e[i]`, and `X{n}` is not a second way to spell it.

The pin is what makes it usable generically: `f = k => k{0}` reads the first component of
whatever *tuple type* `k` is applied to, exactly the laziness the compute-wrapper field reads
rely on (see [compute-kernel-struct.md](compute-kernel-struct.md)).

### Why

Before this, the only way to read a component of a type value was through the wrapper's own
lazy `Field` reads, which require the container's *type* to be positional.  A type-as-value's
type is `Type` (or a kind), not a tuple/struct type, so the guarded reads reject it.  The
type accesses are the escape hatch for generic code that inspects type values.

## The named read `X::a`

The glued `::` postfix reads a *named* component of a TypeStruct value: `X::a` reads field
`a` from the value `X`, whose **type** must itself be a TypeStruct **kind** — `[[TypeId,
names, names_in_order], TypeStruct]` under the kind's `[marker, K]` pair, the shape a
`struct{.a T, …}` value's `ty` is (the name→index table lies directly there, in the marker
payload, at `container_ty[0][0][1]`).

## `:` vs `.` vs `::`

`a(k)` is the guarded **positional** slot read, `a.name` the guarded **named** field read
over a struct *instance*.  `X::a` is the **named** sibling of the positional type access
`X{n}`:

- `a.name` (`.`): requires the container's **kind** to be TypeStruct — the value's type is a
  full struct type `[shape, kind]`, and the name table sits at `container_ty[1][0][0][1]`.
  It reads the field *value*.
- `X::a` (`::`): requires the container's **type** to be a TypeStruct kind — the value is a
  struct *type*, and the name table sits at `container_ty[0][0][1]`.  It reads the field
  *type* as a value, so `struct{.a Int, .b string}::a` is `Int : Type`.

## The check-time requirement

Unlike the positional read, the named read's container type *is* the name table's home, so a
*concretely* non-struct container (an atomic type, a tuple value, an array) is a check-time
`Guard` diagnostic — `expected TypeStruct, found …` — and the read is **not built** behind
that refusal: the name table is reached by walking this container's own *type*, and for a
type that is not a struct the walk lands on whatever sits at that path instead.  Measured on
`l = [10, 20]; l::a`, the walk's last step is the array's own universe, and the lowlevel
refuses to read an array as a table (`unreachable!("TableGet target must be a table")`,
reached from the checker's own forcing of the read's type — that third case is frontend-only
precisely because the refusal stops it before the read is built).  The expression carries the
hole a refused definition carries instead ([`Checker::refused_pair`]), and the refusal is the
whole answer: the read is defined for a struct type value and nothing else.  An *undecided*
container (a parameter, a call result) is **pinned** to the struct kind and stays lazy,
resolving at the apply that binds it.

The read compiles to `Index(Index(value_of(X), TableGet(names, a)), …)`: its value is element
0 of the field at the name table's index, and its **type is element 1 of that same read** —
the *kind* of the field's type value, not a constant `Type`.  `struct{.a Int, .b string}::a`
is `Int : Type`; `struct{.a struct{.b Int}}::a` is `struct{.b Int}: TypeStruct`, and
`(S::a : Type)` over it is refused exactly as `(S : Type)` is (`expected Type, found
TypeStruct`).

That slot is **computed when the read is checked**, which is what makes the read's type a
fact a later check can compare instead of a cell that check writes into.  Measured on
`S = struct{.a Int, .b string}`, with the slot left uncomputed:

| statement | slot uncomputed | slot computed |
|---|---|---|
| `S::a == 1` | accepted, prints `0` | `error: expected Int, found Type` |
| `S::a : Int` | accepted, prints `Int: Int` | `error: expected Int, found Type` |
| `S::a == Int` | `1` | `1` |
| `(5 : Type)` — the decided control | `error: expected Type, found Int` | same |

The forced read stays lazy where nothing can be computed yet (an undecided container), which
is what the per-apply re-check of the pin relies on: `f = s => s::a; f (struct{.a Int}) == 5`
is refused by that re-check, at the argument, with the same wording the direct case now gives.

## `==` on type values

Because a type access yields a *type* value, `==` is generalized to compare any two
same-typed values and yield `0`/`1`: the Int equalities (`s.a == 1`) and the type equalities
(`S::a == Int` is `1`, `S::a == string` is `0`).  A comparison across types (`S::a == 1`, a
type value against an `Int`) is a check-time `BinOp` error.

## Recovered measurements

- The shallow array is typed as a tuple for a measured reason: a homogeneous `Array` type
  would reject `[x, @shallow f(x+1)]` with an `Int` head and a `Stream` tail.  A bare
  `@shallow` position's whole subtree stays lazy in the deep pass, and a read forces the
  single element on demand.
- The out-of-bounds *slot* read this form replaced is now unreachable: it read the
  container's element as a one-element array whose type slot 1 is missing, and a
  tuple-kinded value's components are always `[value, type]` pairs.  `X::a` is the same
  container requirement for a struct kind, read by name rather than by rank.

## Why `==>` for tables, and `<==` for the read

The table literal used to separate key/value with `::`.  Now that `::` is the named type
access's glued postfix, the table separator is spelled `==>` (`table(k ==> v)`), so the
two are never ambiguous.  The table *read* is the infix `t <== k` — the loosest binary
level, left-associative — which is what freed the glued `{` for the positional type access
`X{n}`.
