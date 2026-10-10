# Separating the lexer & parser from the language

> Status: current — the frontend is three layers, not one crate:
> [`crates/lichen-span`](../../crates/lichen-span/) owns the source-position protocol,
> [`crates/lichen-language-lex`](../../crates/lichen-language-lex/) produces tokens,
> [`crates/lichen-language-parser`](../../crates/lichen-language-parser/) owns the AST
> and the parser, and `lichen-highlevel` is span-free.
> [`crates/lichen-language`](../../crates/lichen-language/) re-exports the lex and
> parser crates (`pub use lichen_language_lex as lex; pub use lichen_language_parser as
> parse; pub use lichen_language_parser::ast; pub use lichen_language_parser::path;`) so
> the old module paths hold.
>
> Design principle: **`lex` produces the source span; the parser consumes the token's
> span; `highlevel` is span-free.**

## What it is and why

Before the split `crates/lichen-language` was one crate that owned both the *syntax*
front-end and the *language* semantics, and the syntax half was pulled up into the
type-system stack:

```
text ──lex──▶ tokens ──parse──▶ AST ──lower──▶ IR ──check──▶ Build
        (lex crate)     (parser crate)   (compile.rs)   (lichen-highlevel)
```

Worse, **`Span` was defined in `lichen-highlevel`**, so the type-system stack *owned* the
source-position vocabulary. The lexer, parser, AST, diagnostics, and lowering all imported
it from there, and the highlevel IR stored a span on every node, threading it through every
`alloc*` method.

The downsides that motivated the split:

- **A syntax-only tool pays for the whole VM.** A formatter, a highlighter, or a linter
  needs tokens + AST. Depending on `lichen-language` drags in `lichen-highlevel` (IR,
  checker) *and* `lichen-lowlevel` (the evaluation VM) and the compute stack — nothing a
  printer uses.
- **Source position lived in the wrong layer.** `Span` is a *frontend* concern; it should
  be produced by `lex` and consumed leftward by the parser, and a crate that only needs to
  *name* a position should not have to pull in the lexer or the language crate at all.
  Instead it was owned at the bottom of the stack, so the grammar and the checker were both
  coupled to where it happened to live. Highlevel's own `Loc` diagnostic is already
  source-blind; only an IR node carried a redundant span.
- **The syntax was not independently reusable.** An editor that wants to re-lex/parse a
  buffer and stop had to import the whole dependency tree.

## The dependency graph

The crate split is pinned by this graph (arrows read *"flows into / is consumed by"*; the
compile-time `depends on` edges are the reverse):

```
span ──▶ lex ──▶ parser ──▶ language ◀── highlevel ◀── lowlevel
```

- `lex → span`, `parser → lex`: the lexer and the parser consume the shared
  `Span`/`Token` vocabulary.
- `language → {span, parser, highlevel}`: `language` ties the syntax (the AST) to the
  semantics (the highlevel IR + checker).
- `highlevel → lowlevel`: the checker builds lowlevel modules.
- **`highlevel` has no edge to `span` or `lex`** — it can never see `Span`. That *forces*
  `highlevel` to be span-free, and forces the source-position index to live in `language`
  (the one crate that can see both the AST's spans and highlevel's IR).

## The principle

```
source ──lex──▶ Token{ range, span }        Span lives in lichen-span
                        │  token.span
                        ▼
                    parse ──▶ AST (every node carries a Span)
                        │
                        ▼
              lower ──▶ IR ──▶ highlevel is span-free (no Span anywhere)
```

1. **`lichen-span` defines the source position** (`pub type Span = (u32, u32)`,
   `line_starts`, `line_col`, `line_text`, `offset_of_span`). It is a tiny
   dependency-free crate, so a crate that only needs to *name* a source position — or
   convert a byte offset to one — does not have to pull in the lexer or the language crate.
   Lexing is the one thing that turns raw bytes into a source position, so it is the
   *producer* of the type; `lichen-span` is its shared home, and `lichen-language-lex`
   re-exports it (`pub use lichen_span::{Span, line_col, line_starts, line_text,
   offset_of_span};`), so `lichen_language_lex::Span` still resolves.
2. **The parser consumes the token's span.** `Token.span: Span`. Every AST node's span is
   just the lexer's `Span` carried by the token that started it. The parsed AST is
   `Span`-typed throughout; there is no second span type.
3. **`highlevel` is span-free.** `Expr { kind }` — no `span` field, no `Option<Span>` on
   any `alloc*`, no `Span` type at all. Highlevel never sees a source position. The
   `language` crate keeps positions in its own `ExprId → Span` index, built during
   lowering, consulted only when it maps a checker message back to a source caret.

## The structure

Three crates plus the span-freed highlevel:

```
crates/lichen-span/
  Cargo.toml            # no dependencies
  src/lib.rs            # pub: Span = (u32, u32), line_starts, line_col, line_text,
                        #      offset_of_span

crates/lichen-language-lex/
  Cargo.toml            # deps: lichen-span, logos    (no highlevel/lowlevel)
  src/lib.rs            # pub: Token{ kind, span, range }, TokenKind, Lexed, LexDiag,
                        #      lex / lex_with / lex_resume,
                        #      re-exported Span, line_starts, line_col
  src/tests/            # lexer tests

crates/lichen-language-parser/
  Cargo.toml            # deps: chumsky, stacksafe, lichen-language-lex
  src/lib.rs            # pub mod ast: Expr/Stmt/Program/Binding/… (spans are Span)
                        #        parse: parse, parse_statement_region*,
                        #               collect_error_blocks
                        #        path: the occurrence-path vocabulary
                        #        diag: ParseDiag{span,message}
```

What lives where, and why:

| Piece | Destination | Why |
|---|---|---|
| `Span`, `line_starts`, `line_col` | `crates/lichen-span` | the source-position protocol every frontend crate agrees on; a crate that only names a position need not pull in the lexer |
| the lexer, `Token`/`TokenKind`/`Lexed`, `lex`/`lex_with`/`lex_resume`, `LexDiag` | `crates/lichen-language-lex` | `lex` is the source-position producer; `logos` only |
| the AST, the parser, `ParseDiag`, the `path` vocabulary | `crates/lichen-language-parser` | the AST is the parser's output type; it consumes `lex::Token`/`Span` |
| `Span` + `Expr.span` + alloc span params | **removed** from `lichen-highlevel` | highlevel is span-free (enforced by the graph) |
| `compile.rs` (lowering + resolve, and the `SpanIndex`) | **stays** in `lichen-language` | semantics — the only crate on both the parser edge and the highlevel edge |
| the wide `Diag`/`Stage` | **stays** in `lichen-language` | adds `Resolve`/`Check` + the checker payload |
| the preprocessor | **moved** to `crates/lichen-preprocess` (a `pub use` shim in `language`) | see [preprocessor-isolation](preprocessor-isolation.md) |
| `program.rs`, `session.rs`, `render.rs`, `run.rs`, `package.rs`, `persist.rs` | **stay** in `lichen-language` | semantics / tooling |
| `cli.rs` + the `lichen-compiler` binary | `lichen-compiler` | the library must not link `clap` for every embedder |
| `readme.rs` + the `sync-readme` binary | `lichen-tools` | repo tooling that panics outside a checkout must not ship in a library |
| `lib.rs` (pipeline glue) | **stays** in `lichen-language` | merges lex/parse + resolve + check diagnostics |

### The back-compat re-export in `lichen-language`

```rust
// lichen-language/src/lib.rs
pub use lichen_language_lex as lex;              // lichen_language::lex::Token, ::lex::Span, …
pub use lichen_language_lex::{LexDiag, Span};
pub use lichen_language_parser as parse;         // lichen_language::parse::parse, ::parse::Parsed, …
pub use lichen_language_parser::ast;             // lichen_language::ast::Expr, ::ast::Program, …
pub use lichen_language_parser::path;            // lichen_language::path::Path, …
pub use lichen_language_parser::{ParseDiag, Parsed};
```

Every existing module path (`lichen_language::lex::Token`, `lichen_language::ast::Expr`,
`lichen_language::parse::parse`, the `frontend*`/`compile*`/`BufferSession` pipelines)
resolves identically, so `lichen-language-server` and the tests keep compiling unedited.
A consumer that wants only the syntax can point at `lichen-language-lex` /
`lichen-language-parser` directly instead.

## The decoupling seams

### 1. The span protocol lives in `lichen-span`

`lichen-span` declares `pub type Span = (u32, u32)` with `line_starts`, `line_col`,
`line_text` and `offset_of_span`. `Token.span: Span`, and the parser, AST, `ParseDiag`,
`LexDiag` and `lichen-language::diag` all use that one type. `lichen-highlevel` defines no
span at all.

`Span` stays a **transparent alias** — a tuple, not a newtype. That is deliberate: it is
cheap to copy, trivially comparable, and usable directly wherever the source→span math
lives, with no nominal break between the crates that consume it. The column counts
**bytes**, not characters and not UTF-16 code units; the language server converts to LSP's
0-based UTF-16 `character` at its own boundary.

### 2. The parser consumes token spans

`Token.span` is set by `lex`. Every AST node's span is the `Span` of the token that started
it. The parser never recomputes a position — it only forwards the lexer's; its own
`byte_range` helper turns a `SimpleSpan` into the token's `(u32, u32)` byte range. The
`Glue`/`Separator`/`Eof` bookkeeping that feeds the postfix-vs-application decision stays
in `lex`.

### 3. `highlevel` is span-free (the `language`-level index)

`crates/lichen-highlevel/src/ir.rs` has no `Span`, no `span` field on `Expr`, and no span
parameter on any `alloc*`. Nothing in highlevel's *logic* needed it — the checker's
diagnostic `Loc` is a source-blind `[expr, path]` structure — so the span only existed to
be read back by the language layer.

The `language` crate keeps positions in a **secondary map keyed by the IR id**, filled
exactly where it creates the IR:

```rust
// lichen-language
/// ExprId → the source span the expr lowers from.  Populated by `compile.rs`
/// as it creates each IR node; parallel to `IR.expr` (an index, so a Vec).
pub type SpanIndex = Vec<Option<Span>>;
```

A lowering step is therefore two operations, not one: allocate the IR node through
highlevel's API, then stash the span in this crate's map. The placeholder↔value span copy
becomes a map copy.

- `frontend_at` returns `Frontend { ir, span_index, diagnostics }`.
- `build_report` maps a checker `Loc` back to a source span with
  `span_index[loc.expr.0 as usize]` instead of reading a span off the IR node — the id
  still identifies the node, but the span comes from `language`'s map.
- `Report` carries the `span_index` (`span_index: Option<compile::SpanIndex>`), so the
  language server and tests can read it after the build.

**`SpanIndex` drift is the risk to keep in view.** The index must stay parallel to
`IR.expr` (one `alloc` produces one `Expr` and one `SpanIndex` entry). The guard is that
the `SpanIndex` is built inside the same compiler object that owns the IR, so a node and
its span are written together and cannot desync.

### 4. `Diag` / `Stage` (narrow diagnostics, widened in `language`)

The lex and parser crates produce *check-free* diagnostics with no notion of the checker:

```rust
// lichen-language-lex
pub struct LexDiag { pub span: Option<Span>, pub message: String }
// lichen-language-parser
pub struct ParseDiag { pub span: Option<Span>, pub message: String }
```

These are `Send` (no highlevel payload). `lichen-language::diag` keeps the wide `Stage` and
`Diag` (adding `Resolve`, `Check` and the
`check: Option<Box<highlevel::diagnostic::Diag<LangProgram>>>` field) and maps each into it
— a pure `From`/`extend` over `span`/`message`, no loss. `lib.rs::frontend_at` merges: lex
→ `Diag` at `Stage::Lex`, parse → `Stage::Parse`, preprocess → `Stage::Preprocess`, then
lowering/check diagnostics append at `Resolve`/`Check`. The public
`lichen_language::diag::{Diag, Stage}` contract is unchanged. A parse worker therefore
needs no `!Send`-diagnostic workaround: the parser returns `ParseDiag` directly.

## What stays public in `lichen-language`

`lex`, `parse`, `ast` and `path` (the re-exports above), `Span`, `LexDiag`, `ParseDiag`,
`Parsed`, `pub mod diag` (the wide `Diag`/`Stage`), `program`, `compile`, `session`, `run`,
`render`, `package`, `persist`, `readme`, and the `preprocess` shim all remain. So
`lichen_language::lex::Token`, `lichen_language::ast::Expr`, the
`frontend*`/`compile*`/`BufferSession` pipelines, and `lichen_language::diag::{Diag, Stage}`
resolve as the split's callers expect.
