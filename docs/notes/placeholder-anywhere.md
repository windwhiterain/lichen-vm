# `_` is a placeholder anywhere (type and value)

> Status: **implemented** on `feature/placeholder-anywhere`.  Supersedes the
> "type position only" rule in [`language-spec.md`](../language-spec.md).  The
> incremental-parse note
> [`incremental-parse-compile.md`](incremental-parse-compile.md) still describes
> the `Placeholder` / `ErrorBlock` split — unchanged here.

## Decision: `_` is always a placeholder, never a name

Previously `_` was an inference placeholder **only** in type position (the
right side of `:`, and the components of the type forms under it); in term
(meaning value) position it was an ordinary identifier, so `_ = 5; _` and
`(_ => _) 5` were a discard binding and a discard lambda.

Under **Design B** `_` is a placeholder hole in **any** position — type and
value alike — and is **never** a name:

- `5 : _` — type inference (unchanged).
- `_ : Int` — a typed *value* hole: checks, the type slot binds to `Int`, and
  the value stays underdetermined (`Parameterized`).
- `f _`, `(1, _)` — a value hole the context unifies.
- `_ = 5` and `_ => e` — **parse errors** (`_` cannot be bound / a parameter).

## How it works

`_` is now its own lexer token (`TokenKind::Placeholder`), not a `Name`.  The
`name` parser never matches it, so it can never be a binder name or a lambda
parameter; a bare `_` in an expression slot always parses to `Expr::Placeholder`
and lowers to `ExprKind::Placeholder`.  The type-mode post-pass
(`apply_type_mode`) no longer rewrites `_` — it only flips `Tuple` → `TypeTuple`
in type position.

Because `_` is a distinct token, the discard/binder uses are gone rather than
semantically repurposed: there is no scope-dependent "unbound `_` is a hole,
bound `_` is a name" ambiguity (that was the rejected alternative, Design A).

## Files touched

- `crates/lichen-language-lex/src/lib.rs` — `TokenKind::Placeholder`, `_` mapping.
- `crates/lichen-language-parser/src/parse.rs` — placeholder primary; type-mode
  pass no longer rewrites `_`.
- `crates/lichen-language-parser/src/ast.rs` — `Expr::Placeholder` doc.
- `crates/lichen-language-server/src/analysis.rs` — semantic-token
  classification for the new token.
- `docs/language-spec.md` — grammar + semantics + compile table.
- Tests: lexer, parser, and `pipeline` placeholder tests updated.

## Follow-up (not blocking)

The tree-sitter grammar / Zed extension highlight `_` as an identifier; a
placeholder node could be added for accurate editor coloring.  This is a
highlighting-only concern and does not affect the compiler.
