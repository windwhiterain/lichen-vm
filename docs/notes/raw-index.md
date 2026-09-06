# The raw index `X<e>`

> Status: current.
> Points at: `crates/lichen-language-lex` (`KwArray`), `crates/lichen-language-parser`
> (`Expr::RawIndex`, `array_type`), `crates/lichen-language/src/compile.rs`
> (`ExprKind::RawIndex`), `crates/lichen-highlevel/src/ir.rs`
> (`ExprKind::RawIndex`), `crates/lichen-highlevel/src/checker.rs` (`check_raw_index`),
> `crates/lichen-render/src/render.rs`.  Syntax and semantics are the spec's business:
> [language-spec.md §2, §2.1, §3](../language-spec.md).

The glued `<` postfix — the delimiter the array type used to occupy — now reads element
`e` of an expression's **value** with **no type validation**: `X<e>`.  The array type is
spelled `array<T, n>` (keyword-led), so the two cannot be confused.

## What it does

`X<e>` lowers to the lowlevel `Index` directly, bypassing every guard the typed forms
apply:

- no array-type pinning (unlike `e[i]` → `Index`),
- no `IndexTarget` / shape-derived type (unlike `a(k)` → `Field`),
- no bounds assert.

So it reads a component of a *type-as-value*: `<Int, string><0>` is the `Int` type,
`struct<Int, string><1>` is the `string` type.  Because it is unvalidated, an index into
a concretely non-positional value (an atomic type, an `Int`) or an out-of-bounds index is
a **runtime** lowlevel `Index` error, never a static diagnostic.

The result is the element's own pair — value slot element 0, type slot element 1 — both
read lazily.  An unbound container (a parameter, a call result) stays lazy and resolves at
the apply, which is what makes it usable generically: `f = k => k<0>` reads the first
field of whatever type `k` is applied to, exactly the laziness the compute-wrapper field
reads rely on (see [compute-kernel-struct.md](compute-kernel-struct.md)).

## Why

Before this, the only way to read a component of a type value was through the wrapper's
own lazy `Field` reads, which require the container's *type* to be positional.  A
type-as-value's type is `Type` (or a kind), not a tuple/struct type, so the guarded reads
reject it.  `X<e>` is the escape hatch for generic code that inspects type values.
