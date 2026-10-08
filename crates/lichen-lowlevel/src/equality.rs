use std::collections::HashSet;

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, FunctionIdentity, LowShape,
    LowValue, Module, Node, NodeId, Program, StaticFunctionRef, StaticNodeId, ValueExt as _,
    ancestors::AncestorPairs,
};
use lichen_utils::disjoint::{self, Node as _};
use lichen_utils::extend::AsEnum;

/// One step of a failed unification's descent: the array element the
/// traversal moved into, and the two child operands it compared there.  The
/// flat sequence of steps (in order) is the structural *path* from the
/// unified root operands down to the failing pair — the "step by step" the
/// highlevel needs to pinpoint which component of a compound type is the
/// actual conflict.  A static child resolves to [`NodeId::default()`] (it
/// materializes into a fresh leaf), so only `index` is authoritative; the
/// highlevel re-reads the path through the graph when it needs positions.
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
    /// The two top-level operands the trigger framed — the checker's
    /// source-meaningful sides for a checker-issued unify, the cloned
    /// parameter and argument for an apply-time parameter check.  These are
    /// what the highlevel attributes the diagnostic to; the raw `a`/`b`
    /// leaves below are the deep conflict.
    pub root_a: NodeId,
    pub root_b: NodeId,
    /// The descent path (element by element) from `root_a`/`root_b` to the
    /// conflict, per [`UnifyStep`].  Empty for a top-level (non-array) clash.
    pub steps: Vec<UnifyStep>,
    /// The conflicting classes, as the lowlevel recorded them.
    pub a: NodeId,
    pub b: NodeId,
    pub value_a: Option<P::Value>,
    pub value_b: Option<P::Value>,
}

/// A side of a unification: a node's class, a bare value, or both.
///
/// A side **without** a node is a value that has no class to merge — the result
/// an operation just produced, or a value a write is distributing.  Asking
/// `unify` of such a side is asking whether the two can be one value, which is
/// the same question [`Module::unify_inner`]'s arms answer for two classes: the
/// absence of a class is not a variant of the recursion, it is a side with
/// nothing to merge.
#[derive(Clone, Copy)]
struct Side<P: Program> {
    node: Option<NodeId>,
    value: Option<P::Value>,
}

/// The depth bound for a comparison reached without classes: such a pair cannot
/// name itself, so its descent is bounded instead of cycle-guarded.  Past the
/// bound a pair is given the benefit of the doubt, as the unifier's cycle guard
/// does.
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

    /// The side an array item names: its node when it has one, else its value —
    /// a static ref has no dynamic node to merge, so it takes part as the value
    /// it is.
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
    /// Merge the classes of `a` and `b`, carrying the merged class's decided
    /// value to the members that **hold nothing**.
    ///
    /// The two low-type reads at the top are the shape half; the value half is
    /// the class-channel invariant ([`Self::write_node_value`]) applied to the
    /// merge that **grows** a class rather than to the write that fills one.
    /// Both sides are read **before** the union, which re-elects a
    /// representative: what either side already knew has to be read off the
    /// class it was on, not off a node that may stop being the one a class read
    /// reaches.  A merge that read only the two representatives' slots would
    /// see no value and carry nothing, leaving the cell that just joined the
    /// class undecided while the class has been decided all along — a read that
    /// happened before the unification which decided the class, with nothing to
    /// wake it afterwards (`docs/notes/eval-before-unify.md` §2.1).
    ///
    /// **The distribution reaches the members that hold nothing, and only
    /// those.**  A merge is the one place two decided sides meet without their
    /// values being compared for equality: the positional descent agrees they
    /// *unify*, which is not the same as their being one value — two type terms
    /// that unify still name different nodes — so a class may hold two values,
    /// on two members, and each member's slot keeps the one it holds.  A merge
    /// that overwrote them would move a node out of a slot the structure still
    /// names, and a later walk descending through that structure would silently
    /// take the other branch; that is how a parameter's pinned dependent length
    /// became an undecided cell, and why this is a rule and not a tolerance
    /// (`docs/notes/class-channel.md` §1.1.3).
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        // Both sides' low types are read *before* the union (which leaves the
        // authoritative copy on whichever node becomes the representative) and
        // joined onto it after, so a merge never drops a decided shape.
        let left = self.class_low_type(a).cloned();
        let right = self.class_low_type(b).cloned();
        // The same argument for the value, and for the same reason: the union
        // re-elects the representative, so what either side already knew has to
        // be read **before** it and committed to the winner afterwards.  An
        // **undecided** class (an empty slot) is not a fact to carry, exactly as
        // [`Self::write_node_value`] treats it.  Both sides decided is not a
        // conflict here — `unify_inner`'s arms decide that, and only agree to
        // merge two decided sides.
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

    /// The class's value, read through its representative — the value the
    /// unification machinery sees (`bind`/`unify` read the representative's
    /// slot).  `&self`, no path compression, so a read never mutates the
    /// union-find tree.
    pub fn class_value(&self, node: NodeId) -> Option<P::Value> {
        let mut root = node;
        while let Some(parent) = self.nodes[root].equality.parent() {
            root = parent;
        }
        self.nodes[root].value
    }

    /// The class's **low type**, read through its representative — the lower
    /// bound a backend compiles against.  `&self`, walked the same way
    /// [`Self::class_value`] is, so a read never mutates the tree and never
    /// depends on which member of the class resolved first.
    ///
    /// `None` is an untraced class (scaffolding a backend never reaches);
    /// `Some(LowShape::Unknown)` is a traced but undecided one.
    pub fn class_low_type(&self, node: NodeId) -> Option<&LowShape> {
        self.nodes.get(self.class_root(node))?.low_shape.as_ref()
    }

    /// The class's recursive low type: [`Self::class_low_type`] with every
    /// still-undecided position refined from the element classes the value
    /// structurally covers.  Deep shapes are never unfolded at write time — a
    /// read recurses, because the element classes refine independently, as
    /// they bind, of the array that holds them.
    ///
    /// A value with no decided shape reads back unchanged (`None` or
    /// `Some(Unknown)`), and a self-referential structure is cut by the path
    /// guard, so a cyclic value cannot recurse forever.
    pub fn low_type_of_node(&self, node: NodeId) -> Option<LowShape> {
        let shape = self.class_low_type(node)?.clone();
        let mut seen = HashSet::new();
        Some(self.deepen_low_type(node, shape, &mut seen))
    }

    /// Record `shape` as a lower bound on `node`'s class — the **seed** of the
    /// computation route, the one thing the value graph can never decide for a
    /// template (the parameter positions: a template is never evaluated, and an
    /// apply binds the *clones*, not it).  A layer above the lowlevel that has
    /// the type calls this before running the fixed-point pass; a seed is an
    /// ordinary refinement, never a widening.
    ///
    /// Returns whether the class's low type changed.
    pub fn seed_class_low_type(&mut self, node: NodeId, shape: LowShape) -> bool {
        self.refine_class_low_type(node, shape)
    }

    /// The union-find representative of `node` — the `&self`, no-compression
    /// form of [`Self::equality_representative`], so the class-routed reads
    /// stay read-only.  **Public** because the control-flow graph resolves a
    /// value's definition through the class too, and two walks of one union-find
    /// is one walk too many.
    pub fn class_root(&self, node: NodeId) -> NodeId {
        let mut root = node;
        while let Some(parent) = self.nodes[root].equality.parent() {
            root = parent;
        }
        root
    }

    /// The array items of `node`'s class value, read through the
    /// representative — the structural descent every deep low-type read walks.
    fn class_array_items(&self, node: NodeId) -> Option<&'static [ArrayItem]> {
        let LowValue::Array(array) = self.class_value(node)?.as_enum()? else {
            return None;
        };
        // SAFETY: the slice points into the array payload's home arena, and the
        // node is read out of `self.nodes` here, so its home block is alive for
        // as long as this borrow of the module can reach it.
        Some(unsafe { array.items() })
    }

    /// Refine `node`'s class low type with `shape` (the lattice join) and
    /// report whether the class moved.  The single write side of the low-type
    /// channel: observation, the merge join, a seed, and the fixed-point pass
    /// all route through it, so refinement is monotone by construction.
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

    /// Refine `node`'s class low type from a concrete value's variant tag.
    ///
    /// O(1) and monotone by construction: only the top-level tag is read, never
    /// the payload.  A value the vocabulary has no shape for (`Str`, the unit
    /// `None`, an empty `Error`) states nothing, so it never widens
    /// and never narrows; a value that refines nothing leaves the class
    /// untouched.  Called from both value-write sites — [`Self::write_node_value`]
    /// and [`Module::add_node`].
    pub(crate) fn observe_class_low_type(&mut self, node: NodeId, value: P::Value) {
        let Some(shape) = observed_low_shape(value) else {
            return;
        };
        self.refine_class_low_type(node, shape);
    }

    /// [`Self::deepen_low_type`] for one structural element: an undecided
    /// position takes the element's own (deep) low type, and a decided one
    /// keeps it — the two descriptions of a decided position cannot disagree,
    /// and the position was decided first.
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

    /// [`Module::low_type_of_node`] at one level, with `seen` cutting the
    /// cycle of a self-referential structure.
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

    /// The controlled value-write API: write `value` onto `node` — its own
    /// answer — then, if the value is concrete, propagate it to the rest of
    /// `node`'s class, so a read of any member (or a later `bind`/`unify`,
    /// which reads through the class) sees it regardless of which member
    /// resolved it.
    ///
    /// This is the single choke-point for value writes: every place a value
    /// lands on a node that might be a member of a unified class goes through
    /// here, so the class-consistency invariant is maintained at exactly one
    /// site.  A `None` value only clears the node's own slot — undecided is not
    /// a fact to propagate.
    ///
    /// **The write is unconditional, and it lands on every member**
    /// ([`docs/notes/class-channel.md`] §1.1).  It is not gated on the slot
    /// being undecided, and it skips nobody — so **one class has one value**: a
    /// concrete write reaches every member *and* the class's representative,
    /// which is where [`Self::class_value`] and [`Self::class_committed_value`]
    /// read it from.  The representative is the class's single value slot, so
    /// no reader has to know which member a write happened to start from.
    ///
    /// It is also one of the two **observation** sites of the low-type layer:
    /// a concrete value refines its class's low type from the value's variant
    /// tag ([`Self::observe_class_low_type`]).  The other is
    /// [`Module::add_node`], where a value arrives already concrete.
    pub fn write_node_value(&mut self, node: NodeId, value: Option<P::Value>) {
        self.nodes[node].value = value;
        if let Some(value) = value {
            // A class whose sole member is the node — `parent` and `next` both
            // `None` is `disjoint::Meta`'s contract for a lone representative —
            // holds nobody to distribute to, so the slot write above is the
            // whole effect.
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

    /// Commit an **operation's answer** — the evaluator's write.
    ///
    /// The answer is a **value**: it has no class of its own, so it meets the
    /// node through the one unification as a node-less side ([`Side::value`]) —
    /// which is exactly the question "can this answer be the value my class
    /// holds", asked *of* the recursion rather than beside it.  The conflict is
    /// the ordinary unification conflict, recorded at this node's roots, so
    /// nothing needs a comparison path of its own.
    ///
    /// **The operator always runs, and this is where its own answer is kept.**
    /// The class's value is distributed to the members, but *not* over the
    /// producing operation's own slot: that slot is the node-local run state —
    /// "this operator produced this" — which is what
    /// [`Module::has_no_result_yet`] reads and what stops a second run.  A
    /// propagated class value must never masquerade as a produced answer, or the
    /// operator that owed one would never run again.
    pub(crate) fn write_node_answer(&mut self, node: NodeId, value: P::Value) {
        let mut path = AncestorPairs::new();
        let mut steps = Vec::new();
        // The answer against what the node's class holds, as two values: the
        // answer has no class of its own, and pulling the class's value out
        // explicitly is what makes the two comparable.
        let held = self.class_committed_value(node);
        self.unify_inner(
            Side::value(Some(value)),
            Side::value(held),
            &mut path,
            0,
            &mut steps,
            (node, node),
        );
        // Distribute to the class, then restore this node's own answer: the walk
        // visits every member, this one included, and a class value landing here
        // would erase the run state the slot carries.
        self.write_node_value(node, Some(value));
        self.nodes[node].value = Some(value);
        self.nodes[node].runned = true;
    }

    /// Distribute a concrete `value` over the class of `representative` — the
    /// distribution half of [`Self::write_node_value`], and **only** that: a
    /// write states the class's value, so it reaches every member.  A merge
    /// does not distribute this way; it fills the members that hold nothing
    /// ([`Self::add_equality`]), because a merge is where two decided sides
    /// meet and neither may be overwritten.
    ///
    /// **Undecided is not a fact to propagate** — there is no such value to
    /// pass here: a class states nothing about its members until it holds a
    /// decided value, and no caller has an undecided `P::Value` to hand over.
    ///
    /// An operation-bearing member keeps its computation: its slot now holds the
    /// class's value while `runned` stays `false`, which is what
    /// [`Module::has_no_result_yet`] reads as "an assertion, so the operator
    /// still owes its own answer".
    pub(crate) fn propagate_class_value(&mut self, representative: NodeId, value: P::Value) {
        let members: Vec<NodeId> = self.class_members(representative).collect();
        for member in members {
            self.nodes[member].value = Some(value);
        }
    }

    /// Fill the class's members that hold nothing with `value` — the merge's
    /// half of the distribution, against [`Self::propagate_class_value`]'s
    /// write.  A member that already holds something **keeps it**: a merge is
    /// where two decided sides meet, and neither may be overwritten
    /// ([`Self::add_equality`], `docs/notes/class-channel.md` §1.1.3).
    fn fill_class_holes(&mut self, representative: NodeId, value: P::Value) {
        let members: Vec<NodeId> = self.class_members(representative).collect();
        for member in members {
            if self.nodes[member].value.is_none() {
                self.nodes[member].value = Some(value);
            }
        }
    }

    /// The write rule's own conflict record for a **value-only** pair: neither
    /// side is a class, so there is no pair of roots to name, and the failure is
    /// attributed to the enclosing unification's roots with the two values that
    /// could not be one.
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

    /// Resolve a side that names a node down to **what its class knows** — the
    /// ordinary reading, because a side that names a node stands for that node's
    /// whole class.  A node-less side already carries its own answer.
    fn answer_class_side(&mut self, side: Side<P>) -> Side<P> {
        let Some(node) = side.node else {
            return side;
        };
        Side {
            node: Some(node),
            value: self.class_committed_value(node),
        }
    }

    /// [`Self::unify`], reporting the range of [`Self::unify_errors`] this
    /// call produced — **empty on success**.  The range is this call's own
    /// failures, so a caller never has to infer ownership from a length
    /// delta taken around the call.
    ///
    /// Invariant: `unify_errors` is append-only, so the range `before..len`
    /// names exactly the entries this call appended and stays valid for as
    /// long as nothing truncates the vec — which is what makes it safe for a
    /// caller to suppress *its own* failures by [`Vec::truncate`].
    pub fn try_unify(&mut self, a: NodeId, b: NodeId) -> (NodeId, std::ops::Range<usize>) {
        let before = self.unify_errors.len();
        let representative = self.unify(a, b);
        (representative, before..self.unify_errors.len())
    }

    /// Structurally unify the classes of `a` and `b`.
    ///
    /// Unification is over values: a class holding no value and no pending
    /// operation (a pure undecided cell) binds to the other side's value; an
    /// unevaluated operation is a *pending computation*, and a concrete
    /// value must never be bound over one — that would silently erase it
    /// (e.g. a dependent type branch that selects a different type per
    /// argument).  Such a computation is forced before comparing; if its
    /// operands are still undecided and it cannot resolve, the unify fails —
    /// except against an all-undecided skeleton (cells and arrays of cells),
    /// which merges with the computation: nothing is erased, and the
    /// computation's eventual value replicates onto the skeleton.  Two
    /// concrete values merge iff they are fully equal
    /// ([`ValueExt::value_eq`]), except arrays, which unify elementwise
    /// (their structure is the value).  A
    /// conflict records a [`UnifyError`] in [`Self::unify_errors`] and
    /// leaves the two classes unmerged.
    ///
    /// Returns the representative of the merged class on success, or of
    /// `a`'s class when unification fails.
    pub fn unify(&mut self, a: NodeId, b: NodeId) -> NodeId {
        let mut path = AncestorPairs::new();

        // The descent path, seeded empty; the root operands are carried
        // separately and recorded in each `UnifyError`'s `root_a`/`root_b`.
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

    /// Whether `id` names a **self-referential two-element array** — a
    /// 2-element array one of whose elements points back at its own class
    /// (dynamically through the class, or statically through the frozen
    /// self-loop).  This is a generic graph shape: it says "this value is a
    /// cycle of length one", not what the cycle *means*.  The highlevel's
    /// universe `K = [Type, ↺]` is the canonical instance, but a program
    /// that recognises its own cycles by meaning (comparing against its
    /// canonical node) is free to, and does so in `lichen-highlevel`'s
    /// `shape` module.  The lowlevel needs the shape alone to unify two such
    /// cycles successfully instead of tripping its cycle guard.
    ///
    /// A **read**: the class is walked without path compression
    /// ([`Self::class_root`]), so asking the question never mutates the
    /// union-find tree and a caller holding a shared borrow can ask it.
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
        // SAFETY: `array` is a static payload read through `sref`, whose home
        // module is registered — the registration pins its arena.
        let items = unsafe { array.items() };
        items.len() == 2
            && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
    }

    /// Whether two function values name **one logical function**, after
    /// resolving re-export/materialization origins — see
    /// [`Module::function_identity`].
    ///
    /// The lowlevel's `Function` identity is by id, and a function can be named
    /// through several refs (a dynamic closure and the frozen function it was
    /// materialized from, or two modules' re-exports of one imported binding),
    /// so unifying values that name it through different refs must merge, not
    /// conflict.
    pub fn function_identity_equal(&self, a: AnyFunctionId, b: AnyFunctionId) -> bool {
        self.function_identity(a) == self.function_identity(b)
    }

    /// The **ultimate identity** of `function`: follow its re-export /
    /// materialization origins to the one real function it is.  A dynamic
    /// closure points at the static function it was materialized from
    /// ([`Function::static_origin`]); a re-exported static function points at
    /// the module that first built it ([`StaticFunction::origin`]).  The chain
    /// terminates at a source-built dynamic function or a module's own static
    /// function, which is the identity.
    ///
    /// The bound stops a corrupt origin cycle from looping; a real chain is at
    /// most one re-export deep per importing module.
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

    /// Whether `node` names a **function type** — the self-referential
    /// `[Function(fid), ↺]` that *is* a function's own type (`f : f`), or a
    /// frozen module's copy of one.  Recognition only: it allocates nothing,
    /// so a caller may ask "is this a function?" as often as it likes.
    ///
    /// Public because "is this a function type" is the one question a layer
    /// above asks — the checker's apply function-ness guard must recognise
    /// exactly what the unifier descends into, or the guard would refuse a
    /// function the unifier is happy to take.
    pub fn is_function_type(&self, node: NodeId) -> bool {
        self.function_type_function(node).is_some()
    }

    /// The function a **function-type node** names: `node`'s class holding the
    /// self-referential `[Function(fid), ↺]` that *is* a function's own type
    /// (`f : f`).  `None` for anything else.
    ///
    /// Recognised by the same self-cycle the universe `[Type, ↺]` uses,
    /// distinguished from it by slot 0 holding a [`LowValue::Function`] (the
    /// universe holds the `Type` marker).  That distinction is what lets the
    /// universe and a function's type stay tellable apart while both are
    /// self-referential arrays — and it is why the universe is *not* the thing
    /// a function's type is.
    ///
    /// Asked of the class's **representative**, which carries the class's value
    /// ([`Self::propagate_class_value`]), and a **read** like
    /// [`Self::is_self_referential`]: the class is walked without path
    /// compression, so a caller holding a shared borrow can ask.
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

    /// The two cells a function's type **is** — the function template's
    /// parameter pair and its return type cell — read from the function-type
    /// node `node` names, or `None` when `node` is not one.
    ///
    /// **The pair, not a copy of it and not two *type* cells.**  A
    /// `Function::parameter` is the `[value, type, attrs…]` node its body
    /// binds the variable to and the apply clone walk clones; unifying two of
    /// those positionally is what lets a signature constrain a **value**, not
    /// only a type (`?a: Int => ?a: Int` lands the same cell in both
    /// positions).  The return side is [`Function::return_type`] rather than
    /// `r#return`, which may be an unevaluated operation node whose own slots
    /// do not name the type.
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
            // A **frozen** function's template is immutable, so its signature
            // is *copied* into fresh dynamic leaves rather than read in place
            // and bound — the frozen original must never move.  This is not a
            // corner case: the whole prelude is a frozen module, and its
            // functions' type nodes carry a static self-cycle, so declining
            // here would leave the unifier unable to see them.
            AnyFunctionId::Static(sref) => return self.materialize_static_signature(sref),
        };
        Some((parameter, return_type))
    }

    /// The **domain and codomain type cells** of the function type `node`
    /// names, or `None` when `node` is not one — the same two positions
    /// [`Self::function_signature`] names, resolved to the *type* cells a
    /// reader decodes rather than to the pair a unify descends into.
    ///
    /// The domain is the parameter pair's **type slot**, not the pair: a
    /// template's parameter *value* cell is empty until an apply binds it, so
    /// the pair itself says nothing about the domain and its type slot is the
    /// decided half.  The codomain is [`Function::return_type`], for the reason
    /// [`Self::function_signature`] gives.  A frozen function's cells are read
    /// from its immutable template ([`Self::static_function_signature`]),
    /// never materialized, because a read must not allocate.
    ///
    /// **A read**, so `&self`: this is the shape half of "is this a function
    /// type", asked by a decoder that holds only a shared borrow
    /// (`lichen-highlevel`'s `shape` module) and must not be forced into a
    /// mutable one for a question it never mutates anything to answer.
    pub fn function_type_signature(&self, node: NodeId) -> Option<(AnyNodeId, AnyNodeId)> {
        match self.function_type_function(node)? {
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

    /// Unify two function types by descending into the two functions' own
    /// cells, the same treatment two arrays get: the parameter pairs unify
    /// positionally (value against value, type against type, attribute against
    /// attribute), the two return type cells unify, and the two classes merge
    /// once the elements agree.  **The same rule, not a lookalike.**
    ///
    /// There is no hook here and no clone, because there is nothing left for a
    /// host to decide.  The old policy had to ask its host *where* a function
    /// type's signature lives and *whether it may be written*, and it answered
    /// those two questions differently for a dynamic function and a frozen
    /// module's — which is how a wrapper in an imported module ended up
    /// reporting `struct<.I raw[?a, ?b], .O raw[?c, ?d]>` for a kernel whose
    /// domain and codomain are plain types (`docs/notes/function-type-merge.md`).
    ///
    /// **What this arm does not compare, and what the merge therefore says.**
    /// The descent names the signature's two positions and never reads slot 0,
    /// because slot 0 is the function the type is attached to rather than a
    /// component of the type.  A merged class keeps **one** carrier and a class
    /// of two function types has two, so the merge takes the left's
    /// ([`Self::add_equality`]) and the merged class answers with one of the two
    /// functions' identities — which one depending on the order the traversal
    /// reached them.  That is a real hole: a later unify through the merged
    /// class descends into whichever signature the carrier names.
    ///
    /// It is left open on purpose.  The question it belongs to is sub-typing —
    /// whether two signatures that agree are *the same type* or a subtype
    /// relation, and a type class with two identities in it is a symptom of
    /// answering that question positionally before it has been asked.  A special
    /// case here would not fix it, only hide it behind an asymmetry with arrays
    /// that the next change would have to unlearn.
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
        // The elements agree, so the two function types are one type — the same
        // merge the array arm makes, with no value written back, because both
        // sides already carry one.
        self.add_equality(ra, rb);
        true
    }

    /// Recursive core of [`Self::unify`]; `path` holds the class pairs on
    /// the current recursion, so a mutually recursive structure (an array
    /// unified with itself) records an error instead of looping.
    ///
    /// `path` guards only pairs that have **nodes**.  A pair of bare values has
    /// no class to name it — a self-referential structure reached through the
    /// node-less arms (`[cell, self]`, the term pair a type is) would repeat
    /// forever — so `depth` bounds that descent, and a pair past the bound is
    /// given the benefit of the doubt exactly as the unifier's cycle guard does.
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
        // A side with a node takes the question to its **class**; a side without
        // one is a bare value with no class to merge, so it can only be answered
        // by comparison.  Reading both before any write keeps the borrow of
        // `nodes` short and the values stable across the merge below.
        let a = self.answer_class_side(a);
        let b = self.answer_class_side(b);
        let va = a.value;
        let vb = b.value;
        let (Some(ra), Some(rb)) = (a.node, b.node) else {
            // **At least one side has no class to merge** — a bare value (an
            // operation's answer, a write being distributed) or a static ref,
            // which is absolute and has no local cell to merge into.  What the
            // other side carries still decides the question, and a **value
            // against a valueless class is a write**: the class learns the
            // value.  It used to pass here, which silently dropped every
            // fact a static ref brought into a unification — an imported
            // `struct<.x Int, .y Int>`'s field types reached the importer's
            // cells as `Int` and were discarded, so a placeholder
            // instantiation across the boundary never learned its field types
            // (`crates/lichen-language/tests/registry.rs`).
            match (a.node, b.node) {
                // A class against a bare value (a static ref's, or an answer a
                // write is distributing).  The **value is the fact the class is
                // missing**, so the class learns it — but only when the class
                // holds nothing.  A class that already holds a value is not a
                // hole, and writing over it would mask the conflict the
                // comparison below exists to find.
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
                // Two absences: a free cell is a wildcard, and a side with no
                // value at all is an absence rather than a pattern.
                return true;
            };
            return match (x.as_enum(), y.as_enum()) {
                (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                    // SAFETY: `pa`/`pb` are payloads of values read out of live
                    // nodes of this module, so their home blocks have not been
                    // dropped.
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
        // What each class knows, asked once and of the **class** rather than of
        // the representative's slot: a merge carries a decided value to the
        // members it adds, so the representative may be the value-less operation
        // node.
        //
        // **Unify does three things, and they are one recursion: it merges the
        // classes, it settles the value the merged class holds, and it reports
        // the conflict.**  There is no separate case for an undecided side —
        // "this class knows nothing" is the `None` of the question every pair is
        // asked, and the arm it lands in is its answer.  An undecided side is not
        // a reason to refuse (unify is called unconditionally), a decided side is
        // not a reason to write into anyone, and an operation on either side is
        // not a reason to compute first: what a class's computation produces is
        // reconciled when it runs, and a reader that needs a value finds it
        // through the class.
        match (va, vb) {
            // Nothing known on either side: the merge is the whole answer.
            (None, None) => {
                self.add_equality(ra, rb);
                true
            }
            // One side knows a value: the merged class holds it.  There is no
            // structure on the other side to descend into — a cell that knows
            // nothing is an absence, not a pattern — so the write is this arm's
            // whole effect.
            (Some(value), None) | (None, Some(value)) => {
                let rep = self.add_equality(ra, rb);
                self.write_node_value(rep, Some(value));
                true
            }
            // Both sides know a value: they must be the **same** value, and for
            // an array "the same" is decided by unifying the elements — an
            // array's elements are nodes that may still be cells, and a
            // comparison reads a free cell as "matches anything" and drops the
            // tie.
            (Some(x), Some(y)) => {
                // **A function's type is the function.**  Two function-type
                // nodes — the self-referential `[Function(fid), ↺]` that is
                // `f : f` — unify by descending into the two functions' own
                // cells, positionally, like two arrays.  The positional match
                // below would instead read both their self-cycles as one
                // structural value and merge two *different* functions' types
                // without ever comparing a signature.
                //
                // The descent reaches the **parameter pair**, so a signature
                // carries attributes and can constrain values rather than only
                // types; that is the capability the merge is for.
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
                    // **Exactly one side, and the other is a self-cycle that is
                    // not a function type**: the universe `Type`, or a recursive
                    // struct's type expression.  The positional match reads any
                    // two self-cycles as one structural value, so it would merge
                    // a function's type with `Type` — and `(\x. x) : Type` must
                    // fail.  A *degenerate* function type (a `[Function(fid),
                    // t]` pair whose type slot names another function rather
                    // than itself, which is what the apply clone walk builds for
                    // a closure a body returns) is **not** a self-cycle by this
                    // test, so it still falls through — and the positional match
                    // is the right answer for it.
                    let other = if function_a.is_some() { rb } else { ra };
                    if self.is_self_referential(AnyNodeId::Dynamic(other)) {
                        self.record_error(ra, rb, steps, root);
                        return false;
                    }
                }
                match (x.as_enum(), y.as_enum()) {
                    (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                        // SAFETY: `pa`/`pb` are the payloads of the reachable
                        // class representatives `ra`/`rb`, both live nodes of
                        // this module, so their home blocks stay alive across the
                        // recursion below — nothing in the descent releases a
                        // block.
                        let (left, right) = (unsafe { pa.items() }, unsafe { pb.items() });
                        if left.len() != right.len() {
                            self.record_error(ra, rb, steps, root);
                            return false;
                        }
                        // Two self-referential universes are the same structural
                        // value even when one is materialized from a static
                        // module; unifying their cycles should be a success, not
                        // a conflict.
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
                    // A materialized static closure and the frozen function it
                    // came from name **one** logical function: their `Function`
                    // values are equal by identity even though one is dynamic and
                    // the other static.  Checked before the generic value
                    // comparison, which compares `AnyFunctionId` by kind and
                    // would call them different.
                    (Some(LowValue::Function(a)), Some(LowValue::Function(b)))
                        if self.function_identity_equal(a, b) =>
                    {
                        self.add_equality(ra, rb);
                        true
                    }
                    // Two concrete values merge iff they are *fully* equal
                    // ([`ValueExt::value_eq`] — handle payloads by content, which
                    // the cheap [`PartialEq`] deliberately does not see).
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

    /// Whether two decided **leaf** values are one value: two functions by
    /// resolved identity, everything else by full value equality
    /// ([`ValueExt::value_eq`]).
    fn value_pair_equal(&self, a: P::Value, b: P::Value) -> bool {
        match (a.as_enum(), b.as_enum()) {
            // A materialized static closure and the frozen function it came from
            // name one logical function even though one is dynamic and the other
            // static — checked before the generic comparison, which reads
            // `AnyFunctionId` by kind and would call them different.
            (Some(LowValue::Function(x)), Some(LowValue::Function(y))) => {
                self.function_identity_equal(x, y)
            }
            _ => a.value_eq(&b),
        }
    }

    /// `rep`'s equality class's members, representative first — the union-find
    /// member list's one walk.  A reader that scans a class for a node carrying
    /// something reads it through here, so the walk lives once.
    fn class_members(&self, rep: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let mut member = Some(rep);
        std::iter::from_fn(move || {
            let current = member?;
            member = self.nodes[current].meta().next();
            Some(current)
        })
    }

    /// Join `reader` into `target`'s class — the evaluation-side half of a read's
    /// own resolution — and then let the target's computation answer.
    ///
    /// A read of a cell is a reference, not a snapshot: unifying the reader with
    /// the target lets a later bind reach it through the class, independent of
    /// evaluation order.  The unification is **unconditional** — the target's
    /// class may hold a decided value, and it may hold a member whose own
    /// computation has not produced an answer yet (that member is the one the
    /// value veto skips, so joining asserts nothing about what it will produce).
    ///
    /// The join alone leaves a target whose operator has not run unanswered: the
    /// reader is now in the target's class, so the read takes the class shortcut
    /// and never re-enters the target's own evaluation, and the read would answer
    /// with a value no operator produced.  Evaluating the target here is what
    /// answers it, and the filter that decides whether there is anything to run
    /// belongs to [`Self::evaluate_node`], not to this caller.
    ///
    /// The reader keeps its operation: the operand edge must stay live for the
    /// apply's clone machinery, and for the read's own resolution path to find it.
    ///
    /// **The join does not report.**  It is the read's own bookkeeping, not a
    /// unification the program states, and a disagreement it meets is the *same*
    /// one the reader's answer is reconciled against when its operator finishes
    /// ([`Self::write_node_answer`]): a failed join merges nothing, so the read
    /// answers with the target's own value and that reconcile reports it against
    /// the reader.  Recording here as well reports one conflict twice, mirrored —
    /// measured on `lichen-highlevel`'s
    /// `a_concrete_type_is_never_bound_over_a_dependent_codomain`, one
    /// disagreement arriving as `expected 1, found 0` (the join, roots reader and
    /// target) and `expected 0, found 1` (the reconcile, the reader twice;
    /// `docs/notes/class-channel.md`).  The dropped range is this call's own, so
    /// the merge's effect, every other failure and the evaluation below are
    /// untouched.
    pub(crate) fn alias_read(&mut self, reader: NodeId, target: NodeId) {
        let (_, errors) = self.try_unify(reader, target);
        self.unify_errors.truncate(errors.start);
        let block = self.nodes[target].block;
        self.evaluate_node(Dyn(target), Some(block));
    }

    /// The concrete value `rep`'s class has committed, if any — the side of a
    /// unification that is not the computation itself.
    ///
    /// It is the class's **one value slot**, read through the representative,
    /// which [`Self::propagate_class_value`] writes on every concrete write and
    /// on every merge.  One parent walk plus one field read, independent of how
    /// many members the class has.
    ///
    /// `None` for a class that has committed nothing — an empty slot, which is
    /// the only representation of undecided.
    ///
    /// No other condition: what the class holds is what a unification compares
    /// against, runned or not — the comparison is unconditional, and whether
    /// the value was produced or merely asserted is the reporting question
    /// ([`Self::write_node_answer`]), not a reason to leave it out.
    pub(crate) fn class_committed_value(&self, rep: NodeId) -> Option<P::Value> {
        self.class_value(rep)
    }

    /// Record a conflict between two **classes** — the pair a unification walked
    /// to, with the descent that reached it.
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

/// The dynamic node id behind an [`AnyNodeId`], or [`NodeId::default()`]
/// for a static ref (which materializes into a fresh leaf before it is
/// compared) — used only to name the operands in a [`UnifyStep`], where
/// `index` is authoritative and the highlevel re-reads the graph.
fn node_or_default(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => NodeId::default(),
    }
}

/// The low type a concrete value's variant tag states, read without touching
/// the payload — the observation half of the low-type layer.
///
/// `None` for a value the vocabulary has no shape for: the `Str` literal, the
/// unit `None`, and an empty `Error`.  Those state nothing at all, which is
/// what keeps observation from
/// ever widening a class it knows more about.
///
/// A payload-carrying shape is recorded with `Unknown` positions: the element,
/// domain, codomain, key, and value classes refine independently as they bind,
/// and [`Module::low_type_of_node`] recurses into them on a read.
fn observed_low_shape(value: impl AsEnum<LowValue>) -> Option<LowShape> {
    match value.as_enum()? {
        LowValue::USize(_) => Some(LowShape::USize),
        LowValue::Float(_) => Some(LowShape::Float),
        LowValue::Array(array) => Some(LowShape::Array(
            Box::new(LowShape::Unknown),
            // SAFETY: only the length is read, and the caller passes a value
            // it is holding on this borrow of the module, so the payload's home
            // arena is alive for the read.
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
