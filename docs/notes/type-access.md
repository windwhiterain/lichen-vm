# The type accesses `X{n}` and `X::a` (removed)

> Status: **removed** — the two forms, their lexer tokens, AST and IR variants,
> checker arms and editor grammar are gone.  The language-level statement is the
> spec's §2.1 and §3 ([language-spec.md](../language-spec.md)); this note records
> the decision, the rule that replaces them, and what the removal took with it.
> Points at: `crates/lichen-language-lex` (the retired `::` token and the glued
> `{`), `crates/lichen-language-parser` (the retired `TypeAccess` /
> `NamedTypeAccess` expressions), `crates/lichen-language` (`compile`, `resolve`,
> `spans`, `dirty`, `content_key`, `render`), `crates/lichen-highlevel`
> (`check_type_access`, `check_named_type_access`, `element_read`,
> `RuntimeTypeAccessElement`), `crates/lichen-language-server`,
> `crates/lichen-testkit`, and `tree-sitter-lichen`.

## The decision

Any access that reads a **type's element** — a struct type's field type, a tuple
type's component type, an array type's element type — is avoidable.  There are
exactly two ways a type reaches a function, and each has its own replacement:

1. **The type arrived in a parameter's type slot.**  Read the *value*'s element
   and take its type.  A struct instance's field type is `type_of s.a`, not a
   read of the type value `S`; a tuple value's component type is
   `type_of (x(0))`, not a read of `tuple{…}`.  The type is still available as a
   value, but it is the type of a value the program already has.
2. **The type arrived in a parameter's value slot.**  Needing one of its
   elements is the signature of a **higher-order** type.  The parameter should
   not receive the type at all: it receives the type's *parameters*, and the
   call site builds the type with a type function (`Pair = T => tuple{T, T}`,
   then `Pair Int`; `S = T => struct{.a T}`).  The parameter is then an ordinary
   value — a struct instance, a tuple — whose elements are readable.

So the language keeps one rule: an element of a **value** is read (`a[i]`,
`a(k)`, `a.name`, and `type_of` for its type), and an element of a **type
value** is not a runtime read at all.  The two retired forms were the escape
hatch for generic code that inspected type values; the decision closes the
hatch, and generic code is written in the two shapes above.

## Measured replacements

Both replacements were run through `lichen-compiler` at this removal:

| program | result |
|---|---|
| `type_of = x => {t = _; x: t; t}` ; `S = struct{.a Int, .b string}` ; `s = S(.a 1, .b "h")` ; `type_of s.a` | `Int: Type` |
| `Pair = T => tuple{T, T}` ; `Pair Int` | `tuple{Int, Int}: TypeTuple` |
| `f = x => x` ; `f{0}` — the retired spelling | `0: Int` (a block argument, not a type read) |
| `tuple{Int, string}{0}` — the retired spelling | `error: expected ?a -> ?b, found TypeTuple` |

## What was removed

- `X{n}` — the glued-`{` positional read of a tuple type value's component,
  lowering to a raw lowlevel `Index` and resolving the component structurally
  (no array-type pin, no `IndexTarget`, no bounds assert), with its tuple-kind
  unify and its `RuntimeTypeAccessElement` diagnostic.
- `X::a` — the glued-`::` named read of a TypeStruct value's field *type*,
  including its name-table walk, its named-field miss, and the `::` token.
- The `Glue` marker before `{` and `::`: `{` is now always a block delimiter
  (`X{0}` and `X {0}` both apply `X` to `{0}`), and `::` is not a token.
- `Expr::TypeAccess` / `Expr::NamedTypeAccess`, `ExprKind::TypeAccess` /
  `ExprKind::NamedTypeAccess`, `Checker::check_type_access` /
  `check_named_type_access` / `element_read`, the `RuntimeTypeAccessElement`
  diagnostic and its `is_type_access_element` locator, the testkit builders, the
  four language-server walker arms, the tree-sitter rules, and the type-access
  tests and example.

## What did not change

`==` still compares any two same-typed values, including type values
(`type_of 1 == Int` is `1`), because that is the equality's own rule and does
not need the retired reads.  `type_of` remains an ordinary standard-library
function ([type-of-in-std](type-of-in-std.md)).  Struct instances still read
their fields by name (`.a`, whose requirement is the container's *kind*), and a
tuple value still reads a position with `a(k)`.
