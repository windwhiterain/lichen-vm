# Static modules & the shared registry

> Status: current
> Points at: `crates/lichen-lowlevel/src/static_module.rs` and `lib.rs` (`Registry`,
> `StaticModule`, `ModuleKey`, `AnyNodeId`), plus
> `crates/lichen-language/src/persist.rs` (the persistent device store).

A **static module** is a compiled, frozen program fragment that importers use *in
place* — its values are read, never copied. This is the mechanism the package store
builds on ([packages](packages.md)).

## Key-carrying refs, absolute from birth

Every static ref names its home module globally:

- `ModuleKey(u64)` — a module's device key, allocated by the persistent store.
- `StaticNodeId { module, index }`, `AnyNodeId = Dynamic | Static`, and the
  handle/function equivalents.

There is **no relative form and no re-basing**: a ref is identical in the module's own
payloads and in any importer's storage. Resolution is `Registry::get(key)`. This
collapsed an earlier two-level owner-context design and a read-time re-base copy
design, which were O(N²) across a dependency tree — because a ref names its module
from birth, every importer shares one arena and there is nothing to retarget.

## The registry

`Registry<P>` is the process-wide `Arc<RwLock<Registry<P>>>` singleton that registers,
stores, and resolves static modules while evaluating. `Module` is per-thread; the
shared substrate is the registry.

- `freeze_mapped(&mut self, module, key: ModuleKey, hash) -> Freeze` — compile a module
  into a static artifact and file it under a caller-provided device `key` and content
  `hash` (the key is allocated by the persistent device store, so a loaded artifact's
  refs are baked with their final key).
- `get(key) -> Option<&Package>` — the filesystem read (`None` = no such key; a ref
  naming an unregistered key is a broken graph and panics at its resolution site).
- `insert_module(key, hash, module)` — file an already-deserialized artifact.

`Module::freeze` / `Module::freeze_mapped` are the module-facing conveniences. Keys are
**reclaimed** (via `cache gc`), so the key space stays bounded, and re-inserting a key
after reclamation under a different `hash` is recognized as a new artifact.

## Persistent device store

Under `~/.lichen` (`$LICHEN_HOME` overrides; `crate::persist::lichendir()`), the
**device registry** (`persist::DeviceRegistry`) owns the keys and the **file-ID
keyed** artifact files (`artifacts/<sha256(file_id)>.module`). A **file ID** is a
compiled unit's identity: an on-disk `.lichen` file's canonical path, or
`virtual:<name>` for an embedded source. Each file keeps **one** cache slot —
recompiling a modified file **overwrites** that slot rather than accumulating a
new content-addressed artifact. The lowlevel registry stays the in-memory
runtime map; the device registry owns the keys.

- The `Hash` (SHA-256 of source ‖ dependency keys in source order) is the artifact's
  *identity* for verification — transitive and deterministic.
- **Incremental load:** verify the recorded dependency graph (one source-file hash per
  node plus key lookups); recompile only the chain that changed; otherwise deserialize
  and register, skipping the compile.
- **CLI:** `lichen-compiler cache gc` is a *clean*: it removes every artifact whose
  file ID is **not** a `.lichen` path and **not** a `virtual:` path (a bare `[depend]`
  / `load_package` only admits `.lichen` files, so this prunes out-of-band or stale
  entries), keeping exactly the on-disk and embedded lichen sources.
- **Only `.lichen` files are packages:** `load_package` rejects a non-`.lichen` path
  (the `virtual:` embedded sources are the exception), so the cache invariant — every
  file ID is a `.lichen` or `virtual:` path — holds by construction.

## Reads & materialize

`Module::static_read(sref) -> P::Value` is a `get` plus a table read — verbatim, no
copy, no `&mut`; the value's payloads stay in the module's shared arena. `evaluate_node`
takes `AnyNodeId` with a static early-return arm; the deep pass treats static refs as
decided leaves. Applying a static function re-opens a materialize walk with a per-call
remap (see `static_function_apply`).

## GC & asserts

GC keeps static handles verbatim: the static arena has no block to vacate, its item refs
are all static, and re-homing the payload would break identity with every other reader.
`check_asserts` reads static subtrees as decided leaves.
