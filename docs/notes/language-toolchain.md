# Language toolchain: one frontend, many tools

> Status: design (current)
> Points at: the crate boundaries below, the frontend in
> [`crates/lichen-language`](../../crates/lichen-language/), and the two new tool
> crates `lichen-language-server` and `lichen-language-zed`.

This note is the design for the editor/toolchain layer of lichen-vm: a language
server and a Zed extension today, and the hooks for more tools (a formatter, a
linter, a repl, …) later. The whole design turns on one principle, and the rest
of this note is that principle unpacked.

## The one principle: the frontend is the artifact, not the tool

A language frontend produces a chain of artifacts from a single source text:

```
text ──lex──▶ tokens ──parse──▶ AST(Expr/Stmt/Program) ──lower──▶ IR ──check──▶ Build
        (spans, byte ranges)      (source spans)         (names resolved)   (diagnostics)
```

Every tool in the toolchain — the CLI, the language server, a future formatter,
a linter, a test harness — wants **the same upstream artifacts**: the tokens (with
byte ranges, for highlighting and editing), the AST (with source spans, for
hover / go-to-definition / folding), and the checked build (with diagnostics,
for error reporting). None of them wants its *own* copy of the grammar; the
whole point is that the grammar and every downstream artifact live in exactly
one place, so the LSP, the formatter and the compiler can never drift apart.

So the rule is:

> **`crates/lichen-language` owns the frontend.** Every tool *depends on it* and
> consumes its public modules (`lex`, `ast`, `parse`, `compile`, `diag`,
> `frontend`, `session`) directly. Any tool that needs the AST gets the *same*
> `lichen_language::ast::Expr` the compiler lowers from; any tool that needs
> tokens gets the *same* `lichen_language::lex::Token` the parser feeds on.

That is the whole sharing story. There is no second lexer, no second parser, no
second AST. A formatter is correct by construction, because it prints the very
tree the compiler parses.

## Why not one crate per CLI tool

A natural-but-wrong shape is a crate per tool — `lichen-language-server`,
`lichen-language-formatter`, `lichen-language-linter`, … each a full crate with
its own bin and its own copy of "how to turn the AST into editor output". That is
not ideal for three reasons:

1. **It duplicates the editor-view.** The span↔LSP-position math, the
   position→node index, the diagnostics→LSP conversion are all the same for the
   server, the formatter, and a linter. One crate per tool would re-implement or
   reach across crate boundaries for it.
2. **It multiplies boilerplate, not capability.** Each crate needs its own
   `Cargo.toml`, its own `main`, its own triage of what part of the frontend to
   expose. The incremental value per crate is small.
3. **It hides the shared artifacts behind tool names.** When a formatter lives in
   `lichen-language-formatter`, the fact that it must depend on the *frontend* —
   the real shared artifact — gets lost in the noise.

So the tools are **not split by tool**. They are split by *package kind*:

```
crates/
  lichen-language/           the frontend + the `lichen` CLI (this c't is the
                             single home of lex / ast / parse / compile / diag)
  lichen-language-server/    the TOOLING crate: lib.rs = the shared editor-view
                             (span↔position, node index, diagnostics→LSP), and
                             the LSP server as a [[bin]].
                             Future tool binaries (formatter, …) are ADDED AS
                             MORE [[bin]] TARGETS HERE, not as new crates.
  lichen-language-zed/       the Zed editor plugin. A separate crate because it
                             is a different *package kind* (a WASM plugin that
                             speaks `zed_extension_api`), not because it is a
                             "different tool".
```

The tooling crate is the trick. It bundles two things that belong together:

- a **library** (`lib.rs`) — the reusable editor-view of the shared frontend
  artifacts. This is what the Zed extension imports to avoid re-implementing the
  same span math, and what a future `lichen-format` bin would import;
- a **binary** (`[[bin]] lichen-language-server`) — the actual LSP server.

When a formatter is added, it is a second `[[bin]]` in this same crate, reusing
the same library. That is what "not one crate per cli tool" buys: one tooling
crate, many thin entry points, one shared editor-view library.

### What about a "lean frontend" crate?

`lichen-language` currently also carries the CLI and the runtime-called names
(`wasmi`, `wasm-encoder` for the compute package), so a pure formatter that only
wants tokens+AST would be pulling in more than it prints. For now the tools
depend on `lichen-language` directly — the sharing wins matter far more than the
weight, and `wasmi`/`wasm-encoder` are already compiled into any tool that
evaluates a program. If the weight ever matters, the *clean* seam is to lift the
syntax-only modules (`lex`, `ast`, `parse`, and the syntactic half of `diag`)
into a `lichen-language-frontend` crate and `pub use` them back through
`lichen-language`, so existing module paths (`lichen_language::lex::…`) never
change. That is a refactor, not a redesign; the artifact contract above is
unchanged by it.

### What about cross-process sharing?

A language server and the CLI are separate processes; they cannot share a live
`Build`. But lichen already has a **cross-process artifact store** — a frozen
[`StaticModule`](../../crates/lichen-lowlevel/src/static_module.rs) serialized to
a content-addressed cache (`persist.rs`'s `DeviceRegistry` under `~/.lichen`,
keyed by a transitive, deterministic [`artifact_hash`](artifact-cache.md)) — and
`PackageStore` (`package.rs`) loads those artifacts without recompiling, with
incremental dependency-graph verification. **The tooling crates reuse that store
for settled per-file artifacts, and [`BufferSession`](incremental-parse-compile.md)
for the live buffer**, rather than building a second cache. See
[`artifact-cache.md`](artifact-cache.md) for the whole mechanism.

## The artifact contract (what the tools import)

Concretely, the shared artifacts — all re-exported from `crates/lichen-language`:

| Artifact | Module | What a tool does with it |
|---|---|---|
| `TokenKind`, `Token`, `Lexed` | `lex` | syntax highlighting, edit-aware relex (`lex_resume`) |
| `Expr`, `Stmt`, `Program`, `Binding` | `ast` | hover, go-to-definition, folding, formatting |
| `Err`/`ErrorBlock` | `ast` | "don't format/flag this broken region" (masked) |
| `Span` (`(u32,u32)`, 1-based line/col) | `lichen_highlevel::ir` | the universal source position |
| `Diag`, `Stage` | `diag` | diagnostics (lex/parse/resolve/check) |
| `Frontend`, `frontend*`, `Report`, `compile*` | `lib` | the full text→IR→check pipeline |
| `BufferSession`, `SessionReport` | `session` | incremental, diff-gated diagnostics |

The one thing the raw frontend does **not** give you is the *editor view* of
these — the reverse mapping from a cursor position back to a node, the
line/col→LSP-utf16 conversion, and the "which binding does this name use"
resolution you can index for go-to-definition. That is exactly what
`lichen-language-server`'s library adds. The frontend owns the *syntax*; the
tooling crate owns the *lookup*.

### Why the tooling re-derives name resolution

The compiler resolves names at lowering time and, in doing so, collapses a
name *use* onto the binder's own `ExprId` (`compile.rs`): compiling `Expr::Name`
returns the binder's id without allocating a node, so the use's own source span
is **not** recorded in the IR. The AST keeps the use's span but not its binding.
To answer "go to definition" from a cursor on a use, the tooling crate therefore
walks the AST with its own scope stack (mirroring the compiler's scope rules:
block-wide bindings entered before values, restrictive `let` entered after their
value, lambda parameters in body scope) and records a
`use-span → binding-span` map. This is self-contained in the tooling library and
is exactly what it means to interpret the shared artifact for an editor. The
frontend stays a single source of truth for the *syntax*; resolution for
*editing* is a tooling concern built on top of it, not a fork of the compiler.

## The two new crates

### `lichen-language-server` (tooling crate)

- `lib.rs` — the shared editor-view library:
  - `lsp` — the canonical `lsp_types` protocol types (`Position`/`Range`/
    `Diagnostic`) plus the `Span`/`line_starts` ↔ LSP-utf16 conversion;
  - `analysis` — `Doc`: parse a source once, hold the tokens, AST, pipeline
    diagnostics and the resolution index; `hover_at`, `definition_at`,
    `lsp_diagnostics` on top of it.
- `src/bin/lichen-language-server.rs` — a [`tower_lsp::LanguageServer`] (stdlib
  JSON-RPC transport via `LspService`/`Server`): `initialize` (capabilities:
  full text-sync, hover, definition), `textDocument/didOpen|didChange|didClose`
  (→ publish diagnostics), `textDocument/hover`, `textDocument/definition`,
  `shutdown`/`exit`. `tower-lsp` owns framing, dispatch, cancellation and error
  codes; the binary only decides how to answer each request.

`Doc` is built by cutting the leading `@{…@}` block with `preprocess`, then
compiling the remainder with `frontend_at`/`build_report` (absolute spans). The
server holds only the *source text* per open document and re-runs the frontend on
demand in a blocking task, because `Doc` is `!Send` (it owns raw pointers into the
frontend arena via the diagnostics). Making the frontend artefacts `Send` is the
follow-up that would let the server cache a `Doc` per URI.

`tower-lsp` is a **non-default `server` feature** of this crate, and the `zed`
extension depends on it with `default-features = false`, so the WASM plugin does
not pull the tokio/tower async stack.

### `lichen-language-zed` (editor plugin)

- A `zed_extension_api` extension that (a) declares the `lichen` language, and
  (b) points Zed's LSP integration at `lichen-language-server`. It reuses
  `lichen-language-server`'s editor-view library so its span math and hover /
  go-to-definition logic match the server byte-for-byte. It is a separate crate
  only because it is a WASM plugin for a different host, not because it is a
  different tool.
- The installable extension is this crate directory: `extension.toml` (id, name,
  the `[language_servers.lichen-language-server]` and `language_ids` wiring) plus
  `languages/lichen/config.toml`. Build the WASM with:
  `cargo build -p lichen-language-zed --features zed --target wasm32-wasip2
  --release` and install it as a dev extension from this directory.
- **Distribution shape:** for the official registry, the extension does not need
  its own repo — the whole `lichen-vm` repository is added as a public submodule
  with `path = "crates/lichen-language-zed"` (and `default-features = false` on
  the `lichen-language-server` dependency keeps the tower-lsp/tokio stack out of
  the WASM). Install-as-dev-extension needs no Git or registry at all.
- **Known gap:** Lichen's lexer/parser is hand-rolled, so there is no
  `tree-sitter-lichen` grammar yet. Until one exists (a separate repo registered
  under `[grammars.lichen]`), Zed can load the language and run the language
  server, but there is no syntax highlighting.

## Fitting future tools into the model

- **Formatter** — a `[[bin]]` in `liche-language-server` (the tooling crate),
  using `lex::Token`s (byte ranges) + `ast` to re-print. It must *not* touch the
  checker; it prints the tree the parser produced, so a formatting round-trip is
  guaranteed to parse back to the same AST.
- **Linter / repl / test-harness** — same pattern: reuse the frontend (and, where
  relevant, the tooling library) rather than owning a copy of the grammar.

The rule of thumb: **the frontend is one crate; the tools are many entry points;
only the things that are a different *package kind* (a Zed plugin, a VS Code
extension, an npm package) get their own crate.**
