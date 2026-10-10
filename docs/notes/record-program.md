# The top level is a block: record programs (modules)

> Status: current
> Points at: [`crates/lichen-language-parser`](../../crates/lichen-language-parser/)
> (`src/parse.rs`, `src/ast.rs`), [`crates/lichen-language`](../../crates/lichen-language/)
> (`src/compile.rs`, `src/session.rs`),
> [`crates/lichen-language-server`](../../crates/lichen-language-server/) (`src/analysis.rs`),
> the language spec, and the module/library `*.lichen` files.

The top level of a source file is exactly a `{ … }` block body, terminated by the
end of the input — **not** by a separator. A body is a (possibly `pub`-marked)
statement list separated by separators, with an **optional tail expression**. The
tail is identified **by its position at the end of the body**, in a separate
resolution step — not baked into the token grammar. A body ending in a binding
(no tail) is a **record program**: a module whose value is an anonymous struct of
the exported bindings.

This means a library file exports its bindings *directly*, with no wrapping block:

```lichen
succ = x => x + 1
add = x => y => x + y
```

which `import "math.lichen"` resolves to the same anonymous struct as an explicit
`{ … }` form would. Since the top level is a block, `let` and `pub` work there too
(`pub a = 1\nb = 2` exports only `a`).

## How the shape is resolved

- **Grammar** (`parse.rs`): `block_body` returns the **raw item list**
  (`Vec<(BlockItem, range)>`) — the separator-separated *syntax* only.
  `split_block_items` is the **later** resolution: `return e` anywhere is the tail,
  else the *last* bare expression is the tail, else no tail (a record). Both
  `program_parser` and `block` call `split_block_items`, so the top level and a
  `{ … }` body share one grammar.
- **AST** (`ast.rs`): `Program.statements` is `Vec<BlockStmt>` (`pub`-capable,
  matching a block body); `Program.expr` is `Option<Expr>` (`None` ⇒ record
  program); `Program.stmt_ranges` has one entry per statement, plus one for the
  tail when present.
- **Compiler** (`compile.rs`): `compile_record_fields` is shared by
  `Expr::RecordBlock` and a record program. A record program compiles its
  statements, sets `stmt_roots`, and builds the record root (the module struct).
- **Session** (`session.rs`): the splice and the signature walk handle the
  record / no-tail shape; a tail ↔ record **shape change** forces a full re-sign
  (the per-statement hash method differs — a module signs its statements with
  their `pub`/field identity). The statement-region parser returns `BlockStmt`.

## Two constraints worth keeping in view

- **A broken binding value must not consume `Eof`.** The recovery skip treats
  every non-separator token as skippable, so it must stop before `Eof`: a broken
  binding value at the very end (`ner = (2`) would otherwise eat the `Eof` and
  make the whole program unparseable.
- **The splice's statement-region parser excludes `Eof`** (including it breaks
  chumsky's backtracking). So a parse error at the very end of a program ("found
  the end of the program") is reported one column earlier by a bracket window
  than by the whole-program parser. The spliced *program* is identical; only that
  error column differs.

## Recovered measurements

- **The top level is a list of `stmt_roots`, not a tuple cascade.** The program
  does *not* wrap its statements in `Index(Tuple([stmt₁, …, stmtₙ, final]), n)`;
  the statement ids are recorded as `stmt_roots`, which the checker type-checks
  and evaluates one by one, and the build root is the final expression directly.
  The consequence is that a non-terminating statement is reported as a
  diagnostic instead of being silently deferred by the cascade. Nested blocks
  still use the tuple wrap (`Compiler::wrap`), so that wire form survives for
  blocks only.
- **A block root may be any expression kind.** The frontend transplants the
  value's kind into the binding's reserved placeholder, so the checker's cycle
  cut gates on **block-root membership alone** rather than on a hand-maintained
  list of kinds. A block-wide binding referencing itself through `a(0)`, `a.x` or
  `a::x` therefore checks like the `a = a + 1` control — no diagnostics, and the
  root deep-evaluates to *undecided* rather than hanging. The record block
  (`a = {x = a}`) is a fourth shape: its value is concrete, so the deep pass
  terminates on the runtime cycle guard instead.
- **A membership test over type values compares structurally.**
  `ValueExt::value_eq` compares array *handles*, so it cannot answer a test whose
  members are type values — a class node out of another module would never match.
  A class domain is therefore compared by structure, which is what keeps a
  class's nominal identity meaningful across modules.
