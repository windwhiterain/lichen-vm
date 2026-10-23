//! The apply tail `function.rs` and `static_module.rs` share, so the two paths cannot drift.

use crate::{
    AnyFunctionId, AnyNodeId, ApplyError, BlockId, BudgetExhausted, LowValue, Module, NodeId,
    Program,
};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
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
        // The attribute tail is the checker's reconciliation, not the engine's.
        let lengths = (
            unsafe { self.array_items(cloned_param) }.map(|items| items.len()),
            unsafe { self.array_items(argument) }.map(|items| items.len()),
        );
        match lengths {
            (Some(parameter), Some(supplied)) if parameter > 2 || supplied > 2 => {
                // SAFETY: both nodes are live members of this module; the unify
                // below only reads them.
                let parameters = unsafe { self.array_items(cloned_param) };
                let supplieds = unsafe { self.array_items(argument) };
                let width = parameters
                    .map_or(0, |items| items.len())
                    .max(supplieds.map_or(0, |items| items.len()));
                // **Every position is compared**: the highlevel pardons an attribute's
                // failure (`docs/notes/array-attribute-view.md`).
                for slot in 0..width {
                    let (Some(parameters), Some(supplieds)) = (parameters, supplieds) else {
                        break;
                    };
                    let (Some(left), Some(right)) = (parameters.get(slot), supplieds.get(slot))
                    else {
                        continue;
                    };
                    let left = self.as_dynamic(left.node, block);
                    let right = self.as_dynamic(right.node, block);
                    self.unify(left, right);
                }
            }
            _ => {
                self.unify(cloned_param, argument);
            }
        };
        if self.unify_errors.len() == pre_unify_errors {
            return false;
        }

        // Record it and **continue**: a failed unify must not shrink the
        // checked set (`docs/notes/array-attribute-view.md`).

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
        false
    }

    /// Wire the apply result: cache the return pair, unify it with the cloned return
    /// node.
    ///
    /// # Invariant
    /// This level learns nothing about which element of a pair is the value and which is
    /// the type: it caches the pair it produced and stops. A caller that needs an element
    /// bound binds it itself, and the two applications an apply encodes are one class
    /// because the caller unifies them.
    pub(super) fn wire_apply_result(
        &mut self,
        node: NodeId,
        result: Option<P::Value>,
        applied: NodeId,
        block: BlockId,
    ) -> Option<P::Value> {
        // The wildcard is not an answer to hand on: an apply that produced it is
        // undecided, exactly as a read of it is.
        if matches!(
            result.and_then(|value| value.as_enum()),
            Some(LowValue::Any)
        ) {
            return None;
        }
        match result.and_then(|value| value.as_enum()) {
            // SAFETY: `array` is `result`'s live payload; this covers the `items()` call.
            Some(LowValue::Array(array)) if unsafe { array.items() }.len() >= 2 => {
                // The result is written only into a class that holds nothing.
                if self.nodes[node].value.is_none() {
                    self.write_node_value(node, result);
                }
                self.unify(node, applied);
                // Only a deep pass resolves the unmasked elements; a shallow one leaves
                // them as the body wrote them.
                if self.deep_depth > 0 {
                    for item in unsafe { array.items() } {
                        if item.shallow {
                            continue;
                        }
                        let item = self.as_dynamic(item.node, block);
                        self.evaluate_node(crate::AnyNodeId::Dynamic(item), Some(block));
                    }
                }
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
