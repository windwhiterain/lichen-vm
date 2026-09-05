# Separating the lexer & parser from the language

> Status: proposed
> Points at: [`crates/lichen-language`](../../crates/lichen-language/) (`src/lex.rs`,
> `src/parse.rs`, `src/ast.rs`, `src/diag.rs`, `src/compile.rs`, `src/lib.rs`,
> `src/preprocess/`), [`crates/lichen-highlevel`](../../crates/lichen-highlevel/)
> (`src/ir.rs`), [`crates/lichen-language-server`](../../crates/lichen-language-server/)
> (`src/analysis.rs`), and the language spec.
> Design principle (revised): **`lex` owns the source span; the parser consumes the
> token's span; `highlevel` is span-free.**

## The problem

Today `crates/lichen-language` is one crate that owns both the *syntax* front-end and
the *language* semantics, and the syntax half is pulled up into the type-system stack:

```
text ──lex──▶ tokens ──parse──▶ AST ──lower──▶ IR ──check──▶ Build
        (src/lex.rs)     (src/parse.rs)   (src/compile.rs)   (lichen-highlevel)
```

Worse, **`Span` is defined in `lichen-highlevel`** (`pub type Span = (u32, u32)` in
`ir.rs`), so the type-system stack *owns* the source-position vocabulary. `lex.rs`,
`parse.rs`, `ast.rs`, `diag.rs`, and `compile.rs` all import it from there, and the
highlevel IR stores a span on every node (`Expr { kind, span }`), threading it through
every `alloc*` method.

The downsides:

- **A syntax-only tool pays for the whole VM.** A formatter, a highlighter, or a linter
  needs tokens + AST. Depending on `lichen-language` drags in `lichen-highlevel`
  (IR, checker) *and* `lichen-lowlevel` (the evaluation VM) and the compute/`wasmi`
  stack — nothing a printer uses.
- **Source position lives in the wrong layer.** `Span` is a *frontend* concern; it
  should be produced by `lex` and consumed leftward by the parser. Instead it is owned
  at the bottom of the stack, so the grammar and the checker are both coupled to where
  it happens to live. Highlevel's own `Loc` diagnostic is already "source-blind"
  (`ir.rs` docs) — only the IR node carries a redundant span.
- **The syntax is not independently reusable.** An editor that wants to re-lex/parse a
  buffer and stop must import the whole dependency tree.

## The target dependency graph

The crate split is pinned by this graph (arrows read *"flows into / is consumed by"*;
the compile-time `depends on` edges are the reverse):

```
lex ──▶ parser ──▶ language ◀── highlevel ◀── lowlevel
```

- `parser → lex`: the parser consumes `lex`'s `Token`/`Span`.
- `language → {parser, highlevel}`: `language` ties the syntax (the AST) to the
  semantics (the highlevel IR + checker).
- `highlevel → lowlevel`: the checker builds lowlevel modules.
- **`highlevel` has no edge to `lex`** — it can never see `Span`. That *forces*
  `highlevel` to be span-free, and forces the source-position index to live in
  `language` (the one crate that can see both the AST's spans and highlevel's IR).

## The principle

```
source ──lex──▶ Token{ range, span: Span }     Span DEFINED IN lex
                        │  token.span
                        ▼
                    parse ──▶ AST (every node carries a lex::Span)
                        │
                        ▼
              lower ──▶ IR ──▶ highlevel is span-free (no Span anywhere)
```

1. **`lex` defines the source span.** `pub type Span = (u32, u32)` lives in the lexer
   crate, along with `line_starts`/`line_col`/`byte_range`. Lexing is the one thing that
   turns raw bytes into a source position, so it owns the type.
2. **The parser consumes the token's span.** `Token.span: Span`. Every AST node's span is
   just the lexer's `Span` carried by the token that started it. The parsed AST is
   `lex::Span`-typed throughout; there is no second span type.
3. **`highlevel` is span-free.** `Expr { kind }` — no `span` field, no `Option<Span>` on
   any `alloc*`, no `Span` type at all. Highlevel never sees a source position (its `Loc`
   diagnostic is already source-blind). The `language` crate keeps positions in its own
   `ExprId → Span` index, built during lowering, consulted only when it maps a checker
   message back to a source caret.

## Current coupling (inventory)

| File | Depends on (outside the crate) | Role |
|---|---|---|
| `src/lex.rs` | `logos`, `lichen_highlevel::ir::Span`, `crate::diag` | tokens, `Lexed`, `lex`/`lex_with`/`lex_resume`, `line_starts`/`line_col` |
| `src/ast.rs` | `lichen_highlevel::ir::Span` | AST node definitions (`Expr`/`Stmt`/`Program`…) |
| `src/parse.rs` | `chumsky`, `lichen_highlevel::ir::Span`, `crate::ast`, `crate::lex`, `crate::diag` | parser, `collect_error_blocks`, `parse_statement_region*` (no type-mode pass — see [`no-type-mode.md`](no-type-mode.md)) |
| `src/diag.rs` | `lichen_highlevel::ir::Span`, `lichen_highlevel::diagnostic::Diag<LangProgram>`, `crate::program::LangProgram` | `Diag { span, message, stage, check }`, `Stage { Preprocess,Lex,Parse,Resolve,Check }` |
| `src/compile.rs` | `lichen_highlevel`, `crate::ast`, `crate::diag`, `crate::preprocess::ResolvedImport` | AST → IR lowering + name resolution; passes spans to `alloc*`, copies spans placeholder↔value |
| `src/preprocess/{lex,parse}.rs` | `logos` (lex only) | directive-block body lexer/parser (byte `(u32,u32)` ranges, self-contained) |
| `src/preprocess/mod.rs` | `lichen_highlevel::ir::Span`, `lichen_lowlevel::StaticNodeId`, `crate::package::PackageStore`, `crate::lex`, `crate::diag` | `scan_block`, `split_block`, import resolution |
| `crates/lichen-highlevel/src/ir.rs` | — | **defines** `pub type Span`; `Expr { kind, span }`; `Option<Span>` on `alloc*` methods |

Existing readers of **IR node spans** (the sites that must move to an external index
when highlevel goes span-free):

| Site | Reads |
|---|---|
| `crates/lichen-language/src/lib.rs:153` | `build.ir[loc.expr].span` — map a checker `Loc` back to a source span |
| `crates/lichen-language/src/compile.rs:201` | copies the resolved value's span onto the placeholder `self.ir.expr[p].span` |
| `crates/lichen-language/src/tests/compile_tests.rs:105,148` | asserts `expr.span` / `ir[root].span` |
| `crates/lichen-language-server/src/analysis.rs:208,250,344` | `e.span`, `build.ir[id].span`, `build.ir[container].span` |
| `crates/lichen-highlevel/tests/checker.rs` (many) | sets/reads `ir.expr[…].span` to assert diagnostic positions |

Existing external consumers of the syntax surface:

| Consumer | Uses |
|---|---|
| `crates/lichen-language-server/src/analysis.rs` | `ast::{Expr,Program,Stmt,Binding}`, `diag::{Diag,Stage}`, `lex::*`, `parse`, `preprocess` |
| `crates/lichen-language-server/src/lsp.rs` | `lex::line_col` / `lex::line_starts` (and the `Span` type) |
| `crates/lichen-language/tests/pipeline.rs` | `diag::Stage` |

## Target structure

Two new crates plus the span-freed highlevel:

```
crates/lichen-language-lex/
  Cargo.toml            # deps: logos            (NOTHING else; no highlevel/lowlevel)
  src/lib.rs            # pub: Span, line_starts, line_col, byte_range,
                        #      Token{ kind, span, range }, TokenKind, Lexed,
                        #      lex / lex_with / lex_resume, LexDiag{span,message}

crates/lichen-language-parser/
  Cargo.toml            # deps: chumsky, lichen-language-lex
  src/lib.rs            # pub mod ast: Expr/Stmt/Program/Binding/… (spans are lex::Span)
                        #        parse: parse, parse_statement_region*,
                        #               collect_error_blocks
                        #        diag: ParseDiag{span,message}
```

The `Span` type lives in `lichen-language-lex`; the AST and parser live in
`lichen-language-parser` and import `Span`/`Token` from the lex crate. Nowhere in
`lichen-highlevel` is a span. `language` re-exports both crates so existing paths hold.

What moves, what stays:

| Piece | Destination | Why |
|---|---|---|
| `src/lex.rs` + `Span`/`line_starts`/`line_col`/`byte_range` | `crates/lichen-language-lex` | `lex` is the source-position authority; `logos` only |
| `src/ast.rs` | `crates/lichen-language-parser` | the AST is the parser's output type |
| `src/parse.rs` | `crates/lichen-language-parser` | parser over `lex::Token`; consumes `lex::Span` |
| `lichen-highlevel::ir::Span` + `Expr.span` + alloc span params | **removed** from `lichen-highlevel` | highlevel is span-free (enforced by the graph) |
| `compile.rs` (lowering + resolve) | **stays** in `lichen-language` (builds the `SpanIndex`) | semantics — the only crate on both the parser edge and the highlevel edge |
| `diag.rs` wide `Diag`/`Stage` | **stays** in `lichen-language` | adds `Resolve`/`Check` + checker payload |
| `src/preprocess/mod.rs` (orchestrator) | **stays** in `lichen-language` | resolves imports via `PackageStore` (`StaticNodeId`) |
| `src/preprocess/{lex,parse}.rs` (directive block) | **stays** in `lichen-language` (see seam #5) | preprocessor/package-specific; byte-range, not `Span` |
| `program.rs`, `session.rs`, `render.rs`, `run.rs`, `package.rs`, `persist.rs`, `readme.rs`, `main.rs` | **stay** in `lichen-language` | semantics / tooling |
| `lib.rs` (pipeline glue) | **stays** in `lichen-language` | merges lex/parse + resolve + check diagnostics |

### Back-compat re-export in `lichen-language`

```rust
// lichen-language/src/lib.rs
pub use lichen_language_lex as lex;              // lichen_language::lex::Token, ::lex::Span, …
pub use lichen_language_parser as parse;         // lichen_language::parse::parse, ::parse::Parsed, …
pub use lichen_language_parser::ast;             // lichen_language::ast::Expr, ::ast::Program, …
```

Every existing module path (`lichen_language::lex::Token`, `lichen_language::ast::Expr`,
`lichen_language::parse::parse`, the `frontend*`/`compile*`/`BufferSession` pipelines)
resolves identically, so `lichen-language-server` and the tests keep compiling unedited
until you choose to point them at the new crates.

## The decoupling seams

### 1. `Span` lives in `lichen-language-lex`

`lex` declares `pub type Span = (u32, u32)` (and `line_starts`, `line_col`,
`byte_range`). `Token.span: Span`. The parser, AST, `ParseDiag`, `LexDiag`, and
`lichen-language::diag` all import `Span` from `lichen-language-lex`. `lichen-highlevel`
stops defining it entirely.

`Span` stays a **transparent alias** — a tuple, not a newtype. That is deliberate: it is
cheap to copy, trivially comparable, and usable directly wherever the source→span math
lives, with no nominal break between the lex crate and the rest of the language layer.

### 2. The parser consumes token spans

`Token.span` is set by `lex`. Every AST node's span is the `lex::Span` of the token that
started it (`Expr::Int(n, t.span)`, `span_at(tokens, …)`, etc.). The parser never
recomputes a position — it only forwards the lexer's. The `Glue`/`Separator`/`Eof`
bookkeeping that feeds the postfix-vs-application decision stays in `lex` (it already
does).

### 3. `highlevel` is span-free (the `language`-level index)

Remove from `crates/lichen-highlevel/src/ir.rs`:

- `pub type Span = (u32, u32)` (and its doc comment);
- the `pub span: Option<Span>` field on `Expr<L>`;
- the `span: Option<Span>` parameter from every `alloc*` method (`alloc`,
  `alloc_annotation`, `alloc_tuple`, `alloc_type_tuple`, `alloc_type_struct`,
  `alloc_instantiate`, `alloc_record`, `alloc_array`, `alloc_table`,
  `alloc_shallow_array`, …).

Nothing in highlevel's *logic* used the span — the checker's diagnostic `Loc` is already
a source-blind `[expr, path]` structure (`ir.rs` docs). The span only existed to be read
back by the language layer. So removing it is mechanical.

The `language` crate keeps positions in a **secondary map keyed by the IR id**, filled
exactly where it creates the IR:

```rust
// lichen-language
/// ExprId → the source span the expr lowers from.  Populated by `compile.rs`
/// as it creates each IR node; parallel to `IR.expr` (an index, so a Vec).
pub type SpanIndex = Vec<Option<Span>>;   // Span = lichen_language_lex::Span
```

The lowering loop becomes a two-step call, not one:

```rust
// compile.rs (in language) — the AST→IR lowering
let Span { .. } = node_span;
let id = self.ir.alloc(kind);        // 1. call the highlevel API → get an ExprId
self.span_index[id] = Some(node_span); // 2. stash the span in this crate's secondary map
```

Highlevel's `alloc*` signature shrinks from `alloc(kind, span) -> ExprId` to
`alloc(kind) -> ExprId`; the span never crosses the boundary. The placeholder↔value span
copy (`self.ir.expr[p].span = self.ir.expr[value].span`) becomes a map copy,
`self.span_index[p] = self.span_index[value]`.

- `frontend_at` returns `Frontend { ir, span_index, diagnostics }`.
- `build_report(ir, span_index, diagnostics, registry, native_ops)` maps a checker `Loc`
  back to a source span with `span_index[loc.expr.0 as usize]` instead of
  `build.ir[loc.expr].span` — the id still identifies the node, but the span comes from
  `language`'s map, not the IR.
- `Report` carries the `span_index` (`span_index: Option<SpanIndex>`), so the language
  server and tests can read it after the build.
- `lichen-language-server/src/analysis.rs:208,250,344` and the `compile_tests.rs`
  assertions read `span_index[id]` instead of `build.ir[id].span`.

### 4. `Diag` / `Stage` (narrow diagnostics, widened in `language`)

The lex and parser crates produce *check-free* diagnostics with no notion of the checker:

```rust
// lichen-language-lex
pub struct LexDiag { pub span: Option<Span>, pub message: String }
// lichen-language-parser
pub struct ParseDiag { pub span: Option<Span>, pub message: String }
```

These are `Send` (no highlevel payload). `lichen-language::diag` keeps the wide `Stage`
and `Diag` (adding `Resolve`, `Check` and the
`check: Option<Box<highlevel::diagnostic::Diag<LangProgram>>>` field) and maps each into
it — a pure `From`/`extend` over `span`/`message`, no loss. `lib.rs::frontend_at` merges:
`lex` → `Diag` at `Stage::Lex`, `parse` → `Stage::Parse`, `preprocess` →
`Stage::Preprocess`, then lowering/check diagnostics append at `Resolve`/`Check`. The
public `lichen_language::diag::{Diag, Stage}` contract is unchanged. Bonus: the parse
worker no longer needs a `!Send`-diag workaround (the old `(Span, String, Stage)` tuple
was only because `Diag` was `!Send`); the parser returns `ParseDiag` directly.

### 5. The `@{…@}` preprocessor block

The block *body* is checker-free and byte-range-typed. The clean option is to split it
too — move `preprocess/lex.rs` to `lichen-language-lex` as a `block` module and
`preprocess/parse.rs` to `lichen-language-parser` as a `block` module (it produces a
`Directive`, not the main AST). The simpler default keeps them in `lichen-language`
`preprocess/` for now, because the block is tiny, byte-range (not `Span`) based, and
tightly bound to `PackageStore` import resolution. Either choice leaves the orchestrator
`preprocess/mod.rs` in `lichen-language`. The `split_block`/`block_directives`/
`block_metadata` helpers used by `readme`/`sync-readme` are thin wrappers over whichever
home the block lexer/parser land in.

## Migration plan (each step keeps `cargo check`/`cargo test` green)

1. **Scaffold the lex crate.** Add `crates/lichen-language-lex` (dep `logos` only),
   register in workspace `members`. Move `Span`, `line_starts`, `line_col`, `byte_range`,
   `Token`/`TokenKind`/`Lexed`, `lex`/`lex_with`/`lex_resume`, and `src/lex.rs`'s tests.
   In `lichen-language`: `pub use lichen_language_lex as lex;` — old paths hold.
2. **Scaffold the parser crate.** Add `crates/lichen-language-parser` (deps `chumsky`,
   `lichen-language-lex`). Move `src/ast.rs` (imports `Span` from the lex crate) and
   `src/parse.rs`. In `lichen-language`:
   `pub use lichen_language_parser as parse; pub use lichen_language_parser::ast;`.
3. **Split `diag.rs`.** Introduce `LexDiag`/`ParseDiag`; have `lichen-language::diag`
   map them into the wide `Diag`/`Stage`. Restore the exact round-trip in
   `frontend_at`/`build_report`.
4. **Make `highlevel` span-free.** Remove `Span`/`Expr.span`/alloc span params from
   `ir.rs`; update `crates/lichen-highlevel/tests/checker.rs` (it sets/reads
   `ir.expr[…].span`) to assert positions through their own param or a local map.
5. **Thread `SpanIndex` through the pipeline.** `compile.rs` populates it;
   `frontend_at`/`build_report`/`Report` carry it; `lib.rs:153`, `compile_tests.rs`, and
   `lichen-language-server/src/analysis.rs` read it instead of `build.ir[id].span`.
6. **Decouple deps.** `lichen-language` drops direct `logos`/`chumsky` (they live in the
   lex/parser crates). Its remaining deps: the two new crates, `lichen-highlevel`,
   `lichen-lowlevel`, `lichen-{compute,doc,perspective,render,utils}`, and `sha2`.
7. **Optional: point consumers at the new crates.** `lichen-language-server`
   (`analysis.rs`/`lsp.rs`), `pipeline.rs` (`Stage`), and future tool crates take
   `lichen_language_lex`/`lichen_language_parser` paths directly. The re-export shim can
   be dropped once no internal consumer uses it.

### What stays public in `lichen-language` (unchanged)

`pub use lichen_language_lex as lex;` and `pub use lichen_language_parser as parse;`
`pub use lichen_language_parser::ast;` keep `lichen_language::{lex, parse, ast}` alive.
`pub mod diag` (wide `Diag`/`Stage`), `pub mod program`, `compile`, `session`, `run`,
`render`, `package`, `persist`, `readme`, `preprocess`, and `pub use lichen_language_lex::Span;`
all remain. So `lichen_language::lex::Token`, `lichen_language::ast::Expr`, the
`frontend*`/`compile*`/`BufferSession` pipelines, and `lichen_language::diag::{Diag, Stage}`
resolve exactly as today. `Report` gains a `span_index` field (additive).

## Verification

Per [`AGENTS.md`](../../AGENTS.md), after each step:

```
cargo check            # compilation passes
cargo test             # behaviour correct (lex/parse/session/pipeline/readme/lsp/highlevel checker)
cargo fix --allow-dirty && cargo fmt   # final tidy
cargo run -p lichen-language -- crates/lichen-language/examples/programs   # example parity
```

The lex/parser move is a pure relocation (types unchanged, `Span` a transparent alias).
The `highlevel` span-free step is the one with real footguns — it touches `ir.rs`,
`compile.rs` (the span copy), `lib.rs` and `analysis.rs` (the `build.ir[id].span`
readers), and `checker.rs`/`compile_tests.rs`. The suite stays green because every
`.span` read is replaced by the equivalent `span_index[id]` read, and the `SpanIndex` is
populated with exactly the spans the old node carried.

## Risks

- **`SpanIndex` drift.** The index must stay parallel to `IR.expr` (an `alloc` produces one
  `Expr` and one `SpanIndex` entry). The cleanest guard: build the `SpanIndex` inside the
  same `Compiler` that owns `ir`, so a node and its span are written together and can
  never desync.
- **`Send` isolation.** The lex/parser crates' diagnostics (and the parser worker) become
  `Send` — the old `!Send` `Diag` never enters those crates. The wide `Diag` stays `!Send`
  and is assembled in `language`. Net effect: `Doc: Send` becomes reachable (the
  language-toolchain note flags this as the follow-up that would let the server cache a
  `Doc` per URI).
- **The Zed vendor copy.** `lichen-language-zed/grammars/lichen/crates/…` is a
  vendored snapshot of the whole workspace for the WASM build. The crate move *and* the
  `highlevel` span-free change must be mirrored there or the Zed build breaks.
- **Public `Span` import churn.** `lichen_highlevel::ir::Span` disappears, so every
  external import of it (the language server's `analysis.rs`/`lsp.rs`, `compile.rs`,
  `diag.rs`, `render.rs`) must switch to `lichen_language::lex::Span` (or
  `lichen_language_lex::Span`). `cargo build` surfaces these; one-line import fixes.
- **Rustdoc intra-links.** The moved modules cross-reference `crate::compile`,
  `crate::program`, and the checker `Diag`. When they land in the lex/parser crates those
  links must be re-pointed to `lichend-language`'s paths (or reworded) or `cargo doc`
  will warn/break.
