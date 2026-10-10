# No type mode: `(a, b)` is always a tuple value, `<a, b>` always a type

> Status: current
> Points at: `crates/lichen-language-parser/src/parse.rs` (the grammar, the tuple
> primary and the tuple-type atom), `crates/lichen-language-parser/src/ast.rs`
> (`Tuple` / `TypeTuple`), and
> [`language-spec.md`](../language-spec.md) ("One grammar, no type mode").
> Companion to [`placeholder-anywhere.md`](placeholder-anywhere.md), which makes
> `_` a placeholder in the same way.

The lichen grammar has **one mode**. A type is an expression, and *which*
delimiter an expression is written with — not where it sits — is what makes it a
type. The parser has no type-mode pass and never needs to know whether it is
parsing a type or a value. `expr : expr` parses both sides identically:

- `(a, b)` — always a `Tuple` *value*.
- `<a, b>` — always a `TypeTuple` *type*, in every position.
- `_` — always a placeholder (its own token; see
  [`placeholder-anywhere.md`](placeholder-anywhere.md)).

This makes the spelling of a tuple type explicit. `x : <Int, Int>` is a tuple of
two `Int`s, while `x : (Int, Int)` is a *value* tuple whose elements are the
type-values `Int` and `Int` (so `x : <Type, Type>`). Angle brackets are
type-level throughout: `struct<T1, T2>` is a struct type and the array type is
the keyword-led `array<T, n>`.

## Why

A tuple type previously depended on which side of `:` its `(…)` sat — a
grammar-position distinction, implemented by a *type-mode* post-pass that flipped
`(a, b)` into a `TypeTuple` when it appeared in a type position (the right side
of `:`, a struct field, and the like). Removing it:

- makes the grammar uniform — "types are expressions", and type-ness is spelled
  by the delimiter;
- removes the last place the parser had to know the type/value position at all,
  so the parser is a plain token-to-AST function with no positional pass;
- leaves `_` with no position-dependent treatment, which is what
  [`placeholder-anywhere.md`](placeholder-anywhere.md) rests on.

## Writing it today

Paren tuple *types* are written with angle brackets. The `.lichen` examples
already spell tuple types with `<…>` and value tuples with `(…)`; a program
written against the old grammar changes only those spellings, e.g.

```lichen
compute.jit (p : (Int, Int) => …)   // no longer a tuple type
compute.jit (p : <Int, Int> => …)   // a tuple of two Ints
```

The parse tests for the paren/angle tuple spellings are the behavioural record.
