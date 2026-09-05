# No type mode: `(a, b)` is always a tuple value, `<a, b>` always a type

> Status: **implemented** on `feature/no-type-mode`.  Follows
> [`placeholder-anywhere.md`](placeholder-anywhere.md) (which made `_` a
> placeholder in any position).  Companion change to
> [`language-spec.md`](../language-spec.md), specifically the "One grammar, no
> type mode" bullet.

## The change

The parser's *type-mode* post-pass (`apply_type_mode`) is gone entirely.  It
existed only to flip `(a, b)` into a `TypeTuple` when it appeared in a type
position (the right side of `:`, a struct field, etc.).  There is no mode now —
`expr : expr` parses both sides identically:

- `(a, b)` — always a `Tuple` *value* (`check_tuple_term`).
- `<a, b>` — always a `TypeTuple` *type* (`check_tuple_type`), in every position.
- `_` — always a placeholder (its own token, see `placeholder-anywhere.md`).

This makes the spelling of a tuple type explicit: `x : <Int, Int>` is a tuple
of two `Int`s, while `x : (Int, Int)` is a *value* tuple whose elements are the
type-values `Int` and `Int` (so `x : <Type, Type>`).

## Why

A tuple type previously depended on which side of `:` its `(…)` sat — a
grammar-position distinction.  Removing it makes the grammar uniform ("types
are expressions"; type-ness is spelled by the delimiter), which also removes the
last place the parser had to know the type/value position at all.

## Migration

Paren tuple *types* become angle brackets.  The only in-tree consumers were the
`compute` tests (`compute.jit (p : (Int, Int) => …)` → `(p : <Int, Int> => …)`)
and a few comments; the `.lichen` examples already spell tuple types with
`<…>` (value tuples in `(…)`).

## Files touched

- `crates/lichen-language-parser/src/parse.rs` — delete `apply_type_mode` and its
  two call sites; comments updated.
- `crates/lichen-language-parser/src/ast.rs` — `Tuple`/`TypeTuple` docs.
- `crates/lichen-language/tests/compute.rs` — tuple types to angle brackets.
- `crates/lichen-language-parser/src/tests/parse_tests.rs` — the
  paren/angle tuple tests.
- `docs/language-spec.md`, `docs/notes/{lichen-compute,lichen-compute-parallel,
  placeholder-anywhere,frontend-syntax-separation}.md` — synced.
