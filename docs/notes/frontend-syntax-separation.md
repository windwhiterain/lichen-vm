# Separating the lexer & parser from the language

> Status: proposed
> Points at: [`crates/lichen-language`](../../crates/lichen-language/) (`src/lex.rs`,
> `src/parse.rs`, `src/ast.rs`, `src/diag.rs`, `src/preprocess/`), the
> [`language-toolchain`](language-toolchain.md) note's §"What about a lean frontend
> crate?", and the language spec.
> Seam proposed there: lift the syntax-only modules (`lex`, `ast`, `parse`, and the
> syntactic half of `diag`) into a `lichen-language-frontend` crate and `pub use`
> them back through `lichen-language`, so existing module paths never change.

## The problem

Today `crates/lichen-language` is one crate that owns both the *syntax* front-end and
the *language* semantics, and the syntax half is pulled up into the type-system stack:

```
text ──lex──▶ tokens ──parse──▶ AST ──lower──▶ IR ──check──▶ Build
        (src/lex.rs)     (src/parse.rs)   (src/compile.rs)   (lichen-highlevel)
```

The syntax modules (`lex.rs`, `parse.rs`, `ast.rs`) depend on `lichen-highlevel` for
their source-position type (`lichen_highlevel::ir::Span`) and — through `diag.rs` —
for the checker's structured diagnostic (`lichen_highlevel::diagnostic::Diag`). The
downside of that single crate:

- **A syntax-only tool pays for the whole VM.** A formatter, a highlighter, or a
  linter needs tokens + AST. Depending on `lichen-language` drags in `lichen-highlevel`
  (IR, checker) *and* `lichen-lowlevel` (the evaluation VM) and the compute/`wasmi`
  stack — nothing a printer uses.
- **The syntax is not independently reusable.** An editor that wants to re-lex/parse
  a buffer and stop (no type checking) must import the language crate and its whole
  dependency tree. `lichen-language-server` already does exactly this today.
- **The type-system stack leaks into the grammar.** `ast.rs` uses `Span` from
  `lichen-highlevel`, so the AST's shape is coupled to where `Span` happens to live,
  not to anything semantically necessary.

The goal is a *clean seam*: a new crate that owns text → tokens → AST, depending on
nothing but a parser combinator and a lexer generator, with the semantics (lowering,
checking, evaluation) living strictly above it.

## Current coupling (inventory)

| File | Depends on (outside the crate) | Role |
|---|---|---|
| `src/ast.rs` | `lichen_highlevel::ir::Span` | AST node definitions (`Expr`/`Stmt`/`Program`…) |
| `src/lex.rs` | `logos`, `lichen_highlevel::ir::Span`, `crate::diag` | tokens, `Lexed`, `lex`/`lex_with`/`lex_resume`, `line_starts`/`line_col` |
| `src/parse.rs` | `chumsky`, `lichen_highlevel::ir::Span`, `crate::ast`, `crate::lex`, `crate::diag` | parser, `apply_type_mode` (mode post-pass), `collect_error_blocks`, `parse_statement_region*` |
| `src/diag.rs` | `lichen_highlevel::ir::Span`, `lichen_highlevel::diagnostic::Diag<LangProgram>`, `crate::program::LangProgram` | `Diag { span, message, stage, check }`, `Stage { Preprocess,Lex,Parse,Resolve,Check }` |
| `src/preprocess/lex.rs` | `logos` | directive-block body lexer (byte `(u32,u32)` ranges, self-contained) |
| `src/preprocess/parse.rs` | `crate::preprocess::lex` | directive-block body parser (`Directive`) |
| `src/preprocess/mod.rs` | `lichen_highlevel::ir::Span`, `lichen_lowlevel::StaticNodeId`, `crate::package::PackageStore`, `crate::lex` (`line_col`/`line_starts`), `crate::diag` | `scan_block`, `split_block`, import resolution |
| `src/compile.rs` | `lichen_highlevel`, `crate::ast`, `crate::diag`, `crate::preprocess::ResolvedImport` | AST → IR lowering + name resolution (semantics) |

Existing external consumers of the syntax surface:

| Consumer | Uses |
|---|---|
| `crates/lichen-language-server/src/analysis.rs` | `ast::{Expr,Program,Stmt,Binding}`, `diag::{Diag,Stage}`, `lex::*`, `parse`, `preprocess` |
| `crates/lichen-language-server/src/lsp.rs` | `lex::line_col` / `lex::line_starts` |
| `crates/lichen-language/tests/pipeline.rs` | `diag::Stage` |
| `tree-sitter-lichen` | independent grammar, does **not** consume the frontend |

Note `Span = (u32, u32)` is a **transparent type alias** (a tuple). That is the key to
the whole separation being a refactor: two crates can each declare
`pub type Span = (u32, u32)` and the types interop freely, because `Span` *is*
`(u32, u32)` — there is no nominal newtype to break.

## Target structure

Introduce `crates/lichen-language-frontend`:

```
crates/lichen-language-frontend/
  Cargo.toml            # deps: logos, chumsky.  NO lichen-highlevel,
                        #      NO lichen-lowlevel, NO wasmi, NO compute.
  src/
    span.rs             # pub type Span = (u32,u32); line_starts, line_col, byte_range
    diag.rs             # SyntaxDiag { span, message, stage: SyntaxStage }
                        #   SyntaxStage { Preprocess, Lex, Parse }   (checker-free)
    lex.rs              # the main lexer (moves from lichen-language/src/lex.rs)
    ast.rs              # Expr/Stmt/Program/… (moves from src/ast.rs)
    parse.rs            # the parser (moves from src/parse.rs)
    directive.rs        # the @{…@} block body lexer+parser + scan/split
                        #   (moves from src/preprocess/lex.rs + parse.rs; pure syntax)
    lib.rs              # re-exports: pub mod span/lex/ast/parse/directive/diag
```

What moves, what stays:

| Piece | Destination | Why |
|---|---|---|
| `ast.rs`, `lex.rs`, `parse.rs` | frontend `src/{ast,lex,parse}.rs` | pure syntax, `logos` + `chumsky` only |
| `preprocess/{lex,parse}.rs` | frontend `src/directive.rs` | the block body is already checker-free (byte ranges, no `Span`) |
| `span.rs` (`line_starts`/`line_col`/`Span`) | frontend `src/span.rs` | shared by main lexer, directive block, and tooling |
| the Lex/Parse/Preprocess half of `diag.rs` | frontend `src/diag.rs` as `SyntaxDiag` | check-free diagnostics |
| `preprocess/mod.rs` (the orchestrator) | **stays** in `lichen-language` | resolves imports via `PackageStore` (semantics, `StaticNodeId`) |
| `compile.rs` (lowering + resolve) | **stays** in `lichen-language` | semantics |
| `program.rs`, `session.rs`, `render.rs`, `run.rs`, `package.rs`, `persist.rs`, `readme.rs`, `main.rs` | **stay** in `lichen-language` | semantics / tooling |
| `lib.rs` (pipeline glue) | **stays** in `lichen-language` | merges frontend + resolve + check diagnostics |

### Dependency graph, after

```
lichen-language-frontend   logos, chumsky            ← text → tokens → AST (+ SyntaxDiag)
        │ (typed AST + spans)
        ▼
lichen-language            + lichen-highlevel        ← lower/resolve/check
        │                                            (owns Diag/Stage, Program, session)
        ▼
lichen-highlevel                                     ← checker, IR
        │
        ▼
lichen-lowlevel                                      ← VM

lichen-language-server    → lichen-language-frontend (syntax)  + lichen-language (full)
lichen-language-zed       → lichen-language-frontend (syntax)
tree-sitter-lichen        (independent grammar)
```

**The win:** `lichen-language-frontend` has *no* dependence on `lichen-highlevel`,
`lichen-lowlevel`, or the compute/`wasmi` stack. A printer / highlighter / linter can
depend on the frontend crate alone.

## The decoupling seams

### 1. `Span`

`Span` is `pub type Span = (u32, u32)` — a transparent alias. The frontend declares its
own `pub type Span = (u32, u32)`; `lichen_highlevel::ir` keeps its own identical alias.
Because these are the *same* underlying tuple type, a frontend AST's `Span` is accepted
anywhere a highlevel `Span` is expected, with zero conversion. No newtype, no nominal
break. (If a real newtype were ever introduced, it would be the one breaking change to
design around — worth avoiding, hence the alias.)

### 2. `Diag` / `Stage`

The frontend produces `Lex`/`Parse`/`Preprocess` diagnostics and knows nothing of the
checker. It defines a check-free

```rust
// lichen-language-frontend::diag
pub enum SyntaxStage { Preprocess, Lex, Parse }
pub struct SyntaxDiag { pub span: Option<Span>, pub message: String, pub stage: SyntaxStage }
```

`lichen-language::diag` keeps the wide `Stage` and `Diag` (adding `Resolve`, `Check` and
the `check: Option<Box<highlevel::diagnostic::Diag<LangProgram>>>` field) and maps a
`SyntaxDiag` into it — a pure `From`/`extend` over `span`/`message`/`stage`, no loss.
`lib.rs::frontend_at` merges: `lex`/`parse` (frontend, `SyntaxStage`) → `Diag` at
`Stage::Lex`/`Stage::Parse`, `preprocess` (frontend, `SyntaxStage::Preprocess`) →
`Diag` at `Stage::Preprocess`, then lowering/check diagnostics append at
`Resolve`/`Check`. The public `lichen_language::diag::{Diag, Stage}` contract is unchanged.

### 3. The `@{…@}` preprocessor block

The block *body* is checker-free already: `preprocess/lex.rs` + `parse.rs` use only byte
ranges and produce `Directive`s. Those move to `frontend/src/directive.rs` (with
`scan_block`/`split_block`). The *orchestrator* `preprocess/mod.rs` stays in
`lichen-language`: it needs `PackageStore::resolve_import` and `liche_lowlevel::StaticNodeId`
to turn a `Directive::Import` into a [`ResolvedImport`](../../crates/lichen-language/src/preprocess/mod.rs).
`split_block`/`block_directives`/`block_metadata` (no resolution, used by `readme` /
`sync-readme`) are thin wrappers over the frontend directive module and can either stay
in `liche-language` (re-exporting the frontend pieces) or move to the frontend.

## Migration plan (each step keeps `cargo check`/`cargo test` green)

The existing note already commits to **back-compat by re-export**: existing module paths
(`lichen_language::lex::…`) never change, so `lichen-language-server` and the tests keep
compiling unedited until you choose to point them at the new crate.

1. **Scaffold the crate.** Add `crates/lichen-language-frontend` (deps `logos`,
   `chumsky` only) and register it in the workspace `Cargo.toml` `members`. Empty
   `lib.rs`. `cargo check` still green (no consumers yet).
2. **Move `Span` utilities.** Add `span.rs` (`line_starts`, `line_col`, `byte_range`,
   `pub type Span`). `lichen-language`'s `lex.rs`/`preprocess` switch imports to the
   frontend `span` (or the frontend re-exports). Identity — `(u32,u32)` — means no
   behavior change.
3. **Move the AST.** `ast.rs` → frontend `ast`. Straight `pub mod ast` re-export from
   `lichen-language`: `pub use lichen_language_frontend::ast;`.
4. **Move the lexer.** `lex.rs` → frontend `lex` (switch to `SyntaxDiag`/`SyntaxStage`).
   Re-export: `pub use lichen_language_frontend::lex;`.
5. **Move the parser.** `parse.rs` → frontend `parse` (switch `Diag`/`Stage` → the
   frontend's, keep the wide `Diag` mapping in `lichen-language`). Re-export.
6. **Move the directive mini-frontend.** `preprocess/{lex,parse}.rs` → frontend
   `directive`. `preprocess/mod.rs` re-exports the frontend directive lexer/parser and
   keeps the import-resolution orchestrator.
7. **Split `diag.rs`.** Add `SyntaxDiag`/`SyntaxStage` to the frontend; narrow
   `lichen-language::diag` to reference them (or keep a copy and convert). Restore the
   exact round-trip in `frontend_at`/`build_report`.
8. **Decouple `lichen-language`'s deps.** Drop the now-redundant direct dep edges that
   no longer need to be direct (e.g. `chumsky`/`logos` move to the frontend crate).
9. **Optional: point consumers at the frontend crate.** `lichen-language-server`
   `analysis.rs`/`lsp.rs` (syntax imports), `pipeline.rs` (`Stage`), and any future
   formatter/linter take `lichen_language_frontend::` paths directly. The re-export shim
   can be removed once no internal consumer uses it — but keeping it is harmless and
   keeps downstream (the Zed WASM `grammars/lichen` vendored copy) source-compatible.

### What stays public in `lichen-language` (unchanged)

Even after the move, `lichen-language` re-exports `pub use lichen_language_frontend::{ast, lex, parse};`
(and a frontend `span` / `diag`), and keeps `pub mod diag` (the wide `Diag`/`Stage`),
`pub mod program`, `pub mod compile`, `pub mod session`, `pub mod run`, `pub mod render`,
`pub mod package`, `pub mod persist`, `pub mod readme`, `pub mod preprocess`. So
`lichen_language::lex::Token`, `lichen_language::ast::Expr`, and the `frontend*`/
`compile*`/`BufferSession` pipelines all resolve exactly as today.

## Verification

Per [`AGENTS.md`](../../AGENTS.md), after each step:

```
cargo check            # compilation passes
cargo test             # behaviour correct (lex/parse/session/pipeline/readme/lsp)
cargo fix --allow-dirty && cargo fmt   # final tidy
cargo run -p lichen-language -- crates/lichen-language/examples/programs   # example parity
```

Because the AST/lexer/parser move is a pure relocation (the types are the same, `Span`
is a transparent alias), the suite should be untouched except for import-path updates.
The one place to watch is the `Symbol =>`/`Diag` round-trip in `frontend_at` (the
`SyntaxDiag` → `Diag` merge) and the `session.rs` incremental path, which calls
`lex::lex_resume` and `parse::parse_statement_region` — those signatures must survive
identical across the move.

## Risks

- **`Diag` `Send` / arena isolation.** `parse.rs` runs the combinator grammar on a
  large-stack worker *specifically* because `Diag` is `!Send` (it owns a boxed highlevel
  `Diag`). Moving `parse` to the frontend (which has no highlevel `Diag`) lets the
  frontend's `SyntaxDiag` be `Send`; the wide `Diag` stays `!Send` and is assembled in
  `lichen-language`. Net effect: the frontend's parse worker can be a small-stack/simple
  one, and `Doc: Send` becomes reachable (the language-toolchain note already flags this
  as the follow-up that would let the server cache a `Doc` per URI).
- **The Zed vendor copy.** `crates/lichen-language-zed/grammars/lichen/crates/…` is a
  vendored snapshot of the whole workspace for the WASM build. Any crate move must be
  mirrored there or the Zed build breaks. Treat the vendored copy as a checkpoint to
  re-sync, not as a live input.
- **Re-export aliasing.** Keeping `pub use …frontend::lex` in `lichen-language` keeps
  the old paths, but means the `lex` module is no longer *defined* there — tooling that
  `pub use`s it directly (beyond re-export) still works, but any macro/codegen that
  introspects the module path is unaffected (module paths re-export transparently).
- **Rustdoc intra-links.** The moved modules cross-reference `crate::compile`
  (`ast.rs` doc comments), `crate::program`, and the checker `Diag`. When they land in
  the frontend crate those links must be re-pointed to `lichen-language`'s paths (or
  reworded) or `cargo doc` will warn/break. This is cosmetic but easy to miss.
