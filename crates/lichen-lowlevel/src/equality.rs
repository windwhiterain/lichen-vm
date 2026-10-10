use std::collections::HashSet;

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, FunctionIdentity, LowShape,
    LowValue, Module, Node, NodeId, Program, StaticFunctionRef, StaticNodeId, ValueExt as _,
    ancestors::AncestorPairs,
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
    /// Merge the classes of `a` and `b`, then fill the members that hold nothing.
    ///
    /// # Invariant
    /// Both sides' low types and decided values are read before the union, which
    /// re-elects a representative; the merge joins the low types and fills only the
    /// members that hold nothing, overwriting nobody — each member keeps the value it
    /// holds. See docs/notes/lowlevel-vm.md.
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        let left = self.class_low_type(a).cloned();
        let right = self.class_low_type(b).cloned();
        let left_value = self.class_committed_value(a);
        let right_value = self.class_committed_value(b);
        let representative = disjoint::union(&mut self.nodes, a, b);
        for shape in [left, right].into_iter().flatten() {
            self.refine_class_low_type(representative, shape);
        }
        if let Some(value) = left_value.or(right_value) {
            self.fill_class_holes(representative, value);
        }
        representative
    }

    pub fn equality_representative(&mut self, node: NodeId) -> NodeId {
        disjoint::find(&mut self.nodes, node)
    }

    /// The class's value through its representative; a read, so no path compression.
    pub fn class_value(&self, node: NodeId) -> Option<P::Value> {
        let mut root = node;
        while let Some(parent) = self.nodes[root].equality.parent() {
            root = parent;
        }
        self.nodes[root].value
    }

    /// The class's **low type** through its representative. See docs/notes/lowlevel-low-types.md.
    pub fn class_low_type(&self, node: NodeId) -> Option<&LowShape> {
        self.nodes.get(self.class_root(node))?.low_shape.as_ref()
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
        let LowValue::Array(array) = self.class_value(node)?.as_enum()? else {
            return None;
        };
        // SAFETY: the slice points into the value's home arena, alive for this borrow.
        Some(unsafe { array.items() })
    }

    /// Join `shape` into `node`'s class low type: the single monotone write side.
    pub fn refine_class_low_type(&mut self, node: NodeId, shape: LowShape) -> bool {
        let Some(entry) = self.nodes.get_mut(self.class_root(node)) else {
            return false;
        };
        match &mut entry.low_shape {
            None => {
                entry.low_shape = Some(shape);
                true
            }
            Some(current) => {
                let joined = LowShape::join(current, &shape);
                let changed = joined != *current;
                *current = joined;
                changed
            }
        }
    }

    /// Refine `node`'s class low type from a concrete value's top-level tag, O(1) and monotone.
    pub(crate) fn observe_class_low_type(&mut self, node: NodeId, value: P::Value) {
        let Some(shape) = observed_low_shape(value) else {
            return;
        };
        self.refine_class_low_type(node, shape);
    }

    /// One structural element: an undecided position takes the element's low type, a decided one keeps it.
    fn deepen_position(
        &self,
        position: LowShape,
        element: AnyNodeId,
        seen: &mut HashSet<NodeId>,
    ) -> LowShape {
        if position.is_known() {
            return position;
        }
        let AnyNodeId::Dynamic(element) = element else {
            return LowShape::Unknown;
        };
        let Some(shape) = self.class_low_type(element).cloned() else {
            return LowShape::Unknown;
        };
        self.deepen_low_type(element, shape, seen)
    }

    /// [`Module::low_type_of_node`] at one level; `seen` cuts a self-referential cycle.
    fn deepen_low_type(
        &self,
        node: NodeId,
        shape: LowShape,
        seen: &mut HashSet<NodeId>,
    ) -> LowShape {
        if !shape.is_known() {
            return shape;
        }
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
                            self.deepen_position(position.clone(), element.node, seen)
                        })
                        .collect(),
                )
            }
            (LowShape::Array(element, length), Some(elements)) if *length == elements.len() => {
                let element = elements
                    .iter()
                    .fold((**element).clone(), |accumulated, item| {
                        LowShape::join(
                            &accumulated,
                            &self.deepen_position(LowShape::Unknown, item.node, seen),
                        )
                    });
                LowShape::Array(Box::new(element), *length)
            }
            _ => shape,
        };
        seen.remove(&root);
        deepened
    }

    /// Write `value` onto `node`, then propagate a concrete value to its whole class.
    ///
    /// # Invariant
    /// The write is unconditional and reaches every member, so one class has one
    /// value; a `None` clears only the node's own slot — undecided is not a fact to
    /// propagate. This is the single value-write choke-point and one of the class's
    /// two low-type observation sites. See docs/notes/lowlevel-vm.md.
    pub fn write_node_value(&mut self, node: NodeId, value: Option<P::Value>) {
        self.nodes[node].value = value;
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

    /// Commit an operation's answer, then restore the node's own slot so its run state survives.
    ///
    /// # Invariant
    /// The class's value is distributed, but the node's own slot keeps the operator's
    /// answer: a propagated class value must never masquerade as a produced answer, or
    /// the operator that owed one would never run again ([`Module::has_no_result_yet`]).
    /// The answer meets the class as a node-less side, so a conflict is the ordinary
    /// unification conflict. See docs/notes/lowlevel-vm.md.
    pub(crate) fn write_node_answer(&mut self, node: NodeId, value: P::Value) {
        let mut path = AncestorPairs::new();
        let mut steps = Vec::new();
        let held = self.class_committed_value(node);
        self.unify_inner(
            Side::value(Some(value)),
            Side::value(held),
            &mut path,
            0,
            &mut steps,
            (node, node),
        );
        self.write_node_value(node, Some(value));
        self.nodes[node].value = Some(value);
        self.nodes[node].runned = true;
    }

    /// Distribute a concrete `value` over the whole class of `representative`.
    ///
    /// # Invariant
    /// Undecided is not a fact to propagate, so only a decided value reaches here. An
    /// operation-bearing member keeps its computation: its slot holds the class's value
    /// while `runned` stays false, which [`Module::has_no_result_yet`] reads as the
    /// operator still owing its own answer. A merge does not use this path — it fills
    /// only the holes. See docs/notes/lowlevel-vm.md.
    pub(crate) fn propagate_class_value(&mut self, representative: NodeId, value: P::Value) {
        let members: Vec<NodeId> = self.class_members(representative).collect();
        for member in members {
            self.nodes[member].value = Some(value);
        }
    }

    /// Fill the class's members that hold nothing; a member that holds something keeps it.
    fn fill_class_holes(&mut self, representative: NodeId, value: P::Value) {
        let members: Vec<NodeId> = self.class_members(representative).collect();
        for member in members {
            if self.nodes[member].value.is_none() {
                self.nodes[member].value = Some(value);
            }
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
            value: self.class_committed_value(node),
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
        let mut path = AncestorPairs::new();
        let mut steps = Vec::new();
        self.unify_inner(
            Side::node(a),
            Side::node(b),
            &mut path,
            0,
            &mut steps,
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
                if unsafe { array.items() }.len() != 2 {
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
        items.len() == 2
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
    fn function_type_function(&self, node: NodeId) -> Option<AnyFunctionId> {
        let carrier = self.class_root(node);
        if !self.is_self_referential(AnyNodeId::Dynamic(carrier)) {
            return None;
        }
        // SAFETY: `carrier` is a live node of this module; nothing here drops
        // a block.
        let items = (unsafe { self.array_items(carrier) })?;
        if items.len() != 2 {
            return None;
        }
        self.node_value(items[0].node)
            .and_then(|v| match v.as_enum()? {
                LowValue::Function(fid) => Some(fid),
                _ => None,
            })
    }

    /// The parameter pair and return type cell a function's type is, or `None` when it is not one.
    fn function_signature(&mut self, node: NodeId) -> Option<(NodeId, NodeId)> {
        let (parameter, return_type) = match self.function_type_function(node)? {
            AnyFunctionId::Dynamic(function) => {
                let (parameter, return_type) = {
                    let function = &self.functions[function];
                    (function.parameter, function.return_type)
                };
                // A hand-built function (a lowlevel test) may leave
                // `return_type` unset, so its signature is not readable.
                if !self.nodes.contains_key(return_type) {
                    return None;
                }
                (parameter, return_type)
            }
            // A frozen template is immutable, so its signature is copied into fresh leaves.
            AnyFunctionId::Static(sref) => return self.materialize_static_signature(sref),
        };
        Some((parameter, return_type))
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
        if items.len() != 2 {
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

    /// Unify two function types by descending their signature cells. See docs/notes/function-type-merge.md.
    fn unify_function_types(
        &mut self,
        ra: NodeId,
        rb: NodeId,
        signature_a: (NodeId, NodeId),
        signature_b: (NodeId, NodeId),
        steps: &mut Vec<UnifyStep>,
        root: (NodeId, NodeId),
    ) -> bool {
        let pre = self.unify_errors.len();
        self.unify(signature_a.0, signature_b.0);
        self.unify(signature_a.1, signature_b.1);
        if self.unify_errors.len() > pre {
            self.record_error(ra, rb, steps, root);
            return false;
        }
        // The elements agree, so the types are one; no value is written back — both carry one.
        self.add_equality(ra, rb);
        true
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
                    // SAFETY: both are payloads of values read out of live nodes of this
                    // module, so their home blocks have not been dropped.
                    let (left, right) = (unsafe { pa.items() }, unsafe { pb.items() });
                    left.len() == right.len()
                        && left.iter().zip(right.iter()).all(|(ia, ib)| {
                            self.unify_inner(
                                Side::of(ia.node, self.node_value(ia.node)),
                                Side::of(ib.node, self.node_value(ib.node)),
                                path,
                                depth + 1,
                                steps,
                                root,
                            )
                        })
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
        if path.contains(ra, rb) {
            self.record_error(ra, rb, steps, root);
            return false;
        }
        // Ask each class's value once, of the class: the representative may be a valueless node.
        match (va, vb) {
            // Nothing known on either side: the merge is the whole answer.
            (None, None) => {
                self.add_equality(ra, rb);
                true
            }
            // One side knows a value: the merged class holds it; there is nothing to descend into.
            (Some(value), None) | (None, Some(value)) => {
                let rep = self.add_equality(ra, rb);
                self.write_node_value(rep, Some(value));
                true
            }
            // Both sides know a value: they must be the same; for an array the elements decide it.
            (Some(x), Some(y)) => {
                // Two function types descend into their own cells, not into the self-cycle shape.
                let function_a = self.function_type_function(ra);
                let function_b = self.function_type_function(rb);
                if function_a.is_some() || function_b.is_some() {
                    if let (Some(signature_a), Some(signature_b)) =
                        (self.function_signature(ra), self.function_signature(rb))
                    {
                        return self.unify_function_types(
                            ra,
                            rb,
                            signature_a,
                            signature_b,
                            steps,
                            root,
                        );
                    }
                    // Exactly one function type against a self-cycle: refuse, or the match merges them.
                    let other = if function_a.is_some() { rb } else { ra };
                    if self.is_self_referential(AnyNodeId::Dynamic(other)) {
                        self.record_error(ra, rb, steps, root);
                        return false;
                    }
                }
                match (x.as_enum(), y.as_enum()) {
                    (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                        // SAFETY: both payloads come from live nodes of this module, so their home
                        // blocks survive the recursion.
                        let (left, right) = (unsafe { pa.items() }, unsafe { pb.items() });
                        if left.len() != right.len() {
                            self.record_error(ra, rb, steps, root);
                            return false;
                        }
                        // Two self-referential universes are one value, even from a static module.
                        if self.is_self_referential(Dyn(ra)) && self.is_self_referential(Dyn(rb)) {
                            self.add_equality(ra, rb);
                            return true;
                        }
                        path.insert(ra, rb);
                        let mut ok = true;
                        for (i, (na, nb)) in left.iter().zip(right.iter()).enumerate() {
                            // Record the descent step before recursing, so the
                            // deep failure's trace carries the full element path.
                            steps.push(UnifyStep {
                                index: i,
                                a: node_or_default(na.node),
                                b: node_or_default(nb.node),
                            });
                            let child_ok = self.unify_inner(
                                Side::of(na.node, self.node_value(na.node)),
                                Side::of(nb.node, self.node_value(nb.node)),
                                path,
                                depth + 1,
                                steps,
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
                    // A materialized static closure and its frozen function name one logical function.
                    (Some(LowValue::Function(a)), Some(LowValue::Function(b)))
                        if self.function_identity_equal(a, b) =>
                    {
                        self.add_equality(ra, rb);
                        true
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
    fn class_members(&self, rep: NodeId) -> impl Iterator<Item = NodeId> + '_ {
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
    /// The join is unconditional, and the reader keeps its operation so the operand
    /// edge stays live for the apply clone walk. It does not report its failures:
    /// the reader's own reconcile owns that conflict, so recording here would report
    /// one disagreement twice. See docs/notes/lowlevel-vm.md.
    pub(crate) fn alias_read(&mut self, reader: NodeId, target: NodeId) {
        let (_, errors) = self.try_unify(reader, target);
        self.unify_errors.truncate(errors.start);
        let block = self.nodes[target].block;
        self.evaluate_node(Dyn(target), Some(block));
    }

    /// The concrete value `rep`'s class has committed, or `None`. See docs/notes/lowlevel-vm.md.
    pub(crate) fn class_committed_value(&self, rep: NodeId) -> Option<P::Value> {
        self.class_value(rep)
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

/// The low type a concrete value's tag states, never reading the payload; payload starts `Unknown`.
fn observed_low_shape(value: impl AsEnum<LowValue>) -> Option<LowShape> {
    match value.as_enum()? {
        LowValue::USize(_) => Some(LowShape::USize),
        LowValue::Float(_) => Some(LowShape::Float),
        LowValue::Array(array) => Some(LowShape::Array(
            Box::new(LowShape::Unknown),
            // SAFETY: only the length is read, and the caller holds the value on this borrow of the module.
            unsafe { array.items().len() },
        )),
        LowValue::Table(_) => Some(LowShape::Table(
            Box::new(LowShape::Unknown),
            Box::new(LowShape::Unknown),
        )),
        LowValue::Function(_) => Some(LowShape::Function(
            Box::new(LowShape::Unknown),
            Box::new(LowShape::Unknown),
        )),
        _ => None,
    }
}
