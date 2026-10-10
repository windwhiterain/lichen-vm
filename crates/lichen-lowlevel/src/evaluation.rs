use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, BlockId, BudgetExhausted, EvaluatedDeep,
    LowOperator, LowValue, Module, NodeId, OperatorExt, Program, StaticModuleCache,
    ancestors::AncestorPairs, table::KeyState,
};
use lichen_utils::extend::AsEnum;

/// The evaluation-attempt mark of one node, released on drop; see [`Module::retain_node`].
struct VisitGuard<'a, P: Program> {
    module: &'a mut Module<P>,
    node: NodeId,
}

impl<P: Program> VisitGuard<'_, P> {
    /// Run `body` with the mark held for the marked node.
    fn run<R>(self, body: impl FnOnce(&mut Module<P>, NodeId) -> R) -> R {
        let node = self.node;
        body(self.module, node)
    }
}

impl<P: Program> Drop for VisitGuard<'_, P> {
    fn drop(&mut self) {
        self.module.nodes[self.node].visiting = false;
    }
}

/// A runtime evaluation failure: an out-of-bounds [`LowOperator::Index`], a table miss,
/// or an unapplyable target.
///
/// # Invariant
/// The offending nodes are [`AnyNodeId`]s, so a static one has no importer span.
#[derive(Debug, Clone, Copy)]
pub enum EvalError {
    /// An out-of-bounds array read.  `index` is the index operand node —
    /// its source span attributes the diagnostic.
    Index {
        index: AnyNodeId,
        index_value: usize,
        length: usize,
    },
    /// A [`LowOperator::TableGet`] miss: an unhashable key or an empty target.  An
    /// **undecided** key answers undecided.
    TableMiss { table: AnyNodeId, key: AnyNodeId },
    /// A table build dropped an entry whose key could not be forced concrete.
    TableKeyUndecided { key: AnyNodeId },
    /// A [`LowOperator::Index`] whose target is not an array — a recorded failure, never an
    /// internal error.
    IndexTarget { target: AnyNodeId },
    /// A [`LowOperator::Index`] whose **subscript** is not an index — also a recorded failure.
    IndexSubscript { subscript: AnyNodeId },
    /// A [`LowOperator::Apply`] whose **target** cannot be applied — a recorded failure, never
    /// a panic.
    ApplyTarget { function: AnyNodeId },
}

impl<P: Program> Module<P> {
    /// `#[stacksafe]`: apply recursion runs one frame per level here, so the depth guards
    /// must be able to grow the stack.
    #[stacksafe]
    pub fn evaluate_node(&mut self, node: AnyNodeId, referer: Option<BlockId>) -> Option<P::Value> {
        // A static ref is a decided leaf: read its solved value; nothing is evaluated or marked.
        match node {
            Dyn(node) => self.evaluate_node_body(node, referer),
            AnyNodeId::Static(sref) => self.static_read(sref),
        }
    }

    /// Take the mark of the node an evaluation attempt is computing, held until the returned
    /// guard drops.
    ///
    /// # Invariant
    /// A frame owns `Node::visiting` for exactly its own scope, and the guard clears it on
    /// every exit — a cached answer, an undecided answer, and an unwinding panic alike, so a
    /// later pass never reads a stale flag as a cycle.  [`Drop`] is what makes this hold on
    /// the unwind path.
    fn retain_node(&mut self, node: NodeId) -> VisitGuard<'_, P> {
        self.nodes[node].visiting = true;
        VisitGuard { module: self, node }
    }

    /// The body of [`Self::evaluate_node`], past the static-ref leaf rule (no mark to own).
    #[stacksafe]
    fn evaluate_node_body(&mut self, node: NodeId, referer: Option<BlockId>) -> Option<P::Value> {
        let block = self.nodes[node].block;
        debug_assert!(
            self.blocks.contains_key(block),
            "node {node:?} references released block {block:?}"
        );
        // A child-block root delegates whole; returning before this frame takes a mark keeps one
        // attempt, one mark.
        if let Some(referer) = referer
            && self.blocks[block].parent == Some(referer)
        {
            return self.evaluate_block(node);
        }
        if let Some(value) = self.nodes[node].value {
            // A value a unification wrote is not the operator's answer: the run gate's `runned` term
            // tests this operator.
            if self.has_no_result_yet(node) && !self.nodes[node].visiting {
                let guard = self.retain_node(node);
                guard.run(|module, node| module.evaluate_node_operation(node));
                return self.nodes[node].value;
            }
            return Some(value);
        }
        // Visiting with no cached value means an inner frame is computing it, so this read is
        // cyclic.
        if self.nodes[node].visiting {
            unreachable!("cycle detected: node {node:?} is being evaluated");
        }
        // An empty slot is undecided; with no operation behind it there is nothing to run.
        if !self.has_no_result_yet(node) {
            return None;
        }
        let guard = self.retain_node(node);
        guard.run(|module, node| module.evaluate_node_operation(node))
    }

    /// The operation dispatch, run while the caller holds the node's [`VisitGuard`].
    #[stacksafe]
    fn evaluate_node_operation(&mut self, node: NodeId) -> Option<P::Value> {
        let block = self.nodes[node].block;
        let operation = self.nodes[node].operation.unwrap();
        let operator = operation.operator;
        let value = match operator.as_enum() {
            Some(LowOperator::Index) => {
                let Some(operands) = operation.operand else {
                    unreachable!("Index expects an operand array node")
                };
                // An undecided operand chain means the index can't be resolved
                // yet — stay lazy so the definition pass can flag the node.
                let Some(operand) = self.evaluate_node(Dyn(operands), Some(block)) else {
                    return self.ran_undecided(node);
                };
                match operand.as_enum() {
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is the value the module just
                        // evaluated for the operand node; its home block is
                        // alive and not dropped.
                        let operands = unsafe { array.items() };
                        let Some(subscript) = self.evaluate_node(operands[1].node, Some(block))
                        else {
                            return self.ran_undecided(node);
                        };
                        match subscript.as_enum() {
                            // An empty index propagates: the read's own
                            // failure was recorded where the
                            // `Error` was produced.
                            Some(LowValue::Error) => Some(P::Value::from(LowValue::Error)),
                            Some(LowValue::USize(index)) => {
                                let Some(target) =
                                    self.evaluate_node(operands[0].node, Some(block))
                                else {
                                    return self.ran_undecided(node);
                                };
                                match target.as_enum() {
                                    // A computed-nothing target propagates
                                    // the same way — no second diagnostic.
                                    Some(LowValue::Error) => Some(P::Value::from(LowValue::Error)),
                                    Some(LowValue::Array(array)) => {
                                        // SAFETY: `array` is the value the
                                        // module just evaluated for the target
                                        // node; its home block is alive.
                                        let array = unsafe { array.items() };
                                        // An out-of-bounds index is a user error, not an invariant violation: record and yield
                                        // `Error`.
                                        if index < array.len() {
                                            // A read of a pure cell is a reference: joining the reader to its class lets a later
                                            // bind reach it.
                                            let element = array[index].node;
                                            match element {
                                                Dyn(element) => {
                                                    self.alias_read(node, element);
                                                    // An element that is its own class never re-enters evaluation; the class's
                                                    // committed value answers it.
                                                    if self.equality_representative(element)
                                                        == self.equality_representative(node)
                                                    {
                                                        let rep =
                                                            self.equality_representative(node);
                                                        self.class_committed_value(rep)
                                                    } else {
                                                        self.evaluate_node(
                                                            Dyn(element),
                                                            Some(block),
                                                        )
                                                    }
                                                }
                                                // A static element is immutable: no class to join, and its value caches here like any
                                                // other.
                                                AnyNodeId::Static(sref) => self.static_read(sref),
                                            }
                                        } else {
                                            self.eval_errors.push(EvalError::Index {
                                                index: operands[1].node,
                                                index_value: index,
                                                length: array.len(),
                                            });
                                            Some(P::Value::from(LowValue::Error))
                                        }
                                    }
                                    // Every read failure here is a user error, never an invariant violation: record it and
                                    // yield an empty value.
                                    _ => {
                                        self.eval_errors.push(EvalError::IndexTarget {
                                            target: operands[0].node,
                                        });
                                        Some(P::Value::from(LowValue::Error))
                                    }
                                }
                            }
                            // A non-index subscript is a user error like a non-container target: recorded, with an
                            // empty value.
                            _ => {
                                self.eval_errors.push(EvalError::IndexSubscript {
                                    subscript: operands[1].node,
                                });
                                Some(P::Value::from(LowValue::Error))
                            }
                        }
                    }
                    _ => unreachable!("Index operand must be an array of [array, index]"),
                }
            }
            // An extension operator reports undecided as `None`, which is what this answers.
            None => operator.run_deferred(operation.operand, block, self),
            Some(LowOperator::Apply) => {
                let Some(operands) = operation.operand else {
                    unreachable!("Apply expects an operand array node")
                };
                // An undecided target — the body's own parameter during the
                // definition pass — stays lazy instead of panicking.
                let Some(operand) = self.evaluate_node(Dyn(operands), Some(block)) else {
                    return self.ran_undecided(node);
                };
                match operand.as_enum() {
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is a live operand value; its home block is not dropped.
                        let operands = unsafe { array.items() };
                        let Some(callee) = self.evaluate_node(operands[0].node, Some(block)) else {
                            return self.ran_undecided(node);
                        };
                        match callee.as_enum() {
                            Some(LowValue::Function(function)) => {
                                // Element 2 is the checker-wired result cell; a static ref is materialized into a leaf
                                // first.
                                let argument = self.as_dynamic(operands[1].node, block);
                                let cell = operands
                                    .get(2)
                                    .map(|item| self.as_dynamic(item.node, block));
                                match function {
                                    AnyFunctionId::Dynamic(function) => {
                                        self.function_apply(function, argument, block, node, cell)
                                    }
                                    AnyFunctionId::Static(sref) => self
                                        .static_function_apply(sref, argument, block, node, cell),
                                }
                            }
                            // A scalar, string, table or unit value can never be callable: the apply is refused and
                            // recorded.
                            Some(
                                LowValue::USize(_)
                                | LowValue::Float(_)
                                | LowValue::Str(_)
                                | LowValue::Table(_)
                                | LowValue::None,
                            ) => {
                                self.eval_errors.push(EvalError::ApplyTarget {
                                    function: operands[0].node,
                                });
                                Some(P::Value::from(LowValue::Error))
                            }
                            // An empty value is the residue of an
                            // already-recorded failure: propagate it without
                            // recording a second one.
                            Some(LowValue::Error) => Some(P::Value::from(LowValue::Error)),
                            // A structural array or the program's own value reach here; `OperatorExt::is_callable`
                            // decides.
                            _ => {
                                if <P::Operator as OperatorExt<P>>::is_callable(
                                    self,
                                    operands[0].node,
                                ) {
                                    self.ran_undecided(node)
                                } else {
                                    self.eval_errors.push(EvalError::ApplyTarget {
                                        function: operands[0].node,
                                    });
                                    Some(P::Value::from(LowValue::Error))
                                }
                            }
                        }
                    }
                    _ => unreachable!("Apply operand must be an array of [function, argument]"),
                }
            }
            Some(LowOperator::TableGet) => {
                let Some(operands) = operation.operand else {
                    unreachable!("TableGet expects an operand array node")
                };
                let Some(operand) = self.evaluate_node(Dyn(operands), Some(block)) else {
                    return self.ran_undecided(node);
                };
                match operand.as_enum() {
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is a live operand value; its home block is not dropped.
                        let operands = unsafe { array.items() };
                        let table = operands[0].node;
                        let key = operands[1].node;
                        let Some(target) = self.evaluate_node(table, Some(block)) else {
                            return self.ran_undecided(node);
                        };
                        match target.as_enum() {
                            Some(LowValue::Table(payload)) => {
                                // An undecided key is not a miss: the lookup has not happened, so the read stays lazy.
                                match self.key_state(key) {
                                    KeyState::Undecided => {
                                        // The lookup never happened: the slot stays empty and `runned` stays false.
                                        return None;
                                    }
                                    KeyState::Unhashable => {
                                        self.eval_errors.push(EvalError::TableMiss { table, key });
                                        return Some(P::Value::from(LowValue::Error));
                                    }
                                    KeyState::Hashed(hash) => {
                                        // SAFETY: `payload` is the evaluated
                                        // table value of a live node of this
                                        // module; its home block is alive.
                                        let items = unsafe { payload.items() };
                                        let start = items.partition_point(|item| item.hash < hash);
                                        let mut path = AncestorPairs::new();
                                        let mut found = None;
                                        for item in &items[start..] {
                                            if item.hash != hash {
                                                break;
                                            }
                                            if self.key_eq(key, item.key, &mut path) {
                                                found = Some(item.value);
                                                break;
                                            }
                                        }
                                        self.finish_table_get(node, found, block)
                                    }
                                }
                            }
                            // A computed-nothing target is a miss like any other: recorded, never a panic.
                            Some(LowValue::Error) => {
                                self.eval_errors.push(EvalError::TableMiss { table, key });
                                Some(P::Value::from(LowValue::Error))
                            }
                            _ => unreachable!("TableGet target must be a table"),
                        }
                    }
                    _ => unreachable!("TableGet operand must be an array of [table, key]"),
                }
            }
        };
        // An undecided answer is not cached: the slot stays empty so the next read re-runs the
        // operator.
        let Some(value) = value else {
            self.nodes[node].runned = true;
            return None;
        };
        // Commit through the one value-write path: the class's value reaches the representative
        // and is reconciled with it.
        self.write_node_answer(node, value);
        Some(value)
    }

    /// An operation that **ran** but could not decide: the slot stays empty, `runned` is set.
    fn ran_undecided(&mut self, node: NodeId) -> Option<P::Value> {
        self.nodes[node].runned = true;
        None
    }

    /// Run [`Self::evaluate_node`] for every node in the reachable subtree of `id` — the only
    /// deep walk.
    ///
    /// # Invariant
    /// A shallow-marked position is not descended into, and an operation's operand edge is not
    /// forced: a `LowValue` is a computed answer, not a thunk.
    #[stacksafe]
    pub fn evaluate_node_deep(
        &mut self,
        node: NodeId,
        current: Option<BlockId>,
    ) -> Option<P::Value> {
        let mut cache = StaticModuleCache::new();
        self.evaluate_node_deep_inner(Dyn(node), current, &mut cache)
    }

    /// The tail of a [`LowOperator::TableGet`]: a found element is read by reference, an absent
    /// one is a recorded miss.
    fn finish_table_get(
        &mut self,
        node: NodeId,
        found: Option<AnyNodeId>,
        block: BlockId,
    ) -> Option<P::Value> {
        match found {
            Some(Dyn(element)) => {
                self.alias_read(node, element);
                self.evaluate_node(Dyn(element), Some(block))
            }
            // A static element is immutable: no class to join, and its value caches here.
            Some(AnyNodeId::Static(sref)) => self.static_read(sref),
            None => {
                let (table, key) = self.table_get_operands(node);
                self.eval_errors.push(EvalError::TableMiss { table, key });
                Some(P::Value::from(LowValue::Error))
            }
        }
    }

    /// The `[table, key]` operand nodes, for attributing a miss; the shape already holds.
    fn table_get_operands(&self, node: NodeId) -> (AnyNodeId, AnyNodeId) {
        let operand = self.nodes[node]
            .operation
            .and_then(|op| op.operand)
            .expect("a TableGet node reached the miss path carries its operand");
        match self
            .node_value(Dyn(operand))
            .and_then(|value| value.as_enum())
        {
            Some(LowValue::Array(array)) => {
                // SAFETY: `array` is a live node's payload; its block is not dropped.
                let items = unsafe { array.items() };
                (items[0].node, items[1].node)
            }
            _ => unreachable!("a TableGet operand is the [table, key] array"),
        }
    }

    /// The deep walk's core; `cache` is the walk's one-entry static-module resolution cache.
    #[stacksafe]
    fn evaluate_node_deep_inner(
        &mut self,
        node: AnyNodeId,
        current: Option<BlockId>,
        cache: &mut StaticModuleCache<P>,
    ) -> Option<P::Value> {
        // A static ref is a decided leaf: nothing to evaluate, descend or mark.
        if let AnyNodeId::Static(sref) = node {
            return cache.read(self, sref);
        }
        let node = match node {
            Dyn(node) => node,
            AnyNodeId::Static(_) => unreachable!(),
        };
        // A structural cycle is cut here: the node is being deep-evaluated by an outer frame and
        // already holds its cached value.
        if self.nodes[node].visiting
            && let Some(value) = self.nodes[node].value
        {
            // The cut assumes this node concrete — the coinductive step a cyclic value needs — until
            // the real verdict is written.
            self.nodes[node].assumed_concrete = true;
            return Some(value);
        }
        self.deep_depth += 1;
        if self.deep_depth > self.evaluate_depth_limit {
            if self.budget_exhausted.is_none() {
                self.budget_exhausted = Some(BudgetExhausted::EvaluateDepth {
                    limit: self.evaluate_depth_limit,
                });
            }
            // Nothing was computed and nothing here can compute it: return `Error`; the counter is
            // restored on the way out.
            self.deep_depth -= 1;
            return Some(P::Value::from(LowValue::Error));
        }
        let value = self.evaluate_node(Dyn(node), current);
        if let Some(LowValue::Array(array)) = value.and_then(|value| value.as_enum()) {
            // The descent may reach `node` again through the array's own items, so mark it for the
            // duration.
            let guard = self.retain_node(node);
            guard.run(|module, node| {
                let block = module.nodes[node].block;
                // SAFETY: `array` is a live `node` value; the descent releases no block.
                for item in unsafe { array.items() } {
                    // A shallow position is a lazy region: its subtree stays unevaluated until read.
                    if item.shallow {
                        continue;
                    }
                    module.evaluate_node_deep_inner(item.node, Some(block), cache);
                }
            });
        }
        // A table's entries are edges like array items; both must be proven concrete by the descent.
        if let Some(LowValue::Table(table)) = value.and_then(|value| value.as_enum()) {
            let guard = self.retain_node(node);
            guard.run(|module, node| {
                let block = module.nodes[node].block;
                // SAFETY: `table` is a live `node` value; the descent releases no block.
                for item in unsafe { table.items() } {
                    module.evaluate_node_deep_inner(item.key, Some(block), cache);
                    module.evaluate_node_deep_inner(item.value, Some(block), cache);
                }
            });
        }
        // An array is undecided while a position is undecided or sits behind a shallow mark.
        let undecided = self.value_is_undecided(cache, value);
        self.nodes[node].evaluated_deep = Some(EvaluatedDeep { undecided });
        // The real verdict supersedes the cycle-cut assumption.
        self.nodes[node].assumed_concrete = false;
        self.deep_depth -= 1;
        value
    }

    /// The three-state concreteness read of one **ref** inside the verdict computation.
    ///
    /// # Invariant
    /// - A node a cycle cut re-entered while its own frame computes it is assumed
    ///   **concrete**; the assumption fills a missing verdict and never overrides one.
    /// - A node the pass **never ran on** reads undecided: for a node no frame is computing,
    ///   `None` must never mean "proven concrete".
    fn ref_is_undecided(&self, cache: &mut StaticModuleCache<P>, id: AnyNodeId) -> bool {
        match id {
            Dyn(node) => {
                let entry = &self.nodes[node];
                match entry.evaluated_deep {
                    Some(deep) => deep.undecided,
                    None => !entry.assumed_concrete,
                }
            }
            AnyNodeId::Static(sref) => cache.node_undecided(self, sref),
        }
    }

    /// Whether `value` is undecided: `None`, or an array or table with a shallow or undecided
    /// position.
    ///
    /// # Invariant
    /// Only the value graph decides this: an operation's operand edge is not value-reachable,
    /// and a decided value cannot depend on an operand its operator did not read.  Each
    /// position's verdict is read through [`Self::ref_is_undecided`].
    fn value_is_undecided(
        &self,
        cache: &mut StaticModuleCache<P>,
        value: Option<P::Value>,
    ) -> bool {
        // An empty answer is undecided; otherwise the extension view is taken once, because every
        // arm tests the same value.
        let Some(value) = value else { return true };
        let view = value.as_enum();
        matches!(
            view,
            Some(LowValue::Array(array))
                // An array holding a shallow position can never be proven concrete.

                // SAFETY: `array` is a live `value` payload; the note covers both `items()` calls.
                if unsafe { array.items() }.iter().any(|item| item.shallow)
                    || unsafe { array.items() }
                        .iter()
                        .any(|item| self.ref_is_undecided(cache, item.node))
        ) || matches!(
            view,
            Some(LowValue::Table(table))
                // SAFETY: `table` is a live `value` payload; the note covers both `items()` calls.
                if unsafe { table.items() }
                    .iter()
                    .any(|item| self.ref_is_undecided(cache, item.key))
                    || unsafe { table.items() }
                        .iter()
                        .any(|item| self.ref_is_undecided(cache, item.value))
        )
    }

    fn evaluate_block(&mut self, root: NodeId) -> Option<P::Value> {
        let value = self.evaluate_node_deep(root, None);
        // The root may not be cached (a budget refusal, or an undecided answer), so compaction has
        // no moved value.
        self.garbage_collect(root).or(value)
    }
}
