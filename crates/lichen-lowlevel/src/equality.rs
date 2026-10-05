use std::collections::HashSet;

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, FunctionIdentity,
    FunctionTypeUnify, LowShape, LowValue, Module, Node, NodeId, Program, StaticFunctionRef,
    StaticNodeId, ValueExt as _, ancestors::AncestorPairs, is_unbound,
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
    /// value to the members the merge adds to it.
    ///
    /// The two low-type reads at the top are the shape half; the value half is
    /// the class-channel invariant ([`Self::write_node_value`]) applied to the
    /// merge that **grows** a class rather than to the write that fills one.  A
    /// decided value is not necessarily on the representative: replication
    /// skips operation-bearing members, so a class whose representative is a
    /// pending operation holds its value on some other member.  A merge that
    /// read only the two representatives' slots would therefore see no value
    /// and carry nothing, leaving the cell that just joined the class
    /// undecided while the class has been decided all along — a read that
    /// happened before the unification which decided the class, with nothing
    /// to wake it afterwards (`docs/notes/eval-before-unify.md` §2.1).
    ///
    /// Gated on the representative's own slot being unbound and the class
    /// having more than one member: a representative that is a pure cell is
    /// written by every commit (replication covers it), and a class of one has
    /// no other member to hide a value on.
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        // Both sides' low types are read *before* the union (which leaves the
        // authoritative copy on whichever node becomes the representative) and
        // joined onto it after, so a merge never drops a decided shape.
        let left = self.class_low_type(a).cloned();
        let right = self.class_low_type(b).cloned();
        // The same argument for the value, and for the same reason: the union
        // re-elects the representative, so what either side already knew has to
        // be read **before** it and committed to the winner afterwards.  A
        // marker is not a fact to carry (`is_unbound`), exactly as
        // [`Self::write_node_value`] treats it.  Both sides decided is not a
        // conflict here — `unify_inner`'s arms decide that, and only agree to
        // merge two decided sides.
        let left_value = self.class_committed_value(a);
        let right_value = self.class_committed_value(b);
        let representative = disjoint::union(&mut self.nodes, a, b);
        for shape in [left, right].into_iter().flatten() {
            self.refine_class_low_type(representative, shape);
        }
        // The class keeps the value it had, and the merged class's carrier has
        // to name it.  The union re-elected a representative, so a chosen side's
        // carrier is re-pointed at the winner — one assignment, and the value
        // itself never moves, which is what keeps a value no member could take
        // (an operation-bearing member's) reachable.
        if let Some(carrier) = left_value.map(|_| a).or(right_value.map(|_| b)) {
            self.commit_class_value(carrier);
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
    /// `None`, a computed-nothing `Void`) states nothing, so it never widens
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
    /// site.  A `None`/`Parameterized` value only sets the node's own slot — a
    /// marker is not a fact to propagate.
    ///
    /// **The rule is member-local, and the write is unconditional**
    /// ([`docs/notes/class-channel.md`] §1.1).  A unification must write: the
    /// write is *not* gated on the slot being unbound, and the test it puts to
    /// a member is that member's own value against the propagated one — never
    /// the class's.  A class here routinely holds *different* values on
    /// different members (a term pair on one, the resolved value on another, a
    /// type cell on a third), so a class-level read turns every ordinary write
    /// into a conflict; that was measured, and it is why the read is
    /// member-local.  The comparison happens in
    /// [`Self::propagate_class_value`]; a member that cannot take the value
    /// keeps the one it has.
    ///
    /// It is also one of the two **observation** sites of the low-type layer:
    /// a concrete value refines its class's low type from the value's variant
    /// tag ([`Self::observe_class_low_type`]).  The other is
    /// [`Module::add_node`], where a value arrives already concrete.
    pub fn write_node_value(&mut self, node: NodeId, value: Option<P::Value>) {
        self.nodes[node].value = value;
        if let Some(value) = value.filter(|v| !is_unbound(Some(*v))) {
            if self.nodes[node].equality.parent().is_none()
                && self.nodes[node].equality.next().is_none()
            {
                self.commit_class_value(node);
                return;
            }
            let representative = self.equality_representative(node);
            self.propagate_class_value(representative, value);
            self.commit_class_value(node);
            self.observe_class_low_type(representative, value);
        }
    }

    /// Commit an **operation's answer** — the evaluator's write.
    ///
    /// The answer is a **value**: it has no class of its own, so it meets the
    /// node through the one unification as a node-less side
    /// ([`Side::value`]) — which is exactly the question "can this answer be the
    /// value my class holds", asked *of* the recursion rather than beside it.
    /// The conflict is the ordinary unification conflict, recorded at this
    /// node's roots, so nothing needs a comparison path of its own.
    ///
    /// `runned` is the part only the evaluator knows: *this* node's operator
    /// produced the value, which is the axis [`Self::has_no_result_yet`] reads
    /// and a value the unifier wrote cannot claim.
    pub(crate) fn write_node_answer(&mut self, node: NodeId, value: P::Value) {
        let mut path = AncestorPairs::new();
        let mut steps = Vec::new();
        // The answer against what the node's class holds, as two values: the
        // answer has no class of its own, and pulling the class's value out
        // explicitly is what makes the two comparable.
        let held = {
            let representative = self.equality_representative(node);
            self.class_committed_value(representative)
        };
        self.unify_inner(
            Side::value(Some(value)),
            Side::value(held),
            &mut path,
            0,
            &mut steps,
            (node, node),
        );
        self.write_node_value(node, Some(value));
        self.nodes[node].runned = true;
    }

    /// Propagate a concrete `value` over `representative`'s class — the second
    /// half of [`Self::write_node_value`], shared with [`Self::add_equality`],
    /// where a merge carries the class's decided value to the members it adds
    /// exactly as a write carries it to the members it finds.
    ///
    /// The walk is **unconditional**: a class has one value, the first member to
    /// have one propagates it to every member that can take it, and no later
    /// pass can meet a member that already holds a *different* one.  There is
    /// therefore no comparison here and nothing to tolerate — a member either
    /// has the class's value already (the write is idempotent) or does not and
    /// takes it.  The condition that used to guard this ("a member that already
    /// knows something is compared") described a state the invariant no longer
    /// admits.
    ///
    /// An **operation-bearing** member is the one veto: its own computation is
    /// what settles it, and a value arriving from elsewhere is not a proof of
    /// what that computation will produce.  That is why the class's value can be
    /// reachable only through the class carrier
    /// ([`Self::class_committed_node`]) rather than from every member's slot.
    ///
    /// The walk visits the representative too: the write site wrote *its own*
    /// node, which need not be the representative — a class whose representative
    /// is a value-less operation node is exactly the case
    /// [`Self::add_equality`] exists for.
    fn propagate_class_value(&mut self, representative: NodeId, value: P::Value) {
        // A class whose sole member is the representative — `parent` and `next`
        // both `None`, `disjoint::Meta`'s contract for a representative with no
        // second member — holds nobody to propagate to, so the write site's own
        // slot write is the whole effect.  See `P4-2` in
        // `docs/notes/code-audit.md`.
        if self.nodes[representative].equality.parent().is_none()
            && self.nodes[representative].equality.next().is_none()
        {
            return;
        }
        let members: Vec<NodeId> = self.class_members(representative).collect();
        for member in members {
            if self.nodes[member].operation.is_some() {
                continue;
            }
            self.nodes[member].value = Some(value);
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
    /// operation (a pure unbound cell) binds to the other side's value; an
    /// unevaluated operation is a *pending computation*, and a concrete
    /// value must never be bound over one — that would silently erase it
    /// (e.g. a dependent type branch that selects a different type per
    /// argument).  Such a computation is forced before comparing; if its
    /// operands are still unbound and it cannot resolve, the unify fails —
    /// except against an all-unbound skeleton (cells and arrays of cells),
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
    pub fn is_self_referential(&mut self, id: AnyNodeId) -> bool {
        match id {
            Dyn(node) => {
                let rep = self.equality_representative(node);
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
                            if self.equality_representative(item) == rep {
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
        let Some(LowValue::Array(array)) = self.static_read(sref).as_enum() else {
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

    /// Whether `node`'s class holds a **function-type node**: the
    /// self-referential `[Function(fid), ↺]` that *is* a function's own type
    /// (`f : f`). Recognised by the same self-cycle the universe
    /// `[Type, ↺]` uses, distinguished from it by slot 0 holding a
    /// [`LowValue::Function`] (the universe holds the `Type` marker). This is
    /// the lowlevel half of the recognition — the highlevel's `shape` module
    /// reads the signature out of the function template the `Function` value
    /// names, and the [`Program::unify_function_type`] hook clones that
    /// signature rather than letting a positional unify bind the template's
    /// shared cells.
    ///
    /// Reads the class's committed carrier (a bare merge may leave the
    /// decided value on a member other than the representative), so a class
    /// unified against a function-type is recognised through whichever member
    /// carries it.
    pub fn is_function_type_node(&mut self, node: NodeId) -> bool {
        let rep = self.equality_representative(node);
        let Some(carrier) = self.class_committed_node(rep) else {
            return false;
        };
        if !self.is_self_referential(AnyNodeId::Dynamic(carrier)) {
            return false;
        }
        // SAFETY: `carrier` is a live node of this module; nothing here drops
        // a block.
        let Some(items) = (unsafe { self.array_items(carrier) }) else {
            return false;
        };
        items.len() == 2
            && self
                .node_value(items[0].node)
                .is_some_and(|v| matches!(v.as_enum(), Some(LowValue::Function(_))))
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
            let va = va.filter(|value| !is_unbound(Some(*value)));
            let vb = vb.filter(|value| !is_unbound(Some(*value)));
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
                // A function-type node — the self-referential `[Function(fid),
                // ↺]` that is a function's own type (`f : f`) — on either side is
                // handled by the program's clone-on-unify policy before the
                // positional match.  The positional match would otherwise either
                // wrongly *merge* two self-referential function-types (binding
                // the shared template's cells, the defect this fixes) or clash a
                // `Function` slot 0 against an array. The policy clones the
                // function's signature and unifies the clone, leaving the
                // template untouched; on success the two sides are resolved
                // *without* merging classes (the function-type stays a distinct,
                // polymorphic class).
                if self.is_function_type_node(ra) || self.is_function_type_node(rb) {
                    match P::unify_function_type(self, ra, rb) {
                        FunctionTypeUnify::Handled => return true,
                        FunctionTypeUnify::Conflict => {
                            self.record_error(ra, rb, steps, root);
                            return false;
                        }
                        // One side is a self-referential `[Function, ↺]` the
                        // program does not treat as a function-type (a build with
                        // no highlevel, which never builds one): fall through to
                        // the positional rules.
                        FunctionTypeUnify::NotFunctionType => {}
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

    /// The depth bound shared by the node-less comparisons: a structure reached
    /// without classes cannot name a repeated pair, so its descent is bounded and
    /// a pair past the bound is given the benefit of the doubt, exactly as the
    /// unifier's cycle guard does.
    /// Whether `rep`'s class holds a computation that has not produced an
    /// answer yet ([`Module::has_no_result_yet`]) — a node whose operator still
    /// owes a result.
    fn class_has_pending_op(&self, rep: NodeId) -> bool {
        self.class_members(rep)
            .any(|member| self.has_no_result_yet(member))
    }

    /// `rep`'s equality class's members, representative first — the union-find
    /// member list's one walk.  Every reader that scans a class for a node
    /// carrying something (a pending operation, a committed value) reads it
    /// through here, so the walk and its bound live once.
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
    pub(crate) fn alias_read(&mut self, reader: NodeId, target: NodeId) {
        self.unify(reader, target);
        let block = self.nodes[target].block;
        self.evaluate_node(Dyn(target), Some(block));
    }

    /// The class's value is named by the **representative**: a class has one
    /// value, it may sit on any member (an operation-bearing member keeps its
    /// own computation and is never written), and the representative records
    /// which member that is.  A reader therefore pays one [`disjoint::find`]
    /// plus one field read, instead of walking the member list looking for a
    /// member that has run.
    ///
    /// The carrier is maintained by the one value-write path
    /// ([`Self::commit_class_value`]), which every write goes through —
    /// [`Self::write_node_value`], the merge in [`Self::add_equality`], the
    /// evaluator, and the claim path — so a carrier cannot outlive the value it
    /// names.  It is dropped when the member it names is released
    /// ([`Self::forget_class_carrier`]).
    pub fn class_committed_node(&self, rep: NodeId) -> Option<NodeId> {
        let representative = self.class_root(rep);
        self.nodes[representative].class_carrier
    }

    /// The concrete value `rep`'s class has already committed, if any — the
    /// side of a unification that is not the computation itself.  One read of
    /// the member the class's carrier names ([`Self::class_committed_node`]),
    /// so the cost does not depend on how many members the class has.
    ///
    /// `None` for a member whose slot is unbound **and** for the
    /// [`LowValue::Parameterized`] marker, which is the same distinction
    /// [`Self::write_node_value`] draws — a marker is not a fact to carry.
    ///
    /// No other condition: what the class holds is what a unification compares
    /// against, runned or not — the comparison is unconditional, and whether
    /// the value was produced or merely asserted is the reporting question
    /// ([`Self::write_node_answer`]), not a reason to leave it out.
    pub(crate) fn class_committed_value(&self, rep: NodeId) -> Option<P::Value> {
        let member = self.class_committed_node(rep)?;
        self.nodes
            .get(member)?
            .value
            .filter(|value| !is_unbound(Some(*value)))
    }

    /// Record that `member` now carries its class's committed value — the one
    /// place the class carrier moves, called by [`Self::commit_class_value`].
    /// A member with nothing decided to carry clears it instead, so the carrier
    /// never names a slot that does not hold a value; a class that already has a
    /// carrier keeps it, because the value it names is still there and the
    /// value-to-value rule says a class does not gain a second one.
    fn commit_class_value(&mut self, member: NodeId) {
        let representative = self.equality_representative(member);
        if is_unbound(self.nodes[member].value) {
            if self.nodes[representative].class_carrier == Some(member) {
                self.nodes[representative].class_carrier = None;
            }
            return;
        }
        if self.nodes[representative].class_carrier.is_none() {
            self.nodes[representative].class_carrier = Some(member);
        }
    }

    /// Re-point a class's carrier after its member list was rebuilt and a
    /// representative re-elected ([`Module::flatten_class`]).  It names the first
    /// survivor that still holds a decided value, so a class whose carrier died
    /// with a dropped block keeps answering from the value the survivors carry.
    pub(crate) fn reselect_class_carrier(&mut self, members: &[NodeId]) {
        let Some((&representative, rest)) = members.split_first() else {
            return;
        };
        let carrier = std::iter::once(representative)
            .chain(rest.iter().copied())
            .find(|&member| !is_unbound(self.nodes[member].value));
        self.nodes[representative].class_carrier = carrier;
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
/// unit `None`, a computed-nothing `Void`, and the undecided `Parameterized`
/// marker.  Those state nothing at all, which is what keeps observation from
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
