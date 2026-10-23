use std::collections::HashSet;

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, ArrayValue,
    FunctionIdentity, LowShape, LowValue, Module, Node, NodeId, PAIR_ATTR_BASE, Program,
    StaticFunctionRef, StaticNodeId, ValueExt as _, ancestors::AncestorPairs,
};
use lichen_utils::disjoint::{self, Node as _};
use lichen_utils::extend::AsEnum;

/// One failed-unification step; `index` is authoritative, a static child names the default.
#[derive(Debug, Clone, Copy)]
pub struct UnifyStep {
    /// The array element index the traversal descended into.
    pub index: usize,
    /// The two child operands compared at that step.
    pub a: NodeId,
    pub b: NodeId,
}

#[derive(Debug, Clone)]
pub struct UnifyError<P: Program> {
    /// The two top-level operands the trigger framed: a diagnostic is attributed
    /// here, not to `a`/`b`.
    pub root_a: NodeId,
    pub root_b: NodeId,
    /// The element-by-element descent to the conflict; empty for a top-level clash.
    pub steps: Vec<UnifyStep>,
    /// The conflicting classes, as the lowlevel recorded them.
    pub a: NodeId,
    pub b: NodeId,
    pub value_a: Option<P::Value>,
    pub value_b: Option<P::Value>,
}

/// A side of a unification: a node's class, a bare value, or both. See docs/notes/lowlevel-vm.md.
#[derive(Clone, Copy)]
struct Side<P: Program> {
    node: Option<NodeId>,
    value: Option<P::Value>,
}

/// The depth bound for a comparison without classes, which cannot name itself to the guard.
const MAX_VALUE_DEPTH: usize = 64;

impl<P: Program> Side<P> {
    /// A side that is a node (its class answers for it).
    fn node(node: NodeId) -> Self {
        Self {
            node: Some(node),
            value: None,
        }
    }

    /// A side that is a bare value, with no class.
    fn value(value: Option<P::Value>) -> Self {
        Self { node: None, value }
    }

    /// The side an array item names: its node, or its value when it is a static ref.
    fn of(id: AnyNodeId, value: Option<P::Value>) -> Self {
        match id {
            Dyn(node) => Self::node(node),
            AnyNodeId::Static(_) => Self::value(value),
        }
    }
}

impl<P: Program> disjoint::Node for Node<P> {
    type Key = NodeId;
    fn meta(&self) -> &disjoint::Meta<NodeId> {
        &self.equality
    }
    fn meta_mut(&mut self, _permit: disjoint::MetaPermit) -> &mut disjoint::Meta<NodeId> {
        &mut self.equality
    }
}

impl<P: Program> Module<P> {
    /// Merge the classes of `a` and `b`, then give the merged class its one value.
    ///
    /// # Invariant
    /// **The caller's contract**: where both sides are decided, they are one value —
    /// [`Module::unify`] establishes that on every path that reaches here, returning on
    /// the conflicting arms instead. A merge of two decided-different classes is not a
    /// merge, and this is the one thing the write rules rely on.
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        let left = self.class_low_type(a).cloned();
        let right = self.class_low_type(b).cloned();
        let left_value = self.nodes[a].value;
        let right_value = self.nodes[b].value;
        let representative = disjoint::union(&mut self.nodes, a, b);
        for shape in [left, right].into_iter().flatten() {
            self.refine_class_low_type(representative, shape);
        }
        if let Some(value) = left_value.or(right_value) {
            // A class that holds a value has no holes, so only a merge with a valueless
            // class fills.
            if left_value.is_none() || right_value.is_none() {
                self.fill_class_holes(representative, value);
            }
        }
        representative
    }

    /// Give `value` to every member of `representative`'s class that holds nothing.
    ///
    /// # Invariant
    /// A member that already holds a value keeps it, so the merge never replaces one. The
    /// filled class is uniform because a merge only happens where the two sides are one
    /// value or one side is undecided: "one class holds one value" follows from that, not
    /// from writing over anybody. See docs/notes/lowlevel-vm.md.
    pub(crate) fn fill_class_holes(&mut self, representative: NodeId, value: P::Value) {
        // As `propagate_class_value`: the list is walked in place, because a hole fill
        // changes no member's class.
        let mut member = Some(representative);
        while let Some(current) = member {
            let node = &mut self.nodes[current];
            member = node.meta().next();
            if node.value.is_none() {
                node.value = Some(value);
            }
        }
    }

    pub fn equality_representative(&mut self, node: NodeId) -> NodeId {
        disjoint::find(&mut self.nodes, node)
    }

    /// The class's **low type** through its representative. See docs/notes/lowlevel-low-types.md.
    pub fn class_low_type(&self, node: NodeId) -> Option<&LowShape> {
        self.nodes.get(self.class_root(node))?.low_shape.as_deref()
    }

    /// The value the node's **class** holds, read through the representative.
    ///
    /// # Invariant
    /// One class holds one value, so every member answers the same; a class holding
    /// nothing answers [`None`]. See docs/notes/lowlevel-vm.md.
    pub fn class_value(&self, node: NodeId) -> Option<P::Value> {
        self.nodes.get(self.class_root(node))?.value
    }

    /// The class's recursive low type, cut by a path guard. See docs/notes/lowlevel-low-types.md.
    pub fn low_type_of_node(&self, node: NodeId) -> Option<LowShape> {
        let shape = self.class_low_type(node)?.clone();
        let mut seen = HashSet::new();
        Some(self.deepen_low_type(node, shape, &mut seen))
    }

    /// Seed `shape` as a lower bound on `node`'s class. See docs/notes/lowlevel-low-types.md.
    pub fn seed_class_low_type(&mut self, node: NodeId, shape: LowShape) -> bool {
        self.refine_class_low_type(node, shape)
    }

    /// The union-find representative of `node`, without path compression; a read.
    pub fn class_root(&self, node: NodeId) -> NodeId {
        let mut root = node;
        while let Some(parent) = self.nodes[root].equality.parent() {
            root = parent;
        }
        root
    }

    /// The array items of `node`'s class value, read through the representative.
    fn class_array_items(&self, node: NodeId) -> Option<&'static [ArrayItem]> {
        let LowValue::Array(array) = self.nodes[node].value?.as_enum()? else {
            return None;
        };
        // SAFETY: the slice points into the value's home arena, alive for this borrow.
        Some(unsafe { array.items() })
    }

    /// Refine `node`'s class low type: the single write side.
    ///
    /// # Invariant
    /// A class's shape is stated once; every later statement is exactly that shape,
    /// because the seed and the observation of one value agree (the highlevel
    /// invariant, `docs/notes/lowlevel-low-types.md`).
    pub fn refine_class_low_type(&mut self, node: NodeId, shape: LowShape) -> bool {
        let root = self.class_root(node);
        let Some(current) = self
            .nodes
            .get(root)
            .map(|entry| entry.low_shape.as_deref().cloned())
        else {
            return false;
        };
        match current {
            None => {
                self.nodes.get_mut(root).expect("a live class").low_shape = Some(Box::new(shape));
                true
            }
            Some(current) => {
                debug_assert_eq!(
                    current, shape,
                    "a class states one shape: its seed and its observation agree (node {node:?})"
                );
                false
            }
        }
    }

    /// Refine `node`'s class low type from a concrete value's top-level tag.
    pub(crate) fn observe_class_low_type(&mut self, node: NodeId, value: P::Value) {
        let Some(shape) = observed_low_shape(value) else {
            return;
        };
        self.refine_class_low_type(node, shape);
    }

    /// One structural element: a stated position is kept, an open one takes the
    /// element's own shape.
    fn deepen_position(
        &self,
        position: Option<LowShape>,
        element: AnyNodeId,
        seen: &mut HashSet<NodeId>,
    ) -> Option<LowShape> {
        if position.is_some() {
            return position;
        }
        let AnyNodeId::Dynamic(element) = element else {
            return None;
        };
        let shape = self.class_low_type(element).cloned()?;
        Some(self.deepen_low_type(element, shape, seen))
    }

    /// [`Module::low_type_of_node`] at one level; `seen` cuts a self-referential cycle.
    fn deepen_low_type(
        &self,
        node: NodeId,
        shape: LowShape,
        seen: &mut HashSet<NodeId>,
    ) -> LowShape {
        let root = self.class_root(node);
        if !seen.insert(root) {
            return shape;
        }
        let elements = self.class_array_items(node);
        let deepened = match (&shape, elements) {
            (LowShape::Tuple(items), Some(elements)) if items.len() == elements.len() => {
                LowShape::Tuple(
                    items
                        .iter()
                        .zip(elements)
                        .map(|(position, element)| {
                            // A compound's element is a slot: the value is its first position.
                            let element = self
                                .element_value_node(element.node)
                                .unwrap_or(element.node);
                            self.deepen_position(position.clone(), element, seen)
                        })
                        .collect(),
                )
            }
            (LowShape::Array(element, length), Some(elements)) if *length == elements.len() => {
                let element = elements
                    .iter()
                    .fold((**element).clone(), |accumulated, item| {
                        // A compound's element is a slot: the value is its first position.
                        let item = self.element_value_node(item.node).unwrap_or(item.node);
                        pick(accumulated, self.deepen_position(None, item, seen))
                    });
                LowShape::Array(Box::new(element), *length)
            }
            _ => shape,
        };
        seen.remove(&root);
        deepened
    }

    /// Write `value` onto `node`, then give its whole class that value.
    ///
    /// # Invariant
    /// A slot that holds a value is never given a different one, so the caller's
    /// contract is that `node` holds nothing — GC's relocation is the one licensed
    /// writer of an occupied slot. A value also enters a class once, so a `Some`
    /// requires the whole class to hold nothing. It always propagates, so one class
    /// holds one value; a `None` clears only the slot it was given.
    pub fn write_node_value(&mut self, node: NodeId, value: Option<P::Value>) {
        debug_assert!(
            self.nodes[node].value.is_none(),
            "a value a slot holds is never replaced by another: {node:?}"
        );
        debug_assert!(
            value.is_none() || self.nodes[node].value.is_none(),
            "a value enters a class once, so only a class holding nothing is written: {node:?}"
        );
        self.set_node_value(node, value);
        if let Some(value) = value {
            // A lone representative has nobody to distribute to; the slot write is all.
            if self.nodes[node].equality.parent().is_none()
                && self.nodes[node].equality.next().is_none()
            {
                return;
            }
            let representative = self.equality_representative(node);
            self.propagate_class_value(representative, value);
            self.observe_class_low_type(representative, value);
        }
    }

    /// The one value-slot assignment.
    ///
    /// # Invariant
    /// Every change to a [`Node::value`] slot goes through here, so the one writer of the
    /// slot is the one every caller's contract is stated against.
    pub(crate) fn set_node_value(&mut self, node: NodeId, value: Option<P::Value>) {
        self.nodes[node].value = value;
    }

    /// [`Self::write_node_value`] for a writer that must not claim an occupied slot.
    pub fn write_node_value_if_absent(&mut self, node: NodeId, value: Option<P::Value>) {
        if self.nodes[node].value.is_some() {
            return;
        }
        self.write_node_value(node, value);
    }

    /// GC's sole licence to rewrite an occupied slot.
    ///
    /// # Invariant
    /// Not a licence to change the value: compaction re-allocates the payload into the
    /// surviving block **from the same items**, so the value is *moved*, not replaced —
    /// a fact of the walk that got here, not one re-proved here. No propagation: the
    /// collect visits every class member itself, so each is relocated once.
    pub(crate) fn relocate_node_value(&mut self, node: NodeId, value: P::Value) {
        self.set_node_value(node, Some(value));
    }

    /// Commit an operation's answer, unifying it against what the class holds.
    ///
    /// # Invariant
    /// **Unify is the gate, and it is the only one.** An empty class takes the answer
    /// whole. A class holding a value is unified with the answer: one value and the
    /// answer is that class's own, so it fills this node's hole; a disagreement is
    /// recorded and nothing is written. A slot that holds a value is left alone either
    /// way, so an answer never overwrites one.
    pub(crate) fn write_node_answer(&mut self, node: NodeId, value: P::Value) {
        let mut path = AncestorPairs::new();
        let mut steps = Vec::new();
        let mut any_path = std::mem::take(&mut self.unify_any_path);
        any_path.clear();
        let held = self.nodes[node].value;
        let agrees = self.unify_inner(
            Side::value(Some(value)),
            Side::value(held),
            &mut path,
            0,
            &mut steps,
            &mut any_path,
            (node, node),
        );
        self.unify_any_path = any_path;
        if agrees {
            self.write_node_value_if_absent(node, Some(value));
        }
        self.stamp_node_ran(node);
    }

    /// Give a concrete `value` to **every** member of `representative`'s class.
    ///
    /// # Invariant
    /// Writing **every** member is what makes one class hold one value after a write. It
    /// is admitted only into a class that holds nothing — a value enters a class once,
    /// asserted by the caller that knows the class was empty *before* the write, which is
    /// why it is not re-checked here. A merge does not come through this path: it uses
    /// [`Module::fill_class_holes`].
    pub(crate) fn propagate_class_value(&mut self, representative: NodeId, value: P::Value) {
        // Walking the member list itself: a write leaves the topology alone, so no
        // snapshot is needed.
        let mut member = Some(representative);
        while let Some(current) = member {
            let node = &mut self.nodes[current];
            member = node.meta().next();
            node.value = Some(value);
        }
    }

    /// Record a value-only conflict at the enclosing unification's roots, with no steps.
    fn record_value_error(
        &mut self,
        root: (NodeId, NodeId),
        a: Option<P::Value>,
        b: Option<P::Value>,
    ) {
        self.unify_errors.push(UnifyError {
            root_a: root.0,
            root_b: root.1,
            steps: Vec::new(),
            a: root.0,
            b: root.1,
            value_a: a,
            value_b: b,
        });
    }

    /// Resolve a node-naming side to what its class knows; a node-less side already carries it.
    fn answer_class_side(&mut self, side: Side<P>) -> Side<P> {
        let Some(node) = side.node else {
            return side;
        };
        Side {
            node: Some(node),
            value: self.nodes[node].value,
        }
    }

    /// [`Self::unify`], reporting the range of [`Self::unify_errors`] this call produced.
    ///
    /// # Invariant
    /// `unify_errors` is append-only, so the returned range names exactly this call's
    /// failures and a caller may truncate its own range to suppress them.
    pub fn try_unify(&mut self, a: NodeId, b: NodeId) -> (NodeId, std::ops::Range<usize>) {
        let before = self.unify_errors.len();
        let representative = self.unify(a, b);
        (representative, before..self.unify_errors.len())
    }

    /// Structurally unify the classes of `a` and `b`; returns `a`'s class representative.
    pub fn unify(&mut self, a: NodeId, b: NodeId) -> NodeId {
        let mut path = std::mem::take(&mut self.unify_path);
        let mut steps = std::mem::take(&mut self.unify_steps);
        let mut any_path = std::mem::take(&mut self.unify_any_path);
        path.clear();
        steps.clear();
        any_path.clear();
        let representative = self.unify_borrowed(a, b, &mut path, &mut steps, &mut any_path);
        self.unify_path = path;
        self.unify_steps = steps;
        self.unify_any_path = any_path;
        representative
    }

    /// [`Self::unify`] over the module's own recursion buffers.
    fn unify_borrowed(
        &mut self,
        a: NodeId,
        b: NodeId,
        path: &mut AncestorPairs<NodeId>,
        steps: &mut Vec<UnifyStep>,
        any_path: &mut AncestorPairs<AnyNodeId>,
    ) -> NodeId {
        self.unify_inner(
            Side::node(a),
            Side::node(b),
            path,
            0,
            steps,
            any_path,
            (a, b),
        );
        disjoint::find(&mut self.nodes, a)
    }

    /// Whether `id` names a self-referential two-element array; a read, no path compression.
    pub fn is_self_referential(&self, id: AnyNodeId) -> bool {
        match id {
            Dyn(node) => {
                let rep = self.class_root(node);
                let Some(LowValue::Array(array)) =
                    self.nodes[node].value.and_then(|value| value.as_enum())
                else {
                    return false;
                };
                // SAFETY: `array` is the payload of `node`, a live node of
                // this module, so its home block has not been dropped.
                if unsafe { array.items() }.len() < 2 {
                    return false;
                }
                // SAFETY: as above — `node` is a live node of this module.
                for item in unsafe { array.items() } {
                    match item.node {
                        Dyn(item) => {
                            if self.class_root(item) == rep {
                                return true;
                            }
                        }
                        AnyNodeId::Static(_) => {
                            if self.is_self_referential(item.node) {
                                return true;
                            }
                        }
                    }
                }
                false
            }
            AnyNodeId::Static(sref) => self.is_static_universe_id(sref),
        }
    }

    fn is_static_universe_id(&self, sref: StaticNodeId) -> bool {
        let Some(LowValue::Array(array)) = self.static_read(sref).and_then(|value| value.as_enum())
        else {
            return false;
        };
        // SAFETY: `array` is a static payload read through `sref`; the registered module pins it.
        let items = unsafe { array.items() };
        items.len() >= 2
            && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
    }

    /// Whether two function values name one logical function, after resolving origins.
    pub fn function_identity_equal(&self, a: AnyFunctionId, b: AnyFunctionId) -> bool {
        self.function_identity(a) == self.function_identity(b)
    }

    /// The ultimate identity of `function` after following its origins. See docs/notes/function-type-merge.md.
    pub fn function_identity(&self, function: AnyFunctionId) -> FunctionIdentity {
        let mut current = function;
        for _ in 0..64 {
            match current {
                AnyFunctionId::Dynamic(id) => {
                    match self.functions.get(id).and_then(|f| f.static_origin) {
                        Some(origin) => current = AnyFunctionId::Static(origin),
                        None => return FunctionIdentity::Dynamic(id),
                    }
                }
                AnyFunctionId::Static(sref) => match self.static_function_origin(sref) {
                    Some(origin) => current = AnyFunctionId::Static(origin),
                    None => return FunctionIdentity::Static(sref),
                },
            }
        }
        match current {
            AnyFunctionId::Dynamic(id) => FunctionIdentity::Dynamic(id),
            AnyFunctionId::Static(sref) => FunctionIdentity::Static(sref),
        }
    }

    /// The real original a static function is a re-export of, or `None` when it
    /// is a module's own function.
    fn static_function_origin(&self, sref: StaticFunctionRef) -> Option<StaticFunctionRef> {
        self.static_module(sref.module)
            .functions
            .get(sref.index.0)
            .and_then(|function| function.origin)
    }

    /// Whether `node` names a function type — the self-referential `[Function(fid), ↺]`.
    pub fn is_function_type(&self, node: NodeId) -> bool {
        self.function_type_function(node).is_some()
    }

    /// The function a function-type node names; `None` otherwise. A read. See
    /// docs/notes/function-type-merge.md.
    pub fn function_type_function(&self, node: NodeId) -> Option<AnyFunctionId> {
        let carrier = self.class_root(node);
        if !self.is_self_referential(AnyNodeId::Dynamic(carrier)) {
            return None;
        }
        // SAFETY: `carrier` is a live node of this module; nothing here drops
        // a block.
        let items = (unsafe { self.array_items(carrier) })?;
        if items.len() < 2 {
            return None;
        }
        self.node_value(items[0].node)
            .and_then(|v| match v.as_enum()? {
                LowValue::Function(fid) => Some(fid),
                _ => None,
            })
    }

    /// The parameter pair and return type cell a function itself states, or `None`
    /// when the template states neither.
    ///
    /// # Invariant
    /// These are the template's own cells, so only a **clone** may descend them:
    /// a caller that descended the template would decide its signature by the
    /// first reader.
    fn function_cells(&mut self, function: AnyFunctionId) -> Option<(NodeId, NodeId)> {
        match function {
            AnyFunctionId::Dynamic(function) => {
                let (parameter, return_type) = {
                    let function = &self.functions[function];
                    (function.parameter, function.return_type)
                };
                // A hand-built function (a lowlevel test) may leave
                // `return_type` unset, so its signature is not readable.
                self.nodes
                    .contains_key(return_type)
                    .then_some((parameter, return_type))
            }
            // A frozen template is immutable, so its signature is copied into fresh leaves.
            AnyFunctionId::Static(sref) => self.materialize_static_signature(sref),
        }
    }

    /// The domain and codomain type cells of the function type `node` names; a read, never cloned.
    pub fn function_type_signature(&self, node: AnyNodeId) -> Option<(AnyNodeId, AnyNodeId)> {
        let function = match node {
            Dyn(node) => self.function_type_function(node)?,
            // A frozen function's own type is the same self-cycle in the artifact.
            AnyNodeId::Static(sref) => self.static_function_type_function(sref)?,
        };
        match function {
            AnyFunctionId::Dynamic(function) => {
                let function = self.functions.get(function)?;
                // A hand-built function (a lowlevel test) may leave
                // `return_type` unset, so its signature is not readable.
                if !self.nodes.contains_key(function.return_type) {
                    return None;
                }
                Some((
                    AnyNodeId::Dynamic(self.pair_type_half(function.parameter)?),
                    AnyNodeId::Dynamic(function.return_type),
                ))
            }
            AnyFunctionId::Static(sref) => self.static_function_signature(sref),
        }
    }

    /// The function a frozen function-type node names, the static mirror of the recogniser.
    fn static_function_type_function(&self, sref: StaticNodeId) -> Option<AnyFunctionId> {
        let value = self.static_read(sref)?;
        let LowValue::Array(array) = value.as_enum()? else {
            return None;
        };
        // SAFETY: a static payload read through `sref`; the registered module pins it.
        let items = unsafe { array.items() };
        if items.len() < 2 {
            return None;
        }
        // Asked by class, as the dynamic recogniser does: a frozen class may keep the cycle on a member.
        let static_module = self.static_module(sref.module);
        let representative = crate::static_module::static_find(&static_module.nodes, sref.index);
        let own_class = |node: AnyNodeId| match node {
            AnyNodeId::Static(tail) if tail.module == sref.module => {
                crate::static_module::static_find(&static_module.nodes, tail.index)
                    == representative
            }
            _ => false,
        };
        if !items.iter().any(|item| own_class(item.node)) {
            return None;
        }
        self.node_value(items[0].node)
            .and_then(|value| match value.as_enum()? {
                LowValue::Function(function) => Some(function),
                _ => None,
            })
    }

    /// How many canonical attributes the array's layout lists.
    ///
    /// # Invariant
    /// The layout is read one entry at a time rather than borrowed: the walk that
    /// needs it mutates the module, so a slice of the registry could not be held
    /// across it.
    fn layout_len(&self, array: &ArrayValue) -> usize {
        let layout = array.layout() as usize;
        match **array {
            AnyHandle::Dynamic(_) => self
                .attribute_layouts
                .get(layout)
                .map_or(0, |layout| layout.len()),
            AnyHandle::Static(handle) => self
                .static_module(handle.module)
                .attribute_layouts
                .get(layout)
                .map_or(0, |layout| layout.len()),
        }
    }

    /// The canonical index at `position` of the array's layout.
    fn layout_entry(&self, array: &ArrayValue, position: usize) -> Option<u32> {
        let layout = array.layout() as usize;
        match **array {
            AnyHandle::Dynamic(_) => self
                .attribute_layouts
                .get(layout)
                .and_then(|layout| layout.get(position))
                .copied(),
            AnyHandle::Static(handle) => self
                .static_module(handle.module)
                .attribute_layouts
                .get(layout)
                .and_then(|layout| layout.get(position))
                .copied(),
        }
    }

    /// Unify two structural arrays, wiring the attribute slots by **canonical index**.
    ///
    /// # Invariant
    /// Positions below [`PAIR_ATTR_BASE`] compare positionally; a side whose layout omits
    /// an attribute is a hole and constrains nothing, so two arrays that spell different
    /// tails still meet attribute to attribute. See `docs/notes/array-attribute-view.md`.
    #[stacksafe]
    fn unify_array_items(
        &mut self,
        a: ArrayValue,
        b: ArrayValue,
        path: &mut AncestorPairs<NodeId>,
        depth: usize,
        steps: &mut Vec<UnifyStep>,
        any_path: &mut AncestorPairs<AnyNodeId>,
        root: (NodeId, NodeId),
    ) -> bool {
        // One payload with itself is one value: nothing to descend; a frozen array
        // read through two paths is the same handle.
        if a == b {
            return true;
        }
        // SAFETY: both payloads come from live nodes of this module, so their home
        // blocks survive the recursion.
        let (left, right) = (unsafe { a.items() }, unsafe { b.items() });
        let plain = a.layout() == 0 && b.layout() == 0;
        if plain && left.len() != right.len() {
            return false;
        }
        let leading = if plain {
            left.len()
        } else {
            left.len().min(right.len()).min(PAIR_ATTR_BASE)
        };
        let mut ok = true;
        let mut descend =
            |this: &mut Self, index: usize, na: AnyNodeId, nb: AnyNodeId, ok: &mut bool| -> bool {
                // Two views of one frozen node are one value: nothing to compare, and
                // a frozen cycle reaches itself this way.
                if let (AnyNodeId::Static(left), AnyNodeId::Static(right)) = (na, nb)
                    && left == right
                {
                    return true;
                }
                // A pair already on the descent is one structure: the frame
                // comparing it owns its interior.
                let guarded =
                    matches!(na, AnyNodeId::Static(_)) || matches!(nb, AnyNodeId::Static(_));
                if guarded {
                    if any_path.contains(na, nb) {
                        return true;
                    }
                    any_path.insert(na, nb);
                }
                steps.push(UnifyStep {
                    index,
                    a: node_or_default(na),
                    b: node_or_default(nb),
                });
                let child_ok = this.unify_inner(
                    Side::of(na, this.node_value(na)),
                    Side::of(nb, this.node_value(nb)),
                    path,
                    depth + 1,
                    steps,
                    any_path,
                    root,
                );
                if guarded {
                    any_path.remove(na, nb);
                }
                steps.pop();
                if !child_ok {
                    *ok = false;
                }
                child_ok
            };
        for (index, (ia, ib)) in left.iter().zip(right.iter()).take(leading).enumerate() {
            descend(self, index, ia.node, ib.node, &mut ok);
            if !ok {
                return false;
            }
        }
        // The two layouts are sorted, so their union is merged in place: the walk
        // allocates nothing per array pair.
        let (mut position_a, mut position_b) = (0usize, 0usize);
        let (len_a, len_b) = (self.layout_len(&a), self.layout_len(&b));
        loop {
            let entry_a = (position_a < len_a)
                .then(|| self.layout_entry(&a, position_a))
                .flatten();
            let entry_b = (position_b < len_b)
                .then(|| self.layout_entry(&b, position_b))
                .flatten();
            let (canonical, from_a, from_b) = match (entry_a, entry_b) {
                (None, None) => break,
                (Some(entry), None) => (entry, true, false),
                (None, Some(entry)) => (entry, false, true),
                (Some(left), Some(right)) if left < right => (left, true, false),
                (Some(left), Some(right)) if right < left => (right, false, true),
                (Some(entry), Some(_)) => (entry, true, true),
            };
            let na = from_a
                .then(|| left.get(PAIR_ATTR_BASE + position_a).map(|item| item.node))
                .flatten();
            let nb = from_b
                .then(|| right.get(PAIR_ATTR_BASE + position_b).map(|item| item.node))
                .flatten();
            position_a += usize::from(from_a);
            position_b += usize::from(from_b);
            let (Some(na), Some(nb)) = (na, nb) else {
                continue;
            };
            descend(self, PAIR_ATTR_BASE + canonical as usize, na, nb, &mut ok);
            if !ok {
                return false;
            }
        }
        ok
    }

    /// Recursive core of [`Self::unify`]; `path` guards class pairs, `depth` the node-less ones.
    #[stacksafe]
    fn unify_inner(
        &mut self,
        a: Side<P>,
        b: Side<P>,
        path: &mut AncestorPairs<NodeId>,
        depth: usize,
        steps: &mut Vec<UnifyStep>,
        any_path: &mut AncestorPairs<AnyNodeId>,
        root: (NodeId, NodeId),
    ) -> bool {
        if depth >= MAX_VALUE_DEPTH {
            return true;
        }
        // A side with a node answers through its class; a node-less side by comparison.
        let a = self.answer_class_side(a);
        let b = self.answer_class_side(b);
        let va = a.value;
        let vb = b.value;
        // The wildcard writes nothing back, so a class holding `Any` is never bound.
        if is_wildcard(va) || is_wildcard(vb) {
            return true;
        }
        let (Some(ra), Some(rb)) = (a.node, b.node) else {
            // A node-less side carries the fact the other class is missing: a value against it is a write.
            match (a.node, b.node) {
                // Only when the class holds nothing; overwriting a decided class would hide a conflict.
                (Some(node), None) => {
                    if va.is_none()
                        && let Some(value) = vb
                    {
                        self.write_node_value(node, Some(value));
                        return true;
                    }
                }
                (None, Some(node)) => {
                    if vb.is_none()
                        && let Some(value) = va
                    {
                        self.write_node_value(node, Some(value));
                        return true;
                    }
                }
                _ => {}
            }
            let (Some(x), Some(y)) = (va, vb) else {
                // Two absences: a free cell is a wildcard, and a side with no value is an absence.
                return true;
            };
            return match (x.as_enum(), y.as_enum()) {
                (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                    self.unify_array_items(pa, pb, path, depth, steps, any_path, root)
                }
                // Two functions are one when their identities resolve to one
                // logical function — the frozen/static pair included.
                _ if self.value_pair_equal(x, y) => true,
                _ => {
                    self.record_value_error(root, Some(x), Some(y));
                    false
                }
            };
        };
        if ra == rb {
            return true;
        }
        // A pair already on the descent is one structure, not a disagreement.
        if path.contains(ra, rb) {
            return true;
        }
        // Ask each class's value once, of the class: the representative may be a valueless node.
        match (va, vb) {
            // Nothing known on either side: the merge is the whole answer.
            (None, None) => {
                self.add_equality(ra, rb);
                true
            }
            // One side knows a value: the merged class holds it; there is nothing to descend into.
            (Some(_), None) | (None, Some(_)) => {
                // The merge already gave every member the value the deciding side kept.
                self.add_equality(ra, rb);
                true
            }
            // Both sides know a value: they must be the same; for an array the elements decide it.
            (Some(x), Some(y)) => {
                match (x.as_enum(), y.as_enum()) {
                    (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                        // A plain pair of arrays still disagrees on arity; an array pair
                        // wires attributes and takes a hole for an absent one.
                        if pa.layout() == 0
                            && pb.layout() == 0
                            && unsafe { pa.items() }.len() != unsafe { pb.items() }.len()
                        {
                            self.record_error(ra, rb, steps, root);
                            return false;
                        }
                        path.insert(ra, rb);
                        let ok = self.unify_array_items(pa, pb, path, depth, steps, any_path, root);
                        path.remove(ra, rb);
                        if ok {
                            self.add_equality(ra, rb);
                        }
                        ok
                    }
                    // A function's own cells, as an array's elements are the array's:
                    // `[parameter, return]` decides it.
                    (Some(LowValue::Function(a)), Some(LowValue::Function(b))) => {
                        let (Some((parameter_a, return_a)), Some((parameter_b, return_b))) =
                            (self.function_cells(a), self.function_cells(b))
                        else {
                            // A template that states no signature states nothing: only
                            // the values themselves can decide.
                            return if self.function_identity_equal(a, b) {
                                self.add_equality(ra, rb);
                                true
                            } else {
                                self.record_error(ra, rb, steps, root);
                                false
                            };
                        };
                        path.insert(ra, rb);
                        let mut ok = true;
                        for (i, (na, nb)) in [(parameter_a, parameter_b), (return_a, return_b)]
                            .into_iter()
                            .enumerate()
                        {
                            steps.push(UnifyStep {
                                index: i,
                                a: na,
                                b: nb,
                            });
                            let child_ok = self.unify_inner(
                                Side::of(Dyn(na), self.node_value(Dyn(na))),
                                Side::of(Dyn(nb), self.node_value(Dyn(nb))),
                                path,
                                depth + 1,
                                steps,
                                any_path,
                                root,
                            );
                            steps.pop();
                            if !child_ok {
                                ok = false;
                                break;
                            }
                        }
                        path.remove(ra, rb);
                        if ok {
                            self.add_equality(ra, rb);
                        }
                        ok
                    }
                    // Fully equal by content ([`ValueExt::value_eq`]), not the cheap [`PartialEq`].
                    _ if x.value_eq(&y) => {
                        self.add_equality(ra, rb);
                        true
                    }
                    _ => {
                        self.record_error(ra, rb, steps, root);
                        false
                    }
                }
            }
        }
    }

    /// Whether two decided leaf values are one: functions by identity, else full value equality.
    fn value_pair_equal(&self, a: P::Value, b: P::Value) -> bool {
        match (a.as_enum(), b.as_enum()) {
            // Functions by resolved identity, before the generic comparison that reads them by kind.
            (Some(LowValue::Function(x)), Some(LowValue::Function(y))) => {
                self.function_identity_equal(x, y)
            }
            _ => a.value_eq(&b),
        }
    }

    /// `rep`'s class members, representative first; the union-find walk lives here once.
    pub(crate) fn class_members(&self, rep: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut member = Some(rep);
        std::iter::from_fn(move || {
            let current = member?;
            member = self.nodes[current].meta().next();
            Some(current)
        })
    }

    /// Join `reader` into `target`'s class and evaluate the target.
    ///
    /// # Invariant
    /// The join skips a wildcard target, which is shared: merging it would write its
    /// value into the reader. The reader keeps its operation, so the operand edge stays
    /// live for the clone walk; a failure is not reported here, because the reader's own
    /// reconcile owns that conflict.
    pub(crate) fn alias_read(&mut self, reader: NodeId, target: NodeId) {
        // A wildcard target answers no reader, so there is no class to join.
        let wildcard = self.nodes[target]
            .value
            .or_else(|| self.nodes[target].value);
        if matches!(
            wildcard.and_then(|value| value.as_enum()),
            Some(LowValue::Any)
        ) {
            return;
        }
        let (_, errors) = self.try_unify(reader, target);
        self.unify_errors.truncate(errors.start);
        let block = self.nodes[target].block;
        self.evaluate_node(Dyn(target), Some(block));
    }

    /// Record a conflict between two classes, with the descent that reached them.
    fn record_error(
        &mut self,
        ra: NodeId,
        rb: NodeId,
        steps: &[UnifyStep],
        root: (NodeId, NodeId),
    ) {
        self.unify_errors.push(UnifyError {
            root_a: root.0,
            root_b: root.1,
            steps: steps.to_vec(),
            a: ra,
            b: rb,
            value_a: self.nodes[ra].value,
            value_b: self.nodes[rb].value,
        });
    }
}

/// The dynamic node behind an [`AnyNodeId`], or [`NodeId::default()`] for a static ref.
fn node_or_default(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => NodeId::default(),
    }
}

/// The stated shape of two options: whichever is `Some`, and the invariant says a
/// second statement is the first.
fn pick(left: Option<LowShape>, right: Option<LowShape>) -> Option<LowShape> {
    match (left, right) {
        (Some(left), Some(right)) => {
            debug_assert_eq!(left, right, "one shape, stated twice");
            Some(left)
        }
        (Some(shape), None) | (None, Some(shape)) => Some(shape),
        (None, None) => None,
    }
}

/// Whether a side's class holds the wildcard `Any`.
fn is_wildcard<V: AsEnum<LowValue>>(value: Option<V>) -> bool {
    matches!(value.and_then(|value| value.as_enum()), Some(LowValue::Any))
}

/// The low type a concrete value's tag states, never reading the payload.
///
/// # Invariant
/// Observation states only a **scalar**: a shape with positions is the checker's to
/// state (its type is the authority), and a partial statement from here would
/// disagree with that seed. A class states one shape (the highlevel invariant).
fn observed_low_shape(value: impl AsEnum<LowValue>) -> Option<LowShape> {
    match value.as_enum()? {
        LowValue::USize(_) => Some(LowShape::USize),
        LowValue::Float(_) => Some(LowShape::Float),
        _ => None,
    }
}
