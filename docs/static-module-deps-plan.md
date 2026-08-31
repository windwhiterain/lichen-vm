# Static module dependencies — canonical plan (2026-08-30, re-written; key-carrying refs same day)

**Goal.** Plug a fully-solved `StaticModule` into an importer `Module` as a dependency whose
values the importer *uses in place* (no copy). Mechanism only, lowlevel, proven by tests. No
`import` syntax, no highlevel seam. Transitive static deps (a static module depending on another
static module) are OUT OF SCOPE: `StaticModule::from_module` asserts the source is dependency-free
— but the ref encoding below is already transitive-ready (a ref names its module globally, so a
future artifact may name another module's key).

**Supersedes, in order.**
1. The two-level owner-context ref design (`{source_dependency, {dependency, index}}`) was
   collapsed to one level after a brainstorm on 2026-08-30.
2. The same day's *read-time re-base* design (relative refs `dependency: 0` inside static
   payloads, rewritten to the importer's plug position on read, copied into a per-importer
   arena and cached) was REPLACED by **key-carrying refs** (below) before it shipped a single
   production use: re-base is one copy per read node per importer — O(N²) across a dependency
   tree — and its only job was making refs absolute per-position. Making refs absolute *from
   birth* (a global module key) deletes the entire copy/cache machinery and shares one arena
   across every importer.

## References (key-carrying, absolute from birth)

- `ModuleKey` — the **device key** of a static module: a slotmap key
  (`new_key_type!`) allocated by the device's [`Registry`] when the module is
  registered, versioned so a removed slot is never confused with its
  successor.  Every static ref — node, function, or handle — carries its
  home module's key.
- `StaticNodeId { module: ModuleKey, index: LocalNodeId }`, `AnyNodeId = Dynamic(NodeId) |
  Static(StaticNodeId)`; `AnyFunctionId = Dynamic(FunctionId) | Static(StaticFunctionRef)`,
  `StaticFunctionRef { module: ModuleKey, index: StaticFunctionId }`;
  `StaticHandle { module: ModuleKey, offset: *const T }` (the offset is a resolved pointer into
  the module's arena — dereferenceable with no context, as `Handle` is).
- **No context rule.** Refs are identical in the module's own payloads and in any importer's
  storage; there is no relative form and nothing to retarget. Resolution is
  `Module::static_module(key)` — a `get` on the shared [`Registry`].  Bare item refs escaping a
  payload (unify's elementwise pairing, `alias_index`, the deep pass, pattern evaluation)
  resolve correctly unchanged — the ref itself names its module.
- **Identity** (`AnyHandle`/`StaticHandle` `PartialEq` = variant + key + offset): two importers
  of the same module share identity — the payload is the same bytes of the same shared arena,
  so `value_eq`'s pointer fast-path hits across importers.
- **Distribution layering (user decision, 2026-08-30):** the key is an *in-process* identity;
  distribution does not use it. A future `DistributeModule` format plus a per-device local
  registry will map the distributed artifact identity onto device keys — assigned at load,
  with the artifact's refs re-keyed once at load (linker-style relocation). The registry is
  the translation point; a cross-process file lock for the backing store is also future work.

## Registry (the device's virtual module file system)

The registry is the process-wide singleton, shared by every module executing (in threads) as
`Arc<RwLock<Registry>>`.  It handles **registering**, **storage**, and resolution **during
evaluating**.  `Module` is a per-thread owned value — `Arc<Module>` does not exist; the shared
substrate is the registry.

```rust
pub struct Registry<P: Program> {
    entries: SlotMap<ModuleKey, Dependency<P>>,   // resident store
}
impl<P: Program> Registry<P> {
    pub fn new() -> Self;
    /// An executing module bound to this registry.
    pub fn new_module(registry: &Arc<RwLock<Registry<P>>>) -> Module<P>;
    /// Compile a dynamic module into a static artifact and file it under a
    /// freshly allocated device key (`SlotMap::try_insert_with_key` hands
    /// the key to the build, so refs are baked with their final key).
    pub fn freeze(&mut self, module: &Module<P>) -> ModuleKey;
    /// The module behind a device key — the file-system `get` (`None` =
    /// "no such key"; a ref naming an unregistered key is a broken graph
    /// and panics at its resolution site).
    pub fn get(&self, key: ModuleKey) -> Option<&Dependency<P>>;
}
```

- **Module::new() keeps a private registry** (zero ripple for dependency-free modules — a
  standalone module's registry is its device registry at one-module scale); modules that share
  the device registry are created through `Registry::new_module`.  `Module::freeze(&source)` is
  the Module-facing convenience.
- **Locks:** resolution takes a read guard and clones the `Arc<StaticModule>` out before any
  work (no guard held across evaluation); guards recover from poisoning (`PoisonError::
  into_inner`) because tests `catch_unwind` panics mid-evaluation.  Nothing takes a second
  registry lock while holding one, and `freeze` never evaluates — no deadlock path.
- **Dependency** is the per-key entry (`{ module: Arc<StaticModule> }`) — the home of future
  per-key access state (user directive).  Shared across importers, since the registry is.

## Dependency (per-key state)

Values are used in place: the module's arena is shared and resolved through its key, so the
entry carries only the module today (see the Registry section above).

**Lifetime contract.** A static handle is valid while *some* importer (or the host) holds the
module's `Arc` — refs stored in an importer's nodes die with that importer, which also pins the
arena. The static arena is the most stable storage in the system: never moved, never compacted
(GC keeps static-handle values verbatim — see GC below), shared by every importer.

## StaticModule

```rust
pub struct StaticModule<P: Program> {
    pub key: ModuleKey,
    pub nodes: Vec<StaticNode<P>>,
    pub functions: Vec<StaticFunction>,
    pub arena: Vec<u8>,                     // flat two-phase arena, hand-laid-out
}
pub struct StaticNode<P: Program> {
    pub value: Option<P::Value>,            // final cached answer (Parameterized if residual)
    pub operation: Option<StaticOperation<P>>,
    pub equality: disjoint::Meta<LocalNodeId>,
    pub parameterized: bool,                // solved flag: copied from the source's evaluated_deep
}
pub struct StaticFunction { pub parameter: LocalNodeId, pub r#return: LocalNodeId, pub asserts: Vec<LocalNodeId> }
```

- **`parameterized`** is *not* derivable from the root value: a static array whose cached value is
  the array while an element is unresolved is parameterized (the deep pass's rule). `from_module`
  copies `evaluated_deep.parameterized`; nodes never deep-passed default to `true` (conservative).
- `StaticNode.equality` preserves the solved class structure so the materialize walk can re-
  establish the template's internal class topology among clones.
- `StaticModule::read(node)` is the solved value; a residual operation + no value reads
  `Parameterized`, never re-run. Static functions have no scope list: reachability from
  (return, parameter, asserts) via operand edges and array items IS the scope.
- Module-level pending asserts of the source are DROPPED (solved = every decidable assert
  decided; undecidable ones were parameter-dependent and die with the module).

## from_module (source → static)

`StaticModule::from_module(module: &Module<P>, key: ModuleKey)` — the key is allocated by the
registry (see `Registry::freeze`); asserts the source's registry `is_empty()`:

1. **Indices**: TWO passes — assign consecutive `LocalNodeId`s over the source's slotmap order
   (`HashMap<NodeId, LocalNodeId>`) for ALL nodes first, then build the static nodes: a class's
   member list points forward in slotmap order, so the map must be complete before any meta is
   remapped. Same for consecutive `StaticFunctionId`s over functions. Remap: operations'
   operands, functions' parameter/return/asserts, the equality meta (parent/next/tail,
   field-by-field, `NodeId → LocalNodeId`), the `parameterized` flag.
2. **Payloads**: walk every node value; collect regions — array item slices and ext-value payload
   bytes (`ValueExt::handle()`) — deduped by `(ptr, len)` so aliased handles keep identity
   equality in the static arena. Two-phase: size + align once (`ArrayItem` align for slices,
   `P::Value::alignment()` for ext payloads), allocate `Vec<u8>` with a `max_align` slack and an
   aligned base pointer, write, then build `StaticHandle { module: key, offset }`.
3. **Rewrite**: every value becomes static form keyed by `key` —
   `Array(AnyHandle::Static(..))` payloads whose *copied* items are rewritten
   `AnyNodeId::Dynamic(n) → Static(StaticNodeId{key, map[n]})` (shallow flags preserved), ext
   values `set_handle(AnyHandle::Static(..))`,
   `Function(FunctionId) → Function(AnyFunctionId::Static(StaticFunctionRef{key, map[f]}))`.

## Reads — verbatim, no conversion

`Module::static_read(&self, sref: StaticNodeId) -> P::Value` = `static_module(sref.module).read(sref.index)`
— a registry `get` plus a table read. No copy, no cache, no `&mut`. The value's payloads stay
in the module's shared arena; refs are absolute, so the value stores anywhere — the Index arm
caches it into the importer node like any other result, and `node_value`'s static arm returns
the raw value safely.

`evaluate_node` takes `AnyNodeId` with a static early-return arm; the deep pass treats static
refs as decided leaves (read the `parameterized` flag, never descend, never re-run — a forced
pass gains nothing from a solved subtree).

## Materialize (static function apply)

`Module::static_function_apply(sref, argument: NodeId, block, node, cell)` — mirrors
`function_apply`'s tail verbatim, with the walk re-opened per call:

- Guards: `apply_depth` / `apply_total` (static recursion runs through here).
- Walk from (return, parameter, asserts) via `StaticApplyCtx { target, module: Arc clone,
  remap: HashMap<LocalNodeId, NodeId> }`. Reserve the clone id and insert into the remap BEFORE
  recursing (diamonds and value cycles resolve to one clone). Per static node:
  - **baked** (`!parameterized`): the clone is a leaf — value = `ctx.module.read` (shared
    payload), then `static_remap_value`; the residual operation (if any) is dead — the value is
    final — and is dropped.
  - **residual** (`parameterized`): an operation node keeps the operation with its operand
    walked and drops the stale (`Parameterized`) cached value — the residual spine re-runs
    against the argument. A parameterized *value* node (no operation — a structural array
    containing the parameter, or the marker itself) keeps its value with items re-pointed at the
    walk's clones, mirroring the dynamic clone rule.
  - **Value edges re-open too** (`static_remap_value`): an item of a value is cloned when the
    walk already made one OR when its static node is parameterized — a residual behind a *value*
    edge (a branch or condition frozen as `Parameterized` at solve time) must re-open against
    the argument, or it reads as unbound forever. Concrete items stay inline absolute static
    refs; the item slice is reallocated only when something changed.
- **Operator contract**: baked static operands legitimately reach program operators (a constant
  operand of a materialized function). An operator must resolve operand refs through
  `Module::node_value` — never `module.nodes[AnyNodeId]`.
- Parameter/asserts/topology/tail, mirrored from `function_apply`: parameter always walked;
  asserts walked through the shared remap, registered in `self.asserts` only when the condition
  is NOT baked (baked = decided at normalize); re-establish template class topology among clones
  by grouping on the static reps (`static_find` over the copied meta) and unifying within groups;
  `evaluate_pattern_argument` + `unify(cloned_param, argument)` + `ApplyError` (its `function`
  field is `AnyFunctionId`) + cell wiring (`self.unify(cell, items[1].node)` after resolving
  the return type).
- The Apply arm pre-materializes a static argument / cell ref into a leaf node
  (`materialize_leaf`, homed in the apply node's block) so `function_apply`'s internals stay
  `NodeId`-typed.

## Unify's AnyNodeId arm

The elementwise recursion can pair a dynamic clone item with an absolute static ref. The arm
resolves the ref into a fresh leaf node (the shared value, homed in the dynamic side's block; the
both-static fallback homes in the module's first block — unreachable from the apply path) and
unifies that. Per-unify materialization is unchanged from the re-base design: the *value* is
shared, the *class* is per-materialize (a static node has no class of its own to join).

## GC and asserts

- GC: a node whose array value carries a **static handle keeps the value verbatim** — the static
  arena has no block to vacate, its item refs are all static (nothing to trace), and re-homing
  the payload would break identity with every other reader. Dynamic payloads re-home as usual
  (static item refs are absolute and copy verbatim); static function values are kept in place
  (no scope walk).
- `check_asserts` unchanged: conditions are dynamic nodes; force-evaluation of a condition whose
  subtree reaches static refs reads them as decided leaves (a static residual stays
  `Parameterized` → assert stays pending).
- `alias_read` / `alias_index` skip static elements (no class to join; the value is immutable).
- `EvalError.index` is `AnyNodeId`.

## Ripple (mechanical)

- `LowValue::Function(FunctionId)` → `Function(AnyFunctionId)`; `ApplyError.function` likewise.
- `ArrayItem.node` is already `AnyNodeId`; every `items()[i].node` consumer splits on the kind.
- `value_eq` compares payload bytes across all handle-kind combinations (content equality; the
  `AnyHandle` `PartialEq` stays identity: variant + key + offset).
- Highlevel: checker wraps `AnyNodeId::Dynamic`/`AnyFunctionId::Dynamic`; diagnostic reads
  `EvalError.index`/`ApplyError.function` through the dynamic arm (a static ref has no importer
  span — fall back). `dyn_node`-based walkers (render `elements`, `IndexTypeDispatch`,
  `kind_is_struct`) panic on static refs — the first highlevel seam must give them static arms
  (resolution is available: `Module::static_module(key)` / `static_read`).

## Test plan (crisis demos)

1. Nested index over a static array (`a[i][j]`) — the inner Index node caches the module's own
   element payload; re-reads resolve.
2. Registry device-key resolution — one freeze, one key; repeated `get`s return the same
   resident module; a re-compilation of the same source is a new artifact under a new key.
3. Static recursive function applied from dynamic — the self-ref cut; the depth guard still
   trips on non-termination.
4. Parameter topology re-establishment — a typed pattern argument unify against a static
   function enforces the template's internal constraints.
5. Per-call asserts — a static function's assert re-checks against each call's argument.
6. GC after materialize — materialized clones in a child block are dropped with it; no dangling.
7. A static closure stored in a static array, applied later from dynamic context.
8. A static function returning a static constant — result is a baked ref whose payload provably
   stays inside the module's arena (no copy into the importer).
9. A cached static-array value on an importer node survives block release verbatim and still
   resolves (the GC verbatim rule).

## Implementation order

1. Lowlevel compiles (mechanical: `AnyHandle` eq/len/as_ptr/items, `value_eq`, value_apply /
   node_apply kind split, utils `AnyHandle::Dynamic` wrapping, test helpers).
2. Keyed refs + `ModuleKey` (slotmap device key) + `Registry` (freeze/get, `Arc<RwLock>`
   singleton) + `from_module(module, key)` + slim `Dependency`.
3. Read path (verbatim `static_read`, `evaluate_node` AnyNodeId arm, Index arm, deep-pass leaf
   rule + static `parameterized` reads).
4. Materialize walk + `static_function_apply` + Apply dispatch + unify arm + `AnyFunctionId`.
5. GC/asserts integration (static-handle verbatim rule).
6. Tests.
7. Highlevel ripple — workspace green.
