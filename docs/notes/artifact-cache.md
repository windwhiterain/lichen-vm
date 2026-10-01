# Cross-process artifact store: it already exists

> Status: current — this note records *what exists* and what the
> toolchain reuses; it is not a proposal to build.
> Points at: `crates/lichen-registry` (the type-independent device layer: the
> byte reader/writer, `ModuleKey`, the disk `DeviceRegistry`, the hashes),
> `crates/lichen-language/src/persist.rs` (the vocabulary artifact codec +
> `load_artifact`, which re-exports the registry layer for the old `persist::*`
> paths), `crates/lichen-language/src/package.rs` (the package store that drives
> it), `crates/lichen-lowlevel/src/static_module.rs` (the frozen artifact).

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

`lichen-registry::device` (re-exported as `persist::artifact_hash`):

```rust
pub fn artifact_hash(source: Hash, deps: &[(ModuleKey, Hash)]) -> Hash
```

SHA-256 over the raw source hash **followed by each direct dependency's key and
identity, in source order**. It is:

- **transitive** — a dependency's identity is the identity *it* was published
  with, so a change anywhere in the recorded closure changes every importer's
  identity, and the importer recompiles;
- **deterministic** — every process computes the same identity for the same
  source chain, so keys agree across processes;
- **precise per stage in effect** — a file's token/AST depend on its own source
  only; only its lower/check depend on the (keyed) dependencies.

**The dependency's identity, not its key, is what makes it transitive.** A
recompile **reuses the key** (the key names the cache slot, not the content
behind it — see `DeviceRegistry::alloc`), so a fold over keys alone leaves an
importer's identity unchanged when a dependency's *content* changes. That is not
a missed recompile but a wrong one: a frozen artifact is full of cross-module
node references written as `(dependency key, index)`, so serving it after its
dependency changed resolves those indices against the dependency's **new** node
layout — wrong answers, and, when the index no longer names a slot, a hard
`invalid SlotMap key used` panic. This was not hypothetical: the language server
crashed on it every time an imported `.lichen` file was edited (recorded in
[code-audit](code-audit.md), `P1-17`).

Three sites answer "what identity does this dependency contribute", and they
must give the **same** answer — a disagreement is not a miss but a permanent one,
since the dependent would then recompile on every single run:

- `DeviceRegistry::artifact_identity` — the write side, which needs the identity
  *before* publishing, because the frozen artifact carries it in its header and
  the load rejects a header that does not match;
- `DeviceRegistry::publish` — the fold recorded with the artifact;
- `DeviceRegistry::verify` — the fold **recomputed** from the graph as it stands
  now, not read from the record. That recomputation is the whole mechanism: a
  dependency republished since folds in its new identity, the answer stops
  matching the frozen header, and the load rejects it.

All three read the **registry's own record**, never the caller's in-memory view:
an embedded dependency (`virtual:<name>`) has no source file and need have no
record, and it must contribute the all-zero sentinel on both sides of the fold
or every dependent would miss forever. `Entry::artifact` carries that recorded
identity per file ID, which is what makes the fold transitive past one level;
the registry file format is version 3 for it, and a version-2 file reads as
unreadable and is recovered as a fresh registry — one full recompile, nothing
else.

`DeviceRegistry::verify(file_id, source)` is the *incremental verification*: it
walks the **recorded** dependency graph (each node compares one source-file hash
and recurses into its recorded deps) — "a source file hash and an index lookup
per node, never a re-parse or a transitive re-hash — and only the chain that
actually changed is recompiled." Note what the walk alone cannot catch: a
dependency that was *already recompiled and republished* earlier in the same
store matches its record again, so the walk passes and only the identity fold
above rejects the importer.

## Cross-process sharing

`DeviceRegistry` (defined in `crates/lichen-registry/src/device.rs`, re-exported
as `persist::DeviceRegistry`) is the disk store under the CLI's `~/.lichen`
(`persist::lichendir`, or `$LICHEN_HOME`):

- **keys are stable across processes** — `ModuleKey` is a compact index the
  store allocates by **file ID** (a compiled unit's identity: an on-disk
  `.lichen` path, or `virtual:<name>` for an embedded source) and reclaims via a
  free-list, so the same file gets the same key in every process;
- **artifacts are file-ID keyed** — stored at `artifacts/<sha256(file_id)>.module`
  and **overwritten** on recompile, so a frequently modified file keeps exactly
  one cache slot (no content-addressed accumulation);
- **writes are atomic** — an artifact file is written via temp + rename
  (`store_artifact`); the registry file the same (`save`); a lost update only
  costs a recompile, never corruption;
- **mutations are serialized across processes** by a `mkdir` lock with
  stale-timeout recovery (`RegistryLock`), while reads (`verify`) lock nothing;
- **bounding** — `gc` is a *clean* that removes every artifact whose file ID is
  not a `.lichen` path and not a `virtual:` path, and `remove(file_id)` drops one
  file's slot, each reclaiming keys and deleting artifact files;

**The store is split in two.**  Everything above that never names a program
value or operator — the byte reader/writer, `ModuleKey`, the `DeviceRegistry`
(key allocation, the file-ID → entry table, the `mkdir` lock, the registry file
format) and the hashes — lives in the leaf [`crates/lichen-registry`](../../crates/lichen-registry),
so a consumer can open a cache and reclaim artifacts without linking the
language/VM stack.  `crates/lichen-language/src/persist.rs` keeps only the
vocabulary half (`ArtifactCodec`, the artifact container serialization,
`load_artifact`) and re-exports the registry items so the old
`persist::{DeviceRegistry, ModuleKey, artifact_hash, …}` paths keep resolving.
The package manager owns `clean` on that seam: it opens the shipping compiler's
base cache root (`lichendir()`) and every plugin-composed compiler slot's
registry (`<lichendir>/compilers/<key>`) and calls `gc()` itself — the compiler
binary has no cache subcommand (see [package-manager](package-manager.md)).

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

### The artifact store is scoped per plugin set

The `DeviceRegistry` cache root is **not** `lichendir()`: the compiler CLI
takes an explicit cache root ([`lichen_compiler::cli::main_with_cache_dir`]), and
**every** compiler — shipping and **plugin-built** — scopes it to a
`compilers/<plugin-set-key>` slot (`lichendir()/compilers/<key>`).  This isolates
the *compiled-artifact* store per vocabulary.  The artifact encoding depends on
the compiler's value/operator leaves (`ProgramCodec`), so a compiler built over a
different plugin set produces a *different* artifact for the same file ID —
sharing the base `lichendir()` store would let one plugin set reuse (or thrash)
another's, and a deserialize-then-recompile churn.  The shipping compiler uses
the **empty plugin set's** slot (`compilers/<toolchain-key>`,
[`persist::shipping_cache_root`]); a plugin-built one uses its own slot.  Only the
compiled-artifact store is scoped; the git **source** cache
(`lichendir()/sources`) stays shared across compilers (it holds the same fetched
plugin sources).

## What this means for the new tooling crates

- **Do not build a store.** `lichen-language-server` and `lichen-language-zed`
  should **reuse** `lichen-language::persist` / `package` for the *settled*
  per-file artifacts, exactly as the CLI does — the LSP, run on the same
  `~/.lichen`, sees the CLI's compiled artifacts and vice versa, which is the
  cross-process sharing demo.
- **The live edit path stays in-process, and it does not reuse the session.**
  The open buffer's re-analysis is the language server's own per-text cache of
  its extracted index (`DocIndex`, `P1-17`) — deliberately separate from the
  frozen cross-process artifacts, because a buffer being typed is not a settled
  module.  [`BufferSession`](incremental-parse-compile.md) is the incremental
  machinery this note once named for that job; it is built and tested but has
  **no production consumer** (`P2-1`), and wiring it is `D6`'s (b) — it would
  avoid the lex and parse when the text *changes*, which the index cache does
  not, and it stays worth doing on its own terms.
- **What is actually new** is the editor-view glue: span↔LSP-position
  conversion, the name-resolution index for hover / go-to-definition, and
  diagnostics→LSP — i.e. the `lichen-language-server` tooling library, layered on
  the existing frontend.

## The remaining seam (only if re-parse is ever measured to dominate)

Nothing here needs a new store. If, later, the *live* per-buffer re-check cost
is measured to dominate and a running `Build` must be shared across processes, a
compile daemon (owning the checked modules, serving them over IPC) is the shape —
deferred until then.
