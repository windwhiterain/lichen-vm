# Static modules & the shared registry

> Status: current
> Points at: `crates/lichen-lowlevel/src/static_module.rs` and `lib.rs` (`Registry`,
> `StaticModule`, `AnyNodeId`), `crates/lichen-registry` (`ModuleKey`, the byte
> codec, the disk `DeviceRegistry` — the type-independent device layer), and
> `crates/lichen-language/src/persist.rs` (the vocabulary artifact codec that
> drives the store).

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

A **function** is carried the same way, and the consequence is that a static callee
has no body in the importer at all: an `AnyFunctionId::Static` resolves through the
artifact and contributes no node list to the importing module, so a host walking
that body finds nothing for it. That is not a shape to work around — a source body
mixes both kinds of call, and a dispatch is only ever the dynamic kind.

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
**reclaimed** (via `lichen clean`), so the key space stays bounded, and re-inserting a key
after reclamation under a different `hash` is recognized as a new artifact.

The registry stores **one artifact per device key**, and the key is what a ref names.
Freezing the same source twice under two keys therefore files two independent artifacts,
and a ref baked under one key resolves through that key alone; repeated `get`s on one key
hand back the *same* resident module (`Arc::ptr_eq`), not a recompile. There is no
content-addressed lookup that would collapse the two — identity in a baked ref is the
key, the key is per artifact, and equality of source content is not part of the
resolution rule at all.

`closure()` expands an equality class **once**, not once per member the walk visits.
`class_members` is a walk of its own and a *shared* type node puts every binding of the
program in one class, so re-expanding it per visited member was the whole cost of the
walk — measured at 363,627 visits and 7 ms for a 611-node closure. Only a node whose
**own value is undecided** takes its class, which keeps a decided leaf from paying for a
class it does not need: such a node's answer *is* its class, so the class is taken whole,
while a node already carrying its own solved value takes nothing from its class. Deciding
it per class instead costs the class's size at every freeze that touches it — ~50 µs for
a 601-member shared type class against ~7 µs for the same freeze in a small program.
Ordering the frozen set is free once the class bookkeeping is settled: `NodeId`'s `Ord`
is its key data's, whose first field is the slot index, so `sort_unstable` is exactly the
order `SlotMap::iter` yields and a closure's local indices are a subsequence of the
whole-module freeze's. The scan it replaced cost one lookup per *module* node per freeze
— measured, ~40 µs to order a 7-node closure in a 3,600-node module.

`freeze_closure` checks the **closure's** dependency keys, not the module's. The
closure's set is what the artifact will reference, so it is the predicate the filing
needs; the module's set is stricter than one cell needs and costs a scan of every node of
the module on every freeze — measured at ~9 µs of a 15 µs freeze at 600 marks. The
`check` closure is handed that evidence *before* anything is frozen, for a reason beyond
speed: a freeze takes the artifact's **release obligations** off the values it freezes,
so a refusal arriving afterwards would have to drop an artifact whose obligations it had
already taken. A refusal therefore transfers nothing. The same "the closure's refs, not
the module's" rule governs the recorded `refs` a `Package` carries: a superset would
refuse to evict an artifact nothing actually references — a leak in the name of safety.

A class the artifact does not hold whole is spliced to the members it does hold, exactly
as the GC splices a class that lost members: the first frozen member in slot order is the
representative, every other member's parent points straight at it, and the member list is
re-linked in that order — `disjoint::rebuild`'s splice, in the artifact's own key space.
Two frozen members of one class therefore stay in one class, which is all the frozen
links are read for (`static_find`, and the apply's clone grouping), and a member the
artifact does not hold has no clone to be grouped with.

Eviction has two preconditions and `Registry::evict` can only check one. It checks that
no **live registered artifact** references the key (the `refs` recorded at freeze time):
a static ref is a raw handle into the artifact's arena, so such an eviction is refused
(`Eviction::StillReferenced`) and the caller may retry once the referencing artifact is
gone too. Two artifacts that reference each other can therefore never be evicted while
both are filed — the honest answer for a cycle. It cannot check the other: a static ref
may also live outside the registry, in a `Module` the caller still holds, and
`Module::static_module` panics on an unregistered key. Eviction is the caller's decision,
made when the caller knows every module that could still hold such a ref is gone.
Related, for the recompile path: dropping the old artifact runs its release obligations,
and those read no other artifact's arenas (`Release` owns host resources, not other
artifacts), so the caller orders the replacement only for the reads it plans.

## Persistent device store

Under `~/.lichen` (`$LICHEN_HOME` overrides; `crate::persist::lichendir()`), the
**device registry** (`DeviceRegistry`, defined in `crates/lichen-registry`,
re-exported as `persist::DeviceRegistry`) owns the keys and the **file-ID
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
- **CLI:** `lichen clean` (the package manager) is a *clean*: it removes every
  artifact whose file ID is **not** a `.lichen` path and **not** a `virtual:`
  path (a bare `[depend]`
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

The materialize walk creates **fresh dynamic clones** in the caller's module, and each
clone records the *origin* the layer above attributes a failure in it by (`Node::origin`,
`Module::node_origin`). A clone of a frozen template has no template node of *this*
module to name — the frozen module's indices are that module's — so the static
materializer records the **apply operation node** that materialized it. That is the node
the importer's checker recorded an argument edge for (`Build::apply_edges`), so a runtime
failure naming a clone (a deferred named instantiation's table miss, say) carries a caret
on the argument the caller passed. The imported file's own position stays out of reach:
an ordinary imported package keeps no source record (`HighPackageMeta::source` is a
built-in's, and giving ordinary packages one would attribute their per-call assert
failures, which `registry.rs` pins as unattributed). The dynamic walk records the
*template* node instead — the node its per-node attribution table is keyed by.

A template cloned out of a static module keeps its **static** identity in
`AssertError::template`. The lowlevel carries provenance — the node a host keyed its own
metadata by, a source position, a user-facing flag — and no knowledge of what that
metadata means, so a host table keyed by the *importing* module's nodes has no entry for
a static template. That is the same answer such a table gave back when the lowlevel
tracked user-facing conditions itself, so nothing regresses for a host that never had an
entry.

`materialize_static_signature` copies the parameter **pair**, not just its type slot,
because the unify arm treats a signature like any other `[value, type, attrs…]` pair —
which is what gives a frozen function's signature the same attribute reach a dynamic one
has, instead of a weaker rule that only ever sees types. This is not a corner case: the
whole prelude is a frozen module, and its functions' type nodes carry a **static
self-cycle**, so a dynamic-only reading of a signature would leave the unifier blind to
every one of them. A static signature is immutable, so nothing is cloned as a *template*:
the leaves are copies, and the frozen original never binds.

The walk follows five rules:

- **The residual spine is re-run per call.** At solve time an operation whose operand is
  the marker parameter freezes the empty cell together with a dead residual operation; the
  walk clones that spine into the importer and evaluates it against the argument, rather
  than trying to re-solve the static side.
- **Constants are baked, not copied.** A solved constant operand becomes an inline
  absolute static ref, so the importer's value shares the frozen module's arena payload
  and no importer node is created for it.
- **Every ref is keyed and absolute, so a cached static value is unambiguous.** A node
  that caches a static payload (an `Index` reading an element of a frozen array) may be
  re-read from any context, and compaction must keep it verbatim — the rule the GC
  section below states.
- **The parameter's topology is re-established among the clones.** Unifications inside
  the frozen parameter pattern are replayed on the clone, so an argument violating that
  topology fails the parameter check; without the replay the mismatch would pass silently.
- **A foreign item stays in place.** An apply re-points the parameter item at its clone
  but must leave a foreign (another module's) static item alone: a foreign local index may
  never be looked up in the host module's node table, and an unguarded lookup is an
  out-of-range panic rather than an error.

## GC & asserts

GC keeps static handles verbatim: the static arena has no block to vacate, its item refs
are all static, and re-homing the payload would break identity with every other reader.
`check_asserts` reads static subtrees as decided leaves.
