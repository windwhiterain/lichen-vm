# Cross-process artifact store

> Status: current
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
the tooling crates sit on it rather than rebuilding it.

## The compile artifact is a frozen `StaticModule`

`StaticModule::from_module` freezes a **fully solved** `Module` into an
immutable, shareable form (`static_module.rs`):

```rust
pub struct StaticModule<P> { key: ModuleKey, nodes: Vec<StaticNode<P>>, functions: Vec<StaticFunction>, arena: Vec<u8> }
```

- Every node holds its final answer (or nothing, for a residual operation).
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
`invalid SlotMap key used` panic. The identity fold is what prevents it (see
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
identity per file ID, which is what makes the fold transitive past one level.

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

## What the tooling crates do with it

- **No new store.** `lichen-language-server` and `lichen-language-zed`
  **reuse** `lichen-language::persist` / `package` for the *settled* per-file
  artifacts, exactly as the CLI does — the LSP, run on the same `~/.lichen`, sees
  the CLI's compiled artifacts and vice versa, which is the cross-process sharing
  demo.
- **The live edit path stays in-process.** The open buffer's re-analysis is the
  language server's own per-document session and its extracted index (`DocIndex`,
  `P1-17`) — deliberately separate from the frozen cross-process artifacts,
  because a buffer being typed is not a settled module.
  [`BufferSession`](incremental-parse-compile.md) is the incremental machinery
  for that job; the server runs it on the compile worker, and what remains open
  is the keystroke path (`P1-17` (b)) — it would avoid the lex and parse when the
  text *changes*, which the index cache does not.
- **The layer on top is the editor view**: span↔LSP-position conversion, the
  name-resolution index for hover / go-to-definition, and diagnostics→LSP — i.e.
  the `lichen-language-server` tooling library, layered on the existing frontend.

## The remaining seam (only if re-parse is ever measured to dominate)

Nothing here needs a new store. If, later, the *live* per-buffer re-check cost
is measured to dominate and a running `Build` must be shared across processes, a
compile daemon (owning the checked modules, serving them over IPC) is the shape —
deferred until then.

## Recovered measurements

### The artifact file format

The file is `artifacts/<hash>.module`:

- header: `magic "LCHN" | version u32 | key u64 | hash 32B | max_align u64 |
  body_digest 32B`;
- body (everything after the header): `export u64 | arena_len u64 | arena bytes |
  node_count u64 | nodes… | function_count u64 | functions…`;
- a **function frame** is `parameter u64, return u64, assert_count u64, [asserts
  u64…], node_count u64, [nodes u64…]`, and that node list is the function's
  template scope in local-index order — the static mirror of `Function::nodes`, so
  a re-homed static closure knows its own scope;
- a **node frame** is `value_flag u8, [value], op_flag u8, [op_tag, operand_flag,
  operand], equality (parent/next/tail: flag+u64, size u32), runned u8,
  evaluated_deep (flag u8, [undecided u8]), low_shape u8 [shape]`. The deep
  verdict is a flag of its own because `None` ("the pass never ran") and
  `Some(undecided)` ("the pass ran and could not decide") are different facts,
  and the materialize pass's carry rule tests them apart.

Refs (node items, function values, array handles) are written as their module's
device key plus the local index — or the arena-relative offset and length for a
handle. Keys are stable across processes, so the serialized form needs no
relocation on load and the loader only re-resolves the arena pointers; a handle's
`offset` is a plain base-relative number, rebuilt against the freshly laid-out
arena with the same alignment formula the freeze used.

**Low-shape tags are the compatibility contract:** `0`–`4` are the decided shapes
(`USize`, `Tuple`, `Array`, `Function`, `Table`) and never change, `5` is
`Unknown` (the lattice bottom) and `6` is `Float`.

### The vocabulary half of the layout

`lichen-lowlevel`'s codec is the part `lichen-language::persist` is generic over;
the only vocabulary-dependent parts are the value and operator encodings, and
this crate owns them because `LowValue`'s handle variants need the arena
relocation the container provides.

- **A value is a one-byte tag plus its payload.** `LowValue`: `0` `USize` (u64),
  `1` `Array` (relocated handle), `2` `Function` (owner `ModuleKey` + function
  index), `3` `None`, `4` *reserved* — the deleted `Parameterized` marker, which
  an artifact carrying it is refused **by name** rather than reused, `5` `Str`
  (u32 length + UTF-8 bytes, leaked to a `&'static str`), `6` `Table` (relocated
  handle), `7` `Error`, `8` `Float` (the 32-bit pattern, because the artifact is
  bytes and a decimal spelling would not round-trip every value).
  `LowOperator` is one byte: `Index` 0, `Apply` 1, `TableGet` 2 — tags are
  additive and the order is not semantic.
- **A relocated handle is three u64s**: the owning module's key, the payload's
  base-relative byte offset, and the element count. The owner is this module for
  `self_key` and a registered dependency otherwise — the one lookup the array and
  the table leaf share, so the two cannot drift. A `Function` ref is checked
  against its module's function count only when the map holds that module, since
  only direct dependencies are in it.
- **The base is `arena_align`, rounded up from the arena's own address** —
  the strictest alignment any arena payload kind needs. Both the freeze layout
  (`StaticModule::from_module`) and this codec derive the base from that one
  function, so writer and reader provably agree; the header records the alignment
  as a corruption guard.
- **Reading a handle is where the format is adversarial.** Every bound is checked
  with checked arithmetic: the offset must be a whole number of items, the count
  must not overflow its byte length, `offset + byte_len` must lie inside the
  arena, and the base must lie inside its own buffer. A crafted `(offset, len)`
  pair is then a clean `Err` — never a wrap, a panic, or a pointer outside the
  arena.

### The arena layout the freeze produces

`StaticModule::from_module` lays the artifact out as consecutive local indices
over the source's slotmap order; the flattenable payloads (array item slices,
table item slices, ext-value bytes) **deduped by `(ptr, len)`**, so two handles
that alias one allocation keep identity equality in the artifact and are laid out
once into a single arena; and every value rewritten to static form keyed by
`key` — absolute from birth, so the arena is shared by every importer. The key is
allocated by the registry **before** the build, so the artifact's refs are baked
with their final key.

Two layout rules make that sound: **the arena base derives from one alignment,
never from the payloads present** (`crate::codec::arena_align::<P>()`, with the
buffer over-allocated by that much so the aligned start exists inside it), so a
serialized artifact round-trips to the *same* base whatever payloads the program
happened to use; and **every payload alignment divides that base** — the
alignments are powers of two and each payload is placed at an `align_up`-ed
cursor. Phase 2 collects and dedupes, phase 3 is the single payload copy plus the
ref rewrite (the item rewrite applies to the arena copy, never the source slice),
and the open-capture verdict runs after phase 3, because the walk follows the
value edges the rewrite produced.

### Integrity, not authenticity

`body_digest` is the SHA-256 of **exactly the body bytes** — the bytes after the
header, and nothing before them (the digest field itself is not covered). The
reader verifies it *before* it reads any body field, so a truncated, mis-copied
or bit-rotted body is a clean "recompile" answer instead of a module that loads
and is silently wrong; field validation cannot replace it, because a corrupted
index can still land inside its declared range. It is **not** authenticity: it is
not a signature and does not restrain a deliberate writer, since whoever can write
the file can recompute the digest. The bound on a deliberate writer is memory
safety plus total field validation, not this field.

### The version history of `ARTIFACT_FORMAT_VERSION`

The reader accepts only the current value, so a change to either half (header
layout or body encoding) retires the artifacts written before it — they fail the
version check and recompile; there is no compatibility path.

- `5` added the low type's bottom, `LowShape::Unknown` (shape tag 5): a node's
  stored low type became a lattice position rather than a decided shape.
- `7` added the float value to the body's value encoding (value tag `8`, written
  as its bits).
- `8` added the struct marker's third field, the field names in definition order
  (`[TypeId, names, names_in_order]`). The node encoding is unchanged — a marker
  is an ordinary array — but its meaning is not: a 2-field marker is no longer a
  struct marker, so an older artifact reads its struct types as unrecognised
  shapes rather than failing.
- `9` moved the `TypeStruct` tag into the struct marker itself
  (`[payload, TypeStruct]`), so a v8 artifact reads its struct kinds as
  unrecognised shapes and its struct types — and every named read over them —
  would be silently wrong. v8 artifacts exist outside the source tree (the device
  cache), so the bump was warranted rather than skipped.
- `10` split a frozen node's collapsed `undecided` byte into `runned` and
  `evaluated_deep`. The read is *almost* a pure function of the deep verdict, but
  not quite: the old byte collapsed "the pass never ran" with "the pass ran and
  could not decide", which the materialize pass's carry rule tests apart, so a v9
  artifact would materialize differently from the source it was frozen from.

### Reader obligations

- `MAX_LOW_SHAPE_DEPTH = 256`: the wire form spends one byte per level, so without
  a cap a megabyte of crafted bytes is a million frames of native stack. A real
  shape's depth is bounded by the nesting of the program's own types, so the cap
  bounds hostile input, not programs.
- Every element of a length-prefixed list costs at least one byte on the wire, so
  a `count` past the reader's remaining bytes is impossible for a well-formed
  artifact — preallocating from `count` on faith is what would let a 64-byte file
  request a 2^60-element allocation and abort the process, while the
  preallocation itself is still wanted because for a real artifact `count` *is*
  the list's exact size.
- The export index is read **before** the node count it names, so it is validated
  separately (`check_node_index(export.index, node_count, "export")`).
- A loaded artifact owns **no release obligations**: a resource handle is
  process-local and cannot be in the bytes.
- The open-capture verdict is **not serialized**; it is recomputed from the
  loaded tables by the same walk the freeze runs, so an artifact cannot carry a
  verdict that disagrees with the graph it ships.

### What is not written, and what that costs

- **A serialization refusal means "no artifact form", not "wrong package".** The
  ordinary case is a package whose top level `$jit`s, which holds a live `Kernel`
  — a process-local registry handle with no on-disk form. The program runs either
  way, so the package is left **uncached** rather than failing a compile that
  would have succeeded. The compute values' codec arms refuse such a value **by
  name** rather than panicking, because the caller's answer is to not cache the
  module. The refusal is also **permanent** for that file: the device entry
  `PackageStore::alloc_key` wrote stays unpublished, and an unpublished entry can
  never verify, so the key is completed rather than reallocated and no slot is
  leaked.
- **Only packages are frozen, so only a package catches it.** A top-level `jit` in
  a *single* file never reaches the codec, because the main program is not
  serialized and only an importer freezes the package holding a kernel; an
  **in-memory** package store therefore exercises nothing here. A reproduction has
  to go through a **cache-backed** store, exactly as the CLI does.

### Store properties

- **The shipping slot is derived once, by both crates.** The compiler reads its
  cache from `<lichendir>/compilers/<key>` and `lichen-package` installs the
  shipping toolchain into that *same* directory; both call
  `lichen_utils::cache::compiler_slot_key` over the same repository and the same
  plugin set. A second derivation — or the same derivation over a different
  repository — on either side makes the compiler read a directory nothing ever
  writes to. `crates/lichen-language/tests/persist.rs` is the only test that
  spans the two crates, because nothing else links them.
- **`alloc` reserves a key before the compile publishes it.** A process that dies
  in between leaves a record with no artifact file beside it; the next run reads
  that record as a miss, recompiles into the **same** key and publishes there, so
  the key is completed rather than reallocated and no slot is leaked. The pending
  record is the other half of "writes are atomic".
- **`gc` is a *clean against*, not only a remover.** It drops every artifact whose
  file ID is neither a `.lichen` path nor a `virtual:` path. That is sound only
  because `PackageStore::load_package` rejects a non-`.lichen` path up front, so
  the only way a stray ID enters the registry is out of band (a native
  `register_native` path, or a hand-edited registry file) — which is why "every
  file ID is a `.lichen` or `virtual:` path" holds by construction rather than by
  a check on the write path.
- **Recovery preserves the bytes a diagnosis may need.** An unreadable registry
  file is never overwritten and never silently dropped: it is moved aside to the
  first free `registry.corrupt[.<n>]` sibling (an earlier quarantine is never
  overwritten) together with the `artifacts/` directory only it could describe,
  and the key space restarts with `next_key` **kept** and the free list dropped,
  so no key this process already handed out is reissued to a different module (a
  `ModuleKey` is a recycled index that artifact bytes embed as an absolute
  reference). Nothing is deleted, so a diagnosis can still read the bytes the
  recovery message names, and the store keeps working by recompiling. A
  *still*-unreadable file after one recovery in the same process is the same
  corruption, not a new one, so the restarted state stands and an
  `alloc`/`publish` pair still completes in memory.
