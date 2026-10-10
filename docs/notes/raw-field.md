# The raw named read `X::a`

> Status: current
> Points at: `crates/lichen-language-lex` (`TableArrow`, the glued `DoubleColon`),
> `crates/lichen-language-parser` (`Expr::RawNamedField`), `crates/lichen-language/src/compile.rs`
> (`ExprKind::RawNamedField`), `crates/lichen-highlevel/src/ir.rs`
> (`ExprKind::RawNamedField`), `crates/lichen-highlevel/src/checker.rs`
> (`check_raw_named_field`, `is_type_struct_kind_any`).  Syntax and semantics are the
> spec's business: [language-spec.md §2, §2.1, §3](../language-spec.md).

The glued `::` postfix reads a *named* component of a TypeStruct value with a
check-time struct requirement: `X::a` reads field `a` from the value `X`, whose **type**
must itself be a TypeStruct **kind** — `[[TypeId, names, names_in_order],
TypeStruct]` under the kind's `[marker, K]` pair, the shape a
`struct<.a T, …>` value's `ty` is (the name→index table lies directly there, in
the marker payload, at
`container_ty[0][0][1]`).

## `:` vs `.` vs `::`

`a(k)` is the guarded **positional** slot read, `a.name` the guarded **named** field read
over a struct *instance*.  `X::a` is the **named** sibling of the raw positional index
`X<e>`:

- `a.name` (`.`): requires the container's **kind** to be TypeStruct — the value's type
  is a full struct type `[shape, kind]`, and the name table sits at
  `container_ty[1][0][0][1]`.
  It reads the field *value*.
- `X::a` (`::`): requires the container's **type** to be a TypeStruct kind — the value is
  a struct *type*, and the name table sits at `container_ty[0][0][1]`.  It reads the field
  *type* as a value, so `struct<.a Int, .b string>::a` is `Int : Type`.

## Check-time, not raw

 Unlike `X<e>` (no validation at all), `X::a` requires a TypeStruct container.  A
*concretely* non-struct container (an atomic type, a tuple value, an array) is a
check-time `Guard` diagnostic — `expected TypeStruct, found …` — and the read is **not
built** behind that refusal: the name table below is reached by walking this container's
own *type*, and for a type that is not a struct the walk lands on whatever sits at that
path instead.  Measured on `l = [10, 20]; l::a`, the walk's last step is the array's own
universe, and the lowlevel refuses to read an array as a table
(`unreachable!("TableGet target must be a table")`, reached from the checker's own forcing
of the read's type — that third case is frontend-only precisely because the refusal
stops it before the read is built).  The expression carries the hole
a refused definition carries instead ([`Checker::refused_pair`]), and the refusal is the
whole answer: the read is defined for a struct type value and nothing else.  An *undecided*
container (a parameter, a call result) is **pinned** to the struct kind and stays lazy,
resolving at the apply that binds it.

The read compiles to `Index(Index(value_of(X), TableGet(names, a)), …)`: its value is
element 0 of the field at the name table's index, and its **type is element 1 of that same
read** — the *kind* of the field's type value, not a constant `Type`.  `struct<.a Int, .b
string>::a` is `Int : Type`; `struct<.a struct<.b Int>>::a` is
`struct<.b Int>: TypeStruct`, and `(S::a : Type)` over it is refused exactly as `(S :
Type)` is (`expected Type, found TypeStruct`).

That slot is **computed when the read is checked**, which is what makes the read's type a
fact a later check can compare instead of a cell that check writes into.  Measured on
`S = struct<.a Int, .b string>`, with the slot left uncomputed:

| statement | slot uncomputed | slot computed |
|---|---|---|
| `S::a == 1` | accepted, prints `0` | `error: expected Int, found Type` |
| `S::a : Int` | accepted, prints `Int: Int` | `error: expected Int, found Type` |
| `S::a == Int` | `1` | `1` |
| `(5 : Type)` — the decided control | `error: expected Type, found Int` | same |

The forced read stays lazy where nothing can be computed yet (an undecided container), which
is what the per-apply re-check of the pin relies on: `f = s => s::a; f (struct<.a Int>) ==
5` is refused by that re-check, at the argument, with the same wording the direct case now
gives.

## `==` on type values

Because a raw named read yields a *type* value, `==` is generalized to compare any two
same-typed values and yield `0`/`1`: the Int equalities (`s.a == 1`) and the type
equalities (`S::a == Int` is `1`, `S::a == string` is `0`).  A comparison across types
(`S::a == 1`, a type value against an `Int`) is a check-time `BinOp` error.

## Why `==>` for tables

The table literal used to separate key/value with `::`.  Now that `::` is the raw named
read's glued postfix, the table separator is spelled `==>`
(`table { k ==> v }`), so the two are never ambiguous.
