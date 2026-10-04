//! The **host loop**: running a converted `@loop` recursion instead of
//! expanding it.
//!
//! `@loop` is permission to convert, and `loop_conversion.rs` is the analysis
//! that answers *whether* a marked recursion is a loop and *what its parts
//! are*. This module is the consumer that makes the answer observable in the
//! host: where the unroll instantiates the body once per nested level, the
//! loop instantiates it once per **iteration** and keeps only one
//! instantiation alive, so the trip count stops costing nesting depth. The
//! value is the same one the unroll produces — the whole point of the
//! equivalence harness — and the difference is what a trip count may be.
//!
//! # The iteration
//!
//! One iteration is an ordinary apply with the nesting removed:
//!
//! 1. **instantiate** the body for the current state ([`Module::instantiate`] —
//!    the same clone-and-unify the unroll uses, parameter check included);
//! 2. evaluate the iteration's **test** ([`LoopTest::condition`], resolved into
//!    this instantiation) and read the arm it chose;
//! 3. a **step** hands the next state (the step's own next-state nodes, in slot
//!    order) to the next iteration; an **exit** ends the loop — the apply's
//!    result is then this instantiation's *return*, evaluated exactly as the
//!    unroll's tail evaluates it, so the base's value, its pair shape and its
//!    type binding are produced by the one code path that already knows how.
//!
//! Nested tests (a conditional inside a conditional on the spine) are resolved
//! within one iteration, because they are one iteration's decision tree — the
//! same reason the conversion records them as a tree rather than as blocks.
//!
//! # What decides, and what is refused
//!
//! An iteration whose test does not decide ends the loop **undecided**: the
//! apply answers with the lazy marker, exactly as a body the evaluator could
//! not decide. That is the kernel-runtime state — a count the host cannot see —
//! and the checker's refusal (`DiagKind::LoopNotRecorded` /
//! `LoopNotEmitted`) is still what a program meets for it.
//!
//! Everything the loop does force is what the next test or the exit reads, so
//! a state component nothing reads stays lazy, which is what keeps a converted
//! loop equivalent to the unroll rather than merely equal on decided inputs.
//!
//! # The budget
//!
//! The entering call is **one** application: it costs one level of nesting and
//! one apply, however many iterations it runs. Each iteration is charged
//! against [`Module::loop_iteration_limit`] instead — a loop is the cheap path,
//! and the budget that makes a runaway *expansion* fail fast would otherwise cap
//! every converted loop at the same count. An accidentally non-terminating
//! marked loop (a step that never changes the state its test reads) is refused
//! by that budget with [`BudgetExhausted::LoopIterations`], not left to hang.

use lichen_utils::extend::AsEnum;

use crate::{
    AnyNodeId::Dynamic as Dyn, ArrayItem, BlockId, BudgetExhausted, FunctionId, LoopArm,
    LoopConversion, LowValue, Module, NodeId, Program,
};

/// What one iteration decided.
enum Iteration {
    /// The test chose a step: go round again with this next state, whose nodes
    /// belong to the instantiation that produced them.
    Step(Vec<NodeId>),
    /// The test chose a base: the apply's result is the pair
    /// `[value, type]` this instantiation resolves the base's value half and
    /// the function's return type half to — or, for a function whose return is
    /// stated bare rather than as a `[value, type]` pair, just the value.
    Exit {
        value: NodeId,
        r#type: Option<NodeId>,
    },
    /// The test did not decide. Nothing is known: the loop ends undecided.
    Undecided,
    /// The test chose an index the spine does not name — the evaluator's own
    /// out-of-bounds read, not a third arm. The template's return is evaluated
    /// so the same `Index` arm records the same failure.
    Unnamed { applied: NodeId },
    /// The argument failed the parameter check, which `instantiate` recorded.
    Refused,
}

impl<P: Program> Module<P> {
    /// Run `conversion` from `argument`, in place of expanding `function`'s
    /// recursion. `node` is the entering apply (for error attribution) and
    /// `cell` its checker-wired result cell.
    pub(super) fn apply_loop(
        &mut self,
        conversion: &LoopConversion,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> P::Value {
        // **The entering call is one application.** It charges the apply budget
        // and one level of nesting like any other call, however many iterations
        // it goes on to run; only what the loop does *inside* is the loop's own
        // work, which is what the flag brackets (see `apply.rs`).
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
    ) -> P::Value {
        let mut argument = argument;
        loop {
            // **An iteration is an application.** The unwound level it replaces
            // is one application too, so the cumulative counter means the same
            // thing on both paths and the limit the host states bounds both;
            // what the loop does not spend is *depth*. Anything the iteration
            // applies in turn charges the counters through the ordinary frame.
            self.apply_total += 1;
            if self.apply_total > self.apply_total_limit {
                if self.budget_exhausted.is_none() {
                    self.budget_exhausted = Some(BudgetExhausted::ApplyTotal {
                        limit: self.apply_total_limit,
                    });
                }
                return P::Value::from(LowValue::Parameterized);
            }
            match self.loop_iteration(conversion, function, argument, block, node) {
                Iteration::Step(next) => {
                    // The state crosses the backedge **lazily**: the step's
                    // next-state nodes become the next iteration's parameter
                    // values through the same unify the unroll uses, so nothing
                    // is computed before the next test asks for it. The state is
                    // decided by the time the loop returns, because the exit
                    // forces its base value here, inside the loop — see the
                    // `Exit` arm.
                    argument = self.loop_argument(conversion, function, &next, argument, block);
                }
                Iteration::Exit { value, r#type } => {
                    // **The result is built, not read off the template's
                    // return.** Evaluating the return would produce the same
                    // `[value, type]` pair, but it leaves the selection's
                    // *untaken* step arm in the graph — an apply of this very
                    // function — and the next deep pass walks it and unrolls one
                    // level, then that level's arm, and so on: the trip count
                    // the loop just avoided paying, charged to the apply budget
                    // after the answer was already known. So the result is
                    // assembled from the base's value half and the return's own
                    // type half, which is the whole of what an apply's result
                    // is, and the wiring an apply's tail does is done here.
                    let decided = self.evaluate_node_deep(value, Some(block));
                    let Some(r#type) = r#type else {
                        // A function whose return is stated bare: the value is
                        // the result, and there is no cell to bind.
                        self.write_node_value(node, Some(decided.clone()));
                        return decided;
                    };
                    let items = vec![ArrayItem::new(Dyn(value)), ArrayItem::new(Dyn(r#type))];
                    let array = self.alloc_array(&items, block);
                    let result = P::Value::from(LowValue::Array(array));
                    self.write_node_value(node, Some(result.clone()));
                    if let Some(cell) = cell {
                        // The return type is resolved before the cell binds, as
                        // the unroll's tail does: the deep pass resolves the
                        // node later but does not replicate to class members.
                        self.evaluate_node(Dyn(r#type), Some(block));
                        self.unify(cell, r#type);
                    }
                    return result;
                }
                Iteration::Unnamed { applied } => {
                    return self.evaluate_node(Dyn(applied), Some(block));
                }
                Iteration::Undecided | Iteration::Refused => {
                    return P::Value::from(LowValue::Parameterized);
                }
            }
        }
    }

    /// One iteration: instantiate the body for `argument`, resolve the spine's
    /// tests until an arm is chosen, and report what that arm asks for.
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
            let Some(LowValue::USize(choice)) = value.as_enum() else {
                return Iteration::Undecided;
            };
            // The selector is an index into `[else, then]`, so `0` is the
            // `else` arm and `1` the `then` arm. **Any other index is the
            // evaluator's own out-of-bounds read**, not a third arm: taking the
            // return here hands the same read to the same `Index` arm, which
            // records it and yields the computed nothing — one rule, one place.
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
                    // The pair's type half is the function's own return type,
                    // resolved into this instantiation — the slot the apply
                    // binds its result cell to. A bare return has none, and the
                    // value is then the result on its own.
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
    /// A scalar state's value half *is* the value; a tuple's is an array of the
    /// slots — the conversion's own paths say which ([`LoopConversion::state`]),
    /// which is the consumer side of "a slot is a path, not an index". The type
    /// half is the entering argument's, so each iteration's parameter check
    /// means what the entry's did: the state's shape is the loop's invariant,
    /// its values are not.
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
        // A bare parameter — a hand-built graph, or anything not written in the
        // checker's `[value, type]` convention — takes the value alone.
        if self
            .pair_value_half(self.functions[function].parameter)
            .is_none()
        {
            return value;
        }
        let r#type = self.pair_type_half(entering).unwrap_or_else(|| {
            // No entering type to re-state: a fresh cell leaves the shape
            // unchecked rather than pinning it to the template's own cell.
            self.add_node(block, None, Some(P::Value::from(LowValue::Parameterized)))
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
