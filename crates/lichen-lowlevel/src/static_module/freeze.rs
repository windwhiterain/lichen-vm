//! Freezing a solved module into static form under its registry-allocated key.

use super::*;
use crate::{Release, is_unbound};
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
        let nodes: Vec<NodeId> = module.nodes.iter().map(|(id, _)| id).collect();
        let functions: Vec<FunctionId> = module.functions.iter().map(|(id, _)| id).collect();
        Self::freeze_set(module, key, &nodes, &functions)
    }

    /// [`Self::from_module`] for the **closure** of `roots` — the per-cell
    /// freeze: only the nodes those roots can reach are filed, so an artifact is
    /// as small as the value it keeps, and an edit freezes only the cells it
    /// dirtied.
    ///
    /// The soundness obligations are the whole-module freeze's: the caller files
    /// the artifact under a key that is not yet taken, in a registry that already
    /// holds every key the closure's values reference.  What is new is that the
    /// closure, not the module, must be self-contained — see [`closure`].
    pub fn freeze_closure(
        module: &Module<P>,
        key: ModuleKey,
        roots: &[NodeId],
    ) -> (Self, HashMap<NodeId, LocalNodeId>) {
        let (nodes, functions) = closure(module, roots);
        Self::freeze_set(module, key, &nodes, &functions)
    }

    /// Freeze exactly `node_ids`/`function_ids`, both in slotmap order so the
    /// artifact's local indices are dense and canonical for the codec.
    ///
    /// Every intra-artifact reference must land inside the set: the maps below
    /// are indexed with `[...]`, so a reference that escapes panics here rather
    /// than producing an artifact that names a node it does not contain.  The
    /// one edge that may leave the set is the equality class — the closure takes
    /// a class whole only from a node whose own value is unbound ([`closure`]) —
    /// so a class the artifact does not hold whole is **spliced** to the members
    /// it does hold, exactly as the GC splices a class that lost members
    /// (`Module::flatten_class`).
    fn freeze_set(
        module: &Module<P>,
        key: ModuleKey,
        node_ids: &[NodeId],
        function_ids: &[FunctionId],
    ) -> (Self, HashMap<NodeId, LocalNodeId>) {
        // Phase 1: indices and per-node facts.  Two passes: every id must
        // be in the map before any meta is remapped (a class's member list
        // points forward in slotmap order).
        let mut node_map: HashMap<NodeId, LocalNodeId> = HashMap::new();
        for (index, &id) in node_ids.iter().enumerate() {
            node_map.insert(id, LocalNodeId { index });
        }
        // The frozen nodes of each class, in slotmap order (so a spliced class's
        // member list is canonical).  A class is held whole when the artifact has
        // as many of its members as the class has — every member present is a
        // distinct member, so the counts can only agree when nothing is missing.
        let mut classes: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for &id in node_ids {
            classes.entry(class_root(module, id)).or_default().push(id);
        }
        // The links a class the artifact does not hold whole is spliced down to,
        // in the source's key space (mapped through `node_map` below): the first
        // frozen member (in slotmap order) is the representative, every other
        // member's parent points straight at it, and the member list is re-linked
        // in that order — `disjoint::rebuild`'s splice, in the artifact's own key
        // space.  Two frozen members of one class therefore stay in one class,
        // which is all the frozen links are read for (`static_find`, the apply's
        // clone grouping), and a member the artifact does not hold has no clone to
        // be grouped with.
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
                // The class's low type, not this member's own slot: the low type
                // is a property of the class, and the authoritative copy lives on
                // its representative — a member's own slot may hold nothing while
                // its class is decided.
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
                // A node the deep pass never ran on is unproven — treated as
                // parameterized (conservative: it materializes as a clone).
                parameterized: node.evaluated_deep.is_none_or(|e| e.parameterized),
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
                asserts: function
                    .asserts
                    .iter()
                    .map(|&condition| node_map[&condition])
                    .collect(),
                nodes: function.nodes.iter().map(|&node| node_map[&node]).collect(),
            });
        }

        // The ownership transfer: every frozen value is asked what it owns
        // outside the arena, and the artifact carries the obligations until it is
        // dropped — which is its eviction (see `ValueExt::release_obligations`).
        let mut releases: Vec<Box<dyn Release>> = Vec::new();
        for value in values.iter().flatten() {
            value.release_obligations(&mut releases);
        }

        // Phase 2: collect, dedupe, and lay out the payload regions.  Only
        // dynamic payloads are laid out; a static payload (an array, function
        // value, or ext handle from a frozen dependency) already lives in its
        // dependency's shared arena and is filed verbatim — no copy.
        let mut unique: Vec<(usize, usize, usize)> = Vec::new(); // (ptr, len, align)
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        for &id in node_ids {
            let Some(value) = module.nodes[id].value else {
                continue;
            };
            if let Some(LowValue::Array(AnyHandle::Dynamic(handle))) = value.as_enum() {
                // SAFETY: a dynamic payload is allocated in a block arena of
                // `module` by `Module::alloc_array`, and the freeze holds
                // `module` borrowed for the whole build, so every block — and
                // every arena in them — is alive here.
                let items = unsafe { &*handle.0 };
                let bytes = std::mem::size_of_val(items);
                let key = (handle.0 as *const u8 as usize, bytes);
                if seen.insert(key) {
                    unique.push((key.0, key.1, std::mem::align_of::<ArrayItem>()));
                }
            } else if let Some(LowValue::Table(AnyHandle::Dynamic(handle))) = value.as_enum() {
                // SAFETY: as in the array arm above — the payload was
                // allocated in a block arena of `module` by
                // `Module::alloc_table`, and the freeze's borrow keeps that
                // block alive.
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
        // The arena base derives from the one alignment the artifact codec
        // derives from the program type ([`crate::codec::arena_align`]) —
        // never from the payloads that happen to be present — so a
        // serialized artifact round-trips to the same base.  Every payload
        // alignment divides it (alignments are powers of two), so each
        // payload stays aligned at base + offset.
        let arena_align = crate::codec::arena_align::<P>();
        let buffer = vec![0u8; cursor + arena_align];
        let base = align_up(buffer.as_ptr() as usize, arena_align) as *mut u8;
        // The single payload copy: phase 3 only rewrites refs inside these
        // arena copies and builds the static handles pointing at them.
        for &(ptr, len, _) in &unique {
            let offset = offsets[&(ptr, len)];
            // SAFETY: `ptr`/`len` were read from a live dynamic payload of
            // `module` above, so the source range is a real allocation;
            // `offset` is the `align_up`-ed cursor position that payload was
            // laid out at and the layout totals `cursor` bytes, with `base`
            // aligned inside `buffer`'s `cursor + arena_align` bytes — so the
            // destination range lies inside the freshly allocated `buffer` and
            // cannot overlap the source.
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
                releases,
            },
            node_map,
        )
    }
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
            // SAFETY: the payload was allocated in a block arena of the module
            // being frozen (`Module::alloc_array`), and that module is
            // borrowed for the whole build, so the arena is alive.
            let items = unsafe { &*handle.0 };
            let bytes = std::mem::size_of_val(items);
            // Phase 2 laid out and copied every dynamic payload of the
            // module, so the lookup is total by construction and the
            // payload is already in the arena — only the item refs of the
            // arena copy are rewritten here.
            let offset = *offsets
                .get(&(handle.0 as *const u8 as usize, bytes))
                .expect("phase 2 laid out every dynamic payload of the module");
            // SAFETY: phase 2 laid out exactly `bytes` at `offset` inside
            // `buffer` and copied the payload there, and `base` is that
            // buffer's aligned start — so the range is in bounds, aligned for
            // `ArrayItem`, and no other reference to the copy exists yet.
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
                    // SAFETY: the same in-bounds arena copy as `copied` above,
                    // at the phase-2 offset for this payload.
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
            // SAFETY: as in the array arm above — the payload was allocated in
            // a block arena of the module being frozen (`Module::alloc_table`)
            // and the build's borrow keeps the arena alive.
            let items = unsafe { &*handle.0 };
            let bytes = std::mem::size_of_val(items);
            // Same invariant as the array arm: phase 2 already laid out and
            // copied the payload; only the entry refs are rewritten here.
            let offset = *offsets
                .get(&(handle.0 as *const u8 as usize, bytes))
                .expect("phase 2 laid out every dynamic payload of the module");
            // SAFETY: as in the array arm above — the arena copy at `offset`
            // is in bounds, aligned for `TableItem`, and uniquely referenced
            // while phase 3 rewrites it.
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
                    // SAFETY: the same in-bounds arena copy as `copied` above,
                    // at the phase-2 offset for this payload.
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
            // Same invariant as the array arm: the payload was copied in
            // phase 2; only the handle is re-keyed here.
            let offset = *offsets
                .get(&(old.as_ptr() as usize, old.len()))
                .expect("phase 2 laid out every dynamic payload of the module");
            let mut value = value;
            value.set_handle(AnyHandle::Static(StaticHandle {
                module: key,
                offset: ptr::slice_from_raw_parts(
                    // SAFETY: phase 2 laid out this ext payload's `old.len()`
                    // bytes at the offset the lookup above returned, inside
                    // the same `buffer` whose aligned start is `base`.
                    unsafe { base.add(offset) } as *const u8,
                    old.len(),
                ),
            }));
            value
        }
        _ => value,
    }
}

/// The **closure** of `roots`: the node and function sets a freeze must contain
/// for the artifact to be self-contained, in slotmap order so the local indices
/// agree with the whole-module freeze's.
///
/// Closed under four edge kinds, plus whatever a value reports of itself:
///
/// - a value's items/entries (arrays, tables);
/// - `operation.operand` — a residual node must be able to re-run later, so
///   unlike the GC's walk, which deliberately does not follow a cached value's
///   operand, this one must;
/// - the equality class of a node whose **own value is still unbound** — the
///   class is what holds that node's answer (the shared inference cell, the
///   template's pattern), so the class is the structure a later read of it
///   depends on and is taken whole.  A node that already carries its own solved
///   value takes nothing from its class: every member of a solved class reads
///   alike, and a member the walk never reaches has no clone to be grouped with
///   (`static_find`, the apply's clone grouping, only ever asks about nodes the
///   artifact holds).  Either way a class is expanded **once**, however many of
///   its members the walk reaches: a shared type node puts every binding of a
///   program in one class, and walking it per visited member was the walk's
///   entire cost.  What the artifact does with a class it does not hold whole is
///   [`freeze_set`]'s business (it splices it, as the GC splices a class that
///   lost members);
/// - the **whole function template** — `StaticFunction.nodes` is the template's
///   member list, and a missing member is a broken template.
///
/// It also takes whatever the value reports through [`ValueExt::traced`] — the
/// contract the GC relies on, and the only way to see a node an opaque ext
/// payload holds: phase 3 rewrites a handle and never the bytes behind it, so a
/// value that holds nodes without saying so would freeze into an artifact that
/// does not contain them.  **A value that fails to answer is not caught**, here
/// exactly as in the GC.
fn closure<P: Program>(module: &Module<P>, roots: &[NodeId]) -> (Vec<NodeId>, Vec<FunctionId>) {
    let mut nodes: HashSet<NodeId> = HashSet::new();
    let mut functions: HashSet<FunctionId> = HashSet::new();
    let mut work: Vec<NodeId> = roots.to_vec();
    // The classes the walk has already expanded.  A class is expanded **once**,
    // not once per member the walk visits: `class_members` is a walk of its own,
    // and a *shared* type node puts every binding of the program in one class —
    // so re-expanding it for each visited member was the whole cost of this walk
    // (measured: 363,627 visits for a 611-node closure, 7 ms).
    //
    // Only a node whose **own value is unbound** takes its class, and that is what
    // keeps a decided leaf from paying for a class it does not need.  Such a
    // node's answer *is* its class — a shared inference cell, a template's
    // pattern — so the class is the structure a later read of it depends on and is
    // taken whole.  A node that already carries its own solved value takes nothing
    // from its class: a solved class reads alike at every member, and a member the
    // walk never reaches has no clone to be grouped with.  Deciding it per class
    // instead — "is every member of this class bound?" — costs the class's size at
    // every freeze that touches it: measured, ~50 µs for a 601-member shared type
    // class, against ~7 µs for the same freeze in a small program.
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
        if is_unbound(entry.value) {
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
                // SAFETY: the payload was allocated in a block arena of `module`
                // by `Module::alloc_array`, and this walk borrows `module`, so
                // the arena is alive.
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
            // A static payload names a frozen dependency: its refs are absolute
            // from birth and its arena belongs to that dependency, so nothing is
            // pulled in.  A leaf holds nothing.
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
    // Slot order, without scanning the module: `NodeId`'s `Ord` is its key data's,
    // whose first field is the slot index, so sorting the frozen set is exactly
    // the order `SlotMap::iter` yields — the order the whole-module freeze files
    // its nodes in, so a closure's local indices are a subsequence of that one's.
    // The scan this replaces was the walk's dominant cost once the closure was
    // small: one lookup per node of the *module* per freeze (measured, ~40 µs to
    // order a 7-node closure in a 3,600-node module).
    node_ids.sort_unstable();
    function_ids.sort_unstable();
    (node_ids, function_ids)
}

/// The representative of `node`'s equality class — up through `parent`, with no
/// path compression, so a read never mutates the tree.
fn class_root<P: Program>(module: &Module<P>, node: NodeId) -> NodeId {
    let mut root = node;
    while let Some(parent) = module.nodes[root].equality.parent() {
        root = parent;
    }
    root
}

/// Every member of `node`'s equality class, read-only: the representative and then
/// across `next` — the list `write_node_value` replicates over.  No path
/// compression, so a read never mutates the tree.
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
