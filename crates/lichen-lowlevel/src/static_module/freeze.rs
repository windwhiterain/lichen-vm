//! Freezing a solved module into static form under its registry-allocated key.

use super::*;
use crate::Release;
impl<P: Program> StaticModule<P> {
    /// The node's solved value, or [`None`] when the node is a residual
    /// computation with no cached answer.
    pub fn read(&self, node: LocalNodeId) -> Option<P::Value> {
        self.nodes[node.index].value
    }

    /// Freeze a solved module into static form under the registry-allocated `key`.
    ///
    /// # Invariant
    ///
    /// The source is fully solved: every node holds its final answer or is a residual
    /// operation with no cached answer, which reads as undecided. Module-level pending
    /// asserts are dropped — a solved module has decided everything decidable.
    ///
    /// # Safety
    ///
    /// Static refs the source already carries name its frozen dependencies and are filed
    /// verbatim, payloads and all. The artifact therefore resolves only inside a registry
    /// holding every key those refs name; [`Registry::freeze_mapped`] checks that first.
    pub fn from_module(module: &Module<P>, key: ModuleKey) -> Self {
        Self::from_module_mapped(module, key).0
    }

    /// [`Self::from_module`] plus the source→statics node map (see
    /// [`Registry::freeze_mapped`]).
    pub fn from_module_mapped(
        module: &Module<P>,
        key: ModuleKey,
    ) -> (Self, HashMap<NodeId, LocalNodeId>) {
        let nodes: Vec<NodeId> = module.nodes.iter().map(|(id, _)| id).collect();
        let functions: Vec<FunctionId> = module.functions.iter().map(|(id, _)| id).collect();
        Self::freeze_set(module, key, &nodes, &functions)
    }

    /// [`Self::from_module`] for the **closure** of `roots`: only the reachable nodes file.
    ///
    /// # Safety
    ///
    /// The closure, not the module, must be self-contained (see [`closure`]), and every key
    /// its values reference must already be filed. `check` gets that evidence *before* the
    /// freeze: freezing takes the artifact's release obligations off the values, so a later
    /// refusal would drop an artifact whose obligations were already taken.
    pub(crate) fn freeze_closure(
        module: &Module<P>,
        key: ModuleKey,
        roots: &[NodeId],
        check: impl FnOnce(HashSet<ModuleKey>),
    ) -> (Self, HashMap<NodeId, LocalNodeId>) {
        let (nodes, functions) = closure(module, roots);
        check(referenced_keys_of(module, &nodes));
        Self::freeze_set(module, key, &nodes, &functions)
    }

    /// Freeze exactly `node_ids`/`function_ids`, in slotmap order.
    ///
    /// # Invariant
    ///
    /// Every intra-artifact reference lands inside the set: the maps are indexed with
    /// `[...]`, so an escaping reference panics rather than naming an absent node. A
    /// closure's indices are a subsequence of the whole-module freeze's; the equality
    /// class may leave, and is spliced unless [`closure`] took it whole from an
    /// undecided node.
    fn freeze_set(
        module: &Module<P>,
        key: ModuleKey,
        node_ids: &[NodeId],
        function_ids: &[FunctionId],
    ) -> (Self, HashMap<NodeId, LocalNodeId>) {
        // Phase 1: indices and facts. A class's member list points forward, so
        // every id is mapped before a meta is remapped.
        let mut node_map: HashMap<NodeId, LocalNodeId> = HashMap::new();
        for (index, &id) in node_ids.iter().enumerate() {
            node_map.insert(id, LocalNodeId { index });
        }
        // Each class's frozen nodes, in slot order. Held whole = all present;
        // each is distinct, so equal counts prove it.
        let mut classes: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for &id in node_ids {
            classes.entry(class_root(module, id)).or_default().push(id);
        }
        // Spliced like `disjoint::rebuild`: the first frozen member represents, the
        // rest parent to it, in slot order.
        let mut spliced: HashMap<NodeId, disjoint::Meta<NodeId>> = HashMap::new();
        for (&class, members) in &classes {
            if members.len() as u32 == module.nodes[class].equality.size() {
                continue;
            }
            for (position, &member) in members.iter().enumerate() {
                spliced.insert(
                    member,
                    disjoint::Meta::new(
                        (position > 0).then_some(members[0]),
                        members.get(position + 1).copied(),
                        (position == 0).then_some(members[members.len() - 1]),
                        // A non-representative's size is never read
                        // (`disjoint::Meta`'s contract); 1 is `make_set`'s value.
                        if position == 0 {
                            members.len() as u32
                        } else {
                            1
                        },
                    ),
                );
            }
        }
        let mut nodes: Vec<StaticNode<P>> = Vec::with_capacity(node_ids.len());
        let mut values: Vec<Option<P::Value>> = Vec::with_capacity(node_ids.len());
        for &id in node_ids {
            let node = &module.nodes[id];
            values.push(node.value);
            // A class held whole carries its links over verbatim; one that is not
            // is spliced to the members the artifact holds.
            let equality = spliced.get(&id).copied().unwrap_or(node.equality);
            nodes.push(StaticNode {
                value: None, // rewritten in phase 2, once arena offsets exist

                // The class's low type, not this member's own — a member's slot may be empty.
                low_shape: module.class_low_type(id).cloned(),
                operation: node.operation.map(|operation| StaticOperation {
                    operator: operation.operator,
                    operand: operation.operand.map(|operand| node_map[&operand]),
                }),
                equality: disjoint::Meta::new(
                    equality.parent().map(|p| node_map[&p]),
                    equality.next().map(|n| node_map[&n]),
                    equality.tail().map(|t| node_map[&t]),
                    equality.size(),
                ),
                // The two axes of [`Node::value`] travel uncollapsed; [`StaticNode::undecided`]
                // collapses on read.
                runned: node.runned,
                evaluated_deep: node.evaluated_deep,
            });
        }
        let mut function_map: HashMap<FunctionId, StaticFunctionId> = HashMap::new();
        let mut functions: Vec<StaticFunction> = Vec::with_capacity(function_ids.len());
        for &id in function_ids {
            let function = &module.functions[id];
            function_map.insert(id, StaticFunctionId(functions.len()));
            functions.push(StaticFunction {
                parameter: node_map[&function.parameter],
                r#return: node_map[&function.r#return],
                // A hand-built function may leave `return_type` unset; fall back to the
                // return node, always mapped.
                return_type: *node_map
                    .get(&function.return_type)
                    .unwrap_or(&node_map[&function.r#return]),
                // A re-exported function keeps its original-position id, so two
                // re-exports of one function compare equal.
                origin: function.static_origin,
                asserts: function
                    .asserts
                    .iter()
                    .map(|&condition| node_map[&condition])
                    .collect(),
                nodes: function.nodes.iter().map(|&node| node_map[&node]).collect(),
                // Filled below, once every value is in static form.
                open_captures: false,
            });
        }

        // The ownership transfer: every frozen value is asked what it owns outside the arena.
        let mut releases: Vec<Box<dyn Release>> = Vec::new();
        for value in values.iter().flatten() {
            value.release_obligations(&mut releases);
        }

        // Phase 2: dedupe and lay out payload regions. A static payload is
        // never copied — it stays in its dependency's arena.

        let mut unique: Vec<(usize, usize, usize)> = Vec::new(); // (ptr, len, align)
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        for &id in node_ids {
            let Some(value) = module.nodes[id].value else {
                continue;
            };
            if let Some(LowValue::Array(AnyHandle::Dynamic(handle))) = value.as_enum() {
                // SAFETY: `Module::alloc_array` put it in a block arena of `module`,
                // which the freeze borrows for the whole build.
                let items = unsafe { &*handle.0 };
                let bytes = std::mem::size_of_val(items);
                let key = (handle.0 as *const u8 as usize, bytes);
                if seen.insert(key) {
                    unique.push((key.0, key.1, std::mem::align_of::<ArrayItem>()));
                }
            } else if let Some(LowValue::Table(AnyHandle::Dynamic(handle))) = value.as_enum() {
                // SAFETY: as in the array arm, with `Module::alloc_table`; the
                // borrow of `module` keeps the block alive.
                let items = unsafe { &*handle.0 };
                let bytes = std::mem::size_of_val(items);
                let key = (handle.0 as *const u8 as usize, bytes);
                if seen.insert(key) {
                    unique.push((key.0, key.1, std::mem::align_of::<TableItem>()));
                }
            } else if value.is_handle() && matches!(value.handle(), AnyHandle::Dynamic(_)) {
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
        // The base comes from `crate::codec::arena_align`, not the payloads
        // present, so a round-trip lands at the same base.
        let arena_align = crate::codec::arena_align::<P>();
        let buffer = vec![0u8; cursor + arena_align];
        let base = align_up(buffer.as_ptr() as usize, arena_align) as *mut u8;
        // The single payload copy: phase 3 only rewrites refs inside these arena copies.
        for &(ptr, len, _) in &unique {
            let offset = offsets[&(ptr, len)];
            // SAFETY: source is a live `module` payload, destination lies in the fresh
            // `buffer`, so the ranges are disjoint.
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

        // The open-capture verdict runs after phase 3: before it, every value is `None`.
        let mut artifact = StaticModule {
            key,
            nodes,
            functions,
            arena,
            releases,
        };
        artifact.fill_open_captures();
        (artifact, node_map)
    }
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// The phase-3 value rewrite: dynamic payloads and refs → static form keyed by `key`.
///
/// # Invariant
///
/// A value that is already static is returned verbatim: its refs are absolute from birth
/// and its payload lives in the dependency's shared arena. The item rewrite applies to
/// the arena copy, never the source slice.
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
            // SAFETY: `Module::alloc_array` put it in a block arena of `module`,
            // which is borrowed for the whole build.
            let items = unsafe { &*handle.0 };
            let bytes = std::mem::size_of_val(items);
            // Phase 2 laid out and copied every dynamic payload, so the lookup is total.
            let offset = *offsets
                .get(&(handle.0 as *const u8 as usize, bytes))
                .expect("phase 2 laid out every dynamic payload of the module");
            // SAFETY: phase 2 laid out `bytes` at `offset` in `buffer`; `base`
            // is its aligned start: in bounds, aligned, unaliased.
            let copied = unsafe {
                std::slice::from_raw_parts_mut(base.add(offset) as *mut ArrayItem, items.len())
            };
            for item in copied.iter_mut() {
                item.node = match item.node {
                    AnyNodeId::Dynamic(node) => AnyNodeId::Static(StaticNodeId {
                        module: key,
                        index: node_map[&node],
                    }),
                    // A static ref the source carries is filed verbatim and keeps naming
                    // the dependency.
                    static_ref @ AnyNodeId::Static(_) => static_ref,
                };
            }
            P::Value::from(LowValue::Array(AnyHandle::Static(StaticHandle {
                module: key,
                offset: ptr::slice_from_raw_parts(
                    // SAFETY: the in-bounds arena copy of `copied` above, at this
                    // payload's phase-2 offset.
                    unsafe { base.add(offset) } as *const ArrayItem,
                    items.len(),
                ),
            })))
        }
        // A static payload is the dependency's shared arena, keyed by its final
        // key — verbatim, never re-keyed or copied.
        Some(LowValue::Array(AnyHandle::Static(_)))
        | Some(LowValue::Table(AnyHandle::Static(_)))
        | Some(LowValue::Function(AnyFunctionId::Static(_))) => value,
        Some(LowValue::Table(AnyHandle::Dynamic(handle))) => {
            // SAFETY: as in the array arm, but `Module::alloc_table`; the build
            // borrows `module` for the whole freeze.
            let items = unsafe { &*handle.0 };
            let bytes = std::mem::size_of_val(items);
            // As in the array arm: phase 2 laid out and copied the payload;
            // only the entry refs are rewritten here.
            let offset = *offsets
                .get(&(handle.0 as *const u8 as usize, bytes))
                .expect("phase 2 laid out every dynamic payload of the module");
            // SAFETY: as in the array arm above, with `TableItem` alignment.
            let copied = unsafe {
                std::slice::from_raw_parts_mut(base.add(offset) as *mut TableItem, items.len())
            };
            for item in copied.iter_mut() {
                item.key = match item.key {
                    AnyNodeId::Dynamic(node) => AnyNodeId::Static(StaticNodeId {
                        module: key,
                        index: node_map[&node],
                    }),
                    // A static ref the source carries is filed verbatim and keeps
                    // naming the dependency.
                    static_ref @ AnyNodeId::Static(_) => static_ref,
                };
                item.value = match item.value {
                    AnyNodeId::Dynamic(node) => AnyNodeId::Static(StaticNodeId {
                        module: key,
                        index: node_map[&node],
                    }),
                    static_ref @ AnyNodeId::Static(_) => static_ref,
                };
                // The stored hash travels verbatim: a static key reads the same
                // solved content.
            }
            P::Value::from(LowValue::Table(AnyHandle::Static(StaticHandle {
                module: key,
                offset: ptr::slice_from_raw_parts(
                    // SAFETY: the in-bounds arena copy of `copied` above, at this
                    // payload's phase-2 offset.
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
            // An ext payload: only a dynamic handle is laid out and re-keyed; a
            // static one stays in its dependency's arena.
            if matches!(value.handle(), AnyHandle::Static(_)) {
                return value;
            }
            let old = value.handle();
            // Same invariant as the array arm: the payload was copied in
            // phase 2; only the handle is re-keyed here.
            let offset = *offsets
                .get(&(old.as_ptr() as usize, old.len()))
                .expect("phase 2 laid out every dynamic payload of the module");
            let mut value = value;
            value.set_handle(AnyHandle::Static(StaticHandle {
                module: key,
                offset: ptr::slice_from_raw_parts(
                    // SAFETY: as in the array arm — `old.len()` bytes at the
                    // looked-up offset, inside the same `buffer`.
                    unsafe { base.add(offset) } as *const u8,
                    old.len(),
                ),
            }));
            value
        }
        _ => value,
    }
}

/// The **closure** of `roots`: the node and function sets a freeze must contain to be
/// self-contained, in slotmap order.
///
/// # Invariant
///
/// Closed under four edge kinds: a value's items/entries; a residual node's
/// `operation.operand` (the GC's walk skips a cached operand, this one cannot); the
/// equality class of a node whose own value is undecided (it holds that node's answer,
/// taken whole once however many members the walk reaches); and the whole function
/// template. A class not held whole is spliced by [`freeze_set`].
///
/// # Safety
///
/// A value must report every node its opaque ext payload holds through
/// [`ValueExt::traced`]: phase 3 rewrites a handle and never the bytes behind it, so a
/// payload that holds nodes silently would freeze into an artifact that lacks them. A
/// value that fails to answer is not caught, here exactly as in the GC.
fn closure<P: Program>(module: &Module<P>, roots: &[NodeId]) -> (Vec<NodeId>, Vec<FunctionId>) {
    let mut nodes: HashSet<NodeId> = HashSet::new();
    let mut functions: HashSet<FunctionId> = HashSet::new();
    let mut work: Vec<NodeId> = roots.to_vec();
    // Classes already expanded, once each — not once per visited member.
    let mut decided: HashSet<NodeId> = HashSet::new();
    while let Some(node) = work.pop() {
        if !nodes.insert(node) {
            continue;
        }
        // A released node still named by a template's member list is skipped,
        // as the GC skips one.
        let Some(entry) = module.nodes.get(node) else {
            continue;
        };
        if entry.value.is_none() {
            let class = class_root(module, node);
            if decided.insert(class) {
                for member in class_members(module, class) {
                    work.push(member);
                }
            }
        }
        if let Some(operand) = entry.operation.and_then(|operation| operation.operand) {
            work.push(operand);
        }
        let Some(value) = entry.value else { continue };
        let mut traced = Vec::new();
        value.traced(module, &mut traced);
        work.extend(traced);
        match value.as_enum() {
            Some(LowValue::Array(AnyHandle::Dynamic(handle))) => {
                // SAFETY: `Module::alloc_array` put it in a block arena of `module`,
                // which this walk borrows.
                for item in unsafe { &*handle.0 } {
                    if let AnyNodeId::Dynamic(node) = item.node {
                        work.push(node);
                    }
                }
            }
            Some(LowValue::Table(AnyHandle::Dynamic(handle))) => {
                // SAFETY: as in the array arm above (`Module::alloc_table`).
                for item in unsafe { &*handle.0 } {
                    if let AnyNodeId::Dynamic(node) = item.key {
                        work.push(node);
                    }
                    if let AnyNodeId::Dynamic(node) = item.value {
                        work.push(node);
                    }
                }
            }
            Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
                if functions.insert(function) {
                    let template = &module.functions[function];
                    work.push(template.parameter);
                    work.push(template.r#return);
                    work.extend(template.asserts.iter().copied());
                    work.extend(template.nodes.iter().copied());
                }
            }
            // A static payload names a frozen dependency, whose arena owns it; a
            // leaf holds nothing.
            _ => {}
        }
    }
    let mut node_ids: Vec<NodeId> = nodes
        .into_iter()
        .filter(|&node| module.nodes.contains_key(node))
        .collect();
    let mut function_ids: Vec<FunctionId> = functions
        .into_iter()
        .filter(|&function| module.functions.contains_key(function))
        .collect();
    // Slot order without a module scan: `NodeId`'s `Ord` leads with the slot
    // index, which is `SlotMap::iter`'s own order.
    node_ids.sort_unstable();
    function_ids.sort_unstable();
    (node_ids, function_ids)
}

/// `node`'s class representative — up through `parent`, no path compression, so a
/// read never mutates the tree.
fn class_root<P: Program>(module: &Module<P>, node: NodeId) -> NodeId {
    let mut root = node;
    while let Some(parent) = module.nodes[root].equality.parent() {
        root = parent;
    }
    root
}

/// `node`'s class, read-only: the representative, then across `next` — the list
/// `write_node_value` replicates over.
fn class_members<P: Program>(module: &Module<P>, node: NodeId) -> Vec<NodeId> {
    let root = class_root(module, node);
    let mut members = vec![root];
    let mut current = root;
    while let Some(next) = module.nodes[current].equality.next() {
        members.push(next);
        current = next;
    }
    members
}
