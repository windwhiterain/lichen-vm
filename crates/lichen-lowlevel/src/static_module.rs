//! Static module dependencies: a fully-solved [`StaticModule`] registered
//! in the device's [`Registry`] and used in place by an importer
//! [`Module`].
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
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, BlockId,
    FunctionId, LocalNodeId, LowValue, Module, ModuleKey, NodeId, Operation, Program,
    StaticFunction, StaticFunctionId, StaticFunctionRef, StaticHandle, StaticModule, StaticNode,
    StaticNodeId, StaticOperation, TableItem, ValueExt as _,
};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

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
    /// the raw value is safe to store anywhere.
    pub fn node_value(&self, id: AnyNodeId) -> Option<P::Value> {
        match id {
            Dyn(node) => self.nodes[node].value,
            AnyNodeId::Static(sref) => {
                self.static_module(sref.module).nodes[sref.index.index].value
            }
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

    /// Apply a static function: materialize its graph into fresh dynamic
    /// clones in `block`, then run the standard apply tail (parameter unify,
    /// `ApplyError`, cell wiring) shared with [`Module::function_apply`].
    #[stacksafe]
    pub fn static_function_apply(
        &mut self,
        function: StaticFunctionRef,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> P::Value {
        self.with_apply_frame(|module| {
            let static_module = module.static_module(function.module);
            let (r#return, parameter, asserts) = {
                let f = &static_module.functions[function.index.0];
                (f.r#return, f.parameter, f.asserts.clone())
            };
            let mut ctx = StaticApplyCtx {
                target: block,
                module: static_module,
                remap: HashMap::new(),
            };
            let applied = module.static_node_apply(r#return, &mut ctx);
            // The parameter is an entry point of the walk, not just a node the
            // return subtree happens to reach: an ignored parameter still must
            // be satisfied, and a parameter read a type annotation pinned is
            // invisible from the return.
            module.static_node_apply(parameter, &mut ctx);
            // The body's asserts are the function's own registry entries: each
            // condition instantiates through the shared remap.  A baked
            // condition is per-call invariant (decided at solve time) and is not
            // re-registered; a cloned one re-checks against the argument.
            for &condition in &asserts {
                let baked = !ctx.module.nodes[condition.index].parameterized;
                let instantiated = module.static_node_apply(condition, &mut ctx);
                if !baked {
                    module.asserts.push(instantiated);
                }
            }
            // The parameter unify: same shape as `function_apply` — re-establish
            // the template's internal class topology among the clones (grouped
            // by the solved static reps), evaluate the argument to the pattern's
            // depth, unify, and record an `ApplyError` on failure.
            if let Some(&cloned_param) = ctx.remap.get(&parameter) {
                if ctx.remap.len() > 1 {
                    for clones in crate::apply::regroup_clones(
                        ctx.remap.iter().map(|(&template, &clone)| (template, clone)),
                        |template| static_find(&ctx.module.nodes, template),
                    )
                    .values()
                    {
                        let first = clones[0];
                        for &clone in &clones[1..] {
                            module.unify(first, clone);
                        }
                    }
                }
                if module.apply_parameter_check(
                    cloned_param,
                    argument,
                    block,
                    node,
                    AnyFunctionId::Static(function),
                    cloned_param,
                ) {
                    return P::Value::from(LowValue::Parameterized);
                }
            }
            let result = module.evaluate_node(Dyn(applied), Some(block));
            module.wire_apply_result(node, cell, result, applied, block)
        })
    }

    /// Clone one static node into the dynamic world (see the module docs).
    /// `#[stacksafe]`: static recursion runs through here at one frame per
    /// level, so the apply-depth guard must be able to grow the stack.
    #[stacksafe]
    fn static_node_apply(&mut self, local: LocalNodeId, ctx: &mut StaticApplyCtx<P>) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&local) {
            return clone;
        }
        let node = &ctx.module.nodes[local.index];
        let (parameterized, template_operation) = (node.parameterized, node.operation);
        // Reserve the clone id before recursing so diamonds resolve to one
        // clone and value cycles to the clone's own (still evaluating) id.
        let clone = self.add_node(ctx.target, None, None);
        ctx.remap.insert(local, clone);
        if parameterized {
            // Residual: the operation (if any) is kept with its operand
            // walked — the computation re-runs against the argument — and a
            // stale cached value on an operation node is dropped (it was
            // computed against the unbound template parameter).  A
            // parameterized *value* node (no operation — a structural array
            // containing the parameter, or the marker itself) keeps its
            // value, with items re-pointed at the walk's clones, mirroring
            // the dynamic clone rule.
            let operation = template_operation.map(|operation| Operation {
                operator: operation.operator,
                operand: operation
                    .operand
                    .map(|operand| self.static_node_apply(operand, ctx)),
            });
            if operation.is_none() {
                let value = ctx.module.read(local);
                self.nodes[clone].value = Some(self.static_remap_value(value, ctx));
            }
            self.nodes[clone].operation = operation;
        } else {
            // Baked: the solved value in place (shared payload — no copy),
            // with item refs re-pointed at per-call clones where the walk
            // made one; untouched items stay inline absolute static refs.
            // The residual operation (if any) is dead — the value is final.
            let value = ctx.module.read(local);
            self.nodes[clone].value = Some(self.static_remap_value(value, ctx));
        }
        clone
    }

    /// Re-point the items of a value at per-call clones: an item is
    /// cloned (walked) when the walk already made one, or when its static
    /// node is itself parameterized — a residual behind a value edge must
    /// re-open against the argument (a condition or branch frozen as
    /// `Parameterized` at solve time reads as unbound forever otherwise).
    /// Concrete items stay inline absolute static refs.  An item naming
    /// *another* module (a frozen dependency the applied function's module
    /// itself imported) is not this template's to clone: local indices are
    /// per-module, so only a ref keyed by `ctx.module` may consult the
    /// remap or the module's parameterized flags — a foreign ref is
    /// concrete by construction (the apply that kept it verbatim proved it)
    /// and stays in place, resolved through the registry.  The item slice is
    /// reallocated only when something changed — the common all-baked case
    /// shares the payload.
    fn static_remap_value(&mut self, value: P::Value, ctx: &mut StaticApplyCtx<P>) -> P::Value {
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return value;
        };
        let items = array.items();
        let mut changed = false;
        let mut remapped = Vec::with_capacity(items.len());
        for item in items {
            let node = match item.node {
                AnyNodeId::Static(sref)
                    if sref.module == ctx.module.key
                        && (ctx.remap.contains_key(&sref.index)
                            || ctx.module.nodes[sref.index.index].parameterized) =>
                {
                    changed = true;
                    Dyn(self.static_node_apply(sref.index, ctx))
                }
                node => node,
            };
            remapped.push(ArrayItem { node, ..*item });
        }
        if !changed {
            return value;
        }
        P::Value::from(LowValue::Array(self.alloc_array(&remapped, ctx.target)))
    }
}

/// The fixed context of one static materialize pass: where the clones land,
/// the module being materialized (an `Arc` clone, so reads never borrow
/// `self` while clones are created), and the running remap (static node →
/// its dynamic clone).
struct StaticApplyCtx<P: Program> {
    target: BlockId,
    module: Arc<StaticModule<P>>,
    remap: HashMap<LocalNodeId, NodeId>,
}

/// The solved union-find representative of `key` in the static meta — the
/// static side of `disjoint::find`, walked without path compression (the
/// solved structure is immutable).
fn static_find<P: Program>(nodes: &[StaticNode<P>], key: LocalNodeId) -> LocalNodeId {
    let mut current = key;
    while let Some(parent) = nodes[current.index].equality.parent {
        current = parent;
    }
    current
}

impl<P: Program> StaticModule<P> {
    /// The node's solved value — `Parameterized` when the node is a
    /// residual computation with no cached answer.
    pub fn read(&self, node: LocalNodeId) -> P::Value {
        self.nodes[node.index]
            .value
            .unwrap_or_else(|| P::Value::from(LowValue::Parameterized))
    }

    /// Freeze a solved module into static form under the registry-allocated
    /// `key`: consecutive local indices over the source's slotmap order, the
    /// flattenable payloads (array item slices and ext-value bytes, deduped
    /// by `(ptr, len)` so aliased handles keep identity equality) laid out
    /// once into `arena`, and every value rewritten to static form keyed by
    /// `key` — absolute from birth, shared by every importer.  The key is
    /// allocated by the [`Registry`] before the build
    /// ([`SlotMap::try_insert_with_key`]), so refs are baked with their
    /// final key.
    ///
    /// The source must be fully solved: every node holds its final answer,
    /// or a residual operation whose `Parameterized` value is the answer.
    /// Module-level pending asserts of the source are dropped — a solved
    /// module has decided everything decidable.
    ///
    /// Static refs the source already carries name its frozen dependencies.
    /// They are absolute from birth (keyed by the dependency's final key), so
    /// they are filed **verbatim** — never rewritten to `key`, their payloads
    /// never copied into the new arena.  The artifact is therefore only sound
    /// inside a registry that holds every referenced key;
    /// [`Registry::freeze_mapped`] checks exactly that before building.
    pub fn from_module(module: &Module<P>, key: ModuleKey) -> Self {
        Self::from_module_mapped(module, key).0
    }

    /// [`Self::from_module`] plus the source→statics node map (see
    /// [`Registry::freeze_mapped`]).
    pub fn from_module_mapped(
        module: &Module<P>,
        key: ModuleKey,
    ) -> (Self, HashMap<NodeId, LocalNodeId>) {
        // Phase 1: indices and per-node facts.  Two passes: every id must
        // be in the map before any meta is remapped (a class's member list
        // points forward in slotmap order).
        let mut node_map: HashMap<NodeId, LocalNodeId> = HashMap::new();
        for (index, (id, _)) in module.nodes.iter().enumerate() {
            node_map.insert(id, LocalNodeId { index });
        }
        let mut nodes: Vec<StaticNode<P>> = Vec::with_capacity(module.nodes.len());
        let mut values: Vec<Option<P::Value>> = Vec::with_capacity(module.nodes.len());
        for (_, node) in module.nodes.iter() {
            values.push(node.value);
            nodes.push(StaticNode {
                value: None, // rewritten in phase 2, once arena offsets exist
                operation: node.operation.map(|operation| StaticOperation {
                    operator: operation.operator,
                    operand: operation.operand.map(|operand| node_map[&operand]),
                }),
                equality: disjoint::Meta {
                    parent: node.equality.parent.map(|p| node_map[&p]),
                    next: node.equality.next.map(|n| node_map[&n]),
                    tail: node.equality.tail.map(|t| node_map[&t]),
                    size: node.equality.size,
                },
                // A node the deep pass never ran on is unproven — treated as
                // parameterized (conservative: it materializes as a clone).
                parameterized: node.evaluated_deep.is_none_or(|e| e.parameterized),
            });
        }
        let mut function_map: HashMap<FunctionId, StaticFunctionId> = HashMap::new();
        let mut functions: Vec<StaticFunction> = Vec::with_capacity(module.functions.len());
        for (id, function) in module.functions.iter() {
            function_map.insert(id, StaticFunctionId(functions.len()));
            functions.push(StaticFunction {
                parameter: node_map[&function.parameter],
                r#return: node_map[&function.r#return],
                asserts: function
                    .asserts
                    .iter()
                    .map(|&condition| node_map[&condition])
                    .collect(),
            });
        }

        // Phase 2: collect, dedupe, and lay out the payload regions.  Only
        // dynamic payloads are laid out; a static payload (an array, function
        // value, or ext handle from a frozen dependency) already lives in its
        // dependency's shared arena and is filed verbatim — no copy.
        let mut unique: Vec<(usize, usize, usize)> = Vec::new(); // (ptr, len, align)
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        for (_, node) in module.nodes.iter() {
            let Some(value) = node.value else { continue };
            if let Some(LowValue::Array(AnyHandle::Dynamic(handle))) = value.as_enum() {
                let items = unsafe { &*handle.0 };
                let bytes = std::mem::size_of_val(items);
                let key = (handle.0 as *const u8 as usize, bytes);
                if seen.insert(key) {
                    unique.push((key.0, key.1, std::mem::align_of::<ArrayItem>()));
                }
            } else if let Some(LowValue::Table(AnyHandle::Dynamic(handle))) = value.as_enum() {
                let items = unsafe { &*handle.0 };
                let bytes = std::mem::size_of_val(items);
                let key = (handle.0 as *const u8 as usize, bytes);
                if seen.insert(key) {
                    unique.push((key.0, key.1, std::mem::align_of::<TableItem>()));
                }
            } else if value.is_handle()
                && matches!(value.handle(), AnyHandle::Dynamic(_))
            {
                let handle = value.handle();
                let key = (handle.as_ptr() as usize, handle.len());
                if seen.insert(key) {
                    unique.push((key.0, key.1, P::Value::alignment()));
                }
            }
        }
        let mut cursor = 0usize;
        let mut offsets: HashMap<(usize, usize), usize> = HashMap::new();
        for &(ptr, len, align) in &unique {
            let offset = align_up(cursor, align);
            offsets.insert((ptr, len), offset);
            cursor = offset + len;
        }
        let max_align = unique.iter().map(|&(_, _, align)| align).max().unwrap_or(1);
        let buffer = vec![0u8; cursor + max_align];
        let base = align_up(buffer.as_ptr() as usize, max_align) as *mut u8;
        for &(ptr, len, _) in &unique {
            let offset = offsets[&(ptr, len)];
            unsafe { ptr::copy_nonoverlapping(ptr as *const u8, base.add(offset), len) };
        }
        let arena = buffer;

        // Phase 3: rewrite every value to static form keyed by `key`.
        for (node, value) in nodes.iter_mut().zip(&values) {
            let Some(value) = value else { continue };
            node.value = Some(rewrite_value::<P>(
                *value,
                key,
                base,
                &offsets,
                &node_map,
                &function_map,
            ));
        }

        (
            StaticModule {
                key,
                nodes,
                functions,
                arena,
            },
            node_map,
        )
    }
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
        let Some(value) = node.value else { continue };
        match value.as_enum() {
            Some(LowValue::Array(AnyHandle::Static(handle))) => {
                keys.insert(handle.module);
                // Safe: the payload lives in the dependency's shared arena,
                // pinned by the registry the artifact is filed into.
                for item in unsafe { &*handle.offset } {
                    if let AnyNodeId::Static(sref) = item.node {
                        keys.insert(sref.module);
                    }
                }
            }
            Some(LowValue::Array(AnyHandle::Dynamic(handle))) => {
                // Safe: the handle points into one of the module's own block
                // arenas, alive as long as the module is.
                for item in unsafe { &*handle.0 } {
                    if let AnyNodeId::Static(sref) = item.node {
                        keys.insert(sref.module);
                    }
                }
            }
            Some(LowValue::Table(AnyHandle::Static(handle))) => {
                keys.insert(handle.module);
                // Safe: the payload lives in the dependency's shared arena,
                // pinned by the registry the artifact is filed into.
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
    keys
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// The phase-3 value rewrite: dynamic payloads → static-arena payloads
/// (`AnyHandle::Static` keyed by `key`), item refs → `AnyNodeId::Static`
/// into `key`, function refs → `AnyFunctionId::Static` into `key`.
/// The item rewrite applies to the arena copy, never the source slice.
/// A value that is already static (an array payload, function ref, or ext
/// handle from a frozen dependency) is returned verbatim: its refs are
/// absolute from birth and its payload lives in the dependency's shared
/// arena — nothing to rewrite, nothing to copy.
fn rewrite_value<P: Program>(
    value: P::Value,
    key: ModuleKey,
    base: *mut u8,
    offsets: &HashMap<(usize, usize), usize>,
    node_map: &HashMap<NodeId, LocalNodeId>,
    function_map: &HashMap<FunctionId, StaticFunctionId>,
) -> P::Value {
    match value.as_enum() {
        Some(LowValue::Array(AnyHandle::Dynamic(handle))) => {
            let items = unsafe { &*handle.0 };
            let bytes = std::mem::size_of_val(items);
            let offset = offsets[&(handle.0 as *const u8 as usize, bytes)];
            unsafe { ptr::copy_nonoverlapping(handle.0 as *const u8, base.add(offset), bytes) };
            let copied = unsafe {
                std::slice::from_raw_parts_mut(base.add(offset) as *mut ArrayItem, items.len())
            };
            for item in copied.iter_mut() {
                item.node = match item.node {
                    AnyNodeId::Dynamic(node) => AnyNodeId::Static(StaticNodeId {
                        module: key,
                        index: node_map[&node],
                    }),
                    // A static ref the source carries names a frozen
                    // dependency — absolute from birth, so it is filed
                    // verbatim and keeps pointing into the dependency.
                    static_ref @ AnyNodeId::Static(_) => static_ref,
                };
            }
            P::Value::from(LowValue::Array(AnyHandle::Static(StaticHandle {
                module: key,
                offset: ptr::slice_from_raw_parts(
                    unsafe { base.add(offset) } as *const ArrayItem,
                    items.len(),
                ),
            })))
        }
        // A static payload is the dependency's shared arena, keyed by the
        // dependency's final key — verbatim, never re-keyed or copied.
        Some(LowValue::Array(AnyHandle::Static(_)))
        | Some(LowValue::Table(AnyHandle::Static(_)))
        | Some(LowValue::Function(AnyFunctionId::Static(_))) => value,
        Some(LowValue::Table(AnyHandle::Dynamic(handle))) => {
            let items = unsafe { &*handle.0 };
            let bytes = std::mem::size_of_val(items);
            let offset = offsets[&(handle.0 as *const u8 as usize, bytes)];
            unsafe { ptr::copy_nonoverlapping(handle.0 as *const u8, base.add(offset), bytes) };
            let copied = unsafe {
                std::slice::from_raw_parts_mut(base.add(offset) as *mut TableItem, items.len())
            };
            for item in copied.iter_mut() {
                item.key = match item.key {
                    AnyNodeId::Dynamic(node) => AnyNodeId::Static(StaticNodeId {
                        module: key,
                        index: node_map[&node],
                    }),
                    // A static ref the source carries names a frozen
                    // dependency — absolute from birth, so it is filed
                    // verbatim and keeps pointing into the dependency.
                    static_ref @ AnyNodeId::Static(_) => static_ref,
                };
                item.value = match item.value {
                    AnyNodeId::Dynamic(node) => AnyNodeId::Static(StaticNodeId {
                        module: key,
                        index: node_map[&node],
                    }),
                    static_ref @ AnyNodeId::Static(_) => static_ref,
                };
                // The stored hash travels verbatim: a static key reads the
                // same solved content a dynamic one did, so the hash stays
                // the artifact's own.
            }
            P::Value::from(LowValue::Table(AnyHandle::Static(StaticHandle {
                module: key,
                offset: ptr::slice_from_raw_parts(
                    unsafe { base.add(offset) } as *const TableItem,
                    items.len(),
                ),
            })))
        }
        Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => P::Value::from(
            LowValue::Function(AnyFunctionId::Static(StaticFunctionRef {
                module: key,
                index: function_map[&function],
            })),
        ),
        _ if value.is_handle() => {
            // An ext-value payload: only a dynamic handle is laid out into
            // the new arena and re-keyed; a static one stays in its
            // dependency's arena, verbatim.
            if matches!(value.handle(), AnyHandle::Static(_)) {
                return value;
            }
            let old = value.handle();
            let offset = offsets[&(old.as_ptr() as usize, old.len())];
            unsafe { ptr::copy_nonoverlapping(old.as_ptr(), base.add(offset), old.len()) };
            let mut value = value;
            value.set_handle(AnyHandle::Static(StaticHandle {
                module: key,
                offset: ptr::slice_from_raw_parts(
                    unsafe { base.add(offset) } as *const u8,
                    old.len(),
                ),
            }));
            value
        }
        _ => value,
    }
}
