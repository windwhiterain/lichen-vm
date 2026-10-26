//! Static module dependencies: a fully-solved [`StaticModule`] used in place. Design:
//! `docs/notes/static-modules.md`.

use std::collections::{HashMap, HashSet};
use std::ptr;
use std::sync::{Arc, PoisonError};

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, ArrayValue, BlockId,
    Function, FunctionId, LocalNodeId, LowValue, Module, ModuleKey, NodeId, Operation,
    PendingAssert, Program, StaticFunction, StaticFunctionId, StaticFunctionRef, StaticHandle,
    StaticModule, StaticNode, StaticNodeId, StaticOperation, TableItem, ValueExt as _,
};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

mod apply;
mod freeze;

pub use apply::Rehome;

/// A one-entry cache for a value walk: the lock and `Arc` clone are paid once per
/// *distinct* module, not once per ref.
///
/// # Invariant
///
/// Each lookup takes the lock as [`Module::static_module`] does and releases it before
/// the next: the lock is never held across the walk, so a writer is never blocked.
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

    /// Whether the node behind `sref` is marked undecided: the module's
    /// solved flag, read without a registry lookup per ref.
    pub(crate) fn node_undecided(&mut self, host: &Module<P>, sref: StaticNodeId) -> bool {
        self.module(host, sref.module).nodes[sref.index.index].undecided()
    }

    /// [`StaticModule::read`] through the cache.
    pub(crate) fn read(&mut self, host: &Module<P>, sref: StaticNodeId) -> Option<P::Value> {
        self.module(host, sref.module).read(sref.index)
    }
}

impl<P: Program> Module<P> {
    /// The static module behind `key`; its `Arc` is cloned out of the lock guard, so no
    /// borrow persists.
    pub(crate) fn static_module(&self, key: ModuleKey) -> Arc<StaticModule<P>> {
        self.registry
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .expect("static ref into a module that is not registered")
            .module
            .clone()
    }

    /// Whether the frozen body returns its parameter: its return expr *is* the
    /// parameter expr.
    pub fn static_function_returns_parameter(&self, function: StaticFunctionRef) -> bool {
        self.static_module(function.module)
            .functions
            .get(function.index.0)
            .is_some_and(|function| function.parameter == function.r#return)
    }

    /// Read a static node's solved value verbatim, or [`None`] for a residual
    /// with no cached answer.
    pub fn static_read(&self, sref: StaticNodeId) -> Option<P::Value> {
        self.static_module(sref.module).read(sref.index)
    }

    /// The compute operator a frozen function's body computes, or [`None`].
    ///
    /// # Invariant
    /// Answered from the artifact's structure, never from a value: the body is
    /// reachable by [`StaticFunctionRef`] whether or not anything evaluated.
    /// `None` for a body that computes no operator or more than one.
    ///
    /// **A read's position is a subscript, not a computation**: a field read
    /// computes `TableGet(names, name) + <base>`, not the body's operator
    /// (`docs/notes/loop-conversion.md`).
    pub fn static_function_compute_operator(
        &self,
        function: StaticFunctionRef,
    ) -> Option<(P::Operator, StaticNodeId)> {
        let key = function.module;
        let module = self.static_module(key);
        let function = module.functions.get(function.index.0)?;
        let mut found: Option<(P::Operator, LocalNodeId)> = None;
        // Each entry says whether the node was reached as a read's position.
        let mut pending = vec![(function.r#return, false)];
        // A node first met as a position and later as a computation is a
        // computation, so the walk visits it twice.
        let mut seen: HashMap<LocalNodeId, bool> = HashMap::new();
        // The nodes already counted, so a revisit does not count twice.
        let mut counted: HashSet<LocalNodeId> = HashSet::new();
        while let Some((local, position)) = pending.pop() {
            if let Some(was_position) = seen.get(&local)
                && (!*was_position || position)
            {
                continue;
            }
            seen.insert(local, position);
            let node = module.nodes.get(local.index)?;
            if let Some(operation) = &node.operation {
                let low = crate::AsEnum::<crate::LowOperator>::as_enum(&operation.operator);
                let read = matches!(
                    low,
                    Some(crate::LowOperator::Index | crate::LowOperator::TableGet)
                );
                if !position
                    && (low.is_none() || matches!(low, Some(crate::LowOperator::External { .. })))
                {
                    // **An operand-less call is no compute operator**: a nominal id reads none.
                    if let Some(operand) = operation.operand
                        && counted.insert(local)
                    {
                        // Two compute operators in one body have no single answer.
                        if found.is_some() {
                            return None;
                        }
                        found = Some((operation.operator, operand));
                    }
                }
                if let Some(operand) = operation.operand {
                    // The operand array's items: for a read, the second one is the
                    // position it reads at.
                    if let Some(crate::LowValue::Array(array)) = module
                        .read(operand)
                        .and_then(|value| crate::AsEnum::<crate::LowValue>::as_enum(&value))
                    {
                        // SAFETY: `array` is a payload of a value read out of a
                        // live frozen module; nothing here releases an arena.
                        for (index, item) in unsafe { array.items() }.iter().enumerate() {
                            if let crate::AnyNodeId::Static(inner) = item.node
                                && inner.module == key
                            {
                                pending.push((inner.index, position || (read && index == 1)));
                            }
                        }
                    }
                }
            } else if let Some(crate::LowValue::Array(array)) = node
                .value
                .and_then(|value| crate::AsEnum::<crate::LowValue>::as_enum(&value))
            {
                // SAFETY: `array` is a payload of a value read out of a live
                // frozen module; nothing here releases an arena.
                for item in unsafe { array.items() } {
                    if let crate::AnyNodeId::Static(inner) = item.node
                        && inner.module == key
                    {
                        pending.push((inner.index, position));
                    }
                }
            }
            // A constructor can place the operator in the returned cell's class
            // rather than in its value graph.
            for member in static_class_members(&module, local) {
                let wildcard = module
                    .nodes
                    .get(member.index)
                    .and_then(|member| member.value.as_ref())
                    .and_then(|value| crate::AsEnum::<crate::LowValue>::as_enum(value))
                    .is_some_and(|value| matches!(value, crate::LowValue::Any));
                if !wildcard {
                    pending.push((member, position));
                }
            }
        }
        let (operator, operand) = found?;
        Some((
            operator,
            StaticNodeId {
                module: key,
                index: operand,
            },
        ))
    }

    /// The static mirror of [`Module::equality_representative`]: a `parent`
    /// walk that never mutates the artifact.
    ///
    /// # Invariant
    ///
    /// A reader that *names* cells must use it: the freeze keeps the class of an
    /// undecided node whole, so two refs in one class are one variable. Keying a name
    /// table by the ref alone printed an imported polymorphic `?a -> ?a` as `?a -> ?b`.
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

    /// The raw value behind `id` — no evaluation. A released dynamic node reads `None`.
    ///
    /// # Invariant
    /// A static ref is read **in its own module**, directly, exactly as a dynamic node
    /// is: no reader copies it into a dynamic node first. A copy would give each frozen
    /// position a fresh class, so one value would read as several
    /// (`docs/notes/kernel-parameter-class.md`).
    pub fn node_value(&self, id: AnyNodeId) -> Option<P::Value> {
        match id {
            Dyn(node) => self.nodes.get(node).and_then(|node| node.value),
            AnyNodeId::Static(sref) => {
                self.static_module(sref.module).nodes[sref.index.index].value
            }
        }
    }

    /// Whether `id` is a static node whose artifact was frozen `ReadOnly`.
    ///
    /// # Invariant
    /// Answers only for a key already registered, as [`Self::static_module`] does.
    pub(crate) fn read_only_static(&self, id: AnyNodeId) -> bool {
        match id {
            Dyn(_) => false,
            AnyNodeId::Static(sref) => self.static_module(sref.module).read_only,
        }
    }

    /// Whether `node`'s operator **owes an answer** — the evaluator's run gate
    /// [`Self::evaluate_node`] cannot drift from.
    ///
    /// # Invariant
    ///
    /// Both axes of [`Node::value`] meet here: an operation is present, and either the
    /// slot holds no value — the operator never ran, or ran and could not decide — or
    /// the value is **not this operator's**, because a unification wrote it and the
    /// operator still owes its own reconciliation with it (`write_node_answer`). A run
    /// stamp of zero is the never-ran half of the second term.
    pub fn has_no_result_yet(&self, node: NodeId) -> bool {
        self.nodes
            .get(node)
            .is_some_and(|node| node.operation.is_some() && (node.value.is_none() || !node.runned))
    }

    /// The dynamic node behind `id`: a static ref becomes a fresh leaf in
    /// `block`, so `NodeId` machinery binds it.
    pub fn materialize_leaf(&mut self, sref: StaticNodeId, block: BlockId) -> NodeId {
        let value = self.static_read(sref);
        self.add_node(block, None, value)
    }

    /// [`AnyNodeId::Dynamic`] as-is, or a static ref materialized into a leaf.
    pub fn as_dynamic(&mut self, id: AnyNodeId, block: BlockId) -> NodeId {
        match id {
            Dyn(node) => node,
            AnyNodeId::Static(sref) => self.materialize_leaf(sref, block),
        }
    }

    /// The static function's **signature cells** — parameter type and return type — as
    /// un-materialized refs.
    pub fn static_function_signature(
        &self,
        sref: StaticFunctionRef,
    ) -> Option<(AnyNodeId, AnyNodeId)> {
        let (param_expr, return_type) = {
            let module = self.static_module(sref.module);
            let function = module.functions.get(sref.index.0)?;
            (function.parameter, function.return_type)
        };
        // The parameter expr's slot 1 is the parameter type cell.
        let param_expr_ref = StaticNodeId {
            module: sref.module,
            index: param_expr,
        };
        let param_type = match self
            .static_read(param_expr_ref)
            .and_then(|value| value.as_enum())
        {
            // SAFETY: `array` is a payload in a registered static module's arena, which
            // the registration pins.
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
}
/// The solved union-find representative of `key` — no path compression (the solved
/// structure is immutable).
pub(crate) fn static_find<P: Program>(nodes: &[StaticNode<P>], key: LocalNodeId) -> LocalNodeId {
    let mut current = key;
    while let Some(parent) = nodes[current.index].equality.parent() {
        current = parent;
    }
    current
}

/// Whether a frozen function is `_x => _`: an open result independent of its parameter.
pub(crate) fn static_wildcard_constructor<P: Program>(
    module: &StaticModule<P>,
    function: StaticFunctionRef,
) -> bool {
    if function.module != module.key {
        return false;
    }
    let function = &module.functions[function.index.0];
    let halves = |node| {
        let LowValue::Array(array) = module.read(node)?.as_enum()? else {
            return None;
        };
        // SAFETY: the payload belongs to the frozen module being read.
        let items = unsafe { array.items() };
        Some((items.first()?.node, items.get(1)?.node))
    };
    let open = |node| match node {
        AnyNodeId::Static(sref) if sref.module == module.key => {
            let node = &module.nodes[sref.index.index];
            node.value.is_none() && node.operation.is_none()
        }
        _ => false,
    };
    let root = |node| match node {
        AnyNodeId::Static(sref) if sref.module == module.key => {
            Some(static_find(&module.nodes, sref.index))
        }
        _ => None,
    };
    let Some((return_value, _)) = halves(function.r#return) else {
        return false;
    };
    let Some((parameter_value, parameter_type)) = halves(function.parameter) else {
        return false;
    };
    let return_type = AnyNodeId::Static(StaticNodeId {
        module: module.key,
        index: function.return_type,
    });
    open(return_value)
        && open(return_type)
        && root(return_value) != root(parameter_value)
        && root(return_type) != root(parameter_type)
}

/// The static identity of a node in `module`: what a host's tables key a cloned assert on.
fn static_ref<P: Program>(module: &StaticModule<P>, node: LocalNodeId) -> AnyNodeId {
    AnyNodeId::Static(StaticNodeId {
        module: module.key,
        index: node,
    })
}

/// The cells in a frozen equality class, including its representative.
fn static_class_members<P: Program>(
    module: &StaticModule<P>,
    node: LocalNodeId,
) -> Vec<LocalNodeId> {
    if module.nodes.get(node.index).is_none() {
        return Vec::new();
    }
    let mut members = Vec::new();
    let mut seen = HashSet::new();
    let mut current = static_find(&module.nodes, node);
    while seen.insert(current) {
        members.push(current);
        let Some(next) = module.nodes[current.index].equality.next() else {
            break;
        };
        current = next;
    }
    members
}

/// Every module key a static ref in the module's solved values names — its frozen
/// dependencies.
pub(crate) fn referenced_keys<P: Program>(module: &Module<P>) -> HashSet<ModuleKey> {
    let mut keys = HashSet::new();
    for node in module.nodes.values() {
        if let Some(value) = node.value {
            collect_referenced_keys::<P>(value, &mut keys);
        }
    }
    // The default is module state the artifact files: its home module is a
    // dependency, as one a node's value names is.
    collect_array_referenced_keys(module.default_attributes, &mut keys);
    keys
}

/// [`referenced_keys`] over `node_ids`: the closure a per-cell freeze files. A self
/// ref is not a dependency.
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

/// One array payload's contribution to a reference walk: its home module and
/// every module its items name.
fn collect_array_referenced_keys(
    array: Option<AnyHandle<[ArrayItem]>>,
    keys: &mut HashSet<ModuleKey>,
) {
    let Some(array) = array else {
        return;
    };
    match array {
        AnyHandle::Static(handle) => {
            keys.insert(handle.module);
        }
        AnyHandle::Dynamic(_) => {}
    }
    // SAFETY: the payload's home storage is live — a dependency's shared
    // arena, or a block arena of the borrowed module.
    for item in unsafe { array.items() } {
        if let AnyNodeId::Static(sref) = item.node {
            keys.insert(sref.module);
        }
    }
}

/// One value's contribution to [`referenced_keys`]: the single site of this match.
fn collect_referenced_keys<P: Program>(value: P::Value, keys: &mut HashSet<ModuleKey>) {
    match value.as_enum() {
        Some(LowValue::Array(array)) => match *array {
            AnyHandle::Static(handle) => {
                keys.insert(handle.module);
                // SAFETY: the payload lives in the dependency's shared arena, pinned by the
                // registry holding it.
                for item in unsafe { &*handle.offset } {
                    if let AnyNodeId::Static(sref) = item.node {
                        keys.insert(sref.module);
                    }
                }
            }
            AnyHandle::Dynamic(handle) => {
                // SAFETY: `Module::alloc_array` put it in a block arena of `module`,
                // alive as long as the module is.
                for item in unsafe { &*handle.0 } {
                    if let AnyNodeId::Static(sref) = item.node {
                        keys.insert(sref.module);
                    }
                }
            }
        },
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
            // SAFETY: as in the dynamic array arm, via `Module::alloc_table`.
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

/// **Open captures**: its body reaches an undecided node outside its own
/// template scope (`StaticFunction::nodes`).
///
/// # Invariant
///
/// A scope's own open cells re-open per call through the residual clone rule, but a
/// capture sits outside it: its binding was made by whichever application minted this
/// closure, so only re-homing the closure through the apply's shared remap clones it
/// alongside the applied parameter.
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
        if sn.undecided() && !scope.contains(&node) {
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
                    // SAFETY: `array` is a payload in an artifact arena the caller
                    // holds alive for this walk.
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
    /// Fill every function's [`StaticFunction::open_captures`] from the artifact's own
    /// tables, once, at build time.
    ///
    /// # Safety
    ///
    /// The graph is the only source: a host that assembles a [`StaticModule`] by hand
    /// must call this after assembly, so a stored verdict cannot disagree with the graph.
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
    /// Every module key a static ref in the artifact's values names: [`referenced_keys`]
    /// for an already-static artifact.
    ///
    /// # Invariant
    ///
    /// The artifact's **own** key appears when a value references a node of this module,
    /// so the caller decides what to do with a self-reference: the registry's eviction
    /// check ignores it, or the artifact could never be evicted.
    pub(crate) fn referenced_keys(&self) -> HashSet<ModuleKey> {
        let mut keys = HashSet::new();
        for node in &self.nodes {
            let Some(value) = node.value else { continue };
            match value.as_enum() {
                Some(LowValue::Array(array)) => {
                    collect_array_referenced_keys(Some(*array), &mut keys)
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
        // As in the dynamic walk: the default is module state the artifact files.
        collect_array_referenced_keys(self.default_attributes.map(AnyHandle::Static), &mut keys);
        keys
    }
}
