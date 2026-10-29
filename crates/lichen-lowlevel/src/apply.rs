//! The apply tail `function.rs` and `static_module.rs` share, so the two paths cannot drift.

use std::collections::HashMap;

use crate::{
    AnyFunctionId, AnyNodeId, ApplyError, BlockId, BudgetExhausted, LowOperator, LowValue, Module,
    NodeId, Program,
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

    /// Unify the parameter's positions with the argument's, **without recording**
    /// anything.
    ///
    /// # Invariant
    /// Every position is unified, the attribute positions included: the parameter's
    /// positions and the argument's are the same expression's slots
    /// (spec §3, "Every attribute is handled like the type").
    pub(super) fn unify_parameter_slots(&mut self, cloned_param: NodeId, argument: AnyNodeId) {
        // The reporting side owns the failure; this binding only needs the classes.
        let recorded = self.unify_errors.len();
        let lengths = (
            unsafe { self.array_items(cloned_param) }.map(|items| items.len()),
            // SAFETY: the argument ref is live; the compare only reads it.
            unsafe { self.array_items_ref(argument) }.map(|items| items.len()),
        );
        match lengths {
            (Some(parameter), Some(supplied)) if parameter > 2 || supplied > 2 => {
                // SAFETY: both nodes are live members of this module; the unify only
                // reads them.
                let parameters = unsafe { self.array_items(cloned_param) };
                // SAFETY: the argument ref is live; the compare only reads it.
                let supplieds = unsafe { self.array_items_ref(argument) };
                let width = parameters
                    .map_or(0, |items| items.len())
                    .max(supplieds.map_or(0, |items| items.len()));
                for slot in 0..width {
                    let (Some(parameters), Some(supplieds)) = (parameters, supplieds) else {
                        break;
                    };
                    let (Some(left), Some(right)) = (parameters.get(slot), supplieds.get(slot))
                    else {
                        continue;
                    };
                    self.unify_ref(left.node, right.node);
                }
            }
            _ => {
                self.unify_ref(AnyNodeId::Dynamic(cloned_param), argument);
            }
        }
        self.unify_errors.truncate(recorded);
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
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        function: AnyFunctionId,
        parameter_type_source: NodeId,
    ) -> bool {
        // Evaluate the argument only to the depth the pattern references, so an
        // opaque position stays lazy.
        self.evaluate_pattern_argument(cloned_param, argument, block);
        let pre_unify_errors = self.unify_errors.len();
        // Every position is compared, attributes included: the order
        // (`declared | value`) is decided only after the compare.
        let lengths = (
            unsafe { self.array_items(cloned_param) }.map(|items| items.len()),
            unsafe { self.array_items_ref(argument) }.map(|items| items.len()),
        );
        match lengths {
            (Some(parameter), Some(supplied)) if parameter > 2 || supplied > 2 => {
                // SAFETY: both nodes are live members of this module; the unify
                // below only reads them.
                let parameters = unsafe { self.array_items(cloned_param) };
                let supplieds = unsafe { self.array_items_ref(argument) };
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
                    self.unify_ref(left.node, right.node);
                }
            }
            _ => {
                self.unify_ref(AnyNodeId::Dynamic(cloned_param), argument);
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
        // A failed check's diagnostics name a node; the copy binds nothing.
        let argument_node = argument
            .dynamic()
            .unwrap_or_else(|| self.as_dynamic(argument, block));
        // SAFETY: `argument_node` is a live node of this module.
        let argument_type = unsafe { self.array_items(argument_node) }
            .and_then(|items| items.get(1))
            .map(|item| self.as_dynamic(item.node, block))
            .unwrap_or(argument_node);
        if self.apply_error_nodes.insert(node) {
            self.apply_errors.push(ApplyError {
                function,
                parameter_type,
                argument_type,
                argument: argument_node,
                apply_node: node,
                error_index: pre_unify_errors,
            });
        }
        false
    }

    /// Rebind this apply's field-source projections to the supplied argument's fields.
    ///
    /// # Invariant
    /// Only constant `Index` chains rooted at the cloned parameter are resolved, and only
    /// against the same slots of the actual argument; no node is unified to create provenance.
    pub(crate) fn bind_field_sources(
        &mut self,
        parameter_image: NodeId,
        argument: AnyNodeId,
        instantiated: &[NodeId],
    ) {
        let parameter_value = self
            .expr_value_half(parameter_image)
            .unwrap_or(parameter_image);
        let mut replacements = HashMap::new();
        for &candidate in instantiated {
            let mut current = candidate;
            let mut path = Vec::new();
            for _ in 0..8 {
                if current == parameter_image || current == parameter_value {
                    break;
                }
                let Some(operation) = self.node_operation(current) else {
                    break;
                };
                if !matches!(
                    AsEnum::<LowOperator>::as_enum(&operation.operator),
                    Some(LowOperator::Index)
                ) {
                    break;
                }
                let Some(operand) = operation.operand else {
                    break;
                };
                let Ok(operands) = self.operand_expr(operand, "Index") else {
                    break;
                };
                let Some(position) = self.usize_value(operands[1]) else {
                    break;
                };
                path.push(position);
                current = operands[0];
            }
            if (current != parameter_image && current != parameter_value) || path.is_empty() {
                continue;
            }
            path.reverse();
            let mut supplied = argument;
            let mut resolved = true;
            for position in &path {
                let Some(items) = (unsafe { self.array_items_of(supplied) }) else {
                    resolved = false;
                    break;
                };
                let Some(item) = items.get(*position) else {
                    resolved = false;
                    break;
                };
                supplied = item.node;
            }
            if resolved
                && supplied != argument
                && let Some(supplied) = supplied.dynamic()
            {
                replacements.insert(candidate, supplied);
            }
        }
        for &required in instantiated {
            let Some(supplied) = self.field_sources.get(&required).copied() else {
                continue;
            };
            if let Some(actual) = replacements.get(&supplied).copied() {
                self.field_sources.insert(required, actual);
            }
        }
    }

    /// Record this call's identity on a clone.
    ///
    /// # Invariant
    /// One current call identity per clone: a later call of the same clone replaces it, so a
    /// recursive chain neither grows without bound nor resolves against an outer call.
    pub(super) fn record_clone_parameter(&mut self, clone: NodeId, formal: NodeId, image: NodeId) {
        self.clone_parameters.insert(clone, (formal, image));
        self.parameter_images.insert(formal, image);
    }

    /// Resolve a shared constant `Index` projection against the call its container belongs to.
    ///
    /// # Invariant
    /// Only constant `Index` steps are followed. A step is answered only when every image of
    /// the formal with a decided leaf agrees; otherwise the operation stays open and is retried
    /// when a parameter check binds an image. No node is unified and no shared projection
    /// receives a source.
    pub fn parameter_projection(
        &mut self,
        container: NodeId,
        leaf: NodeId,
        block: BlockId,
    ) -> Option<NodeId> {
        let mut root = leaf;
        let mut path = Vec::new();
        for _ in 0..8 {
            let Some(operation) = self.node_operation(root) else {
                break;
            };
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&operation.operator),
                Some(LowOperator::Index)
            ) {
                break;
            }
            let Some(operand) = operation.operand else {
                return None;
            };
            let operands = self.operand_expr(operand, "Index").ok()?;
            let target = *operands.first()?;
            let selector = *operands.get(1)?;
            self.evaluate_node(AnyNodeId::Dynamic(selector), Some(block));
            path.push(self.usize_value(selector)?);
            root = target;
        }
        if path.is_empty() {
            return None;
        }
        let mut images = Vec::new();
        let mut current = container;
        for _ in 0..8 {
            if let Some(&(parameter, image)) = self.clone_parameters.get(&current)
                && parameter == root
                && !images.contains(&image)
            {
                images.push(image);
            }
            let Some(origin) = self.node_origin(current) else {
                break;
            };
            current = origin;
        }
        if let Some(&image) = self.parameter_images.get(&root)
            && !images.contains(&image)
        {
            images.push(image);
        }
        let mut decided: Option<(NodeId, LowValue)> = None;
        for image in images {
            let Some(resolved) = self.projection_at(image, &path, block) else {
                continue;
            };
            let Some(value) = self
                .node_value(AnyNodeId::Dynamic(resolved))
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
            else {
                continue;
            };
            if matches!(value, LowValue::Any) {
                continue;
            }
            if let Some((_, previous)) = decided {
                if previous != value {
                    self.defer_current_operation(root);
                    return None;
                }
            } else {
                decided = Some((resolved, value));
            }
        }
        if let Some((resolved, _)) = decided {
            return Some(resolved);
        }
        self.defer_current_operation(root);
        None
    }

    /// Follow one constant `Index` path from an actual parameter image.
    fn projection_at(&mut self, image: NodeId, path: &[usize], block: BlockId) -> Option<NodeId> {
        let mut current = AnyNodeId::Dynamic(image);
        for &position in path.iter().rev() {
            self.evaluate_node(current, Some(block));
            // SAFETY: `current` is a live node of this module, and this walk releases no block.
            current = unsafe { self.array_items_of(current) }?.get(position)?.node;
        }
        self.evaluate_node(current, Some(block));
        current.dynamic()
    }

    /// Register the current operation until a call of `formal` binds an actual image.
    ///
    /// # Invariant
    /// Only the operation the evaluator is running is touched, once per formal, and only while
    /// its value slot is empty; the parameter check owns the retry.
    fn defer_current_operation(&mut self, formal: NodeId) {
        let Some(node) = self.operator_for_diagnostics else {
            return;
        };
        if self.nodes[node].value.is_some() {
            return;
        }
        self.nodes[node].evaluated_deep = None;
        self.stamp_node_ran_if(node, false);
        let operations = self.pending_projections.entry(formal).or_default();
        if !operations.contains(&node) {
            operations.push(node);
        }
    }

    /// Retry the operations waiting for this formal once its actual image is bound.
    ///
    /// # Invariant
    /// The list is removed before any operation runs, so one retry cannot re-enter its own
    /// pair; an operation that answered meanwhile is left alone.
    pub(super) fn replay_pending_projections(&mut self, formal: NodeId, block: BlockId) {
        let Some(operations) = self.pending_projections.remove(&formal) else {
            return;
        };
        for node in operations {
            if self.nodes[node].value.is_some() {
                continue;
            }
            self.nodes[node].evaluated_deep = None;
            self.stamp_node_ran_if(node, false);
            self.evaluate_node(AnyNodeId::Dynamic(node), Some(block));
        }
    }

    /// Record an apply's returned expression and its value's owner.
    pub(super) fn record_applied_expr(&mut self, node: NodeId, applied: NodeId) {
        self.applied_exprs.insert(node, applied);
        let value = self.expr_value_half(applied).unwrap_or(applied);
        self.apply_result_owners.insert(value, node);
        if let Some(supplied) = self.field_sources.get(&node).copied() {
            self.field_sources.insert(value, supplied);
        }
    }

    /// Wire the apply result: cache the return expr, unify it with the cloned return
    /// node.
    ///
    /// # Invariant
    /// This level learns nothing about which element of an expr is the value and which is
    /// the type: it caches the expr it produced and stops. A caller that needs an element
    /// bound binds it itself, and the two applications an apply encodes are one class
    /// because the caller unifies them.
    pub(super) fn wire_apply_result(
        &mut self,
        node: NodeId,
        result: Option<P::Value>,
        applied: NodeId,
        block: BlockId,
    ) -> Option<P::Value> {
        self.record_applied_expr(node, applied);
        if let Some(&(parameter, image)) = self.clone_parameters.get(&applied) {
            self.record_clone_parameter(node, parameter, image);
        }
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
                        self.evaluate_node(item.node, Some(block));
                    }
                }
                result
            }
            _ => result,
        }
    }
}

/// Whether `clone` is a **plain cell**: no operator of its own and no answer.
///
/// # Invariant
/// A plain cell owes no answer, so joining two of them cannot make a class
/// member run early.  The template's other pairs are the runtime's own: a
/// reader joins the cell it reads when the read runs
/// ([`Module::alias_read`]), and the conditional's arms are one class that
/// holds one arm's answer, so joining it early would put the conditional's
/// result there.
pub(super) fn clone_is_a_plain_cell<P: Program>(module: &Module<P>, clone: NodeId) -> bool {
    let clone = &module.nodes[clone];
    clone.operation.is_none() && clone.value.is_none()
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
/// order the unification exprs them in.
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
