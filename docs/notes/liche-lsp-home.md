# Lichen Home for the LSP: persist the cache, self-heal = (re)create lazily

> Status: implemented (landed).
> Points at: `crates/lichen-language-server/src/{home.rs,server.rs,analysis.rs}`
> (the change), `crates/lichen-language/src/{package.rs,persist.rs}` (the store
> that already exists), and the notes
> [artifact-cache](artifact-cache.md),
> [language-toolchain](language-toolchain.md),
> [incremental-parse-compile](incremental-parse-compile.md).

## The problem

`lichen-language-server` never touched Lichen Home. Its `Backend` held only the
per-buffer source text, and each request re-ran the whole frontend in
`spawn_blocking`:

```rust
// server.rs (before)
sources: Mutex<HashMap<Url, String>>   // the only state
...
Doc::<P>::new_with_base(text, base).lsp_diagnostics()
```

And `Doc::new_with_base` built a **fresh in-memory store every time**:

```rust
// analysis.rs (before)
let mut store = PackageStore::<P>::new();   // in-memory, dropped at end of request
```

So the LSP recompiled every `@import`ed package on every keystroke, never wrote
an artifact to disk, and never shared compiled artifacts with the `lichen`
compiler across processes. This contradicts what `artifact-cache.md` already said
should happen — the LSP should reuse `persist`/`package` for settled per-file
artifacts exactly as the CLI does.

## The change

Route the LSP's *settled imported packages* through the persistent device store
at Lichen Home. The open buffer's own text stays in-process (`BufferSession`),
as `artifact-cache.md`/`incremental-parse-compile.md` require; only the settled
imported `.lichen` packages are cached on disk and shared cross-process.

### 1. `home.rs` — `LichenHome` (new)

A tiny `Send + Sync` type that owns the "the home must exist" half of the
self-heal. It holds no `P` type, so it never reintroduces the composed-program
`!Send` problem into `Backend<P>`.

```rust
pub struct LichenHome {
    dir: PathBuf,          // resolved home ($LICHEN_HOME, else ~/.lichen)
}

impl LichenHome {
    pub fn resolve() -> LichenHome { ... }        // persist::lichendir()
    pub fn ensure(&self) -> &Path { ... }         // create_dir_all(dir/artifacts)
    pub fn cache_root(&self) -> &Path { &self.dir }
}
```

### 2. `analysis.rs` — `Doc::new_with_cache`

```rust
pub fn new_with_base(source, base) -> Doc<P> { Doc::new_with_cache(source, base, None) }

pub fn new_with_cache(source, base, cache_root: Option<&Path>) -> Doc<P> {
    let mut store: PackageStore<P> = match cache_root {
        Some(root) if P::Codec::PERSISTENT => PackageStore::with_cache_dir(root.to_path_buf()),
        _ => PackageStore::new(),
    };
    // ... identical body; the store is passed to preprocess/build_report as before
}
```

The store is persistent only when a cache root is supplied **and** the program's
artifact codec can serialize; otherwise it is in-memory. `new_with_base` (tests,
`NoPersist` embeddings) stays in-memory by passing `None`.

### 3. `server.rs` — `Backend` owns the home

`Backend` gains `home: Arc<LichenHome>` (initialized in `new`, which calls
`resolve()` + `ensure()` — the home is (re)created lazily at LSP start, i.e. when
a lichen buffer is opened). Every request switches its `Doc` to
`new_with_cache(text, base, Some(self.home.cache_root()))`. `Backend<P>` remains
`Send + Sync` (`Arc<LichenHome>` is `Send + Sync` because `LichenHome` is a
`PathBuf`; `sources` is `Mutex<HashMap>`).

### 4. Store root is per plugin set (consistency with the compiler)

Per `artifact-cache.md`, the compiled-artifact store is scoped per plugin set: the
shipping server uses `lichendir()`, a plugin-composed server uses
`lichendir()/compilers/<plugin-set-key>`. The composed server can derive its root
the same way `plugin.rs` does; the shipping path passes `lichendir()` directly.

## Self-heal: just "missing/corrupt home (re)created lazily"

The self-heal is deliberately minimal and is exactly the two cases the store
already covers, plus the LSP supplying a root at all:

- **Missing home → created lazily.** `LichenHome::ensure()` runs `create_dir_all`
  at server start (first lichen buffer), so `~/.lichen` and its `artifacts/`
  appear on demand — never at install time.
- **Corrupt home → repaired lazily by the store.** `DeviceRegistry` already
  reloads the last-known (or empty) state from an unparseable `registry` and
  repairs it on the next `save`; a corrupt/missing artifact makes `try_reuse`
  return `Ok(None)`, which recompiles. We add no extra reset/repair layer — the
  store heals and the LSP just benefits from its fault-tolerant reload.
- **Never in-memory unless intended.** An in-memory store is used only when the
  program's codec cannot serialize (`NoPersist`, `P::Codec::PERSISTENT == false`)
  or no root is supplied (`Doc::new_with_base` / tests). A healthy home is never
  silently dropped to a separate in-memory path: if the home is not writable, the
  store's own `let _ =` write paths simply stop persisting while the server keeps
  compiling from source — the LSP keeps serving diagnostics either way.

## What is deliberately NOT added (and why)

- **No degrade-to-in-memory state machine.** No `Off`/`Degraded` enum, no
  per-request fallback. The store is already fault-tolerant; routing the LSP
  through it is sufficient.
- **No lock-hang / key-reclaim special handling.** The store's `RegistryLock`
  stale-timeout (10 s) and its fault-tolerant reload already cover the crash
  cases; the edge where a device key is reclaimed under a still-running process
  remains a CLI-oriented `panic!` in `try_reuse`, untouched here. If the server
  ever hits it in practice it is a separate hardening pass.
- **No opportunistic GC / compile daemon.** Cache growth is bounded by the
  existing `cache gc`; a cross-process compile daemon is already noted as out of
  scope in `artifact-cache.md`.
- **No in-memory registry held across requests,** which would reintroduce the
  `!Send` problem for plugin-composed servers. Persistence is the disk store.

## Files changed

- `crates/lichen-language-server/src/home.rs` (new) — `LichenHome`.
- `crates/lichen-language-server/src/analysis.rs` — `Doc::new_with_cache`;
  `new_with_base` delegates to it.
- `crates/lichen-language-server/src/server.rs` — `Backend` holds
  `Arc<LichenHome>`; every request threads `cache_root()`.
- `crates/lichen-language-server/src/lib.rs` — `pub mod home;`.

## Verification

- `cargo check` passes.
- `cargo test -p lichen-language-server` (see below).

New tests (following `crates/lichen-language/tests/persist.rs`, but through
`Doc::new_with_cache`) cover: a package compiled into a temp cache is reused by a
second `Doc` on the same cache; a missing home is created; a corrupt `registry`
is tolerated (still produces diagnostics). A composed server's `Backend<P>` is
still `Send + Sync` (the `Arc<LichenHome>` addition keeps that invariant).
