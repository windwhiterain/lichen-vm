# `_` is a placeholder anywhere (type and value)

> Status: current
> Points at: `crates/lichen-language-lex/src/lib.rs` (`TokenKind::Placeholder`),
> `crates/lichen-language-parser/src/parse.rs` (the placeholder primary),
> `crates/lichen-language-parser/src/ast.rs` (`Expr::Placeholder`),
> `crates/lichen-language-server/src/analysis.rs` (semantic-token
> classification), and [`language-spec.md`](../language-spec.md) (grammar,
> semantics, compile table).
> Companion to [`no-type-mode.md`](no-type-mode.md). The incremental-parse note
> [`incremental-parse-compile.md`](incremental-parse-compile.md) describes how
> the resulting `Placeholder` / `ErrorBlock` split is handled; it is unchanged by
> this.

`_` is a **placeholder hole** in any position — type and value alike — and is
**never a name**. It is the syntax for "the inference decides this"; it is not a
discard binding and not a parameter.

- `5 : _` — type inference (the type slot is a hole).
- `_ : Int` — a typed *value* hole: it checks, the type slot binds to `Int`, and
  the value stays underdetermined (an empty slot).
- `f _`, `(1, _)` — a value hole the context unifies.
- `_ = 5` and `_ => e` — **parse errors**: `_` cannot be bound and cannot be a
  parameter.

## How it works

`_` is its own lexer token (`TokenKind::Placeholder`), not a `Name`. The `name`
parser never matches it, so it can never be a binder name or a lambda parameter;
a bare `_` in an expression slot always parses to `Expr::Placeholder` and lowers
to `ExprKind::Placeholder`.

Because `_` is a distinct token, the discard/binder uses are gone rather than
semantically repurposed: there is no scope-dependent rule of the form "an
unbound `_` is a hole, a bound `_` is a name". There is also no type-mode
post-pass that rewrites `_` — `(a, b)` is always a tuple *value* and `<a, b>`
always a tuple *type*, so a placeholder is a placeholder in both positions with
no position-dependent treatment (see [`no-type-mode.md`](no-type-mode.md)).

Anyone adding a new token to the lexer should note that `_`'s
`TokenKind` is also what the language server's semantic-token classifier keys on;
a new placeholder-like spelling must be classified there too.

## Editor highlighting (open)

The tree-sitter grammar and thus the Zed extension highlight `_` as an
identifier; a placeholder node could be added for accurate editor coloring. This
is a highlighting-only concern and does not affect the compiler. See
[tree-sitter-generated-files](tree-sitter-generated-files.md) and
[zed-extension-testing](zed-extension-testing.md) for how the generated grammar
is built and verified.
