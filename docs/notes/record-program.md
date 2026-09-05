# The top level is a block: record programs (modules)

> Status: implemented
> Points at: [`crates/lichen-language-parser`](../../crates/lichen-language-parser/)
> (`src/parse.rs`, `src/ast.rs`), [`crates/lichen-language`](../../crates/lichen-language/)
> (`src/compile.rs`, `src/session.rs`),
> [`crates/lichen-language-server`](../../crates/lichen-language-server/) (`src/analysis.rs`),
> the language spec, and the module/library `*.lichen` files.

## The idea

The top level of a source file is exactly a `{ … }` block body, terminated by the
end of the input — **not** by a separator.  A body is a (possibly `pub`-marked)
statement list separated by separators, with an **optional tail expression**.  The
tail is identified **by its position at the end of the body**, in a separate
resolution step — not baked into the token grammar.  A body ending in a binding
(no tail) is a **record program**: a module whose value is an anonymous struct of
the exported bindings.

This means a library file exports its bindings *directly*, with no wrapping block:

```
# before: { succ = x => x + 1; add = x => y => x + y }
succ = x => x + 1
add = x => y => x + y
```

which `import "math.lichen"` resolves to the same anonymous struct as the old
`{ … }` form.  Since the top level is a block, `let` and `pub` work there too
(`pub a = 1\nb = 2` exports only `a`).

## Structure changes

- **Grammar** (`parse.rs`): `block_body` returns the **raw item list**
  (`Vec<(BlockItem, range)>`) — the separator-separated *syntax* only.
  `split_block_items` is the **later** resolution: `return e` anywhere is the tail,
  else the *last* bare expression is the tail, else no tail (a record).  Both
  `program_parser` and `block` call `split_block_items`, so the top level and a
  `{ … }` body share one grammar.
- **AST** (`ast.rs`): `Program.statements` is `Vec<BlockStmt>` (`pub`-capable,
  matching a block body); `Program.expr` is `Option<Expr>` (`None` ⇒ record
  program); `Program.stmt_ranges` has one entry per statement, plus one for the
  tail when present.
- **Compiler** (`compile.rs`): `compile_record_fields` is shared by
  `Expr::RecordBlock` and a record program.  A record program compiles its
  statements, sets `stmt_roots`, and builds the record root (the module struct).
- **Session** (`session.rs`): the splice and the signature walk handle the
  record / no-tail shape; a tail ↔ record **shape change** forces a full re-sign
  (the per-statement hash method differs — a module signs its statements with
  their `pub`/field identity).  The statement-region parser returns `BlockStmt`.

## Fixes along the way

- **Binding value recovery no longer consumes the `Eof` token.**  The old
  `any().filter(!Separator)` skip treated `Eof` as a skippable token, so a broken
  binding value at the very end (e.g. `ner = (2`) ate the `Eof` and made the whole
  program unparseable.  The skip now stops before `Eof`.

## Known micro-discrepancy

The splice's statement-region parser **must exclude `Eof`** to parse (including it
breaks chumsky's backtracking).  So a parse error at the very end of a program
("found the end of the program") is reported one column earlier by a bracket
window than by the whole-program parser.  The spliced *program* is identical; only
that error column differs.  Pre-existing to the region parser, orthogonal to this
feature.
