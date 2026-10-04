//! The [`Module`] inherent API: construction, the freeze conveniences, node,
//! function and assert construction, and the node-state readers.

use super::*;
impl<P: Program> Default for Module<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Module<P> {
    pub const MAX_APPLY_DEPTH: usize = 10_000;
    /// The default total-application budget.  Each application clones its
    /// function's body (~tens of nodes), so this also bounds the module's
    /// growth — an indeterminate recursion stops before it drains memory.
    pub const MAX_APPLY_TOTAL: usize = 100_000;
    pub const MAX_DEEP_DEPTH: usize = 300_000;

    pub fn new() -> Self {
        Self::with_registry(Arc::new(RwLock::new(Registry::new())))
    }

    /// A module bound to the given registry — every static ref it touches
    /// resolves through it.  Every module in one thread shares the one
    /// registry `Arc` ([`Registry::new_module`]); a standalone module owns
    /// a private registry, which is the same thing at one-module scale.
    pub(super) fn with_registry(registry: Arc<RwLock<Registry<P>>>) -> Self {
        Module {
            registry,
            nodes: SlotMap::with_key(),
            blocks: SlotMap::with_key(),
            functions: SlotMap::with_key(),
            apply_depth_limit: Self::MAX_APPLY_DEPTH,
            apply_total_limit: Self::MAX_APPLY_TOTAL,
            evaluate_depth_limit: Self::MAX_DEEP_DEPTH,
            unify_errors: Vec::new(),
            eval_errors: Vec::new(),
            asserts: Vec::new(),
            assert_errors: Vec::new(),
            apply_errors: Vec::new(),
            apply_error_nodes: HashSet::new(),
            extension_diagnostics: Vec::new(),
            global_ext: P::GlobalExt::default(),
            apply_depth: 0,
            apply_total: 0,
            deep_depth: 0,
            budget_exhausted: None,
        }
    }

    /// Compile `source` into a static artifact and register it with this
    /// module's registry under `key` — the device key allocated by the
    /// device registry (the caller provides it so the artifact's refs are
    /// baked with their final key).  Convenience over [`Registry::freeze`].
    /// `source` must be a different module — freezing a module into itself
    /// would deadlock its own registry lock.
    pub fn freeze(&mut self, source: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> ModuleKey {
        self.freeze_mapped(source, key, hash).key
    }

    /// Compile `source` into a static artifact and register it with this
    /// module's registry under `key`, returning both the device key and the
    /// source→statics node map.  Convenience over
    /// [`Registry::freeze_mapped`].
    pub fn freeze_mapped(&mut self, source: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> Freeze {
        // Hard in release too: a self-freeze (reachable only through a raw
        // pointer, since the borrows of `self` and `source` exclude it)
        // deadlocks on the registry write lock.
        assert!(
            !std::ptr::eq(self, source),
            "freezing a module into itself would deadlock its registry lock"
        );
        self.registry
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .freeze_mapped(source, key, hash)
    }

    /// Resets the per-run evaluation budgets ([`Self::apply_depth`],
    /// [`Self::apply_total`], [`Self::deep_depth`]) so a
    /// host can drive the module in a long-running loop (e.g. one kernel call
    /// per GUI frame) without the cumulative apply count exhausting
    /// [`Self::apply_total_limit`]. The budgets guard *one* run; a host that
    /// resets them per run keeps the guard while shedding lifetime
    /// accumulation. The limits themselves are unchanged.
    ///
    /// [`Self::budget_exhausted`] resets with them: it is a per-run verdict,
    /// so a host starting a new run must not read the previous run's
    /// refusal.
    pub fn reset_apply_budget(&mut self) {
        self.apply_depth = 0;
        self.apply_total = 0;

        self.deep_depth = 0;
        self.budget_exhausted = None;
    }

    pub fn add_block(&mut self, parent: Option<BlockId>) -> BlockId {
        let block = self.blocks.insert(Block {
            arena: Bump::new(),
            parent,
            children: Vec::new(),
            nodes: Vec::new(),
            functions: Vec::new(),
        });
        if let Some(parent) = parent {
            self.blocks[parent].children.push(block);
        }
        block
    }

    pub fn add_node(
        &mut self,
        block: BlockId,
        operation: Option<Operation<P>>,
        value: Option<P::Value>,
    ) -> NodeId {
        let node = self.nodes.insert(Node {
            value,
            operation,
            low_shape: None,
            function: None,
            origin: None,
            block,
            visiting: false,
            evaluated_deep: None,
            assumed_concrete: false,
            equality: disjoint::Meta::default(),
        });
        disjoint::make_set(&mut self.nodes, node);
        self.blocks[block].nodes.push(node);
        // Allocation with a concrete value is the second of the two value-write
        // sites (`write_node_value` is the first), so the observation runs here
        // too — otherwise a literal, a marker, or a freshly built array would
        // carry no low type at all, and only the evaluated spine ever would.
        if let Some(value) = value.filter(|v| !is_unbound(Some(*v))) {
            self.observe_class_low_type(node, value);
        }
        node
    }

    /// The block `node` is homed in — the garbage-collection unit
    /// [`Self::garbage_collect`] moves the node out of when it is released,
    /// and the arena its compound payloads live in.  A node is homed in
    /// exactly one live block; a released block's nodes are removed with it.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_block(&self, node: NodeId) -> BlockId {
        self.nodes[node].block
    }

    /// The function whose template owns `node`, or [`None`] when the node
    /// belongs to no template (a top-level node, or one created at runtime).
    ///
    /// A reader may rely on the tag naming the function that owns the node:
    /// the apply clone walk's membership test is this tag's chain through
    /// [`Function::parent`] reaching the applied function — *not* a lookup
    /// in [`Function::nodes`] — so a wrongly tagged node is cloned or
    /// referenced as a member of the wrong template.  A node joins a body
    /// through [`Self::register_in_function`], which decides the tag and the
    /// scope list together; the clone walks re-stamp the clones they
    /// instantiate, and a function's own value node is tagged without
    /// joining the scope ([`Self::add_function`]).
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_function(&self, node: NodeId) -> Option<FunctionId> {
        self.nodes[node].function
    }

    /// The node `node` is **attributed through**, or [`None`] when `node` is
    /// not an apply clone.
    ///
    /// An apply instantiates a function template by cloning the members that
    /// depend on the call's argument, and each clone records here the node the
    /// layer above attributes a failure in it by ([`Self::node_function`] names
    /// the *template* the clone joined; this names the node *within* it).  A
    /// reader that holds a clone because a runtime failure mentioned it — the
    /// per-call node a recorded [`EvalError`](crate::EvalError) names — can
    /// therefore reach the node an attribution table holds.
    ///
    /// Which node that is depends on the template.  A **dynamic** clone records
    /// the template node it instantiates, which is what per-node attribution is
    /// keyed by.  A clone of a **frozen** template records the apply that
    /// materialized it: the template's nodes belong to the frozen module, so no
    /// node of this module stands for them, and the call is the only
    /// caller-side fact the failure belongs to.  Either way following the
    /// origin reaches a node the checker attributed, so one step suffices.
    ///
    /// The origin is **not a keep-alive edge**: garbage collection moves each
    /// node with its own home block, so an origin node whose block was released
    /// is absent from [`Self::nodes`] — use [`Self::nodes`]' `get` before
    /// reading it rather than assuming liveness.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_origin(&self, node: NodeId) -> Option<NodeId> {
        self.nodes[node].origin
    }

    /// The operation `node` computes — its operator and single operand edge
    /// — or [`None`] for a node that carries a value instead.
    ///
    /// A reader may rely on the operation being **fixed once**: it comes
    /// from [`Self::add_node`] or, when the operand is only nameable after
    /// the node exists, from [`Self::close_operation_cycle`].  It is never
    /// replaced, so an operand edge read once is the edge that computes the
    /// node.  [`Some`] does not mean "unevaluated": an operation node caches
    /// its result, and [`Self::node_value`] is the current value.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_operation(&self, node: NodeId) -> Option<Operation<P>> {
        self.nodes[node].operation
    }

    /// Whether an evaluation attempt — or a deep-pass descent cut against
    /// it — is computing `node` **at this moment**.
    ///
    /// A reader may rely on the mark being a *liveness* flag, not a "was
    /// visited" flag.  It is taken when a frame starts computing the node and
    /// released when that frame exits, on the cached-answer, lazy-answer and
    /// unwinding-panic paths alike (see the invariant on the module's
    /// evaluation-attempt mark, `retain_node`), so `true` means an active
    /// frame holds the node right now; it never means "already evaluated"
    /// (read [`Self::node_value`]) and never means "known concrete" (read
    /// [`Self::node_evaluated_deep`]).  Because the mark is never sticky,
    /// `true` on a node with no cached value is a genuine cyclic read.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_visiting(&self, node: NodeId) -> bool {
        self.nodes[node].visiting
    }

    /// The deep pass's verdict for `node`, or [`None`] when it never ran
    /// there.
    ///
    /// A reader may rely on [`Some`] meaning the deep pass
    /// ([`Self::evaluate_node_deep`], [`Self::evaluate_node_forced`]) ran on
    /// this node and [`EvaluatedDeep::parameterized`] recording whether any
    /// node in its reachable subtree is [`LowValue::Parameterized`] — i.e.
    /// whether the pass could **not** prove the subtree concrete.  [`None`]
    /// means concreteness is *unknown*, which a reader must treat as
    /// parameterized, never as proven concrete: the apply clone walk and the
    /// operation postlude both do, and a budget refusal as well as a node
    /// reached only as an operand leave [`None`].  The verdict covers the
    /// node's graph at the time it was reached; it is cleared when a late
    /// operation edge is added ([`Self::close_operation_cycle`]).
    ///
    /// **One case has no verdict yet.**  A node a **cycle cut** re-entered — the
    /// structural cycle cut in `evaluate_node_deep_inner` — is reached by a
    /// parent's verdict computation before its own frame writes, so this field
    /// still reads [`None`] there.  The verdict computation assumes such a node
    /// **concrete**: that is the coinductive step which lets a self-referential
    /// value be proven at all (the canonical universe `[Type, ↺]` is reached
    /// from inside its own descent).  The two `None` cases are told apart by the
    /// cut's own mark on the node, never by this field alone; the defect of
    /// conflating them was `P1-31` in `docs/notes/code-audit.md`.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_evaluated_deep(&self, node: NodeId) -> Option<EvaluatedDeep> {
        self.nodes[node].evaluated_deep
    }

    /// The disjoint-set metadata of `node`'s equality class, as
    /// [`Self::add_equality`] maintains it.
    ///
    /// A reader may rely on `parent` being the union-find link ([`None`]
    /// meaning `node` is its own root) and on `next`, `tail` and `size`
    /// describing the class's member list — which is meaningful only at the
    /// representative.  A reader that needs the representative must not
    /// follow `parent` by hand: use [`Self::equality_representative`], which
    /// also compresses the path.  The metadata is written **only** by the
    /// union-find (merge classes with [`Self::add_equality`]); writing a
    /// parent link by hand would break the size bound and the member list at
    /// once.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_equality(&self, node: NodeId) -> disjoint::Meta<NodeId> {
        self.nodes[node].equality
    }

    /// **Close an operation cycle** — define the operation of a node whose
    /// operand is only nameable after the node itself exists.
    ///
    /// [`Self::add_node`] takes a node's operation at allocation, which is
    /// enough for an acyclic graph: the operands are allocated first.  A
    /// cycle is not — a recursive value's operation names a node allocated
    /// after it, up to the self-referential `node := op(node)` — so it needs
    /// this late half of a two-phase construction, which is the only way to
    /// give an existing node an operation.
    ///
    /// Contract:
    /// - `node` must be freshly allocated with **no** operation: an
    ///   operation is defined once, never replaced (replacing it would
    ///   strand the previous operand edge and contradict every cached value
    ///   and deep verdict derived through it).
    /// - `node` must not already hold a concrete value: a decided node is
    ///   never evaluated again, so its operation would never run — the edge
    ///   would be dead.
    /// - The node's deep-pass verdict ([`Self::node_evaluated_deep`]) is
    ///   cleared, because the proof predates this operand edge and no longer
    ///   describes the node's graph.  On a freshly allocated node the
    ///   verdict is already [`None`], so the clearing is a no-op there; it
    ///   is what makes the contract hold for any other node.
    pub fn close_operation_cycle(&mut self, node: NodeId, operation: Operation<P>) {
        debug_assert!(
            self.nodes[node].operation.is_none(),
            "a node's operation is defined once: {node:?} already computes one"
        );
        debug_assert!(
            is_unbound(self.nodes[node].value),
            "an operation must not be defined on a node that already holds a concrete value: {node:?}"
        );
        self.nodes[node].operation = Some(operation);
        self.nodes[node].evaluated_deep = None;
        // The cycle-cut assumption (if any) predates this edge too, and no frame
        // is computing the node here, so clearing the verdict without it would
        // leave a live "assumed concrete".
        self.nodes[node].assumed_concrete = false;
    }

    /// Register `node` in `function`'s body scope: tag the node as owned by
    /// `function` ([`Self::node_function`]) **and** append it to
    /// [`Function::nodes`].  Both halves are written together because they
    /// are read for different purposes and must agree: the apply clone walk
    /// follows the owner tag's chain, while garbage collection and a nested
    /// closure's clone walk start from the scope list.
    ///
    /// Contract: `node` must have just been allocated for that function's
    /// body.  The function's own value node is deliberately *not* registered
    /// here — it is reached through the template's return subtree, so
    /// [`Self::add_function`] tags it without listing it.
    pub fn register_in_function(&mut self, function: FunctionId, node: NodeId) {
        debug_assert!(
            self.functions.contains_key(function),
            "a node cannot be owned by a function that does not exist: {function:?}"
        );
        self.nodes[node].function = Some(function);
        self.functions[function].nodes.push(node);
    }

    /// Record a diagnostic a **layer above the lowlevel** produced, through the
    /// general channel ([`Self::extension_diagnostics`]).
    ///
    /// A layer records when it has decided something the lowlevel cannot
    /// describe on its behalf — a backend that refuses to lower a shape, a
    /// plugin that cannot compile a body.  The companion decision is always the
    /// caller's: a record is not an error state, and recording one never
    /// changes what the VM does next.
    ///
    /// **The same refusal about the same node is one fact.**  A node is deep-
    /// evaluated more than once — the checker walks the top-level statements and
    /// then the root, and the run walks the root again — and an operator that
    /// refuses records on each attempt.  Refusing twice is not two findings, so
    /// an identical `(category, node, message)` entry is dropped.  A linear
    /// scan is the right shape here: this channel holds the handful of things a
    /// layer declined to do, unlike `apply_errors`, which is recorded per apply
    /// and so keeps a set beside it.
    pub fn record_extension_diagnostic(
        &mut self,
        category: &'static str,
        node: Option<NodeId>,
        message: impl Into<String>,
    ) {
        let message = message.into();
        let duplicate = self
            .extension_diagnostics
            .iter()
            .any(|e| e.category == category && e.node == node && e.message == message);
        if duplicate {
            return;
        }
        self.extension_diagnostics.push(ExtensionDiagnostic {
            category,
            node,
            message,
        });
    }

    /// Registers `condition` as an assert — an explicit constraint, not a
    /// unification, so an unbound condition is *not* bound to `1`, it stays
    /// untriggered until an apply binds it.  [`Self::check_asserts`]
    /// force-evaluates every registered condition (ignoring laziness) and
    /// requires `USize(1)`, see there.  The registry is a worklist; a
    /// condition owned by a function body ([`Function::asserts`]) is cloned
    /// and re-registered per apply, so a body's assert re-checks against
    /// each call's argument.
    pub fn add_assert(&mut self, condition: NodeId) -> NodeId {
        self.asserts.push(PendingAssert {
            condition,
            template: AnyNodeId::Dynamic(condition),
        });
        condition
    }

    /// Create a function template's **shell**, before any of its nodes exist.
    ///
    /// The record this inserts is deliberately incomplete: its
    /// [`Function::parameter`] and [`Function::r#return`] are unset until
    /// [`Self::finish_function`] names them, and nothing may read a function
    /// in between.  Building the shell first is what lets a compiler emit the
    /// parameter nodes *into this function's scope*: the node allocator tags
    /// and registers each node against the function currently being built, so
    /// a parameter allocated after the shell lands in the right template
    /// without being moved there afterwards.
    ///
    /// `parent` is the enclosing template, or `None` at the top level and for
    /// a same-depth sibling (see [`Function::parent`]).
    pub fn begin_function(&mut self, block: BlockId, parent: Option<FunctionId>) -> FunctionId {
        let function = self.functions.insert(Function {
            nodes: Vec::new(),
            r#return: NodeId::default(),
            parameter: NodeId::default(),
            parent,
            asserts: Vec::new(),
            block,
            looping: false,
        });
        self.blocks[block].functions.push(function);
        function
    }

    /// Stamp `function` as a `@loop` binding — its recursion may become a loop.
    ///
    /// **Called once, while the body is being compiled**, by the layer that
    /// knows the source said `@loop`. Not later: the body is the only place the
    /// mark has to survive, because an apply clones templates away and the
    /// analysis that reads the mark runs on templates before any of that.
    ///
    /// Stamping twice is idempotent — it is a flag, and a second stamp would say
    /// nothing new — so this needs no "already stamped" bookkeeping and a
    /// caller cannot get it wrong by stamping twice.
    pub fn mark_looping(&mut self, function: FunctionId) {
        self.functions[function].looping = true;
    }

    /// Whether `function` is a `@loop` binding. See [`Function::looping`] for
    /// what the mark does and does not mean.
    pub fn function_is_looping(&self, function: FunctionId) -> bool {
        self.functions[function].looping
    }

    /// Complete the shell begun by [`Self::begin_function`], naming the
    /// return and parameter slots.
    ///
    /// Both must already be registered in the function's scope: the apply
    /// clone walk reads its members through [`Function::nodes`], and
    /// `parameter` in particular is what it instantiates, so a parameter
    /// missing from the scope is a construction error, not a runtime one.
    pub fn finish_function(&mut self, function: FunctionId, r#return: NodeId, parameter: NodeId) {
        debug_assert!(
            self.functions[function].nodes.contains(&parameter),
            "a function's parameter slot must be registered in its own scope before the shell is finished"
        );
        self.functions[function].r#return = r#return;
        self.functions[function].parameter = parameter;
    }

    pub fn add_function(
        &mut self,
        block: BlockId,
        ret: NodeId,
        param: NodeId,
        nodes: impl IntoIterator<Item = NodeId>,
        asserts: impl IntoIterator<Item = NodeId>,
    ) -> NodeId {
        let nodes: Vec<NodeId> = nodes.into_iter().collect();
        let function = self.begin_function(block, None);
        // The passed nodes are this function's template body: register each
        // with its owner, so the apply clone walk's chain membership test
        // recognizes them and garbage collection keeps them with the
        // function.  The function id must exist before the tags point at it.
        for &node in &nodes {
            self.register_in_function(function, node);
        }
        self.functions[function].asserts = asserts.into_iter().collect();
        self.finish_function(function, ret, param);
        // The value node is the function's own too — tagged with it, so an
        // enclosing template (a nested function's parent link) clones it and
        // instantiates a fresh closure per call instead of referencing the
        // template's function value in place.
        let func_node = self.add_node(
            block,
            None,
            Some(P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(
                function,
            )))),
        );
        self.nodes[func_node].function = Some(function);
        func_node
    }
}
