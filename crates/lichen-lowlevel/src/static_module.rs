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
//!   static function values are always baked (frozen templates).  A residual
//!   clone carries the enclosing template's owner tag, so a caller applied
//!   again re-instantiates it; a baked clone stays unowned and is referenced
//!   in place.  The apply tail (parameter unify, `ApplyError`, cell wiring) is
//!   shared with [`Module::function_apply`] in `apply.rs`.

use std::collections::{HashMap, HashSet};
use std::ptr;
use std::sync::{Arc, PoisonError};

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, BlockId, Function,
    FunctionId, LocalNodeId, LowShape, LowValue, Module, ModuleKey, NodeId, Operation,
    PendingAssert, Program, StaticFunction, StaticFunctionId, StaticFunctionRef, StaticHandle,
    StaticModule, StaticNode, StaticNodeId, StaticOperation, TableItem, ValueExt as _, is_unbound,
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

    /// The representative of a **static** node's equality class — the frozen
    /// mirror of [`Module::equality_representative`], walking `parent` over the
    /// artifact's own local ids (no path compression, so a read never mutates
    /// the artifact).
    ///
    /// A reader that *names* cells must use it, exactly as it uses the dynamic
    /// representative: the freeze keeps the equality class of a node whose own
    /// value is still unbound **whole** (see `freeze::closure`'s contract — the
    /// class is what holds that node's answer), so two refs in one class are one
    /// variable.  Keying a name table by the ref alone prints them as two:
    /// measured, an imported polymorphic `?a -> ?a` rendered `?a -> ?b`, while
    /// the same type rendered dynamically — where the printer does follow the
    /// representative — read `?a -> ?a`.
    pub fn static_equality_representative(&self, sref: StaticNodeId) -> StaticNodeId {
        let module = self.static_module(sref.module);
        let mut index = sref.index;
        // A parent chain visits each node at most once, so it cannot be longer
        // than the artifact's node table.
        for _ in 0..=module.nodes.len() {
            match module.nodes[index.index].equality.parent() {
                Some(parent) => index = parent,
                None => break,
            }
        }
        StaticNodeId {
            module: sref.module,
            index,
        }
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

    /// Whether `node` has **produced an answer** — its own slot carries one,
    /// decided or undecided.
    ///
    /// This is the *has run* half of the value slot, and it is deliberately
    /// not [`is_unbound`]: an operation node whose answer is the undecided
    /// marker ([`LowValue::Parameterized`]) **has** run — the attempt
    /// happened and could not resolve — whereas a node with no cached value
    /// at all never ran.  A reader deciding whether to *run* the node's
    /// operation asks [`Self::has_no_result_yet`]; a reader deciding whether
    /// it may *compare* the node's value asks [`is_unbound`], because an
    /// undecided answer is not comparable.  Conflating the two is what made
    /// `is_unbound(node_value)` the effective authority for both.
    ///
    /// A released node (absent from [`Self::nodes`]) reads `false`: there is
    /// nothing left to run.
    pub fn has_run(&self, node: NodeId) -> bool {
        !is_unbound(self.node_value(Dyn(node)))
    }

    /// Whether `node`'s operation — the computation it will produce its
    /// answer by — **has not produced an answer yet**.
    ///
    /// `false` for a node with no operation (a marker, a bound constant, a
    /// released node): there is no computation left to run.  For an operation
    /// node this asks whether the slot holds an answer, decided **or**
    /// undecided, disagreeing with [`is_unbound`] exactly on the undecided
    /// marker: that marker is an answer that says "not decided", so it is
    /// *not* pending.  The evaluator declines to cache the marker (see
    /// [`Self::evaluate_node_operation`]'s postlude), which only matters on a
    /// path that writes the slot directly — the apply clone walk preserves a
    /// source's value on an operation-free node and drops an operation node's
    /// cached value entirely ([`crate::function`]), so an operation node's
    /// slot holds a decided answer or nothing, and a pure cell holding the
    /// marker is correctly not "run" (nothing ran).
    pub fn has_no_result_yet(&self, node: NodeId) -> bool {
        self.nodes[node].operation.is_some() && (!self.has_run(node) || !self.nodes[node].runned)
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

    /// The static function's **signature cells** — its parameter type and its
    /// return type — as refs a renderer can read without materializing.
    /// `None` when `sref` is not a static function of a registered module, or
    /// its parameter is not a pair.
    pub fn static_function_signature(
        &self,
        sref: StaticFunctionRef,
    ) -> Option<(AnyNodeId, AnyNodeId)> {
        let (param_pair, return_type) = {
            let module = self.static_module(sref.module);
            let function = module.functions.get(sref.index.0)?;
            (function.parameter, function.return_type)
        };
        // The parameter pair's slot 1 is the parameter type cell.
        let param_pair_ref = StaticNodeId {
            module: sref.module,
            index: param_pair,
        };
        let param_type = match self.static_read(param_pair_ref).as_enum() {
            // SAFETY: `array` is a static payload read through `param_pair_ref`,
            // whose home module is registered — the registration pins its
            // arena.
            Some(LowValue::Array(array)) => unsafe { array.items() }.get(1)?.node,
            _ => return None,
        };
        Some((
            param_type,
            AnyNodeId::Static(StaticNodeId {
                module: sref.module,
                index: return_type,
            }),
        ))
    }

    /// The materialized `(parameter, return type)` of a **static** function's
    /// signature: the frozen template's parameter pair and return type cell,
    /// copied into fresh dynamic leaves so the unify arm can descend into them
    /// positionally.  `None` when `sref` is not a static function of a
    /// registered module.
    ///
    /// The pair, not just its type slot, because the unify arm treats a
    /// signature like any other `[value, type, attrs…]` pair — that is what
    /// gives a frozen function's signature the same attribute reach a dynamic
    /// one has, instead of a weaker rule that only ever sees types.
    ///
    /// A static signature is immutable, so nothing is cloned as a *template*:
    /// the leaves are copies, and the frozen original never binds.  This is not
    /// a corner case — the whole prelude is a frozen module, and its functions'
    /// type nodes carry a static self-cycle, so a dynamic-only reading would
    /// leave the unifier blind to every one of them.
    pub fn materialize_static_signature(
        &mut self,
        sref: StaticFunctionRef,
    ) -> Option<(NodeId, NodeId)> {
        let (param_pair, return_type) = {
            let module = self.static_module(sref.module);
            let function = module.functions.get(sref.index.0)?;
            (function.parameter, function.return_type)
        };
        let block = self.blocks.iter().next().map(|(block, _)| block)?;
        let parameter = self.materialize_leaf(
            StaticNodeId {
                module: sref.module,
                index: param_pair,
            },
            block,
        );
        let codomain = self.materialize_leaf(
            StaticNodeId {
                module: sref.module,
                index: return_type,
            },
            block,
        );
        Some((parameter, codomain))
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

/// Whether static function `index` has **open captures**: its body graph
/// reaches a `parameterized` node outside its own template scope
/// ([`StaticFunction::nodes`], which covers the parameter, the return, and
/// every body-owned node).  A scope's own open cells re-open per call through
/// the residual clone rule, but a capture sits outside the scope: its binding
/// was made by whichever application minted this closure — the solve-time
/// one, with marker cells, for a closure frozen into an artifact — and only
/// re-homing the closure through the apply's shared remap clones the capture
/// alongside the applied parameter, so the regroup re-joins their frozen
/// class and the parameter unify binds this call's values.
///
/// Nested same-module closures are entered through their entry points and
/// their scopes join the allowed set: a capture one closure layer down is
/// still a capture of this one.  The walk answers through the nested
/// function's entry points rather than descending into a function *value*
/// node, which is a leaf of the graph it rides in.
///
/// The answer is the artifact's [`StaticFunction::open_captures`], filled
/// once when the artifact is built — this takes the node and function tables
/// directly so both the freeze and the loader can fill it before the
/// [`StaticModule`] is assembled.
fn static_closure_has_open_captures<P: Program>(
    key: ModuleKey,
    nodes: &[StaticNode<P>],
    functions: &[StaticFunction],
    index: StaticFunctionId,
) -> bool {
    fn enter(
        functions: &[StaticFunction],
        index: StaticFunctionId,
        scope: &mut HashSet<LocalNodeId>,
        stack: &mut Vec<LocalNodeId>,
    ) {
        let f = &functions[index.0];
        scope.extend(f.nodes.iter().copied());
        stack.push(f.r#return);
        stack.extend(f.asserts.iter().copied());
    }
    let mut scope = HashSet::new();
    let mut visited = HashSet::new();
    let mut stack = Vec::new();
    enter(functions, index, &mut scope, &mut stack);
    while let Some(node) = stack.pop() {
        if !visited.insert(node) {
            continue;
        }
        let sn = &nodes[node.index];
        if sn.parameterized && !scope.contains(&node) {
            return true;
        }
        if let Some(operation) = sn.operation
            && let Some(operand) = operation.operand
        {
            stack.push(operand);
        }
        if let Some(value) = sn.value {
            match value.as_enum() {
                Some(LowValue::Array(array)) => {
                    // SAFETY: `array` is a payload in the artifact's arena (the
                    // module being built, or a dependency the registry pins),
                    // and the caller holds every module alive for this walk.
                    for item in unsafe { array.items() } {
                        if let AnyNodeId::Static(sref) = item.node
                            && sref.module == key
                        {
                            stack.push(sref.index);
                        }
                    }
                }
                Some(LowValue::Table(table)) => {
                    // SAFETY: as in the array arm above.
                    for item in unsafe { table.items() } {
                        for node in [item.key, item.value] {
                            if let AnyNodeId::Static(sref) = node
                                && sref.module == key
                            {
                                stack.push(sref.index);
                            }
                        }
                    }
                }
                Some(LowValue::Function(AnyFunctionId::Static(sref))) if sref.module == key => {
                    enter(functions, sref.index, &mut scope, &mut stack);
                }
                _ => {}
            }
        }
    }
    false
}

impl<P: Program> StaticModule<P> {
    /// Fill every function's [`StaticFunction::open_captures`] from this
    /// artifact's own tables — once, at build time, so the materialize pass
    /// reads a field instead of walking a body per function-valued position.
    /// A host that assembles a [`StaticModule`] by hand (a decoder reading
    /// serialized bytes) calls it after assembly; the graph is the only
    /// source, so a stored verdict that disagreed with the graph cannot arise.
    pub fn fill_open_captures(&mut self) {
        let key = self.key;
        let computed: Vec<bool> = (0..self.functions.len())
            .map(|index| {
                static_closure_has_open_captures(
                    key,
                    &self.nodes,
                    &self.functions,
                    StaticFunctionId(index),
                )
            })
            .collect();
        for (function, open) in self.functions.iter_mut().zip(computed) {
            function.open_captures = open;
        }
    }
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
