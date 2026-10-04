use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{
    AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, Deferral, LowOperator, LowShape, LowValue,
    Module, Node, NodeId, Operation, PendingSide, PendingSides, Program, StaticModuleCache,
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
        let representative = disjoint::union(&mut self.nodes, a, b);
        for shape in [left, right].into_iter().flatten() {
            self.refine_class_low_type(representative, shape);
        }
        if is_unbound(self.nodes[representative].value)
            && self.nodes[representative].meta().next().is_some()
            && let Some(value) = self.class_committed_value(representative)
        {
            self.replicate_class_value(representative, value);
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

    /// The controlled value-write API: write `value` onto `node`, then — if
    /// the value is concrete — replicate it to every unbound *pure-cell*
    /// member of `node`'s class, so a read of any member (or a later
    /// `bind`/`unify`, which reads the representative's slot) sees it
    /// regardless of which member resolved it.
    ///
    /// This is the single choke-point for value writes: every place a value
    /// lands on a node that might be a member of a unified class goes through
    /// here, so the class-consistency invariant (the linked-list replication
    /// `bind`/`force_pending` perform) is maintained at exactly one site.  A
    /// `None`/`Parameterized` value only sets the node's own slot — a marker is
    /// not a fact to propagate.  A singleton class is a no-op (the walk visits
    /// only the node itself).  Only operation-free (pure) cells are touched,
    /// matching `force_pending`, so a pending computation is never overridden.
    ///
    /// It is also one of the two **observation** sites of the low-type layer:
    /// a concrete value refines its class's low type from the value's variant
    /// tag ([`Self::observe_class_low_type`]).  The other is
    /// [`Module::add_node`], where a value arrives already concrete.
    pub fn write_node_value(&mut self, node: NodeId, value: Option<P::Value>) {
        self.nodes[node].value = value;
        if let Some(value) = value.filter(|v| !is_unbound(Some(*v))) {
            // A class whose sole member is `node` — `parent` and `next` both
            // `None`, `disjoint::Meta`'s contract for a representative with no
            // second member — holds nobody to replicate to, so the write above
            // is the whole effect.  See `P4-2` in `docs/notes/code-audit.md`.
            if self.nodes[node].equality.parent().is_none()
                && self.nodes[node].equality.next().is_none()
            {
                return;
            }
            let representative = self.equality_representative(node);
            self.replicate_class_value(representative, value);
            self.observe_class_low_type(representative, value);
        }
    }

    /// Replicate a concrete `value` over the unbound pure cells of
    /// `representative`'s class — the second half of
    /// [`Self::write_node_value`], shared with [`Self::add_equality`], where a
    /// merge carries the class's decided value to the members it adds exactly
    /// as a write carries it to the members it finds.
    ///
    /// Only operation-free (pure) cells are written, matching `force_pending`,
    /// so a pending computation is never overridden: an operation node's own
    /// slot is its computation's to settle.  Deliberately **not** the low-type
    /// observation [`Self::write_node_value`] performs — observation is a class
    /// *gaining* a decided value, and this merge adds no fact to the class, only
    /// members.
    fn replicate_class_value(&mut self, representative: NodeId, value: P::Value) {
        let mut member = representative;
        loop {
            let next = self.nodes[member].meta().next();
            if self.nodes[member].operation.is_none() && is_unbound(self.nodes[member].value) {
                self.nodes[member].value = Some(value);
            }
            let Some(next) = next else { break };
            member = next;
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
        let mut materialized = HashMap::new();
        // The descent path, seeded empty; the root operands are carried
        // separately and recorded in each `UnifyError`'s `root_a`/`root_b`.
        let mut steps = Vec::new();
        self.unify_inner(
            Dyn(a),
            Dyn(b),
            &mut path,
            &mut materialized,
            &mut steps,
            (a, b),
        );
        disjoint::find(&mut self.nodes, a)
    }

    /// A static side of a unification has no class to join: materialize it
    /// into a fresh leaf node holding its value (homed in the other
    /// side's block — or the module's first block when both sides are static,
    /// which the apply path cannot produce), then unify that.  This is the
    /// only place a static value enters the class machinery; refs are
    /// absolute, so the value needs no conversion to be storable there.
    ///
    /// A static ref is materialized at most once per [`Self::unify`]
    /// traversal (the cache is threaded through [`Self::unify_inner`]).  This
    /// matters for static self-referential structures: without the cache,
    /// each recursive encounter of a static universe ref would mint a fresh
    /// leaf and the path guard could never see a repeated class pair.
    fn unify_side(
        &mut self,
        id: AnyNodeId,
        other: AnyNodeId,
        materialized: &mut HashMap<StaticNodeId, NodeId>,
    ) -> NodeId {
        match id {
            Dyn(node) => node,
            AnyNodeId::Static(sref) => {
                if let Some(&node) = materialized.get(&sref) {
                    return node;
                }
                let block = match other {
                    Dyn(node) => self.nodes[node].block,
                    AnyNodeId::Static(_) => self
                        .blocks
                        .iter()
                        .next()
                        .map(|(block, _)| block)
                        .expect("module has at least one block"),
                };
                let node = self.materialize_leaf(sref, block);
                materialized.insert(sref, node);
                node
            }
        }
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

    /// Recursive core of [`Self::unify`]; `path` holds the class pairs on
    /// the current recursion, so a mutually recursive structure (an array
    /// unified with itself) records an error instead of looping.
    #[stacksafe]
    fn unify_inner(
        &mut self,
        a: AnyNodeId,
        b: AnyNodeId,
        path: &mut AncestorPairs<NodeId>,
        materialized: &mut HashMap<StaticNodeId, NodeId>,
        steps: &mut Vec<UnifyStep>,
        root: (NodeId, NodeId),
    ) -> bool {
        let a = self.unify_side(a, b, materialized);
        let b = self.unify_side(b, Dyn(a), materialized);
        let ra = disjoint::find(&mut self.nodes, a);
        let rb = disjoint::find(&mut self.nodes, b);
        if ra == rb {
            return true;
        }
        if path.contains(ra, rb) {
            self.record_error(ra, rb, steps, root);
            return false;
        }
        let va = self.nodes[ra].value;
        let vb = self.nodes[rb].value;
        // A class that is unbound and holds no unevaluated operation is a
        // pure cell: bind it to the other side.  A class with an unevaluated
        // operation is not bindable — it is a pending computation, and a
        // concrete value bound over it would erase the computation.  A read
        // of the class's own cell is not a pending computation — it is a
        // self-reference that resolves via replication when the class binds.
        let cell_a = is_unbound(va) && self.class_is_pure_cell(ra);
        let cell_b = is_unbound(vb) && self.class_is_pure_cell(rb);
        if cell_a || cell_b {
            self.bind(ra, rb, va, vb);
            return true;
        }
        // Neither side is a pure cell: force any pending computations so the
        // comparison sees their resolved values.  A computation whose
        // operands are still unbound cannot resolve; an `Index` over a
        // concrete array with a concrete index is instead resolved as a pure
        // reference to the selected element ([`Self::alias_index`]) — the
        // read then pins the element to whatever the other side unifies
        // with.  A computation that is neither forceable nor a resolvable
        // `Index` cannot be compared, so this fails rather than binding over
        // it.
        loop {
            let ra = disjoint::find(&mut self.nodes, ra);
            let rb = disjoint::find(&mut self.nodes, rb);
            let pending_a = self.class_has_pending_op(ra);
            let pending_b = self.class_has_pending_op(rb);
            if !pending_a && !pending_b {
                break;
            }
            let resolved_a = !pending_a || self.force_pending(ra).is_some() || {
                let other = self.nodes[rb].value;
                self.alias_index(ra, other)
            };
            let resolved_b = !pending_b || self.force_pending(rb).is_some() || {
                let other = self.nodes[ra].value;
                self.alias_index(rb, other)
            };
            if !resolved_a || !resolved_b {
                // A pending computation whose operands are still unbound is
                // compatible with an all-unbound skeleton on the other side:
                // the skeleton holds no concrete value and no computation, so
                // merging the classes erases nothing — the computation
                // resolves later and its value replicates onto the skeleton.
                // (The annotation `x : T => if …` hits this: the return type
                // is a pending computation at check time, the annotation's
                // `_` codomain is a skeleton, and they must simply join.)
                if (pending_a && self.class_is_skeleton(rb))
                    || (pending_b && self.class_is_skeleton(ra))
                {
                    self.add_equality(ra, rb);
                    return true;
                }
                // A pending *field/positional read* whose own type is being
                // unified against a *type value* is a type round-trip, not a
                // value comparison: the read resolves to the field's actual
                // type once the container binds, and a genuine mismatch
                // surfaces at apply time against the real container.
                // Recognising "this class holds a type" needs the program's
                // own encoding, so the decision is the program's — see
                // [`Program::defer_pending`].  Only an unresolvable `Index`
                // qualifies (never a resolved read, nor arithmetic or a
                // dependent-type branch), so an unresolvable real
                // computation still records an error.
                let sides = PendingSides {
                    a: PendingSide {
                        representative: ra,
                        pending: pending_a,
                        pending_index_read: pending_a && self.is_pending_index_read(ra),
                        pending_apply: pending_a && self.is_pending_apply(ra),
                        skeleton: self.class_is_skeleton(ra),
                        pure_cell: self.class_is_pure_cell(ra),
                    },
                    b: PendingSide {
                        representative: rb,
                        pending: pending_b,
                        pending_index_read: pending_b && self.is_pending_index_read(rb),
                        pending_apply: pending_b && self.is_pending_apply(rb),
                        skeleton: self.class_is_skeleton(rb),
                        pure_cell: self.class_is_pure_cell(rb),
                    },
                };
                if let Some(verdict) = P::defer_pending(self, &sides)
                    && verdict == Deferral::Merge
                {
                    let rep = self.add_equality(ra, rb);
                    self.pin_committed_value(rep);
                    return true;
                }
                // A pending *field/positional read* unified against another
                // pending field read — both over (ultimately) unbound
                // containers — is unified the same way.  Neither half has a
                // concrete value yet, so there is nothing to compare now; the
                // two lazy reads resolve to a common type later, at apply
                // time, when their containers bind.
                if pending_a
                    && pending_b
                    && self.is_pending_index_read(ra)
                    && self.is_pending_index_read(rb)
                {
                    self.add_equality(ra, rb);
                    return true;
                }
                self.record_error(ra, rb, steps, root);
                return false;
            }
        }
        let ra = disjoint::find(&mut self.nodes, ra);
        let rb = disjoint::find(&mut self.nodes, rb);
        let va = self.nodes[ra].value;
        let vb = self.nodes[rb].value;
        let pair = (
            va.as_ref().and_then(|value| value.as_enum()),
            vb.as_ref().and_then(|value| value.as_enum()),
        );
        match pair {
            (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                // SAFETY: `pa`/`pb` are the payloads of the reachable class
                // representatives `ra`/`rb`, both live nodes of this module, so
                // their home blocks stay alive across the recursion below —
                // nothing in the descent releases a block.
                let (left, right) = (unsafe { pa.items() }, unsafe { pb.items() });
                if left.len() != right.len() {
                    self.record_error(ra, rb, steps, root);
                    return false;
                }
                // Two self-referential universes are the same structural
                // value even when one is materialized from a static module;
                // unifying their cycles should be a success, not a conflict.
                if self.is_self_referential(Dyn(ra)) && self.is_self_referential(Dyn(rb)) {
                    self.add_equality(ra, rb);
                    return true;
                }
                path.insert(ra, rb);
                let mut ok = true;
                for (i, (na, nb)) in left.iter().zip(right.iter()).enumerate() {
                    // Record the descent step before recursing, so the deep
                    // failure's trace carries the full element path.
                    steps.push(UnifyStep {
                        index: i,
                        a: node_or_default(na.node),
                        b: node_or_default(nb.node),
                    });
                    let child_ok =
                        self.unify_inner(na.node, nb.node, path, materialized, steps, root);
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
            // Two concrete values merge iff they are *fully* equal
            // ([`ValueExt::value_eq`] — handle payloads by content, which
            // the cheap [`PartialEq`] deliberately does not see); two
            // classes holding no value at all merge too, which nothing
            // above could bind.
            _ if match (&va, &vb) {
                (Some(a), Some(b)) => a.value_eq(b),
                (None, None) => true,
                _ => false,
            } =>
            {
                self.add_equality(ra, rb);
                true
            }
            _ => {
                self.record_error(ra, rb, steps, root);
                false
            }
        }
    }

    /// Merge two classes; when the merged class holds a concrete value,
    /// replicate it to every member.  Each member's own `value` slot then
    /// stays locally correct — reads need no representative lookup, and the
    /// binding survives members being garbage-collected (the representative
    /// may die while another member is still live).
    fn bind(&mut self, ra: NodeId, rb: NodeId, va: Option<P::Value>, vb: Option<P::Value>) {
        let concrete = if is_unbound(va) { vb } else { va };
        let rep = self.add_equality(ra, rb);
        // Route the value through the single write API, which already
        // replicates a concrete value to the class's unbound pure-cell
        // members.  Only a *concrete* value is written: an unbound class stays
        // unbound and keeps its existing marker (`None`/`Parameterized`), so
        // the merge never flips a `Parameterized` cell to `None` (which reads
        // as an unevaluated-`operation` panic on a re-read).
        if let Some(value) = concrete.filter(|v| !is_unbound(Some(*v))) {
            self.write_node_value(rep, Some(value));
        }
    }

    /// Whether `rep`'s class holds a pending computation: a member whose
    /// operation has not produced an answer yet
    /// ([`Module::has_no_result_yet`]).  Such nodes are computations, never
    /// bindable cells.
    fn class_has_pending_op(&self, rep: NodeId) -> bool {
        self.class_members(rep)
            .any(|member| self.has_no_result_yet(member))
    }

    /// Whether `rep`'s class is a pure cell for binding purposes: its value
    /// is unbound and it holds no *independent* pending computation.  A read
    /// of the class's own cell ([`Self::is_self_read`]) is excluded — it is
    /// a reference to the class, resolved by replication when the class
    /// binds, not a computation that a bind would erase.
    fn class_is_pure_cell(&self, rep: NodeId) -> bool {
        !self
            .class_members(rep)
            .any(|member| self.has_no_result_yet(member) && !self.is_self_read(member, rep))
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

    /// Whether `rep`'s class is an all-unbound skeleton: every member is a
    /// pure cell or an array whose elements are all skeletons — no
    /// computation, no concrete value.  A pending computation merges onto
    /// such a class without erasing anything; a class that holds any
    /// concrete value or operation is not a skeleton, and binding a
    /// computation onto it would corrupt it.
    ///
    /// "No concrete value" is judged on the node's **value slot**, never on
    /// the [`LowValue`] projection: an extension atom (a value `as_enum`
    /// cannot see) is a decided value the lowlevel cannot read, and treating
    /// it as absent would merge a computation onto a class that holds one.
    fn class_is_skeleton(&self, rep: NodeId) -> bool {
        // One resolution cache for the whole class walk: an array member's
        // static elements usually name one module.
        let mut cache = StaticModuleCache::new();
        let mut member = rep;
        loop {
            if self.nodes[member].operation.is_some() {
                return false;
            }
            match self.nodes[member].value {
                None => {}
                Some(value) => match value.as_enum() {
                    Some(LowValue::Parameterized) => {}
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is the payload of `member`, a live node
                        // of this module, so its home block has not been dropped.
                        let items = unsafe { array.items() };
                        let mut seen = HashSet::new();
                        if items
                            .iter()
                            .any(|item| !self.value_is_skeleton(&mut cache, item.node, &mut seen))
                        {
                            return false;
                        }
                    }
                    // Any other decided value — a structural scalar or an
                    // extension atom — is concrete content.
                    _ => return false,
                },
            }
            let Some(next) = self.nodes[member].meta().next() else {
                return true;
            };
            member = next;
        }
    }

    /// Whether the subtree of array values rooted at `node` is all
    /// skeletons; `seen` cuts the cycle of a self-referential structure
    /// (which is a skeleton only if its own elements are).  A static ref is
    /// a decided leaf: its solved flag says whether it reads `Parameterized`
    /// (a skeleton position) or concrete (not).  An extension atom — a value
    /// the [`LowValue`] projection cannot see — is decided content, never an
    /// empty position.  `cache` is the enclosing walk's static-module
    /// resolution cache.
    fn value_is_skeleton(
        &self,
        cache: &mut StaticModuleCache<P>,
        node: AnyNodeId,
        seen: &mut HashSet<AnyNodeId>,
    ) -> bool {
        if !seen.insert(node) {
            return true;
        }
        let ok = match node {
            AnyNodeId::Static(sref) => cache.node_parameterized(self, sref),
            Dyn(node) => {
                self.nodes[node].operation.is_none()
                    && match self.nodes[node].value {
                        None => true,
                        Some(value) => match value.as_enum() {
                            Some(LowValue::Parameterized) => true,
                            // SAFETY: `array` is the payload of `node`, a live node
                            // of this module, so its home block has not been
                            // dropped.
                            Some(LowValue::Array(array)) => unsafe { array.items() }
                                .iter()
                                .all(|item| self.value_is_skeleton(cache, item.node, seen)),
                            // Any other decided value — a structural scalar or
                            // an extension atom — is concrete content, not an
                            // empty position.
                            _ => false,
                        },
                    }
            }
        };
        seen.remove(&node);
        ok
    }

    /// Whether `op`'s pending `Index` reads a cell of `rep`'s own class — a
    /// self-reference.  The read's target must be resolvable (a concrete
    /// operand, index, and container); a read whose target is not yet known
    /// counts as a pending computation, conservatively.
    fn is_self_read(&self, op: NodeId, rep: NodeId) -> bool {
        let Some(target) = self.index_target(op) else {
            return false;
        };
        // A static target is not a class member — never a self-read.
        let AnyNodeId::Dynamic(target) = target else {
            return false;
        };
        let mut n = target;
        while let Some(parent) = self.nodes[n].equality.parent() {
            n = parent;
        }
        n == rep
    }

    /// The element an `Index` operation reads, when the operand array, the
    /// index, and the container are all concrete.  The element is an
    /// [`AnyNodeId`]: a static container yields a static element, which the
    /// alias machinery refuses (no class behind it).
    fn index_target(&self, op: NodeId) -> Option<AnyNodeId> {
        let Operation { operator, operand } = self.nodes[op].operation?;
        if !matches!(operator.as_enum(), Some(LowOperator::Index)) {
            return None;
        }
        let operand = operand?;
        let operands = self.nodes[operand].value?;
        let Some(LowValue::Array(array)) = operands.as_enum() else {
            return None;
        };
        // SAFETY: `array` is the payload of `operand`, a live node of this
        // module, so its home block has not been dropped.
        let operands = unsafe { array.items() };
        if operands.len() != 2 {
            return None;
        }
        let index_value = self.node_value(operands[1].node)?;
        let Some(LowValue::USize(index)) = index_value.as_enum() else {
            return None;
        };
        let container_value = self.node_value(operands[0].node)?;
        let Some(LowValue::Array(container_ptr)) = container_value.as_enum() else {
            return None;
        };
        // SAFETY: `container_ptr` is the array payload of a live node of this
        // module (read through `Self::node_value` just above), so its home
        // block has not been dropped.
        unsafe { container_ptr.items() }
            .get(index)
            .map(|item| item.node)
    }

    /// Whether `rep`'s class holds a *pending field/positional read*: an
    /// `Index` operation whose operand container (or index) is not yet
    /// concrete, so the read is a lazy reference that resolves once the
    /// container binds.  Distinct from a resolved read (handled by
    /// [`Self::alias_index`]) and from a non-`Index` pending computation
    /// (arithmetic, a dependent-type branch), which must not be deferred.
    fn is_pending_index_read(&self, rep: NodeId) -> bool {
        let Some(op) = self.pending_op(rep) else {
            return false;
        };
        let Some(Operation { operator, .. }) = self.nodes[op].operation else {
            return false;
        };
        if !matches!(operator.as_enum(), Some(LowOperator::Index)) {
            return false;
        }
        self.index_target(op).is_none()
    }

    /// Commit the class's decided value (if any) onto its pending operations'
    /// own slots and onto the representative: a deferred unification the
    /// program's policy accepted is a bet that the computation resolves to
    /// that value, so the class reads as decided now rather than after a
    /// resolution that may never run (an unbound placeholder never binds).
    ///
    /// The operations keep their operation: the operand edge stays live for
    /// the apply's clone machinery, which drops a cached value on an
    /// operation node and recomputes against the real argument
    /// ([`crate::function`]) — the deferred check surfacing at that point, the
    /// same reconcile [`Self::force_pending`] performs against
    /// [`Self::class_committed_value`].  A class with nothing committed pins
    /// nothing.
    fn pin_committed_value(&mut self, rep: NodeId) {
        let Some(value) = self.class_committed_value(rep) else {
            return;
        };
        let ops: Vec<NodeId> = self
            .class_members(rep)
            .filter(|&member| self.has_no_result_yet(member))
            .collect();
        for op in ops {
            self.write_node_value(op, Some(value));
        }
        self.write_node_value(rep, Some(value));
    }

    /// Whether `rep`'s class holds a *pending call*: an `Apply` operation
    /// whose value is still unbound.  The apply stays lazy while its argument
    /// is undecided; like a pending `Index` read it is a suspended reference
    /// rather than an arithmetic computation.
    fn is_pending_apply(&self, rep: NodeId) -> bool {
        let Some(op) = self.pending_op(rep) else {
            return false;
        };
        let Some(Operation { operator, .. }) = self.nodes[op].operation else {
            return false;
        };
        matches!(operator.as_enum(), Some(LowOperator::Apply))
    }

    /// The first pending operation node in `rep`'s class, if any.
    fn pending_op(&self, rep: NodeId) -> Option<NodeId> {
        self.class_members(rep)
            .find(|&member| self.has_no_result_yet(member))
    }

    /// Resolve an unforceable `Index` as a pure reference.  An `Index` over a
    /// concrete array with a concrete index is just a read of that element —
    /// `operand[0][index]` — so the operator node is aliased to the element
    /// (the classes merge).  When the unify's other side holds a concrete
    /// `value`, it is written onto the read immediately — pinning the element
    /// (the "monomorphized" trade for dependent reads) — and the read keeps
    /// its operation, so its operand edge stays live for the apply's clone
    /// machinery to reach the parameter and enforce the pin.  With no value
    /// to pin, the read is a plain alias and the computation is dropped.
    /// Returns `false` when the pending computation is not such an `Index` —
    /// e.g. the index is itself a parameter, so the read genuinely cannot
    /// resolve until it is bound; the caller reports the unify failure.
    fn alias_index(&mut self, rep: NodeId, value: Option<P::Value>) -> bool {
        let Some(op) = self.pending_op(rep) else {
            return false;
        };
        let Some(indexed) = self.index_target(op) else {
            return false;
        };
        // A static element is immutable — there is no class to alias onto,
        // and the read's value is decided by the static module.  (Such a
        // read resolves through `force_pending`'s Index arm instead.)
        let AnyNodeId::Dynamic(indexed) = indexed else {
            return false;
        };
        // Only alias onto a pure cell — the read must be a plain reference,
        // not itself a computation or a concrete value.  The reader's own
        // operation is a self-read of the target's class once the
        // evaluation-time alias joined them, so it does not make the target
        // a pending computation.
        let target = disjoint::find(&mut self.nodes, indexed);
        if !self.class_is_pure_cell(target) || !is_unbound(self.nodes[target].value) {
            return false;
        }
        if let Some(value) = value.filter(|v| !is_unbound(Some(*v))) {
            // Pin the read: merge with the element and replicate the value
            // over the class.  The read keeps its operation — the operand
            // edge must survive for the apply's clone to reach the parameter
            // and enforce the pin.
            self.bind(op, indexed, Some(value), None);
        } else {
            // A plain alias: the read *is* the element, no computation
            // remains.  The node must stay well-formed — every node is
            // either value-carrying or operation-carrying — so an aliased
            // read with no cached value takes the marker, reading as the
            // pure cell it now is.
            self.add_equality(op, indexed);
            self.nodes[op].operation = None;
            if self.nodes[op].value.is_none() {
                self.write_node_value(op, Some(P::Value::from(LowValue::Parameterized)));
            }
        }
        true
    }

    /// Join `reader` into `target`'s class when the target is a pure cell —
    /// the evaluation-side counterpart of [`Self::alias_index`].  A read of
    /// an inference variable is a reference, so the reader unifies with the
    /// cell through the *standard* unify: both unbound → the classes merge,
    /// and a reader whose class already carries a value (an annotation over
    /// the read) replicates it onto the cell — a later conflicting bind then
    /// fails against it, exactly as if the read had been evaluated after the
    /// bind.  The guard is the precondition for the unify's bind path: a
    /// concrete or pending target would force this (mid-evaluation) reader
    /// and re-enter it.  The reader keeps its operation — the operand edge
    /// must stay live for the apply's clone machinery, and for the unify pin
    /// path to find the read.
    pub(crate) fn alias_read(&mut self, reader: NodeId, target: NodeId) -> bool {
        let rep = disjoint::find(&mut self.nodes, target);
        if self.class_has_pending_op(rep) || !is_unbound(self.nodes[rep].value) {
            return false;
        }
        self.unify(reader, target);
        true
    }

    /// The class member carrying the committed value, if any — the same walk
    /// [`Self::class_committed_value`] performs, but naming the node so a
    /// caller can read the encoding behind the value (an array's element
    /// nodes are reachable only from a node, not from the value alone).
    pub fn class_committed_node(&self, rep: NodeId) -> Option<NodeId> {
        self.class_members(rep).find(|&member| self.has_run(member))
    }

    /// The concrete value `rep`'s class has already committed, if any — the
    /// side of a deferred unification that is not the pending computation
    /// itself.  Scans the member list rather than reading only the
    /// representative's slot, because a bare [`Self::add_equality`] merge
    /// leaves the committed value where it was (the representative may be the
    /// value-less pending op node).
    pub(crate) fn class_committed_value(&self, rep: NodeId) -> Option<P::Value> {
        let member = self.class_committed_node(rep)?;
        self.nodes[member].value
    }

    /// Force the first unevaluated operation in `rep`'s class.  When the
    /// computation resolves, its value is replicated to the class's pure
    /// cells so reads stay locally correct — operation-bearing members keep
    /// their own computed value — and the value is returned.  `None` when
    /// the computation stays lazy, because its operands are still unbound.
    #[stacksafe]
    fn force_pending(&mut self, rep: NodeId) -> Option<P::Value> {
        let member = self.pending_op(rep)?;
        let block = self.nodes[member].block;
        // Capture the value the class already committed *before* forcing — the
        // outcome of a pending computation must reconcile with it (a pending
        // computation unified against a concrete value defers the check to
        // this moment, per [`Self::unify_inner`]).  The committed value lives
        // on whichever member carries it, not necessarily the representative
        // (a bare `add_equality` merge leaves it where it was), so scan.
        let prior = self.class_committed_value(rep);
        let value = self.evaluate_node(Dyn(member), Some(block));
        if is_unbound(Some(value)) {
            return None;
        }
        // The computation resolved: it must agree with the value its class was
        // unified against.  A free cell in the committed value is a wildcard
        // (it binds to the computed result); a concrete conflict is the
        // deferred error surfacing now, at the moment the computation ran.
        if let Some(prior) = prior {
            let mut path = AncestorPairs::new();
            if !self.reconcile_value(prior, value, &mut path) {
                self.unify_errors.push(UnifyError {
                    root_a: rep,
                    root_b: member,
                    steps: Vec::new(),
                    a: rep,
                    b: member,
                    value_a: Some(prior),
                    value_b: Some(value),
                });
            }
        }
        // Route through the single write API: it replicates the value to the
        // class's unbound pure-cell members (the same set the loop below
        // visited) and to the representative itself.
        self.write_node_value(rep, Some(value));
        Some(value)
    }

    /// Whether a computed result `b` is compatible with the value `a` that a
    /// class committed (the other side of a deferred unification).  An unbound
    /// cell on either side is a wildcard — it binds to the other — so a free
    /// pattern element (`?elem`) matches whatever the computation produced;
    /// two concrete values must agree.  Arrays recurse elementwise; a cycle is
    /// cut by the path guard (the self-referential universe).  Pure, read-only:
    /// the counterpart of [`Self::key_eq`](crate::table::Module::key_eq), but
    /// tolerant of the unbound cells a checked class may still hold.
    fn reconcile_value(
        &self,
        a: P::Value,
        b: P::Value,
        path: &mut AncestorPairs<AnyNodeId>,
    ) -> bool {
        // A free (unbound) cell matches anything — it resolves by binding.
        if is_unbound(Some(a)) || is_unbound(Some(b)) {
            return true;
        }
        match (a.as_enum(), b.as_enum()) {
            (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                // SAFETY: `pa`/`pb` are values the caller read out of live
                // nodes of this module (`Self::node_value`), so their home
                // blocks have not been dropped.
                let (left, right) = (unsafe { pa.items() }, unsafe { pb.items() });
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right.iter())
                        .all(|(ia, ib)| self.reconcile_node(ia.node, ib.node, path))
            }
            _ => a.value_eq(&b),
        }
    }

    /// [`Self::reconcile_value`] at the node level.
    fn reconcile_node(
        &self,
        a: AnyNodeId,
        b: AnyNodeId,
        path: &mut AncestorPairs<AnyNodeId>,
    ) -> bool {
        if a == b {
            return true;
        }
        if path.contains(a, b) {
            return true;
        }
        path.insert(a, b);
        let ok = match (self.node_value(a), self.node_value(b)) {
            (Some(va), Some(vb)) => self.reconcile_value(va, vb, path),
            // A node without a value is unknown (free or released) — a
            // wildcard, never a conflict.
            _ => true,
        };
        path.remove(a, b);
        ok
    }

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
