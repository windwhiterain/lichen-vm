//! The **host loop**: a converted `@loop` recursion, run instead of
//! expanded. See docs/notes/loop-conversion.md §8.6.

use lichen_utils::extend::AsEnum;

use crate::{
    AnyNodeId::Dynamic as Dyn, ArrayItem, BlockId, BudgetExhausted, FunctionId, LoopArm,
    LoopConversion, LowValue, Module, NodeId, Program,
};

/// What one iteration decided.
enum Iteration {
    /// The test chose a step: go round again with this next state, whose
    /// nodes belong to the instantiation that made them.
    Step(Vec<NodeId>),
    /// The test chose a base: the result is the pair resolving the base's
    /// value half, or the bare value for a bare return.
    Exit {
        value: NodeId,
        r#type: Option<NodeId>,
    },
    /// The test did not decide. Nothing is known: the loop ends undecided.
    Undecided,
    /// The test chose an index the spine does not name: the evaluator's own
    /// out-of-bounds read, not a third arm.
    Unnamed { applied: NodeId },
    /// The argument failed the parameter check, which `instantiate` recorded.
    Refused,
}

impl<P: Program> Module<P> {
    /// Run `conversion` from `argument`. `node` is the entering apply, for
    /// error attribution, and `cell` its result cell.
    pub(super) fn apply_loop(
        &mut self,
        conversion: &LoopConversion,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> Option<P::Value> {
        // **The entering call is one application**: the apply budget and one
        // level of nesting, whatever the iteration count.
        self.with_apply_frame(|module| {
            module.loop_body(conversion, function, argument, block, node, cell)
        })
    }

    /// The iterations, from `argument` to a base.
    fn loop_body(
        &mut self,
        conversion: &LoopConversion,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> Option<P::Value> {
        let mut argument = argument;
        loop {
            // **An iteration is an application**, as the level it replaces is:
            // one counter, so the host's limit bounds both paths.
            self.apply_total += 1;
            if self.apply_total > self.apply_total_limit {
                if self.budget_exhausted.is_none() {
                    self.budget_exhausted = Some(BudgetExhausted::ApplyTotal {
                        limit: self.apply_total_limit,
                    });
                }
                return None;
            }
            match self.loop_iteration(conversion, function, argument, block, node) {
                Iteration::Step(next) => {
                    // The state crosses the backedge **lazily**, through the
                    // unify the unroll uses: nothing is computed until asked.
                    argument = self.loop_argument(conversion, function, &next, argument, block);
                }
                Iteration::Exit { value, r#type } => {
                    // **The result is built, not read off the return**: that
                    // leaves the untaken step arm for the deep pass to unroll.
                    let decided = self.evaluate_node_deep(value, Some(block));
                    let Some(r#type) = r#type else {
                        // A function whose return is stated bare: the value is
                        // the result, and there is no cell to bind.
                        self.write_node_value(node, decided);
                        return decided;
                    };
                    let items = vec![ArrayItem::new(Dyn(value)), ArrayItem::new(Dyn(r#type))];
                    let array = self.alloc_array(&items, block);
                    let result = P::Value::from(LowValue::Array(array));
                    self.write_node_value(node, Some(result.clone()));
                    if let Some(cell) = cell {
                        // The return type is resolved before the cell binds, as
                        // the unroll's tail does.
                        let _ = self.evaluate_node(Dyn(r#type), Some(block));
                        self.unify(cell, r#type);
                    }
                    return Some(result);
                }
                Iteration::Unnamed { applied } => {
                    return self.evaluate_node(Dyn(applied), Some(block));
                }
                Iteration::Undecided | Iteration::Refused => {
                    return None;
                }
            }
        }
    }

    /// One iteration: instantiate the body, resolve the tests until an arm is
    /// chosen, and report what it asks for.
    fn loop_iteration(
        &mut self,
        conversion: &LoopConversion,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
    ) -> Iteration {
        let Some(instantiation) = self.instantiate(function, argument, block, node) else {
            return Iteration::Refused;
        };
        let mut index = 0;
        loop {
            let test = conversion.tests[index];
            let condition = instantiation.node_of(test.condition);
            let value = self.evaluate_node(Dyn(condition), Some(block));
            let Some(LowValue::USize(choice)) = value.and_then(|value| value.as_enum()) else {
                return Iteration::Undecided;
            };
            // The selector indexes `[else, then]`. **Any other index is the
            // evaluator's own out-of-bounds read**, not a third arm.
            let arm = match choice {
                0 => test.on_zero,
                1 => test.on_one,
                _ => {
                    return Iteration::Unnamed {
                        applied: instantiation.applied,
                    };
                }
            };
            match arm {
                LoopArm::Test(next) => index = next,
                LoopArm::Step(step) => {
                    let next = conversion.steps[step]
                        .next
                        .iter()
                        .map(|&value| instantiation.node_of(value))
                        .collect();
                    return Iteration::Step(next);
                }
                LoopArm::Exit(exit) => {
                    // The type half is the function's own return type,
                    // resolved into this instantiation: the slot the apply binds.
                    let r#type = self
                        .pair_type_half(self.functions[function].r#return)
                        .map(|r#type| instantiation.node_of(r#type));
                    return Iteration::Exit {
                        value: instantiation.node_of(conversion.exits[exit].value),
                        r#type,
                    };
                }
            }
        }
    }

    /// The argument for the next iteration: the new state, stated the way the
    /// entering call stated it.
    ///
    /// # Invariant
    ///
    /// A scalar state's value half *is* the value; a tuple's is an array of
    /// its slots, per [`LoopConversion::state`]. The type half is the entering
    /// argument's, so each iteration's parameter check means what the entry's
    /// did: the state's shape is the loop's invariant, its values are not.
    fn loop_argument(
        &mut self,
        conversion: &LoopConversion,
        function: FunctionId,
        next: &[NodeId],
        entering: NodeId,
        block: BlockId,
    ) -> NodeId {
        let scalar = conversion.state.len() == 1 && conversion.state[0].is_empty();
        let value = if scalar {
            next[0]
        } else {
            self.array_of(next, block)
        };
        // A bare parameter — a hand-built graph, or anything outside the
        // `[value, type]` convention — takes the value alone.
        if self
            .pair_value_half(self.functions[function].parameter)
            .is_none()
        {
            return value;
        }
        let r#type = self.pair_type_half(entering).unwrap_or_else(|| {
            // No entering type to re-state: a fresh cell leaves the shape
            // unchecked rather than pinning the template's own cell.
            self.add_node(block, None, None)
        });
        let items = vec![ArrayItem::new(Dyn(value)), ArrayItem::new(Dyn(r#type))];
        let array = self.alloc_array(&items, block);
        self.add_node(block, None, Some(P::Value::from(LowValue::Array(array))))
    }

    /// An array value node over `nodes`, in `block`.
    fn array_of(&mut self, nodes: &[NodeId], block: BlockId) -> NodeId {
        let items: Vec<ArrayItem> = nodes
            .iter()
            .map(|&node| ArrayItem::new(Dyn(node)))
            .collect();
        let array = self.alloc_array(&items, block);
        self.add_node(block, None, Some(P::Value::from(LowValue::Array(array))))
    }
}
