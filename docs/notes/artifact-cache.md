# Cross-process artifact store: it already exists

> Status: implemented (landed) — this note records *what exists* and what the
> toolchain reuses; it is not a proposal to build.
> Points at: `crates/lichen-language/src/persist.rs` (the store + serialization),
> `crates/lichen-language/src/package.rs` (the package store that drives it),
> `crates/lichen-lowlevel/src/static_module.rs` (the frozen artifact).

The request "**no matter how many processes, serialize artifacts, manage their
dirtiness carefully, and share them as much as possible**" is answered by code
that is already in the tree. The toolchain does **not** need a new artifact
cache; it needs to reuse the one that exists. This note records the mechanism so
the new tooling crates sit on it rather than rebuilding it.

## The compile artifact is a frozen `StaticModule`

`StaticModule::from_module` freezes a **fully solved** `Module` into an
immutable, shareable form (`static_module.rs`):

```rust
pub struct StaticModule<P> { key: ModuleKey, nodes: Vec<StaticNode<P>>, functions: Vec<StaticFunction>, arena: Vec<u8> }
```

- Every node holds its final answer (or a residual `Parameterized` operation).
- Payloads (array item slices, ext-value bytes) are laid out once into a single
  flat `arena: Vec<u8>`.
- Every value ref is rewritten to static form keyed by `Self::key` — **absolute
  from birth**, so an importer stores and resolves them verbatim; nothing is
  retargeted or copied, and the module's arena is shared by every importer.

The `Build`/`Module` never gets serialized directly. Freezing it into a
`StaticModule` first is what makes it a serializable artifact — this is the
answer to "can the compiled `StaticModule` serve as the artifact of compile?",
and it is **yes: it already does.**

## Dirtiness = a transitive, deterministic content hash

`persist.rs`:

```rust
pub fn artifact_hash(source: &[u8], dep_keys: &[ModuleKey]) -> Hash
```

SHA-256 over the raw source bytes **followed by its direct dependency keys in
source order**. It is:

- **transitive** — a dependency change changes the importer's hash, so the
  importer recompiles;
- **deterministic** — every process computes the same hash for the same source
  chain, so keys agree across processes;
- **precise per stage in effect** — a file's token/AST depend on its own source
  only; only its lower/check depend on the (keyed) dependencies.

`DeviceRegistry::verify(path)` is the *incremental verification*: it walks the
**recorded** dependency graph (each node compares one source-file hash and
recurses into its recorded deps) — "a source file hash and an index lookup per
node, never a re-parse or a transitive re-hash — and only the chain that
actually changed is recompiled."

## Cross-process sharing

`DeviceRegistry` (`persist.rs`) is the disk store under the CLI's `~/.lichen`
(`persist::lichendir`, or `$LICHEN_HOME`):

- **keys are stable across processes** — `ModuleKey` is a compact index the
  store allocates, maps to content hashes, and reclaims via a free-list, so the
  same content gets the same key in every process;
- **writes are atomic** — an artifact file is written via temp + rename
  (`store_artifact`); the registry file the same (`save`); a lost update only
  costs a recompile, never corruption;
- **mutations are serialized across processes** by a `mkdir` lock with
  stale-timeout recovery (`RegistryLock`), while reads (`verify`) lock nothing;
- **bounding** — `gc` and `remove` mark-and-sweep from root source files through
  the dependency graph, reclaiming keys and deleting artifact files;

`PackageStore` (`package.rs`), with `with_cache_dir(dir)`, drives it:

```
load_package(path)
  ├─ device.verify(path)          # incremental, whole-graph up-to-date check
  │    └─ try_reuse(...)          # no recompile: load+deserialize+register
  └─ build_package(path)          # compile → eval-deep → freeze_mapped →
                                  #   serialize_artifact → store_artifact + publish
```

So the CLI already shares the frozen artifacts of closed files with every other
process using the same cache directory. The `StaticModule` (frozen, keyed,
dependency-aware) *is* the cross-process artifact.

## What this means for the new tooling crates

- **Do not build a store.** `lichen-language-server` and `lichen-language-zed`
  should **reuse** `lichen-language::persist` / `package` for the *settled*
  per-file artifacts, exactly as the CLI does — the LSP, run on the same
  `~/.lichen`, sees the CLI's compiled artifacts and vice versa, which is the
  cross-process sharing demo.
- **The live edit path stays in-process.** The open buffer's incremental
  re-analysis is [`BufferSession`](incremental-parse-compile.md) (already in
  `lichen-language`) — deliberately separate from the frozen cross-process
  artifacts, because a buffer being typed is not a settled module.
- **What is actually new** is the editor-view glue: span↔LSP-position
  conversion, the name-resolution index for hover / go-to-definition, and
  diagnostics→LSP — i.e. the `lichen-language-server` tooling library, layered on
  the existing frontend + session.

## The remaining seam (only if re-parse is ever measured to dominate)

Nothing here needs a new store. If, later, the *live* per-buffer re-check cost
is measured to dominate and a running `Build` must be shared across processes, a
compile daemon (owning the checked modules, serving them over IPC) is the shape —
deferred until then.
