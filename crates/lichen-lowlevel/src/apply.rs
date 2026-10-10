//! The apply tail `function.rs` and `static_module.rs` share, so the two paths cannot drift.

use crate::{
    AnyFunctionId, AnyNodeId, ApplyError, BlockId, BudgetExhausted, LowValue, Module, NodeId,
    Program,
};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// Run `body` inside one application frame: charge the work counter and enforce
    /// its bound.
    ///
    /// # Invariant
    /// An exceeded budget is **recorded, not unwound**: the body is refused,
    /// [`Module::budget_exhausted`] keeps the budget and its limit, and the frame
    /// returns the undecided marker.  Only the **work** counter is charged here;
    /// the nesting guard reads the applied node's [`Self::node_depth`] instead,
    /// so it holds however the expansion was forced.
    pub(super) fn with_apply_frame(
        &mut self,
        body: impl FnOnce(&mut Self) -> Option<P::Value>,
    ) -> Option<P::Value> {
        self.apply_total += 1;
        let exhausted =
            (self.apply_total > self.apply_total_limit).then_some(BudgetExhausted::ApplyTotal {
                limit: self.apply_total_limit,
            });
        if let Some(exhausted) = exhausted {
            if self.budget_exhausted.is_none() {
                self.budget_exhausted = Some(exhausted);
            }
            // The body never ran, so the answer is unknown: `None`, not an `Error`
            // the postlude caches as decided.
            return None;
        }
        body(self)
    }

    /// Whether applying `node` would instantiate a body deeper than [`Self::apply_depth_limit`].
    ///
    /// # Invariant
    /// The depth read is the node's own [`Self::node_depth`], not the walk's, so the
    /// verdict does not depend on how or when the graph was forced.
    pub(super) fn depth_exhausted(&mut self, node: NodeId) -> bool {
        let depth = self.node_depth(node) as usize + 1;
        if depth <= self.apply_depth_limit {
            return false;
        }
        if self.budget_exhausted.is_none() {
            self.budget_exhausted = Some(BudgetExhausted::ApplyDepth {
                limit: self.apply_depth_limit,
            });
        }
        true
    }

    /// The post-clone parameter check shared by both apply paths; `true` when it failed.
    ///
    /// # Invariant
    /// On failure an [`ApplyError`] is recorded and the caller stops the apply without
    /// evaluating the body.  `parameter_type_source` names the node whose type slot
    /// holds the declared parameter type: the template parameter for a dynamic apply,
    /// the instantiated clone for a static one, where the template parameter is not a
    /// dynamic [`NodeId`].
    pub(super) fn apply_parameter_check(
        &mut self,
        cloned_param: NodeId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        function: AnyFunctionId,
        parameter_type_source: NodeId,
    ) -> bool {
        // Evaluate the argument only to the depth the pattern references, so an
        // opaque position stays lazy.
        self.evaluate_pattern_argument(cloned_param, argument, block);
        let pre_unify_errors = self.unify_errors.len();
        self.unify(cloned_param, argument);
        if self.unify_errors.len() == pre_unify_errors {
            return false;
        }

        // Record the apply context, then stop — the body under a mismatched argument
        // may panic.  One error per apply node.

        // SAFETY: `parameter_type_source` is a live node of this module.
        let parameter_type = unsafe { self.array_items(parameter_type_source) }
            .and_then(|items| items.get(1))
            .map(|item| self.as_dynamic(item.node, block))
            .unwrap_or(parameter_type_source);
        // SAFETY: `argument` is a live node of this module.
        let argument_type = unsafe { self.array_items(argument) }
            .and_then(|items| items.get(1))
            .map(|item| self.as_dynamic(item.node, block))
            .unwrap_or(argument);
        if self.apply_error_nodes.insert(node) {
            self.apply_errors.push(ApplyError {
                function,
                parameter_type,
                argument_type,
                argument,
                apply_node: node,
                error_index: pre_unify_errors,
            });
        }
        true
    }

    /// Wire the apply result: cache the return pair, unify it with the cloned return
    /// node.
    ///
    /// # Invariant
    /// The result cell binds to the return type, resolved before the bind: the deep
    /// pass resolves later and does not replicate to class members.  Element 1 is
    /// the type slot of a 2-wide or a 3-wide pair.
    pub(super) fn wire_apply_result(
        &mut self,
        node: NodeId,
        cell: Option<NodeId>,
        result: Option<P::Value>,
        applied: NodeId,
        block: BlockId,
    ) -> Option<P::Value> {
        match (cell, result.and_then(|value| value.as_enum())) {
            // SAFETY: `array` is `result`'s live payload; this covers both `items()` calls.
            (Some(cell), Some(LowValue::Array(array))) if unsafe { array.items() }.len() >= 2 => {
                let items = unsafe { array.items() };
                self.write_node_value(node, result);
                self.unify(node, applied);
                // Resolve the return type first — the deep pass does not replicate
                // to class members.
                let item = self.as_dynamic(items[1].node, block);
                self.evaluate_node(crate::AnyNodeId::Dynamic(item), Some(block));
                self.unify(cell, item);
                result
            }
            _ => result,
        }
    }
}

/// Whether a value's own item slots are open — the `runned` policy; see
/// `docs/notes/apply-clone-ownership.md`.
///
/// # Invariant
/// One level deep by design: a structure whose own elements are decided is a fact
/// a clone may answer with, however open its interior is.
pub(super) fn answer_elements_are_undecided<P: Program>(
    value: P::Value,
    mut slot_is_empty: impl FnMut(AnyNodeId) -> bool,
) -> bool {
    let Some(LowValue::Array(array)) = value.as_enum() else {
        return false;
    };
    // SAFETY: `array` is `value`'s payload, which the caller keeps reachable.
    unsafe { array.items() }
        .iter()
        .any(|item| slot_is_empty(item.node))
}

/// Group one apply pass's clones by template representative, sorted; the caller reads runs.
///
/// # Invariant
/// The sort is stable, so a group's clones keep the walk's insertion order — the
/// order the unification pairs them in.
pub(super) fn regroup_clones<K, I>(remap: I, mut find: impl FnMut(K) -> K) -> Vec<(K, NodeId)>
where
    K: Copy + Ord,
    I: IntoIterator<Item = (K, NodeId)>,
{
    let mut grouped: Vec<(K, NodeId)> = remap
        .into_iter()
        .map(|(template, clone)| (find(template), clone))
        .collect();
    grouped.sort_by_key(|&(representative, _)| representative);
    grouped
}

/// Unify every run [`regroup_clones`] produced, re-establishing the template
/// topology among the fresh clones.
///
/// # Invariant
/// The groups are disjoint sets of fresh nodes, so run order cannot affect the
/// result; a run's own order is preserved.
pub(super) fn unify_clone_groups<K>(groups: Vec<(K, NodeId)>, mut unify: impl FnMut(NodeId, NodeId))
where
    K: Copy + Eq,
{
    let mut start = 0;
    while start < groups.len() {
        let representative = groups[start].0;
        let first = groups[start].1;
        let mut next = start + 1;
        while next < groups.len() && groups[next].0 == representative {
            unify(first, groups[next].1);
            next += 1;
        }
        start = next;
    }
}
