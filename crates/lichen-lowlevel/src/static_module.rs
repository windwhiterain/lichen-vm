//! Static module dependencies: a fully-solved [`StaticModule`] registered
//! in the device's [`Registry`] and used in place by an importer
//! [`Module`].
//!
//! The importer-apply and the freeze each live in a sibling module (`apply`,
//! `freeze`); this file holds the registry-facing reads and the helpers both
//! share.
//!
//! The design (see the feature note `docs/notes/static-modules.md`):
//! - every ref into a static module — node, function, or handle — carries
//!   the module's device key ([`ModuleKey`]), so refs are absolute from
//!   birth.  An importer stores and resolves them verbatim
//!   ([`Module::static_read`] fetches the module from the shared registry);
//!   nothing is retargeted or copied, and the module's arena is shared by
//!   every importer.
//! - applying a static function materializes its reachable graph into fresh
//!   dynamic clones: baked (concrete) nodes become leaves holding the shared
//!   value, residual nodes keep their operations with remapped
//!   operands so the parameter-dependent spine re-runs against the argument;
//!   static function values are always baked (frozen templates).  The apply
//!   tail (parameter unify, `ApplyError`, cell wiring) is shared with
//!   [`Module::function_apply`] in `apply.rs`.

use std::collections::{HashMap, HashSet};
use std::ptr;
use std::sync::{Arc, PoisonError};

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, BlockId, Function,
    FunctionId, LocalNodeId, LowShape, LowValue, Module, ModuleKey, NodeId, Operation,
    PendingAssert, Program, StaticFunction, StaticFunctionId, StaticFunctionRef, StaticHandle,
    StaticModule, StaticNode, StaticNodeId, StaticOperation, TableItem, ValueExt as _,
};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

mod apply;
mod freeze;

/// A one-entry resolution cache for a value walk: a walk over a value's items
/// resolves many static refs, and the common case names one module, so the
/// registry read lock and the `Arc` clone are paid once per *distinct* module
/// instead of once per ref.  Each lookup takes the lock exactly as
/// [`Module::static_module`] does and releases it before the next — the lock
/// is never held across the walk, so a writer is never blocked by one.
pub(crate) struct StaticModuleCache<P: Program> {
    last: Option<(ModuleKey, Arc<StaticModule<P>>)>,
}

impl<P: Program> StaticModuleCache<P> {
    pub(crate) fn new() -> Self {
        Self { last: None }
    }

    /// The registered module behind `key`, resolved at most once per cache.
    fn module<'a>(&'a mut self, host: &'a Module<P>, key: ModuleKey) -> &'a StaticModule<P> {
        if self.last.as_ref().is_none_or(|(cached, _)| *cached != key) {
            self.last = Some((key, host.static_module(key)));
        }
        &self
            .last
            .as_ref()
            .expect("the lookup above filled the cache")
            .1
    }

    /// Whether the node behind `sref` is marked parameterized: the module's
    /// solved flag, read without a registry lookup per ref.
    pub(crate) fn node_parameterized(&mut self, host: &Module<P>, sref: StaticNodeId) -> bool {
        self.module(host, sref.module).nodes[sref.index.index].parameterized
    }

    /// [`StaticModule::read`] through the cache.
    pub(crate) fn read(&mut self, host: &Module<P>, sref: StaticNodeId) -> P::Value {
        self.module(host, sref.module).read(sref.index)
    }
}

impl<P: Program> Module<P> {
    /// The static module behind `key` — the `get` of the device's registry
    /// (its virtual file system).  The `Arc` is cloned out of the lock
    /// guard, so no borrow of `self` or of the guard persists; a static ref
    /// naming an unregistered key is a broken module graph.
    pub(crate) fn static_module(&self, key: ModuleKey) -> Arc<StaticModule<P>> {
        self.registry
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .expect("static ref into a module that is not registered")
            .module
            .clone()
    }

    /// Read a static node through its ref: the module's solved value,
    /// verbatim.  Refs are absolute (keyed), so the value stores anywhere
    /// with no conversion, and its payloads stay in the module's shared
    /// arena — nothing is copied.
    pub fn static_read(&self, sref: StaticNodeId) -> P::Value {
        self.static_module(sref.module).read(sref.index)
    }

    /// The raw value behind `id` — no evaluation.  A static ref reads its
    /// solved value (which may be `Parameterized`); refs are absolute, so
    /// the raw value is safe to store anywhere.  A dynamic ref that names a
    /// released node reads `None` (via `SlotMap::get`), so the read API is
    /// safe for a node the executor may have dropped.
    pub fn node_value(&self, id: AnyNodeId) -> Option<P::Value> {
        match id {
            Dyn(node) => self.nodes.get(node).and_then(|node| node.value),
            AnyNodeId::Static(sref) => {
                self.static_module(sref.module).nodes[sref.index.index].value
            }
        }
    }

    /// The optional [`LowShape`] a layer above the lowlevel computed for
    /// `node` — stored *with* the node's private value, not in a side table.
    /// `None` means the node has no traced shape (it is type-check-only, or
    /// it is materialized before the backend runs), so the backend must not
    /// rely on it.
    pub fn node_shape(&self, node: NodeId) -> Option<&LowShape> {
        self.nodes
            .get(node)
            .and_then(|node| node.low_shape.as_ref())
    }

    /// Record `node`'s [`LowShape`].  Only the layer above the lowlevel that
    /// *has* the type calls this, after the graph is resolved — never the
    /// checker at lowering time (see [`LowShape`]).
    pub fn set_node_shape(&mut self, node: NodeId, shape: Option<LowShape>) {
        if let Some(node) = self.nodes.get_mut(node) {
            node.low_shape = shape;
        }
    }

    /// The dynamic node behind `id`: a static ref materializes into a fresh
    /// leaf node holding its value (homed in `block`), so
    /// `NodeId`-typed machinery (the apply tail, the unify arm) can unify
    /// and bind it like any other node.
    pub fn materialize_leaf(&mut self, sref: StaticNodeId, block: BlockId) -> NodeId {
        let value = self.static_read(sref);
        self.add_node(block, None, Some(value))
    }

    /// [`AnyNodeId::Dynamic`] as-is, or a static ref materialized into a leaf.
    pub fn as_dynamic(&mut self, id: AnyNodeId, block: BlockId) -> NodeId {
        match id {
            Dyn(node) => node,
            AnyNodeId::Static(sref) => self.materialize_leaf(sref, block),
        }
    }
}

/// The solved union-find representative of `key` in the static meta — the
/// static side of `disjoint::find`, walked without path compression (the
/// solved structure is immutable).
fn static_find<P: Program>(nodes: &[StaticNode<P>], key: LocalNodeId) -> LocalNodeId {
    let mut current = key;
    while let Some(parent) = nodes[current.index].equality.parent() {
        current = parent;
    }
    current
}

/// The static identity of a node in `module` — the form a host's own
/// per-node tables can key on when an assert is cloned out of a static
/// module (the importing module has no dynamic node for the template).
fn static_ref<P: Program>(module: &StaticModule<P>, node: LocalNodeId) -> AnyNodeId {
    AnyNodeId::Static(StaticNodeId {
        module: module.key,
        index: node,
    })
}

/// Every module key a static ref in the module's solved values names — the
/// module's frozen dependencies.  A freeze may file the artifact only into a
/// registry that holds all of them ([`Registry::freeze_mapped`] checks), so
/// every ref the rewritten values keep verbatim resolves from any importer.
/// Dynamic array items are not recursed into: each item's node is itself a
/// node of the module, visited in its own right.
pub(crate) fn referenced_keys<P: Program>(module: &Module<P>) -> HashSet<ModuleKey> {
    let mut keys = HashSet::new();
    for node in module.nodes.values() {
        if let Some(value) = node.value {
            collect_referenced_keys::<P>(value, &mut keys);
        }
    }
    keys
}

/// [`referenced_keys`] over `node_ids` instead of the whole module — the closure
/// a per-cell freeze is about to file ([`StaticModule::freeze_closure`]).
///
/// The answer is the dependency half of what the artifact's own values will name:
/// the rewrite keeps every dependency ref verbatim, and a ref into the artifact
/// itself is not a dependency.  So this is exactly the predicate
/// [`Registry::freeze_closure_mapped`] checks — and it is O(closure), where the
/// module-wide set is O(module) and stricter than a single cell needs.
pub(crate) fn referenced_keys_of<P: Program>(
    module: &Module<P>,
    node_ids: &[NodeId],
) -> HashSet<ModuleKey> {
    let mut keys = HashSet::new();
    for &node in node_ids {
        if let Some(value) = module.nodes[node].value {
            collect_referenced_keys::<P>(value, &mut keys);
        }
    }
    keys
}

/// One value's contribution to [`referenced_keys`]: every module key a static ref
/// in `value` names.  The single site of this match, so the module-wide scan and
/// the closure-scoped one cannot disagree.
fn collect_referenced_keys<P: Program>(value: P::Value, keys: &mut HashSet<ModuleKey>) {
    match value.as_enum() {
        Some(LowValue::Array(AnyHandle::Static(handle))) => {
            keys.insert(handle.module);
            // SAFETY: the payload lives in the dependency's shared arena,
            // pinned by the registry that holds the dependency — the registry
            // this freeze is about to file into, which is what the check
            // before it establishes.
            for item in unsafe { &*handle.offset } {
                if let AnyNodeId::Static(sref) = item.node {
                    keys.insert(sref.module);
                }
            }
        }
        Some(LowValue::Array(AnyHandle::Dynamic(handle))) => {
            // SAFETY: the handle points into one of `module`'s own block
            // arenas (`Module::alloc_array`), alive as long as the module
            // is — the `drop_block` contract.
            for item in unsafe { &*handle.0 } {
                if let AnyNodeId::Static(sref) = item.node {
                    keys.insert(sref.module);
                }
            }
        }
        Some(LowValue::Table(AnyHandle::Static(handle))) => {
            keys.insert(handle.module);
            // SAFETY: as in the static array arm above — the payload lives
            // in the dependency's registered shared arena.
            for item in unsafe { &*handle.offset } {
                if let AnyNodeId::Static(sref) = item.key {
                    keys.insert(sref.module);
                }
                if let AnyNodeId::Static(sref) = item.value {
                    keys.insert(sref.module);
                }
            }
        }
        Some(LowValue::Table(AnyHandle::Dynamic(handle))) => {
            // SAFETY: as in the dynamic array arm above — the payload was
            // allocated in one of `module`'s own block arenas by
            // `Module::alloc_table`.
            for item in unsafe { &*handle.0 } {
                if let AnyNodeId::Static(sref) = item.key {
                    keys.insert(sref.module);
                }
                if let AnyNodeId::Static(sref) = item.value {
                    keys.insert(sref.module);
                }
            }
        }
        Some(LowValue::Function(AnyFunctionId::Static(function))) => {
            keys.insert(function.module);
        }
        _ if value.is_handle() => {
            if let AnyHandle::Static(handle) = value.handle() {
                keys.insert(handle.module);
            }
        }
        _ => {}
    }
}

impl<P: Program> StaticModule<P> {
    /// Every module key a static ref in the artifact's values names — its frozen
    /// dependencies, in the form an artifact that is *already* static can answer.
    /// The mirror of `referenced_keys(module)` above, and what a registry needs to
    /// decide whether a key may be evicted: an artifact that another live artifact
    /// still references cannot be freed, because that reference is a raw handle
    /// into this artifact's arena.
    ///
    /// The artifact's **own** key appears when a value references a node of this
    /// module (a self-referential value, which phase 3 rewrites to a ref into
    /// `key`).  The caller decides what to do with that — the registry's eviction
    /// check ignores a self-reference, or the artifact could never be evicted.
    pub(crate) fn referenced_keys(&self) -> HashSet<ModuleKey> {
        let mut keys = HashSet::new();
        for node in &self.nodes {
            let Some(value) = node.value else { continue };
            match value.as_enum() {
                Some(LowValue::Array(AnyHandle::Static(handle))) => {
                    keys.insert(handle.module);
                    // SAFETY: the payload lives in the dependency's shared arena,
                    // which the registry keeps alive for as long as this artifact
                    // is filed there — a dependency is registered before anything
                    // that references it.
                    for item in unsafe { &*handle.offset } {
                        if let AnyNodeId::Static(sref) = item.node {
                            keys.insert(sref.module);
                        }
                    }
                }
                Some(LowValue::Table(AnyHandle::Static(handle))) => {
                    keys.insert(handle.module);
                    // SAFETY: as in the array arm above.
                    for item in unsafe { &*handle.offset } {
                        if let AnyNodeId::Static(sref) = item.key {
                            keys.insert(sref.module);
                        }
                        if let AnyNodeId::Static(sref) = item.value {
                            keys.insert(sref.module);
                        }
                    }
                }
                Some(LowValue::Function(AnyFunctionId::Static(function))) => {
                    keys.insert(function.module);
                }
                _ if value.is_handle() => {
                    if let AnyHandle::Static(handle) = value.handle() {
                        keys.insert(handle.module);
                    }
                }
                _ => {}
            }
        }
        keys
    }
}
