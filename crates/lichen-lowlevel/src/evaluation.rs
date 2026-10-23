use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, BlockId, BudgetExhausted, EvaluatedDeep,
    ExtensionDiagnostic, LowOperator, LowValue, Module, NodeId, OperatorExt, Program,
    StaticModuleCache, ancestors::AncestorPairs, table::KeyState,
};
use lichen_utils::disjoint::Node as _;
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

    /// Evaluate the members of `node`'s class that still owe an answer, before `node` runs.
    ///
    /// # Invariant
    /// The in-progress guard, not the union-find's finiteness, bounds the walk: reading a
    /// member re-enters this same class, so an unguarded walk never returns. A member an
    /// outer frame computes is left to that frame — [`Self::alias_read`] joins the running
    /// reader into the class it reads — and a forced read that reaches one is a dependency,
    /// not a cycle.
    #[stacksafe]
    fn evaluate_class_first(&mut self, node: NodeId) {
        let representative = self.equality_representative(node);
        // A lone node is its whole class: the links the walk follows say so, O(1).
        if self.nodes[representative].meta().parent().is_none()
            && self.nodes[representative].meta().next().is_none()
        {
            return;
        }
        if !self.evaluating_classes.insert(representative) {
            return;
        }
        // A snapshot, because reading a member may merge the class this walk is over;
        // the buffer is the module's.
        let mut members = self.class_snapshots.pop().unwrap_or_default();
        members.clear();
        members.extend(self.class_members(representative));
        for index in 0..members.len() {
            let member = members[index];
            // Only members that still owe an answer: the rest answer from their own slot.
            if member != node && self.has_no_result_yet(member) && !self.nodes[member].visiting {
                self.evaluate_node(Dyn(member), None);
            }
        }
        members.clear();
        self.class_snapshots.push(members);
        self.evaluating_classes.remove(&representative);
    }

    /// Whether reading `node` ends its scope: a declared block at its root, an
    /// undeclared one at any parent read.
    fn is_scope_root_read(&self, node: NodeId, referer: Option<BlockId>) -> bool {
        let block = self.nodes[node].block;
        match self.blocks[block].scope_root {
            Some(root) => root == node,
            None => referer.is_some_and(|referer| self.blocks[block].parent == Some(referer)),
        }
    }

    /// The body of [`Self::evaluate_node`], past the static-ref leaf rule (no mark to own).
    #[stacksafe]
    fn evaluate_node_body(&mut self, node: NodeId, referer: Option<BlockId>) -> Option<P::Value> {
        let block = self.nodes[node].block;
        debug_assert!(
            self.blocks.contains_key(block),
            "node {node:?} references released block {block:?}"
        );
        // A scope root delegates whole; returning before this frame takes a mark keeps one
        // attempt, one mark.
        if self.is_scope_root_read(node, referer) {
            return self.evaluate_block(node);
        }
        // The class before the node runs, so unify's order cannot decide its answer.
        if self.has_no_result_yet(node) && !self.nodes[node].visiting {
            self.evaluate_class_first(node);
        }
        if let Some(value) = self.nodes[node].value {
            // The wildcard is undecided to every reader but the unifier, so it is
            // never a computed answer to hand on.
            if matches!(value.as_enum(), Some(LowValue::Any)) {
                return None;
            }
            // A value a unification wrote is not the operator's answer: the run gate's `runned` term
            // tests this operator.
            if self.has_no_result_yet(node) && !self.nodes[node].visiting {
                let guard = self.retain_node(node);
                guard.run(|module, node| module.evaluate_node_operation(node));
                // The run can write the wildcard into a slot — the operator's class may
                // hold it — and it answers no reader either.
                return match self.nodes[node].value.and_then(|value| value.as_enum()) {
                    Some(LowValue::Any) => None,
                    _ => self.nodes[node].value,
                };
            }
            return Some(value);
        }
        // A read of a node an outer frame computes is cyclic, unless a class walk forced it.
        if self.nodes[node].visiting {
            if self.evaluating_classes.is_empty() {
                unreachable!("cycle detected: node {node:?} is being evaluated");
            }
            return None;
        }
        // An empty slot is undecided; with no operation behind it there is nothing to run.
        if !self.has_no_result_yet(node) {
            return None;
        }

        // The stamp is proof this operator has had its one attempt, so a re-read owes no work.
        if self.run_stamp_is_set(node) {
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
                                                        // The wildcard answers no reader: a read of it
                                                        // is undecided, never the value `Any`.
                                                        match self
                                                            .node_value(Dyn(rep))
                                                            .and_then(|value| value.as_enum())
                                                        {
                                                            Some(LowValue::Any) => None,
                                                            _ => self.node_value(Dyn(rep)),
                                                        }
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
            None => {
                let previous = self.operator_for_diagnostics.replace(node);
                let value = operator.run_deferred(operation.operand, block, self);
                self.operator_for_diagnostics = previous;
                value
            }
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
                            // `f : f`: the value is not concrete yet, so the
                            // apply applies the type element 3 carries.
                            self.apply_callee_type(operands, block, node);
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
                                // Both slots are applied: the type's, then the value's.
                                self.apply_callee_type(operands, block, node);
                                match function {
                                    AnyFunctionId::Dynamic(function) => {
                                        self.function_apply(function, argument, block, node, cell)
                                    }
                                    AnyFunctionId::Static(sref) => {
                                        // Element 2 is the checker-wired result cell.
                                        let cell = operands
                                            .get(2)
                                            .map(|item| self.as_dynamic(item.node, block));
                                        self.static_function_apply(
                                            sref, argument, block, node, cell,
                                        )
                                    }
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
                                        // The lookup never happened: the slot stays empty and the
                                        // node stays unstamped, so the next read tries again.
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
            Some(LowOperator::External { library, instance }) => {
                let Some(operands) = operation.operand else {
                    unreachable!("External expects an operand node")
                };
                let Some(operand) = self.evaluate_node(Dyn(operands), Some(block)) else {
                    return self.ran_undecided(node);
                };
                // An entry cannot report "undecided": an open element keeps it so.
                if let Some(LowValue::Array(array)) = operand.as_enum() {
                    // SAFETY: `array` is a live operand payload; the entry keeps it so.
                    let open = unsafe { array.items() }.iter().any(|item| {
                        !item.shallow && self.evaluate_node(item.node, Some(block)).is_none()
                    });
                    if open {
                        return self.ran_undecided(node);
                    }
                }
                let Some(entry) = self.external(library, instance) else {
                    self.extension_diagnostics.push(ExtensionDiagnostic::note(
                        "native-link",
                        Some(node),
                        format!("library {library} instance {instance} is not installed"),
                    ));
                    return self.ran_undecided(node);
                };
                let mut out = std::mem::MaybeUninit::<P::Value>::uninit();
                // SAFETY: `install_external`'s contract: the pointers are the
                // live module, one of its values, and one value's storage.
                unsafe {
                    entry(
                        (self as *mut Module<P>).cast(),
                        (&raw const operand).cast(),
                        out.as_mut_ptr().cast(),
                    );
                }
                // SAFETY: the entry's contract writes exactly one `P::Value`.
                Some(unsafe { out.assume_init() })
            }
        };
        // An undecided answer is not cached: the run stamp records that it ran.
        let Some(value) = value else {
            self.stamp_node_ran(node);
            return None;
        };
        // Commit through the one value-write path: the class's value reaches the representative
        // and is reconciled with it.
        self.write_node_answer(node, value);
        Some(value)
    }

    /// An operation that **ran** but could not decide: the slot stays empty, the run stamp is set.
    fn ran_undecided(&mut self, node: NodeId) -> Option<P::Value> {
        self.stamp_node_ran(node);
        None
    }

    /// Apply the **type** an apply's operand carries, for a callee whose value is
    /// not concrete yet.
    ///
    /// # Invariant
    /// `f : f` — a function's type names the same function — so the type is
    /// applied where the value cannot be. Element 3 is the callee's type; a
    /// non-function one applies nothing. [`Module::apply_type`] clones the
    /// signature per application, so this never monomorphises the template.
    fn apply_callee_type(&mut self, operands: &[crate::ArrayItem], block: BlockId, node: NodeId) {
        let Some(type_node) = operands
            .get(3)
            .map(|item| self.as_dynamic(item.node, block))
        else {
            return;
        };
        let Some(AnyFunctionId::Dynamic(function)) = self.function_type_function(type_node) else {
            return;
        };
        let argument = self.as_dynamic(operands[1].node, block);
        let cell = operands
            .get(2)
            .map(|item| self.as_dynamic(item.node, block));
        self.apply_type(function, argument, block, node, cell);
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
        // A scope root read ends the scope here: the read is the trigger, so the
        // verdict cache must not answer for it.
        let scope_read = self.is_scope_root_read(node, current);
        // A concrete verdict is final, so its value is the answer; an `undecided` one is asked
        // again (late-writes-map.md §11).
        if !scope_read
            && let Some(deep) = self.nodes[node].evaluated_deep
            && !deep.undecided
            && let Some(value) = self.nodes[node].value
        {
            return Some(value);
        }
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
        let value = match self.nodes[node].value {
            // A wildcard is decided content for the verdict: it must never make
            // a structure unprovable.
            Some(value) if matches!(value.as_enum(), Some(LowValue::Any)) => {
                self.nodes[node].evaluated_deep = Some(EvaluatedDeep { undecided: false });
                self.deep_depth -= 1;
                return Some(value);
            }
            _ => self.evaluate_node(Dyn(node), current),
        };
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
        // A native entry cannot report "undecided": an open operand keeps it open (§17).
        let operand_undecided = match self.nodes[node].operation {
            Some(operation)
                if matches!(
                    operation.operator.as_enum(),
                    Some(LowOperator::External { .. })
                ) =>
            {
                match operation.operand {
                    Some(operand) => {
                        self.evaluate_node_deep_inner(Dyn(operand), current, cache);
                        self.ref_is_undecided(cache, Dyn(operand))
                    }
                    None => false,
                }
            }
            _ => false,
        };
        // An array is undecided while a position is undecided or sits behind a shallow mark.
        let undecided = operand_undecided || self.value_is_undecided(cache, value);
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
    /// Only the value graph decides this, with one exception: a native entry's
    /// answer cannot report undecided itself, so its operand's verdict is folded
    /// in at the deep walk. Each position's verdict is read through
    /// [`Self::ref_is_undecided`].
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

    /// End a scope at its root's read, releasing it if the read decided its answer.
    ///
    /// # Invariant
    /// The read is **shallow**: nothing is evaluated to find out whether the scope is done,
    /// so an unread value never runs because of the marker. The root's own slot is the
    /// signal — a real value, not the wildcard — and the collect's relocation is the value
    /// the reader receives. An undecided root keeps the marker, so a later read retries; a
    /// decided one is released in one go.
    fn evaluate_block(&mut self, root: NodeId) -> Option<P::Value> {
        // One entry per scope: the read below would send the root straight back here.
        let block = self.nodes[root].block;
        self.blocks[block].scope_root = None;
        let value = self.evaluate_node(Dyn(root), None);
        let decided = self.nodes[root]
            .value
            .is_some_and(|value| !matches!(value.as_enum(), Some(LowValue::Any)));
        if !decided {
            self.blocks[block].scope_root = Some(root);
            return value;
        }
        self.garbage_collect(root).or(value)
    }
}
