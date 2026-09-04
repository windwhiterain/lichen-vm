# Cross-process artifact store (serialized, dirtiness-tracked, shared)

> Status: proposal (design)
> Points at: [`language-toolchain`](language-toolchain.md) (the toolchain
> boundary), [`incremental-parse-compile`](incremental-parse-compile.md) (the
> content-addressed signature / `lex_resume` / `parse_statement_region`
> primitives this builds on), `crates/lichen-language/src/{ast,lex,parse,compile,
> session}.rs`, `crates/lichen-highlevel/src/{ir,checker}.rs`.

This note proposes the feature: **no matter how many processes touch a lichen
project — the CLI, the LSP server, a future formatter, a repl — they serialize
the frontend artifacts and share them as much as possible, carefully tracking
each artifact's dirtiness.**

## The problem, restated precisely

"Share the artifacts" has two readings. The *link-time* one (both processes link
the same `lichen-language`) prevents grammar drift but does not prevent
**duplicated work**: the CLI, the LSP and a formatter each re-lex, re-parse and
re-check the same file. The feature here is the *data-level* sharing: a store of
serialized artifacts that every process reads and writes, so a file that the CLI
just compiled is already warm for the LSP.

The crux is the one word in the request that gets no respect: **dirtiness**. An
artifact is not a blob that goes stale as a whole; it is dirty per *stage* and
per *dependents*. Editing one import must dirty the importers' `IR`/`check`
artifacts but **not** their `tokens`/`AST`. Nailing that is the whole feature.

## The model: a content-addressed store where the key *is* the dirtiness

The simplest correct design is to avoid a "dirty flag" entirely. Make each
artifact **immutable and content-addressed**:

```
artifact_key = H(compiler_version, stage, unit_id, input_fingerprint)
```

- An artifact is **current** simply by *existing* at its key.
- An artifact is **dirty** simply by *not existing* at its key (or by the
  input-fingerprint changing → a new key → a miss).
- So "manage dirtiness carefully" reduces to **compute the key correctly** — and
  crucially, compute it so that a change at one stage/unit dirties *only* the
  stages/units that actually depend on it.

This is the sccache / rustc-incremental model, and it gives cross-process safety
for free: entries are written once and never mutated, so two processes that
compute the same key write identical bytes and the race is benign (atomic
rename; readers never see a partial entry).

### Why this composes with what's already there

`incremental-parse-compile.md` already implements the precise dirtiness signal:
[`program_signature`](../../crates/lichen-language/src/session.rs) is a
**name-resolved content hash** — spelling-free (a consistent rename doesn't move
it) and **unresolved-name-stable** (extending a half-typed name doesn't move it).
That is exactly the "did the structure actually change" hash we want as an
`input_fingerprint`. The store is new; the *fingerprint* is not.

## The artifact chain and per-stage keys

A unit (one source file) has a chain; each stage has its own key and its own
dependency set:

| Stage | Output | Key depends on | Serializable? |
|---|---|---|---|
| lex | `Vec<Token>` | source bytes only | ✅ pure data (`TokenKind`/`Token`) |
| parse | `ast::Program` | source bytes only | ✅ pure data (`Expr`/`Stmt`/`Program`) |
| lower | `IR` + resolve diagnostics | source **+ resolved-import fingerprints** | ⚠️ `IR` interns `&'static str` (op/str) — needs re-interning on load |
| check | `Build` + checker diagnostics | IR + **config fingerprint** (registry, `NativeOps`, attr ext) | ❌ `Build`/`Module` is a lowlevel runtime arena |

Two clean facts fall out:

1. **Parse of a file never depends on imports.** So an import edit reuses the
   importer's `tokens`/`AST` and only re-lowers/re-checks it — the *minimum*
   invalidation, exactly the "carefully manage dirtiness" property.
2. **The check-stage key must be config-aware.** The checker's result depends on
   the registry, the native-operator set, and the attribute extension (the
   compute package registers `$jit`/`$launch`/`Kernel`). Deriving the correct
   result from a cache whose config differs is unsound, so the config is part of
   the key (a config fingerprint, e.g. a hash of the enabled native ops +
   registry version).

## Dirtiness semantics (the careful part)

- **Per-stage**: the key selects the stage, so a lex-only edit dirties `tokens`,
  `AST`, `IR`, `check` (a new source → new tokens → new AST → new IR → new
  check), but the check of *other* units is untouched unless their fingerprint
  transitively changed.
- **Transitive but precise** via dependency fingerprints: A's IR key hashes the
  fingerprints of A's imports. If import B changes structure, B's fingerprint
  moves → A's IR/check key moves → A re-lowers + re-checks. But A's `tokens`/`AST`
  key (source-only) does **not** move → A's parse is reused.
- **Error blocks / unresolved names**: `program_signature` already excludes
  error-block content and hashes unresolved names to a single sentinel, so
  editing a broken region (the editor's typing case) or growing an unresolved
  name does **not** move the fingerprint → the build is reused. The store inherits
  this for free; **this is the biggest "share as much as possible" win.**
- **Cycles**: lichen lets a module import itself transitively (mutual recursion).
  A dependency-fingerprint round trip has no topological order in the cycle. The
  store must handle this (e.g. cache cyclic SCCs at a coarser grain, or iterate
  to a fixpoint, or simply not cache within a cycle). Flagged as a real edge.

## How processes share (the mechanism)

A **content-addressed store on disk**, per-project (`<root>/.lichen-cache/`),
plus an optional mmap fast path inside a long-lived process (the server). Writes
are immutable and atomic:

```
put(key, bytes):   write <tmp> then rename to cache/<key>
get(key) -> bytes: read cache/<key> (or mmap it)
```

- **No locks needed.** Content-addressed + write-once + atomic rename is the
  standard lock-free multi-process CAS. Two processes racing to `put` the same
  key produce identical bytes; the rename is atomic; a reader sees either the old
  (absent) or the new (complete) file, never a partial one.
- **Reads are mmap-able**, so the LSP (which re-reads the same file's artifacts
  on every keystroke) pays an mmap, not a parse. mmap is an optimization; the
  on-disk CAS is the source of truth and the sharing substrate.

## Serialization format and versioning

- Use `serde` + `bincode` (compact, fast, `#[derive(Serialize, Deserialize)]` on
  the syntax types). `bincode` is fine for the pure-data stages; re-evaluate
  `rmp-serde`/`postcard` if size matters.
- **Every entry carries a header** `(format_version, compiler_version)` folded
  into the key. A grammar change or a new compiler bumps the version → the store
  is invalidated wholesale (old entries simply become un-reachable). This is the
  honest way to avoid a stale cache after a tool upgrade (the rustc `-C
  incremental` precedent). The existing `Cargo.lock`-derived version or a build
  hash is the natural `compiler_version`.
- **The IR's `&'static str` interning** (op names, string literals) is the one
  serialization impedance: serde a `String`, and on load re-intern into the
  caching interner via the same leak-map. Small, contained; a custom serde on the
  literal/op fields.

## The new crate: `lichen-artifact`

This is *infrastructure*, not a tool — so it is legitimately its own crate (the
"no per-tool crate" rule does not apply to shared substrate). It sits below the
tools, above the frontend:

```
lichen-language (frontend)   ← the artifacts & fingerprints
      ▲
lichen-artifact (store + engine + serialization)   ← NEW, shared by every process
      ▲
lichen-language-server  lichen-language-zed  lichen (CLI)  (future lichen-format)
```

API sketch:

```rust
pub struct Store { dir: PathBuf, version: Version }          // content-addressed CAS
impl Store {
    fn get(&self, key: &Key) -> Option<Arc<[u8]>>;
    fn put(&self, key: &Key, bytes: &[u8]);                  // atomic rename
}

pub struct Engine { store: Store }
impl Engine {
    /// tokens, AST, IR + diagnostics, reusing the store at every stage,
    /// keyed by the stage-specific fingerprint (which encodes deps/config).
    fn analyze(&self, unit: &UnitId, source: &Source, inputs: &StageInputs)
        -> UnitArtifacts;
}
```

The CLI, the server and the zed tooling all call `Engine`. The server can hold
the store's mmap open for the session; the CLI uses the same files.

## Scope — what is actually worth building first

The honest cost ordering:

- **Phase A — the syntax cache (feasible, high value).** Persist `tokens` + `AST`
  keyed by source-content hash. Pure data, no interning, no dep graph, no
  module. The CLI and the LSP share the parse. This is the bulk of the
  duplicated per-process work (lex+parse are string-heavy and dominant for an
  editor) and the cheapest to build correctly.
- **Phase B — per-unit IR + diagnostics (medium).** Add dependency-aware keys
  (import fingerprints) and the IR re-interning. The LSP then publishes
  diagnostics from the cached IR + check output; the `Build` (needed only for
  hover) is derived **on demand** from the cached IR, not re-parsed.
- **Phase C — the checked `Module` / a compile daemon (hard).** Serializing the
  lowlevel `Build`/`Module` (arena + slotmarks + live values) is a large
  sub-project; alternatively a daemon owns the checked modules and serves them
  over IPC. Only justified if Phase B shows re-check is measured to dominate
  cross-process. **Not recommended now.**

## Honest risks / the genuinely hard parts

1. **`Build`/`Module` serialization** is the wall. It is a lowlevel runtime
   arena, not a data structure. Phase A/B deliberately avoid it; Phase C must
   either serialize it (big) or fork to a daemon (bigger). Don't design around
   it until Phase B's numbers say so.
2. **The import dependency graph** and cycle handling. Computing a unit's IR key
   needs its imports' fingerprints; cycles break topological order. Need an SCC
   (or fixpoint) story before Phase B is sound.
3. **Config-awareness of the check key.** Getting the registry/native-ops/attr
   fingerprint wrong is a silent-correctness bug. The key MUST include it.
4. **Cache invalidation after a real (not just version) change** — versioning
   handles compiler upgrades; a *source* change is handled by content addressing.
   The remaining correctness risk is a fingerprint that is *too lazy* (owns a
   change it should have invalidated on). Prefer over-invalidation (a fingerprint
   that changes slightly more often than strictly necessary) to silence this,
   and measure the hit rate.
5. **DDoS by cache fill** — a huge project with every keystroke writing new
   entries. Bound the store (LRU by last-access, or GC unreferenced entries) so
   the cache doesn't grow without limit.

## Summary of the proposal

- **Mechanism**: a per-project, content-addressed, versioned, on-disk artifact
  store; mmap for the fast path. Immutable write-once entries make it
  concurrency-free across processes.
- **Dirtiness = the key, derived not stored.** Reuse the existing
  name-resolved `program_signature` as the input fingerprint; make it
  per-stage and dependency-aware (imports for `IR`/`check`, config for `check`),
  so an edit dirties exactly what it touches and nothing more.
- **Share as much as possible**: parse of a unit is source-only (reused across
  any import change); error-block and unresolved-name edits are fingerprint-free
  (reused wholesale); every process reads the same store.
- **Build in phases**: A (syntax) → B (IR + diagnostics) → C (checked module /
  daemon), with C explicitly deferred.
