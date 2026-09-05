# Zed extension: build & test workflow

> Status: current
> Points at: [`crates/lichen-language-zed/`](../../crates/lichen-language-zed/) (the WASM
> plugin, `extension.toml`, `languages/lichen/`), the LSP it launches
> ([`lichen-language-server`](../../crates/lichen-language-server/)), and the highlighting
> grammar ([`tree-sitter-lichen`](../../tree-sitter-lichen/)). Design:
> [language-toolchain](language-toolchain.md).

The Lichen Zed extension is **not one thing** — it is a WASM plugin, an LSP server it
launches, and a Tree-sitter grammar that colors and outlines the buffer. Testing it
therefore means testing a small stack, not running one binary. This note is the layered
test workflow and the gotchas that come up along the way.

## The stack, and how each layer is tested

```
lichen-language-zed  (WASM plugin)   ── build for wasm32-wasip2, check `zed:api-version`
        │  language_server_command runs:
        ▼
lichen-language-server  (LSP binary) ── cargo test (lib unit tests + stdio LSP smoke)
        │  imports the shared editor-view lib + frontend
        ▼
lichen-language       (frontend)      ── cargo test (the grammar/check/session suite)
        │
        └── tree-sitter-lichen (grammar) ── cargo test (parse every sample, no ERROR nodes)
```

Each layer reuses the frontend from `lichen-language` (see
[language-toolchain](language-toolchain.md) — "the frontend is the artifact, not the
tool"), so the test queue is ordered by dependency and each `cargo test -p <crate>` runs
independently.

### 1. Host compile check (what Zed's dev-extension builder needs)

```bash
cargo check -p lichen-language-zed
```

The `zed` feature is `default` (`Cargo.toml`), so this compiles the extension body and
emits the `zed:api-version` custom section even without `--features zed`. This is the
cheapest gate: it fails fast on a code error.

### 2. Build the WASM, then check the `zed:api-version` section

```bash
rustup target add wasm32-wasip2          # once
cargo build -p lichen-language-zed --features zed --target wasm32-wasip2 --release
```

The artifact lands at
`target/wasm32-wasip2/release/lichen_language_zed.wasm`. Zed refuses an extension with no
`zed:api-version` custom section ("no zed:api-version section"), so after building, confirm
it is present in the file:

```bash
# no wasm-tools on PATH? byte-scan for the section name, then read the 6-byte value
# immediately after it (major/minor/patch as u16-BE). The installed Zed must accept it.
```

The section string always appears in the `.wasm`; the 6 bytes after `"zed:api-version"`
are the API version. For `zed_extension_api = "0.7.0"` that is `00 00 00 07 00 00`
(see `zed_extension_api`'s `build.rs`, which writes `major/minor/patch` as 2-byte BE).

> The crate's `default = ["zed"]` makes a plain `cargo build` (including Zed's own
> dev-extension builder, which passes no `--features`) emit the section. If the section
> ever goes missing, check that the `zed` feature is on.

### 3. Manifest validity

```bash
python -c "import tomllib; tomllib.load(open('crates/lichen-language-zed/extension.toml','rb')); print('ok')"
python -c "import tomllib; tomllib.load(open('crates/lichen-language-zed/languages/lichen/config.toml','rb')); print('ok')"
```

Then confirm the grammar pin: `[grammars.lichen]` has `rev` (a real Git SHA in `lichen-vm`),
`path = "tree-sitter-lichen"`, and a `repository` that resolves. For local dev the
`repository` is a `file://` URL; the matching `rev="9b23892…"` is currently only on the
local branches, so a remote fetch fails until that SHA is pushed (documented in
`extension.toml`).

### 4. Grammar

```bash
cargo test -p tree-sitter-lichen
```

`tests/samples.rs` parses every `.lichen` sample under
`crates/lichen-language/examples/programs` and `tests/fixtures/readme`, plus a set of edge
cases, and asserts the root node has no ERROR nodes. Green means the grammar accepts the
whole corpus.

Zed reads queries from the extension's `languages/lichen/`, not the grammar repo, so the
two copies of `highlights.scm`/`outline.scm` stay in sync by hand. A quick semantic check
is a `git diff --no-index`; comment prose may legitimately differ, the rules should not.

### 5. The LSP server the extension launches

```bash
cargo test -p lichen-language-server
```

This builds the real binary and runs three suites: the `lib` unit tests (span↔position
round-trips, semantic-token delta encoding, `Doc` hover / definition / diagnostics), a
`statement_values` integration test, and — the important one — `tests/lsp_smoke.rs`, which
**spawns the real `lichen-language-server` over stdio** and drives an
`initialize → didOpen → hover → definition → semanticTokens/full → shutdown → exit`
exchange. Green proves the tower-lsp wiring end to end, which is the exact path Zed uses.

### 6. The shared frontend

```bash
cargo test -p lichen-language
```

The extension and server both reuse this crate, so its suite
(lex → parse → compile → check, session/incremental, render, readme, package/store, table,
compute/doc tests) is the real behavioral guarantee. Green here, plus the two crates above,
is the whole signal.

### 7. LSP binary on `$PATH`

The extension does not bundle the server (Zed's publishing rules); `language_server_command`
resolves it with `Worktree::which`, which searches `$PATH`. Install it once, then restart
Zed:

```bash
cargo install --path crates/lichen-language-server     # → ~/.cargo/bin/lichen-language-server
```

Check it is visible:

```powershell
Get-Command lichen-language-server   # must resolve; ~/.cargo/bin must be on $PATH
```

If it is missing, Zed reports "`lichen-language-server` not found on `$PATH`" when a
`.lichen` buffer is opened.

## Gotchas that actually bite

- **Disk space.** The debug test build links many test binaries at once; it needs a few GB
  of headroom. With the release WASM build and the debug suite both living in the same
  workspace `target`, the disk fills fast and the linker dies with
  `LLVM ERROR: IO failure on output stream: no space on device` — a *build* failure, not
  a code failure. Free space (e.g. remove the stale crate-local `crates/lichen-language-zed/target/`,
  a gitignored artifact left by Zed's in-place dev-extension build) and re-run.
- **PowerShell exit codes lie.** Because `cargo` writes its progress lines to stderr, piping
  `cargo … 2>&1` under PowerShell reports the job as `exit code: 1` even on success. Read
  the `test result: … 0 failed` lines and the `Finished … target(s)` line, not the exit code.
- **`wasm-tools` is optional.** It is not on this machine, so verify the custom section with
  a byte scan instead.
- **The `extension.wasm` / `grammars/` in the crate dir are regenerated** by Zed's
  "Install Dev Extension" and are gitignored. Do not treat them as a source of truth (and
  do not patch them by hand); a stale one simply means the last dev-install was before the
  latest source change.

## A green run (observed)

| Layer | Command | Result |
|---|---|---|
| Host check | `cargo check -p lichen-language-zed` | OK (exit 0) |
| WASM build | `cargo build -p lichen-language-zed --features zed --target wasm32-wasip2 --release` | OK, 216,803 bytes |
| `zed:api-version` | byte-scan of the built `.wasm` | Present, `00 00 00 07 00 00` (= 0.7.0) |
| Manifests | `tomllib` parse of `extension.toml` + `config.toml` | OK; grammar `rev` exists |
| Grammar | `cargo test -p tree-sitter-lichen` | 2 passed |
| LSP server | `cargo test -p lichen-language-server` | 27 passed (21 lib + 3 smoke + 3 stmt) |
| Frontend | `cargo test -p lichen-language` | 319 passed |
| LSP on `$PATH` | `Get-Command lichen-language-server` | `~/.cargo/bin/lichen-language-server.exe` |

(Totals across the three test crates: **348 tests passed, 0 failed**.)

## Integrated / automated test methods (beyond `cargo`)

Zed ships **no first-party framework for running an extension's test *inside the real host
GUI*** — the `Extension` trait has no test hook, and the official README's "Testing your
extension" only documents the manual `Install Dev Extension` flow
([extension_api/README.md](https://github.com/zed-industries/zed/blob/b0911ccc/crates/extension_api/README.md)).
But there are three concrete tiers of "integrated" testing, from official to community, and
our setup already covers the middle one.

### 1. Official: the `test-extensions` workflow (what Zed runs for its registry)

The `zed` repo's `xtask` generates a `.github/workflows/test-extensions.yml` used by the
`zed-extensions` org ([`extension_tests.rs`](https://github.com/zed-industries/zed/blob/b0911ccc/tooling/xtask/src/tasks/workflows/extension_tests.rs)).
Per extension it runs two jobs:

- **`check_rust`** — `cargo fmt --check`, `cargo clippy --release --all-features -- -D
  warnings`, `cargo nextest run` on the **host** target (`CARGO_BUILD_TARGET=wasm32-wasip2`).
  This is the headless "run the extension crate's own tests" step.
- **`check_extension`** — downloads a `zed-extension` CLI
  ([`extension_cli/src/main.rs`](https://github.com/zed-industries/zed/blob/2aa36660/crates/extension_cli/src/main.rs))
  and runs `--source-dir . --scratch-dir … --output-dir …`. That CLI (a) compiles the
  extension to release WASM, (b) loads every grammar `.wasm`, (c) parses each language
  `config.toml` and `Query::new(grammar, *).scm` for every `.scm` file (so highlight/outline
  queries are validated against the grammar), (d) validates themes/snippets, then (e)
  packages `archive.tar.gz` + `manifest.json`. This is the build + bundle + structure
  smoke check — the closest first-party "integrated test" of the extension *artifact*.

### 2. Direct LSP stdio test (no host, no GUI) — what we already do

The most reliable way to test the plugin's *behavior* without a GUI is to spawn the real
`lichen-language-server` over stdio and drive it as an LSP client (`tests/lsp_smoke.rs`).
This is the standard community pattern for "integration testing an editor extension's
language layer", and it exercises the exact path Zed uses (initialize → didOpen →
hover → definition → semanticTokens → shutdown → exit).

### 3. Real-host E2E (headless) — GUI/CI

The community `zed-arkts` project documents a full E2E suite
([AUTOMATION_DESIGN.md](https://github.com/liuyanghejerry/zed-arkts/blob/main/docs/AUTOMATION_DESIGN.md)):
install Zed + the extension, launch the real Zed (headless via `xvfb` in CI), then monitor
`~/.local/share/zed/logs/Zed.log` for extension load, LSP startup and protocol messages.
This is true host integration but needs a display/GUI environment.

> For the official registry, `zed-extensions` orchestrates both jobs above via a
> `workflow_call`-based reusable workflow, so an extension repo can reuse it as-is
> ([infra/CI deep-dive](https://deepwiki.com/zed-industries/extensions/4-infrastructure-and-cicd)).

## Run every CI check locally (no CI needed)

The `test-extensions` workflow is a *CI convenience*, but **every check in it has a local
equivalent** — only the `zed-extension` CLI download itself is Linux/CI-specific, and even
its sub-checks are reproducible locally. Run these on a stock dev box (all verified on
Windows here):

| CI job step | Local command | Observed result |
|---|---|---|
| `cargo fmt --check` | `cargo fmt -p lichen-language-zed -p lichen-language-server -- --check` | **clean** |
| verify it compiles | `cargo check -p lichen-language-zed` | **ok** |
| build the WASM + section | `cargo build -p lichen-language-zed --features zed --target wasm32-wasip2 --release` | **ok** (216 KB) |
| run the crate's tests | `cargo test -p lichen-language -p lichen-language-server -p tree-sitter-lichen` | **348 passed** |
| load grammar + validate `.scm` | `tree-sitter query languages/lichen/{highlights,outline}.scm <sample>.lichen` | **ok** |
| `cargo clippy … -D warnings` | `cargo clippy -p lichen-language-zed --all-features -- -D warnings` | **fails at `lichen-highlevel` (not the plugin)** |

The LSP stdio test and the grammar query check are the two that the manual "Install Dev
Extension" flow never covers, and both run headless here.

**The strict `-D warnings` clippy check still fails further up the dependency tree** — a
monorepo-wide lint backlog, not a problem in the plugin. Two of the original blockers are now
fixed (the plugin source itself was never touched):

- `cargo fmt -p tree-sitter-lichen -- --check` was failing on `tests/samples.rs` (comment
  alignment); `cargo fmt` fixed it, and that crate now passes `fmt --check` and its tests.
- `cargo clippy -p lichen-lowlevel --all-features -- -D warnings` was failing
  (`collapsible_if` in `static_module.rs`, `doc_lazy_continuation` in `lib.rs`); both are
  clean now, and `cargo test -p lichen-lowlevel` (125 tests) still passes.

What still blocks a fully-green
`cargo clippy -p lichen-language-zed --all-features -- -D warnings` is **`lichen-highlevel`**
(and likely `lichen-language` / `lichen-language-server` after it):
`type_complexity`, `question_mark`, `collapsible_if`, `get_first`, `needless_range_loop`,
`map_flatten` in `checker.rs` / `diagnostic.rs`. lichen-vm is a monorepo with path deps, so
clippy lints the whole tree under `-D warnings`; a plain `cargo clippy -p lichen-language-zed
--all-features` (no `-D warnings`) reports these as *warnings* and exits 0. That cleanup is
independent of the Zed plugin and touches the checker, so it is a separate task.

The `zed-extension` CLI is a prebuilt **Linux** binary (CI downloads it from
`https://zed-extension-cli.nyc3.digitaloceanspaces.com/…`), so the *packaging* step is not
runnable on Windows — but its individual checks (wasm compile, grammar load, `.scm` query
compile, `config.toml` parse) are each covered by a local command above, and the finished
bundle is produced by Zed's `Install Dev Extension`.

## What this still does not cover (needs the GUI / a human)

The layers above prove the plugin compiles, advertises the right API version, launches a
working LSP, and colors/outlines the buffer via a grammar that accepts the whole corpus.
They do **not** prove Zed's host loads the extension or that `zed_extension_api 0.7.0` is
accepted by the installed Zed. That is a one-time manual check: Zed → Extensions →
**Install Dev Extension** → select `crates/lichen-language-zed/`, then open a `.lichen`
buffer and confirm (a) the extension loads, (b) diagnostics appear from the LSP, and
(c) highlight/outline come from the grammar. The host-side `zed_extension_api` version
must be compatible with the installed Zed; a mismatch surfaces there, not in `cargo`.
