//! The [`Module`] inherent API: construction, freeze conveniences, and the node,
//! function and assert readers.

use super::*;
impl<P: Program> Default for Module<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Module<P> {
    pub const MAX_APPLY_DEPTH: usize = 10_000;
    /// The default total-application budget: each application clones its
    /// function's body, so this also bounds module size.
    pub const MAX_APPLY_TOTAL: usize = 100_000;
    pub const MAX_DEEP_DEPTH: usize = 300_000;

    pub fn new() -> Self {
        Self::with_registry(Arc::new(RwLock::new(Registry::new())))
    }

    /// A module bound to the given registry; a standalone one owns a private registry.
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
            extension_failures: Vec::new(),
            global_ext: P::GlobalExt::default(),
            marks: HashMap::new(),
            gc_classes: HashSet::new(),
            apply_total: 0,
            stamp_depth: 0,
            deep_depth: 0,
            // The epoch starts at one: zero is the run stamp's "never ran".
            value_epoch: 1,
            budget_exhausted: None,
        }
    }

    /// Compile `source` into a static artifact and register it under `key` — see
    /// [`Registry::freeze`].
    pub fn freeze(&mut self, source: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> ModuleKey {
        self.freeze_mapped(source, key, hash).key
    }

    /// [`Registry::freeze_mapped`], plus the source→statics node map.
    pub fn freeze_mapped(&mut self, source: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> Freeze {
        // Hard in release too: a self-freeze deadlocks the registry write lock.
        assert!(
            !std::ptr::eq(self, source),
            "freezing a module into itself would deadlock its registry lock"
        );
        self.registry
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .freeze_mapped(source, key, hash)
    }

    /// Reset the per-run evaluation budgets for a host driving the module in a loop.
    ///
    /// # Invariant
    ///
    /// The budgets guard *one* run; the limits are unchanged, and the nesting guard needs
    /// no reset — it reads a node's [`depth`](Module::node_depth), a fact of the graph.
    /// [`Self::budget_exhausted`] resets with them: it is a per-run verdict, so a new run
    /// must not read the previous one's refusal.
    pub fn reset_apply_budget(&mut self) {
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
            scope_root: None,
        });
        if let Some(parent) = parent {
            self.blocks[parent].children.push(block);
        }
        block
    }

    /// Declare `node` the node whose evaluation collects the block it is homed in.
    ///
    /// # Invariant
    ///
    /// `node` is homed in a block with a parent, so a collect has somewhere to move
    /// the reachable subtree to; a layer that cannot say which of its nodes ends the
    /// scope does not call this, and that block keeps the parent-block-read rule.
    pub fn mark_scope_root(&mut self, node: NodeId) {
        self.blocks[self.nodes[node].block].scope_root = Some(node);
    }

    pub fn add_node(
        &mut self,
        block: BlockId,
        operation: Option<Operation<P>>,
        value: Option<P::Value>,
    ) -> NodeId {
        let node = self.nodes.insert(Node {
            value,
            runned: 0,
            operation,
            low_shape: None,
            function: None,
            origin: None,
            depth: self.stamp_depth,
            block,
            visiting: false,
            evaluated_deep: None,
            assumed_concrete: false,
            equality: disjoint::Meta::default(),
        });
        disjoint::make_set(&mut self.nodes, node);
        self.blocks[block].nodes.push(node);
        // Allocation with a concrete value is the second of the two value-write sites,
        // so the observation runs here too.
        if let Some(value) = value {
            self.observe_class_low_type(node, value);
        }
        node
    }

    /// How many apply levels `node` was created under: `0` for a checker-built
    /// node, `d + 1` for a clone made at depth `d`.
    pub fn node_depth(&self, node: NodeId) -> u32 {
        self.nodes[node].depth
    }

    /// Record that the value `value` carries the marks `payloads`.
    ///
    /// # Invariant
    /// Several marks may pin one expression (`e ~ "a" ~ "b"`), and they accumulate in the
    /// order the checker stamped them. The stamp happens **once**, where the marked expression
    /// lowered, and no clone walk carries it: a clone reaches its template's marks through
    /// [`Self::marks_of`], which is what makes the anchor survive a per-call clone.
    pub fn stamp_marks(&mut self, value: NodeId, payloads: Vec<String>) {
        self.marks.entry(value).or_default().extend(payloads);
    }

    /// The marks the value `node` carries, resolved through the apply clone walk's origins.
    ///
    /// # Invariant
    /// The walk is the **anchor handoff**: a body the dispatcher compiles is a clone, so the
    /// clone itself carries nothing and its origin — the template node the checker stamped —
    /// carries the mark. An origin always names an older node, so the walk terminates, and a
    /// node released by [`Self::garbage_collect`] reads as no mark rather than panicking.
    pub fn marks_of(&self, node: NodeId) -> &[String] {
        self.marks_at(node)
            .map(|(_, payloads)| payloads)
            .unwrap_or_default()
    }

    /// The marks `node` carries, and the node they were stamped on.
    ///
    /// # Invariant
    /// The stamped node is **not** `node` when the walk crossed an origin, and it is what a
    /// reader de-duplicates on: an expanded recurrence lowers the same anchor once per copy,
    /// and those are one mark on one expression rather than one mark per copy.
    pub fn marks_at(&self, node: NodeId) -> Option<(NodeId, &[String])> {
        let mut current = node;
        while self.nodes.contains_key(current) {
            if let Some(payloads) = self.marks.get(&current) {
                return Some((current, payloads));
            }
            match self.nodes[current].origin {
                Some(origin) => current = origin,
                None => return None,
            }
        }
        None
    }

    /// The block `node` is homed in, and the arena its payloads live in.
    /// Panics if `node` is not in [`Self::nodes`].
    ///
    /// # Invariant
    ///
    /// A node is homed in exactly one live block, and a released block's nodes go
    /// with it — [`Self::garbage_collect`] moves a node out only when it is released.
    pub fn node_block(&self, node: NodeId) -> BlockId {
        self.nodes[node].block
    }

    /// The function whose template owns `node`; [`None`] when it owns none.
    /// Panics if `node` is not in [`Self::nodes`].
    ///
    /// # Invariant
    ///
    /// The tag names the owning function: the apply clone walk's membership test is
    /// this tag's chain through [`Function::parent`] reaching the applied function —
    /// *not* a lookup in [`Function::nodes`] — so a wrongly tagged node is cloned as a
    /// member of the wrong template. A node joins a body through
    /// [`Self::register_in_function`], which writes the tag and the scope together.
    pub fn node_function(&self, node: NodeId) -> Option<FunctionId> {
        self.nodes[node].function
    }

    /// The node `node` is **attributed through**, [`None]` when not a clone.
    /// Panics if `node` is not in [`Self::nodes`].
    ///
    /// # Invariant
    ///
    /// A **dynamic** clone records the template node it instantiates, which per-node
    /// attribution is keyed by; a clone of a **frozen** template records the apply that
    /// materialized it. The origin is **not a keep-alive edge**: GC moves each node with
    /// its own block, so use [`Self::nodes`]' `get` rather than assuming liveness.
    pub fn node_origin(&self, node: NodeId) -> Option<NodeId> {
        self.nodes[node].origin
    }

    /// The operation `node` computes, or [`None`] for a node carrying a value.
    /// Panics if `node` is not in [`Self::nodes`].
    ///
    /// # Invariant
    ///
    /// The operation is **fixed once** ([`Self::add_node`], or [`Self::close_operation_cycle`]
    /// when the operand is only nameable later) and never replaced, so an operand edge read
    /// once is the edge that computes the node. [`Some`] does not mean "unevaluated": an
    /// operation node caches its result, and [`Self::has_no_result_yet`] answers whether a
    /// read would still run the operator.
    pub fn node_operation(&self, node: NodeId) -> Option<Operation<P>> {
        self.nodes[node].operation
    }

    /// Whether an evaluation attempt — or a deep-pass descent — computes `node` now.
    ///
    /// # Invariant
    ///
    /// The mark is a *liveness* flag, not a "was visited" flag: taken when a frame starts
    /// computing the node, released when that frame exits — on the cached-answer,
    /// lazy-answer and unwinding-panic paths alike. It never means "already evaluated"
    /// or "known concrete", so `true` on a node with no cached value is a cyclic read.
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_visiting(&self, node: NodeId) -> bool {
        self.nodes[node].visiting
    }

    /// The deep pass's verdict for `node`, or [`None`] when it never ran there.
    ///
    /// # Invariant
    ///
    /// [`None`] means concreteness is *unknown*, and a reader must treat it as
    /// undecided. A node a cycle cut re-entered reads [`None`] too, but its verdict
    /// computation *assumes* it concrete — the coinductive step. The cut's own mark,
    /// not this field, tells the two cases apart.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_evaluated_deep(&self, node: NodeId) -> Option<EvaluatedDeep> {
        self.nodes[node].evaluated_deep
    }

    /// The disjoint-set metadata of `node`'s equality class. Panics if `node` is
    /// not in [`Self::nodes`].
    ///
    /// # Invariant
    ///
    /// `parent` is the union-find link ([`None`] = `node` is its own root); `next`, `tail`
    /// and `size` describe the member list, meaningful only at the representative. Only
    /// the union-find writes it — reach the representative through
    /// [`Self::equality_representative`], not by following `parent`.
    pub fn node_equality(&self, node: NodeId) -> disjoint::Meta<NodeId> {
        self.nodes[node].equality
    }

    /// **Close an operation cycle** — define a node's operation when its operand is only
    /// nameable afterwards.
    ///
    /// # Invariant
    ///
    /// `node` must be freshly allocated with **no** operation, because an operation is
    /// defined once: replacing it would strand the previous operand edge and contradict
    /// every cached value and deep verdict derived through it. It must not hold a
    /// concrete value either — a decided node is never evaluated again, so the edge
    /// would be dead.
    ///
    /// # Safety
    ///
    /// The deep-pass verdict is cleared with the cycle-cut assumption, because both
    /// predate this operand edge and no longer describe the node's graph.
    pub fn close_operation_cycle(&mut self, node: NodeId, operation: Operation<P>) {
        debug_assert!(
            self.nodes[node].operation.is_none(),
            "a node's operation is defined once: {node:?} already computes one"
        );
        debug_assert!(
            self.nodes[node].value.is_none(),
            "an operation must not be defined on a node that already holds a concrete value: {node:?}"
        );
        self.nodes[node].operation = Some(operation);
        self.nodes[node].evaluated_deep = None;
        // The cycle-cut assumption predates this edge, so it clears too.
        self.nodes[node].assumed_concrete = false;
    }

    /// The module's value epoch: how many value-slot writes have happened. See
    /// [`Self::bump_value_epoch`].
    pub fn value_epoch(&self) -> u32 {
        self.value_epoch
    }

    /// Count one value-slot write.
    ///
    /// # Invariant
    ///
    /// Every site that changes a [`Node::value`] slot calls this exactly once per slot
    /// written, so "the epoch did not move" is the same fact as "no node's value changed".
    /// The counter saturates rather than wrapping: a saturated epoch can only make a run
    /// stamp read as stale, which re-runs an operator instead of skipping one.
    pub(crate) fn bump_value_epoch(&mut self) {
        self.value_epoch = self.value_epoch.saturating_add(1);
    }

    /// The value epoch this node's operator ran under; zero means it never ran.
    pub fn node_run_stamp(&self, node: NodeId) -> u32 {
        self.nodes[node].runned
    }

    /// Write the node's run stamp at the current epoch: it has run.
    pub(crate) fn stamp_node_ran(&mut self, node: NodeId) {
        let epoch = self.value_epoch;
        self.nodes[node].runned = epoch;
    }

    /// Write the node's run stamp from the boolean form the clone and freeze walks read.
    pub(crate) fn stamp_node_ran_if(&mut self, node: NodeId, ran: bool) {
        if ran {
            self.stamp_node_ran(node);
        } else {
            self.nodes[node].runned = 0;
        }
    }

    /// Whether the node's operator ran and no value changed since, so a re-read owes no work.
    ///
    /// # Invariant
    ///
    /// The run stamp is written when the operator runs and the epoch moves on every value
    /// write, so equality is proof that every read this operator made would answer the same
    /// now. A node that never ran reads `false`: nothing has been attempted there.
    pub(crate) fn run_stamp_is_current(&self, node: NodeId) -> bool {
        self.nodes
            .get(node)
            .is_some_and(|node| node.runned != 0 && node.runned == self.value_epoch)
    }

    /// Register `node` in `function`'s scope: tag it as owned and append it to
    /// [`Function::nodes`].
    ///
    /// # Invariant
    ///
    /// The tag and the scope list are written together because they are read for
    /// different purposes and must agree: the apply clone walk follows the owner tag's
    /// chain, while garbage collection and a nested closure's clone walk start from the
    /// scope list.
    ///
    /// # Safety
    ///
    /// `node` must have just been allocated for that function's body. The function's own
    /// value node is deliberately *not* registered here — [`Self::add_function`] tags it
    /// without listing it, since it is reached through the return subtree.
    pub fn register_in_function(&mut self, function: FunctionId, node: NodeId) {
        debug_assert!(
            self.functions.contains_key(function),
            "a node cannot be owned by a function that does not exist: {function:?}"
        );
        self.nodes[node].function = Some(function);
        self.functions[function].nodes.push(node);
    }

    /// Record a diagnostic a **layer above the lowlevel** produced.
    ///
    /// # Invariant
    ///
    /// A record is not an error state and never changes what the VM does next. An identical
    /// `(category, node, message)` entry is dropped: a node is deep-evaluated more than once,
    /// so one refusal is one fact. The linear scan is right because this channel holds only
    /// the handful of things a layer declined to do, unlike `apply_errors`.
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

    /// Record a refusal a **layer above the lowlevel** produced that is an error
    /// rather than a note.
    ///
    /// # Invariant
    ///
    /// A record here **fails the build**, so a program is never handed a module
    /// whose own definition a downstream refused. The same
    /// `(category, node, message)` entry is dropped as a duplicate, because the
    /// definition pass and the run both walk a node.
    pub fn record_extension_failure(
        &mut self,
        category: &'static str,
        node: Option<NodeId>,
        message: impl Into<String>,
    ) {
        let message = message.into();
        let duplicate = self
            .extension_failures
            .iter()
            .any(|e| e.category == category && e.node == node && e.message == message);
        if duplicate {
            return;
        }
        self.extension_failures.push(ExtensionDiagnostic {
            category,
            node,
            message,
        });
    }

    /// Register `condition` as an assert: a constraint, not a unification, so an
    /// undecided condition is *not* bound to `1`.
    ///
    /// # Invariant
    ///
    /// The registry is a worklist: a condition owned by a function body
    /// ([`Function::asserts`]) is cloned and re-registered per apply, so a body's assert
    /// re-checks against each call's argument.
    pub fn add_assert(&mut self, condition: NodeId) -> NodeId {
        self.asserts.push(PendingAssert {
            condition,
            template: AnyNodeId::Dynamic(condition),
        });
        condition
    }

    /// Create a function template's shell, before any of its nodes exist.
    ///
    /// # Safety
    ///
    /// The record is deliberately incomplete: [`Function::parameter`] and
    /// [`Function::r#return`] stay unset until [`Self::finish_function`], and nothing may
    /// read the function in between. Building the shell first lets a compiler emit the
    /// parameter nodes *into this scope* rather than moving them there after.
    /// `parent` is the enclosing template, `None` at the top level or a sibling.
    pub fn begin_function(&mut self, block: BlockId, parent: Option<FunctionId>) -> FunctionId {
        let function = self.functions.insert(Function {
            nodes: Vec::new(),
            r#return: NodeId::default(),
            parameter: NodeId::default(),
            return_type: NodeId::default(),
            static_origin: None,
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
    /// # Safety
    ///
    /// Stamped while the body is being compiled, by the layer that knows the source said
    /// `@loop`: the body is the only place the mark has to survive, because an apply
    /// clones templates away. Stamping twice is idempotent, so no bookkeeping is needed.
    pub fn mark_looping(&mut self, function: FunctionId) {
        self.functions[function].looping = true;
    }

    /// Whether `function` is a `@loop` binding. See [`Function::looping`] for
    /// what the mark does and does not mean.
    pub fn function_is_looping(&self, function: FunctionId) -> bool {
        self.functions[function].looping
    }

    /// Complete the shell begun by [`Self::begin_function`], naming the return and
    /// parameter slots.
    ///
    /// # Safety
    ///
    /// Both must already be registered in the function's scope: the apply clone walk
    /// reads its members through [`Function::nodes`], and `parameter` is what it
    /// instantiates.
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
        // The passed nodes are this function's template body; the id must exist
        // before the tags point at it.
        for &node in &nodes {
            self.register_in_function(function, node);
        }
        self.functions[function].asserts = asserts.into_iter().collect();
        self.finish_function(function, ret, param);
        // Tagged, so an enclosing template mints a fresh closure per call, not one
        // read in place.
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
