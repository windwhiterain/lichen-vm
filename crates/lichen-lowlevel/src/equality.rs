use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, FunctionIdentity,
    FunctionTypeUnify, LowOperator, LowShape, LowValue, Module, Node, NodeId, Operation, Program,
    StaticFunctionRef, StaticModuleCache, StaticNodeId, ValueExt as _, ancestors::AncestorPairs,
    is_unbound,
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

    /// Replicate a concrete `value` over the **unbound pure cells** of
    /// `representative`'s class — the second half of
    /// [`Self::write_node_value`], shared with [`Self::add_equality`], where a
    /// merge carries the class's decided value to the members it adds exactly
    /// as a write carries it to the members it finds.
    ///
    /// An **operation-bearing** member is deliberately not written: its own
    /// computation is what settles it, and a value arriving from elsewhere is
    /// not a proof of what that computation will produce.
    fn replicate_class_value(&mut self, representative: NodeId, value: P::Value) {
        let members: Vec<NodeId> = self.class_members(representative).collect();
        for member in members {
            if self.nodes[member].operation.is_none() && is_unbound(self.nodes[member].value) {
                self.nodes[member].value = Some(value);
            }
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
        // **Unify does two things: it unions, and it reports a conflict.**  There
        // is no third case to classify: an undecided side is not a reason to
        // refuse (unify is called unconditionally), a decided side is not a
        // reason to write into anyone, and an operation on either side is not a
        // reason to compute first.  What a class's computation produces is
        // reconciled when it runs; a reader that needs a value finds it through
        // the class.
        if is_unbound(va) || is_unbound(vb) {
            if !self.union_with_value(ra, rb, va, vb) {
                self.record_error(ra, rb, steps, root);
                return false;
            }
            return true;
        }
        let ra = disjoint::find(&mut self.nodes, ra);
        let rb = disjoint::find(&mut self.nodes, rb);
        // A function-type node — the self-referential `[Function(fid), ↺]`
        // that is a function's own type (`f : f`) — on either side is handled
        // by the program's clone-on-unify policy before the positional match.
        // The positional match would otherwise either wrongly *merge* two
        // self-referential function-types (binding the shared template's
        // cells, the defect this fixes) or clash a `Function` slot 0 against
        // an array. The policy clones the function's signature and unifies the
        // clone, leaving the template untouched; on success the two sides are
        // resolved *without* merging classes (the function-type stays a
        // distinct, polymorphic class).
        if self.is_function_type_node(ra) || self.is_function_type_node(rb) {
            match P::unify_function_type(self, ra, rb) {
                FunctionTypeUnify::Handled => return true,
                FunctionTypeUnify::Conflict => {
                    self.record_error(ra, rb, steps, root);
                    return false;
                }
                // One side is a self-referential `[Function, ↺]` the program
                // does not treat as a function-type (a build with no highlevel,
                // which never builds one): fall through to the positional rules.
                FunctionTypeUnify::NotFunctionType => {}
            }
        }
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
            // A materialized static closure and the frozen function it came
            // from name **one** logical function: their `Function` values are
            // equal by identity even though one is dynamic and the other
            // static.  Checked before the generic value comparison, which
            // compares `AnyFunctionId` by kind and would call them different.
            (Some(LowValue::Function(a)), Some(LowValue::Function(b)))
                if self.function_identity_equal(a, b) =>
            {
                self.add_equality(ra, rb);
                true
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

    /// Merge two classes and settle what the unify asserted.
    /// **Union two classes and let the merged class hold what one of them
    /// already knows.**  [`Self::add_equality`] is the union itself and writes
    /// nothing; this wrapper is the unifier's one place of writing, so the
    /// "unify merges classes" and "a class carries a value to its members"
    /// responsibilities stay separable — the former is also used by callers
    /// that must not write (a fresh clone's class, a pin).
    ///
    /// The value comes from whichever side has a decided one.  Both sides may
    /// hold one (the caller compares values before calling this), and a decided
    /// value always reaches every unbound pure cell of the merged class
    /// ([`Self::write_node_value`]'s replication), which is what makes a later
    /// member read the value its class already had.
    ///
    /// **A write never contradicts what the class already holds.**  When the
    /// class carries a decided value and the merge brings a *different* decided
    /// one, that is a conflict — reported, not overwritten.  This is the
    /// unifier's half of "two values that must be equal are not": the
    /// *computation*-versus-value half happens when the computation runs
    /// ([`Self::reconcile_node_claim`]), and neither needs the computation to be
    /// forced here.
    fn union_with_value(
        &mut self,
        ra: NodeId,
        rb: NodeId,
        va: Option<P::Value>,
        vb: Option<P::Value>,
    ) -> bool {
        let incoming = if is_unbound(va) { vb } else { va };
        let incoming = incoming.filter(|value| !is_unbound(Some(*value)));
        // A class that already holds a decided value and a union that brings a
        // *different* one is a conflict, and the comparison has to be the
        // **structural** one — `values_agree` walks the elements, where
        // [`Self::value_eq`] would call two equal-content arrays different.
        if incoming.is_some()
            && (self.class_committed_value(ra).is_some()
                || self.class_committed_value(rb).is_some())
            && !self.values_agree(ra, rb)
        {
            return false;
        }
        let rep = self.add_equality(ra, rb);
        // PROBE: what this union writes, to which **class** (representative), and
        // which node carries the operator.
        if let Some(value) = incoming.as_ref() {
            let len = value.as_enum().map(|value| match value {
                LowValue::Array(array) => {
                    // SAFETY: the payload is read from a value of a live node.
                    unsafe { array.items() }.len()
                }
                _ => usize::MAX,
            });
            eprintln!(
                "PROBE union write: rep={rep:?} ra={ra:?} rb={rb:?} len={len:?} \
                 ra_op={:?} rb_op={:?}",
                self.nodes[ra]
                    .operation
                    .map(|operation| format!("{:?}", operation.operator)),
                self.nodes[rb]
                    .operation
                    .map(|operation| format!("{:?}", operation.operator)),
            );
        }
        if let Some(value) = incoming {
            self.write_node_value(rep, Some(value));
        }
        true
    }

    /// Whether two decided values are the same value (the unifier's comparison,
    /// arrays elementwise).
    fn value_eq(&self, a: P::Value, b: P::Value) -> bool {
        let mut path = AncestorPairs::new();
        self.reconcile_value(a, b, &mut path)
    }

    /// **Structural** agreement between two decided values — the comparison a
    /// reconciliation needs, and deliberately not [`Self::value_eq`].
    ///
    /// `value_eq` is shallow by contract: an array is one allocation, so two
    /// arrays compare equal only when they share it, and it is unification's own
    /// elementwise recursion that answers the structural question
    /// (`ValueExt::value_eq`).  A type is routinely two arrays with equal
    /// contents and distinct allocations — `Int` twice, say — so the shallow
    /// comparison calls them different and the reconciliation refuses a
    /// *matching* value.  This walks the elements instead: a free cell matches
    /// anything, two arrays recurse positionally (each element through its own
    /// node, so an element that is itself a type is compared structurally too),
    /// and anything else falls back to the value comparison.
    fn value_matches(&self, a: P::Value, b: P::Value) -> bool {
        self.value_matches_inner(a, b, 0)
    }

    /// [`Self::value_matches`] with a depth bound, so a self-referential type
    /// (the universe) cannot recurse forever.  A value's tree is as deep as the
    /// program's own nesting; past the bound the comparison gives the benefit of
    /// the doubt, which is what the unifier's cycle guard does too.
    fn value_matches_inner(&self, a: P::Value, b: P::Value, depth: usize) -> bool {
        const MAX_VALUE_DEPTH: usize = 64;
        if depth >= MAX_VALUE_DEPTH {
            return true;
        }
        if is_unbound(Some(a)) || is_unbound(Some(b)) {
            return true;
        }
        match (a.as_enum(), b.as_enum()) {
            (Some(LowValue::Array(pa)), Some(LowValue::Array(pb))) => {
                // SAFETY: `pa`/`pb` are values of live nodes of this module, so
                // their home blocks have not been dropped.
                let (left, right) = (unsafe { pa.items() }, unsafe { pb.items() });
                left.len() == right.len()
                    && left.iter().zip(right.iter()).all(|(ia, ib)| {
                        let (va, vb) = (self.node_value(ia.node), self.node_value(ib.node));
                        match (va, vb) {
                            (Some(va), Some(vb)) => self.value_matches_inner(va, vb, depth + 1),
                            // An element without a value is unknown — a
                            // wildcard, never a conflict.
                            _ => true,
                        }
                    })
            }
            _ => {
                // An element the value slot does not hold — `None` or the lazy
                // marker — is unknown, exactly as it is at the top of this
                // function: a wildcard, never a conflict.
                let (ea, eb) = (a.as_enum(), b.as_enum());
                if ea.is_none() || eb.is_none() {
                    return true;
                }
                if ea == Some(LowValue::Parameterized) || eb == Some(LowValue::Parameterized) {
                    return true;
                }
                let same = a.value_eq(&b);
                same
            }
        }
    }

    /// Whether the two classes' decided values **agree structurally** — the
    /// comparison a union uses to decide that the value it carries is the value
    /// the class already holds.
    ///
    /// It has to be the *structural* one, never [`Self::value_eq`]: the two
    /// sides of a type are routinely two arrays with equal contents but distinct
    /// allocations, and `value_eq` documents that two arrays compare equal only
    /// when they share their allocation (`ValueExt::value_eq`) — its own
    /// elementwise recursion is unification's, not its.  So this walks the
    /// elements exactly as the unifier does (`reconcile_node`): a free cell
    /// matches anything, two arrays recurse positionally, and a cycle is cut by
    /// the path guard.
    fn values_agree(&self, a: NodeId, b: NodeId) -> bool {
        let mut path = AncestorPairs::new();
        self.reconcile_node(Dyn(a), Dyn(b), &mut path)
    }

    /// Reconcile a computation's freshly produced `value` against the `prior`
    /// value its class held — the deferred half of a unification that wrote a
    /// value while the computation could not run.  A free cell in `prior` is a
    /// wildcard; two concrete values that disagree are recorded as a conflict.
    pub(crate) fn reconcile_computed(
        &mut self,
        node: NodeId,
        prior: Option<P::Value>,
        value: P::Value,
    ) {
        let Some(prior) = prior.filter(|prior| !is_unbound(Some(*prior))) else {
            return;
        };
        if !self.value_matches(prior, value) {
            // PROBE: the class's members and which one holds the prior value.
            let rep = disjoint::find(&mut self.nodes, node);
            let members: Vec<String> = self
                .class_members(rep)
                .map(|member| {
                    format!(
                        "{member:?}(op={:?}, has_value={})",
                        self.nodes
                            .get(member)
                            .and_then(|entry| entry.operation)
                            .map(|operation| format!("{:?}", operation.operator)),
                        self.nodes
                            .get(member)
                            .is_some_and(|entry| entry.value.is_some()),
                    )
                })
                .collect();
            eprintln!(
                "PROBE reconcile reject: node={node:?} rep={rep:?} node_op={:?} members=[{}]",
                self.nodes
                    .get(node)
                    .and_then(|entry| entry.operation)
                    .map(|operation| format!("{:?}", operation.operator)),
                members.join(", "),
            );
            self.unify_errors.push(UnifyError {
                root_a: node,
                root_b: node,
                steps: Vec::new(),
                a: node,
                b: node,
                value_a: Some(prior),
                value_b: Some(value),
            });
        }
    }

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

    /// Whether `rep`'s class holds a suspended **field/positional read**: an
    /// `Index` operation whose operand container (or index) is not yet
    /// concrete, so the read is a lazy reference that resolves once the
    /// container binds.  Distinct from a resolved read and from a non-`Index`
    /// model computation (arithmetic, a dependent-type branch).
    fn class_has_index_read(&self, rep: NodeId) -> bool {
        let Some(op) = self.class_first_op(rep) else {
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

    /// The first operation-bearing member of `rep`'s class, if any — the node
    /// whose operator the class's computation belongs to.
    fn class_first_op(&self, rep: NodeId) -> Option<NodeId> {
        self.class_members(rep)
            .find(|&member| self.nodes[member].operation.is_some())
    }

    /// Join `reader` into `target`'s class when the target is a pure cell —
    /// the evaluation-side counterpart of the read's own resolution.  A read of
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
    /// side of a unification that is not the computation itself.  Scans the
    /// member list rather than reading only the representative's slot, because
    /// a bare [`Self::add_equality`] merge leaves the value where it was (the
    /// representative may be the value-less operation node).
    pub(crate) fn class_committed_value(&self, rep: NodeId) -> Option<P::Value> {
        let member = self.class_committed_node(rep)?;
        self.nodes[member].value
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
