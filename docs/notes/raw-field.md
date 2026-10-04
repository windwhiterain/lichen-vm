# The raw named read `X::a`

> Status: current.
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
*concretely* non-struct container (an atomic type, a tuple value) is a check-time
`IndexTarget` diagnostic; an unbound container (a parameter, a call result) stays lazy and
resolves at the apply.  The read compiles to `Index(Index(value_of(X),
TableGet(names, a)), …)` — the value is the field at the name table's index, the type is
element 1 of that read.

## `==` on type values

Because a raw named read yields a *type* (a `Type`-typed value), `==` is generalized to
compare any two same-typed values and yield `0`/`1`: the Int equalities (`s.a == 1`) and
the type equalities (`S::a == Int` is `1`, `S::a == string` is `0`).  A comparison across
types (`S::a == 1`, an `Int` vs a `Type`) is still a check-time `BinOp` error.

## Why `==>` for tables

The table literal used to separate key/value with `::`.  Now that `::` is the raw named
read's glued postfix, the table separator is spelled `==>`
(`table { k ==> v }`), so the two are never ambiguous.
