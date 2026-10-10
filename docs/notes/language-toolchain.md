# Language toolchain: one frontend, many tools

> Status: current
> Points at: the crate boundaries below, the frontend in
> [`crates/lichen-language`](../../crates/lichen-language/), and the two tool
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

The syntax half of that frontend is itself split into three crates —
`lichen-span` (the source-position protocol), `lichen-language-lex` (tokens) and
`lichen-language-parser` (the AST and parser) — which `lichen-language`
re-exports, so the module paths above hold. A tool that wants only tokens and
AST can depend on those crates directly; the concrete layout and the decoupling
seams are [frontend-syntax-separation](frontend-syntax-separation.md).

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
  lichen-span/               the source-position protocol (Span, line_starts,
                             line_col) — dependency-free, shared by the lexer,
                             the parser, the language layer and the preprocessor.
  lichen-language-lex/       the lexer: tokens, byte ranges, LexDiag.
  lichen-language-parser/    the AST, the parser, ParseDiag, the occurrence-path
                             vocabulary.
  lichen-language/           the frontend + semantics, re-exporting the three
                             crates above (this crate is the single home of
                             compile / diag / session / the pipelines).
  lichen-language-server/    the TOOLING crate: lib.rs = the shared editor-view
                             (span↔position, node index, diagnostics→LSP), and
                             the LSP server as a [[bin]].
                             Future tool binaries (formatter, …) are ADDED AS
                             MORE [[bin]] TARGETS HERE, not as new crates.

lichen-language-zed/         the Zed editor plugin, at the repo root. A separate
                             crate because it is a different *package kind*
                             (a WASM plugin that speaks `zed_extension_api`),
                             not because it is a "different tool".

tree-sitter-lichen/          the Tree-sitter grammar, at the repo root so it can
                             be a *sub-directory grammar* of this monorepo for
                             the Zed extension (`[grammars.lichen]` with
                             `path = "tree-sitter-lichen"`). A simple, permissive
                             grammar for highlighting / outline / brackets — not
                             a re-implementation of the strict frontend.
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

The syntax-only crates exist: `lichen-language-lex` owns the tokens (with
`lichen-span` owning the position protocol) and `lichen-language-parser` owns the
AST and parser, both re-exported through `lichen-language`. A tool that only
wants tokens+AST depends on those directly, so a formatter does not have to pull
in the checker or the VM.

`lichen-language` still carries the semantics and the runtime-called names
(`wasmi`, `wasm-encoder` for the compute package), so a tool that compiles a
program pulls those in regardless; `wasmi`/`wasm-encoder` are already compiled
into any tool that evaluates a program. See
[frontend-syntax-separation](frontend-syntax-separation.md) for the crate layout,
the `Span` and `Diag`/`Stage` decoupling seams, and the public surface.

### What about cross-process sharing?

A language server and the CLI are separate processes; they cannot share a live
`Build`. But lichen already has a **cross-process artifact store** — a frozen
[`StaticModule`](../../crates/lichen-lowlevel/src/static_module.rs) serialized to
a **file-ID keyed** cache (`lichen-registry`'s `DeviceRegistry` — re-exported as
`persist::DeviceRegistry` — under `~/.lichen`,
artifacts at `artifacts/<sha256(file_id)>.module`, identity verified by a
transitive, deterministic [`artifact_hash`](artifact-cache.md)) — and
`PackageStore` (`package.rs`) loads those artifacts without recompiling, with
incremental dependency-graph verification. **The tooling crates reuse that store
for settled per-file artifacts, and the language server adds its own per-text
index cache for the live buffer** (`P1-17`), rather than building a second
artifact cache.  `BufferSession` — the incremental lex/parse splice documented in
[`incremental-parse-compile.md`](incremental-parse-compile.md) — wired into the
server's compile worker; the keystroke path is still open (`P1-17` (b)), and that
is what would avoid the lex and parse on
the keystroke path, which the index cache does not.  See
[`artifact-cache.md`](artifact-cache.md) for the whole mechanism.

The store is **scoped per plugin set**: a plugin-built compiler uses its own
`<lichendir>/compilers/<key>/` as the artifact-cache root (via
`lichen_compiler::cli::main_with_cache_dir`), so its compile artifacts never
collide with (or reuse) another vocabulary's — only the shipping compiler uses
the base `lichendir()` root.

## The artifact contract (what the tools import)

Concretely, the shared artifacts — all re-exported from `crates/lichen-language`:

| Artifact | Module | What a tool does with it |
|---|---|---|
| `TokenKind`, `Token`, `Lexed` | `lex` | syntax highlighting, edit-aware relex (`lex_resume`) |
| `Expr`, `Stmt`, `Program`, `Binding` | `ast` | hover, go-to-definition, folding, formatting |
| `Err`/`ErrorBlock` | `ast` | "don't format/flag this broken region" (masked) |
| `Span` (`(u32,u32)`, 1-based line/col) | `lex` (defined in `lichen-span`) | the universal source position |
| `Diag`, `Stage` | `diag` | diagnostics (lex/parse/resolve/check) |
| `Frontend`, `frontend*`, `Report`, `compile*` | `lib` | the full text→IR→check pipeline |
| `BufferSession`, `SessionReport` | `session` | incremental re-analysis: retained cells, dirty propagation, per-edit telemetry |

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
    `Diagnostic`) plus the `Span`/`line_starts` ↔ LSP-utf16 conversion, the
    semantic-token **legend**, and the delta encoding of a token slice into a
    `SemanticTokens` payload;
  - `analysis` — `Doc`: parse a source once, hold the tokens, AST, pipeline
    diagnostics and the resolution index; `hover_at`, `definition_at`,
    `completion_at`, `lsp_diagnostics`, and `semantic_tokens`/`semantic_tokens_lsp`
    on top of it.  `completion_at` offers the names in scope at the cursor (the
    same scope set that an unresolved name's "did you mean" clause uses), and the
    `resolve`-layer diagnostics name the closest in-scope candidates too.  After a
    `.` it offers the container's **field** names instead: an imported module's
    exported fields (read from its `Static` type) or a local struct binding's
    fields (read from its checked struct type).  The *field-access* (named-field
    miss) diagnostic appends the same did-you-mean clause against the struct's
    actual fields, so the error and the completion share one candidate set.
- `src/bin/lichen-language-server.rs` — a [`tower_lsp::LanguageServer`] (stdlib
  JSON-RPC transport via `LspService`/`Server`): `initialize` (capabilities:
  full text-sync, hover, definition, `completionProvider`,
  `semanticTokensProvider`),
  `textDocument/didOpen|didChange|didClose` (→ publish diagnostics),
  `textDocument/hover`, `textDocument/definition`, `textDocument/completion`,
  `textDocument/semanticTokens/full`, `shutdown`/`exit`. `tower-lsp` owns framing,
  dispatch, cancellation and error codes; the binary only decides how to answer
  each request.

`Doc` is built by cutting the leading `---…---` block with `preprocess`, then
compiling the remainder with `frontend_at`/`build_report` (absolute spans). The
server holds only the *source text* per open document; `Doc` is `!Send` (it owns
raw pointers into the frontend arena via the diagnostics), which is why the
per-document sessions live on one dedicated compile worker thread rather than in
`Backend` (see [lichen-home](liche-lsp-home.md) and
[incremental-update](incremental-update.md) §6.4).

**Semantic tokens are the grammar-optional highlighting path.** Lichen's own
frontend classifies every token — literals, keywords, operators, and names
(disambiguated through the AST as binding declarations / lambda parameters /
function calls / `.field` accesses) — into a `SemanticTokens` delta payload served
by `textDocument/semanticTokens/full`. Because the LSP advertises the capability,
an editor colors the buffer from the language's parser even where the tree-sitter
grammar is absent; the grammar (when present) and the semantic tokens are
complementary. The `---…---` preprocessor block is the one comment-like construct
and is colored as a comment.

`tower-lsp` is a **non-default `server` feature** of this crate, and the `zed`
extension depends on it with `default-features = false`, so the WASM plugin does
not pull the tokio/tower async stack.

### `lichen-language-zed` (editor plugin)

- A `zed_extension_api` extension that (a) declares the `lichen` language, and
  (b) points Zed's LSP integration at `lichen-language-server`. It is a separate
  crate only because it is a WASM plugin for a different host, not because it is a
  different tool. It is meant to reuse `lichen-language-server`'s editor-view
  library so its span math and hover / go-to-definition logic match the server
  byte-for-byte — but that reuse is **not wired up yet**, so those crates are
  currently **not** dependencies of the extension.
- The installable extension is this crate directory: `extension.toml` (id, name,
  the `[language_servers.lichen-language-server]` and `language_ids` wiring) plus
  `languages/lichen/config.toml`. Build the WASM with:
  `cargo build -p lichen-language-zed --features zed --target wasm32-wasip2
  --release` and install it as a dev extension from this directory.
- **Distribution shape:** for the official registry, the extension does not need
  its own repo — the whole `lichen-vm` repository is added as a public submodule
  with `path = "lichen-language-zed"`. Install-as-dev-extension needs no Git or
  registry at all.
- **Grammar:** Lichen's lexer/parser is hand-rolled, so syntax highlighting needs
  a Tree-sitter grammar. `tree-sitter-lichen/` at the repo root provides it.
  It is a **deliberately simple, permissive** grammar (not a re-implementation of
  the strict frontend): its job is highlighting, outline and bracket-matching,
  so it glosses over the frontend's whitespace-sensitive "Glue" postfix
  distinction and accepts more than the strict parser. It lives in this repo as
  a sub-directory so the extension can reference it via the grammar `path` field
  (`[grammars.lichen]` with `path = "tree-sitter-lichen"`), which Zed supports
  for (mono)repos holding multiple grammars. The generated parser is **not**
  committed: `src/parser.c` and the rest of `src/` are build outputs of
  `grammar.js`, gitignored and regenerated in-tree by `bindings/rust/build.rs`
  through the pinned `tree-sitter-cli`, which it requires on `PATH` (or under
  `node_modules/.bin`) and panics with install instructions when it is missing.
  Queries live both in `tree-sitter-lichen/queries/` and (mirrored) in the
  extension's `languages/lichen/`, because Zed reads queries from the extension
  directory.
- **Grammar `rev`:** pinned to `d799ade` (a commit containing `tree-sitter-lichen/`
  and pushed to `origin/dev`), with `[grammars.lichen]` `repository` pointing at the
  public HTTPS remote (`https://github.com/windwhiterain/lichen-vm`). The pinned
  `rev` must stay reachable from that remote and in sync with the grammar/query
  paths (the `grammar_consistency` test guards this) so a registry install can clone
  and check it out. A `file://` `repository` is only for local development.
- **Semantic tokens** (the grammar-optional highlight path) are served by the
  LSP, so the extension neither needs the grammar for color nor needs any extra
  client-side config — Zed requests `textDocument/semanticTokens/full` because
  the server advertises the capability.
- **The toolchain is installed into Lichen Home, on demand.** The extension does
  not bundle `lichen-language-server` (per Zed's publishing rules). On first
  launch, if `Worktree::which` cannot find it, the extension reports install
  progress to Zed and runs `lichen path language-server`, which installs the
  **prebuilt** compiler + language server into **Lichen Home**
  (`$LICHEN_HOME/compilers/<plugin-set-key>/`, default `~/.lichen`) from the
  release tagged at the package manager's own commit, then prints the binary
  path. `lichen` is the single canonical copy at `$LICHEN_HOME/tools/lichen` (or a
  `lichen` on `$PATH`); on a machine with neither, the extension downloads the
  prebuilt package manager from the repo's GitHub release **into that same
  `$LICHEN_HOME/tools` slot** via `curl` (mirroring `toolchain::download`) and then
  runs it. Because the extension uses this canonical copy, a later `liche update` —
  which refreshes exactly `$LICHEN_HOME/tools/lichen` — stays in sync with what the
  extension runs (it never keeps a private copy of its own). `liche install` and
  `liche path` always address the toolchain release tagged with the running package
  manager's **own commit**; `liche update` moves it to the **latest published
  release** rather than the repo tip, so a manual-release workflow never leaves
  `update` looking for an unpublished commit. Run it by hand and restart Zed, or
  `lichen update` to move the package manager (and the toolchain it installs) to the
  latest release:

  ```text
  lichen install language-server   # install the prebuilt server into Lichen Home
  lichen path language-server      # print its path (installing if absent)
  lichen path language-server --project <dir>  # compose+print a server over <dir>'s plugins
  lichen update                    # update the package manager to the latest release
  ```

- **Per-project plugin-set LSP.** A project that imports a *native plugin* gets a
  `lichen-language-server` built over that plugin set (`lichen path language-server
  --project <dir>`, which the Zed extension calls with `--project <worktree
  root>`), so the server understands the plugin's value/operator leaves for
  diagnostics / hover / go-to-definition. The composed server is cached
  plugin-set-keyed under `$LICHEN_HOME/compilers/<key>/` (mirroring the compiler
  cache); a project with no plugins falls back to the shipping server. The
  tooling is generic over one program type `P` (see `lichen_language::LangProgramShape`),
  so the same `Doc`/server services the shipping and the composed vocabulary.

  When neither the server nor `lichen` is present, the extension bootstraps a
  prebuilt `lichen` from the repo's GitHub release (published by the
  [`release-lichen`](../../.github/workflows/release-lichen.yml) CI workflow,
  which
  [`release.sh`](../../scripts/release.sh) triggers); the
  download needs a release (tagged at the commit's short SHA, assets
  `<bin>-<host-target>[.exe]`) to actually exist, else it reports "no release
  asset".

## Fitting future tools into the model

- **Formatter** — a `[[bin]]` in `lichen-language-server` (the tooling crate),
  using `lex::Token`s (byte ranges) + `ast` to re-print. It must *not* touch the
  checker; it prints the tree the parser produced, so a formatting round-trip is
  guaranteed to parse back to the same AST.
- **Linter / repl / test-harness** — same pattern: reuse the frontend (and, where
  relevant, the tooling library) rather than owning a copy of the grammar.

The rule of thumb: **the frontend is one crate; the tools are many entry points;
only the things that are a different *package kind* (a Zed plugin, a VS Code
extension, an npm package) get their own crate.**

## Recovered measurements

### Diagnostic rendering

A rendered diagnostic is exactly:

```text
error: unresolved name 'y'
  --> 1:6
   |
 1 | x => y
   |      ^
```

**One caret block per diagnostic, no separator between them.** A diagnostic with
no span (an I/O or package-resolution failure) prints its message alone, with no
position and no caret. A diagnostic whose span is in another file (a built-in
package's own source) uses that file's path for the arrow and that file's text
for the line and caret; the source line comes from the shared line model rather
than a second scan, so the text and the `(line, col)` name the same line.

- **One scan per report, not per diagnostic.** `render_with_line_starts` exists so
  a whole report costs one scan of the source. `checker_message` re-renders the
  highlevel's structured facts in the CLI's vocabulary through one shared
  `TypePrinter` that must carry the checker's arrow registry, so a class keeps a
  single `?a` name across every diagnostic in the report. The `?a` journey line
  is deliberately gone: every expression's type is queryable, so the user inspects
  an expression's type directly instead of reading a source trace.
- **Two sources, one list.** The frontend's diagnostics (lex, parse, resolve) are
  merged with the checker's into one list by `compile`. The checker's messages
  are re-rendered pretty through the same printer as the CLI output, while the
  boxed highlevel `Diag` in `check` stays raw for tests and tooling.
  `Diag::file` names the file a diagnostic is displayed and opened under and the
  text its caret comes from; `related` is the failing *use*'s position and is
  meaningful only alongside `file`; `None` means the source the diagnostic was
  produced for, which is what every frontend and checker diagnostic means.
- **Facts in, a `Loc` out.** The lowlevel records failures as *facts* —
  `Module::unify_errors`, `Module::eval_errors`, `Module::assert_errors` — and the
  highlevel turns each into a structured `Diag`, attributed to a `Loc` through
  the records the checker kept while building: `Build::diary`,
  `Build::apply_edges` and `Build::node_edges`. All three are keyed **by** node
  and never stored **on** one, so the lowlevel graph stays freely shareable.
- **`DiagKind` names its own expected/found category.** Every variant is a
  *type-mismatch* construct, so there is no separate coarse value/type
  discrimination. The three shape guards are: a callee that must be a function
  (`check_lam`/`check_app`); a container that must be an array or a table (the
  index and table-lookup pins); and a membership test's right operand, which
  must be a set.
- **A read-kind assert's subject comes from the failing condition.** `container`
  is filled from the failing condition's operand 0, not from the registered node:
  a per-call clone's subject is the actual argument's type, while the template's
  own cell is still open and would render as a bare `?a`.
- **Span attribution when no `Loc` exists.** A unify error recorded *outside* the
  checker's own checks carries no `Loc` — the lowlevel records it — and the node
  it names may be a per-apply clone the checker never saw. The tables that hold
  such a node are the build's own: `node_edges` (the runtime-attribution edges)
  and, more completely, `state`, which maps every IR expression to the nodes it
  compiled to. Resolve the clone through the node it was instantiated from
  (`Module::node_origin`), then take the position of the expression that owns
  either side. An origin node is not kept alive by the clone, so a released one
  is absent rather than a panic — and `node_origin`'s contract is that the caller
  checks liveness first, because the origin is not a keep-alive edge. When the
  origin is the apply that materialized a frozen template, the fallback `Loc` is
  **pathless**: the argument expression itself is the caret target the apply's
  edge was recorded for.
- **The three runtime kinds spell themselves.** `RuntimeIndexTarget`,
  `RuntimeIndexSubscript`, `RuntimeApplyTarget` and `RuntimeRawElement` all
  arrive as the same lowlevel `EvalError` family — a fact about a *value*, with
  no type to print — so their wording is self-contained, and each stays distinct
  from its static counterpart (`IndexTarget`, `Guard`), which reports a *type*
  the checker refused and therefore names that type. `RuntimeRawElement` is
  separate again because the non-container is the element the read produced rather
  than the container the user wrote: `[1, 2]` was fine and the element `1` is a
  scalar, so the generic "this value is not a container" blames the wrong side.
- **A failed build always carries a diagnostic.** Every consumer of a `Report`
  relies on it. `Build::diagnostics` skips a recorded failure it cannot attribute
  to an expression — an assert cloned out of an imported module has no entry in
  this build's node tables — so `build_report` synthesises exactly **one**
  unattributed failure and `run`, the package store and the editor all inherit
  it instead of each inventing their own. A built-in package's contract is cloned
  out of a **static** module, so this build's tables hold no entry for the
  template and the diagnostic carries the static ref instead: a host that kept that
  module's source resolves it to a position in *that* file, and a host that kept
  none drops it, because the package's own build reported the failure when it
  compiled. The `user_asserts` filter cannot apply there — the flag lives in the
  other build — so "was this the user's assert" is left to the host that knows
  which modules it kept sources for.

### Running a program

- **The output line** is `value<attributes>: type` — `5: Int`,
  `[1, 2, 3]: array<Int, 3>` — formed in exactly one place so `evaluate` and
  `evaluate_raw` cannot drift. The value renders *against its type chain*: a
  struct type value prints `struct<.f Int, .g Type>`, a tuple `(1, Int)`. A value
  one of whose attributes *names* it reads as that name, and that override is the
  general mechanism ([operator-polymorphism](operator-polymorphism.md) §8.1), not
  a rule about a particular attribute — it is what lets a value with no spelling
  of its own (a refinement's predicate, a function) be shown at all. An undecided
  root (an empty slot) renders as the printer's no-value reading.
- **The deep evaluation is where an operator actually executes**, so a plugin's
  *runtime* refusal lands there (a `plrun` whose element count is past the limit,
  say) with the lazy marker as the value; returning that would print
  `parameterized: Int` and say nothing about why. The same channel also carries a
  **provisional** refusal — the checker evaluates speculatively, so a `$jit` whose
  parameter domain is not decided yet records one, and a later attempt with the
  domain known compiles the very same kernel — and that program works and has a
  value. The line between them is the outcome, not the channel: a refusal
  explains a value that never arrived, and a program that produced one has nothing
  to explain.

### The tooling crate

- **The index holds two definition coordinate systems**, and the split is
  load-bearing rather than cosmetic. `DocIndex::defs` and `def_index` are keyed by
  a **document** span; `builtin_names` entries carry a position in **another
  file**. Both start at line 1, so a built-in's definition would collide with a
  document binding's — a built-in therefore never enters `def_index`: only a
  *use* resolves to one, through the scope frame. Everything downstream reads one
  of the two through `ScopeValue::document()` / `ScopeValue::builtin()`, which
  return an index into the corresponding list and never the other. The same split
  governs shadows (`core` is the base frame below the document's, so the prelude
  is **shadowable, not reserved**), field tables (`module_field_types` is keyed by
  module *and* field, because a field access is not a definition site), and
  completion (`completion_item` takes a `ScopedName { document: Option<Span> }`,
  `None` marking the built-in).
- **`lsp` owns exactly one thing: the protocol dialect** LSP forces on the shared
  span — a 0-based line (not `lichen_span`'s 1-based one) and a `character`
  counted in **UTF-16 code units** (not bytes). Two clamps come with the dialect
  and only with it: the offset is clamped to `source.len()`, and a byte inside a
  multi-byte character clamps **down** to that character's start, because
  `character` cannot name a mid-character position. The byte model performs
  neither — it is given no source — so the two agree about every byte on a
  character boundary inside the source, which is every byte the frontend reports.
  This is why the dialect lives once in the library: the language server and the
  Zed extension must agree on it byte-for-byte.

### The lowering

- **A lambda has exactly one binder, so "free variable" is a positional fact.**
  `x => body` is the only lambda form, and the resolver pushes exactly one binder
  per lambda (`ExprKind::Parameter` per `Expr::Lambda` in `compile.rs`). A "free
  variable" is therefore not a closure-capture fact but simply a name the body
  reads that is not the parameter — which is what makes `s` in `(s : ParS) => { … }`
  a *parameter* and `doubler`/`data` free ones.
- **A deferred named instantiation's supplying key is all-or-nothing (a known
  limit).** It carries a type only when **every** argument's type is decided. One
  undecided argument (say `.y _`) drops the type from every key in the call, so a
  *sibling* field's `string`-against-`Int` mismatch is not checked and the program
  is accepted. A per-argument key form would check `.x` and refuse it; that
  mechanism is not in place. This is a limit pinned deliberately, not the intended
  behaviour.

### The analyzer and the compiler CLI

- **`lichen-analyze` is the debugger of last resort** for a graph question that
  neither a rendered value nor a diagnostic answers: which node holds what, and
  through which class. It asks the same questions the printers and the backends
  ask — a node's own value, the class's committed value, the width of its item
  list, the class it shares — and every read goes through the lowlevel's public
  API, so a conclusion drawn from it is a conclusion about the compiled program
  and not about a second implementation of it. `Analysis::evaluate` exists because
  an apply's behaviour is a fact about its clones, so a reader that asks about an
  applied program asks after evaluation.
- **`lichen-compiler <program.lichen>` compiles and runs one program**, printing
  its output; a directory path runs every `.lichen` file in it, printing
  `file: output` per program. `run` and `build` are accepted as subcommands too,
  and the command name is overridden at runtime from `argv[0]`, so a
  plugin-built `lichen-compiler-<name>` reports its own name in usage/help. The
  binary is **depend-aware**: a file's `depend "url"` directives resolve against
  the lichen-home source cache the package manager populated (`lichen fetch`), so
  running a file with dependencies needs no git access here — the compiler only
  *reads* what the package manager put in the cache, and it is the package
  manager that invokes this binary for its `run`/`build` commands, which is how a
  plugin-built compiler's vocabulary takes effect.

### A known frontend limit

- **A comma *and* a newline between two tuple elements is two separators**, which
  the tuple grammar does not tolerate; the same program fails on `dev`. The
  operator tests write their tuples on one line so they do not depend on the
  wart.
