//! The shared tail of dynamic and static function application.
//!
//! Dynamic functions (`function.rs`) and static function materialization
//! (`static_module.rs`) differ in how they clone the applied template, but
//! they share the same apply bookkeeping: enter/leave the apply budget,
//! unify the cloned parameter against the argument, record a failed
//! parameter check, and wire the returned pair into the apply node/cell.
//! Keeping those pieces here prevents the two apply paths from drifting
//! apart.

use std::collections::HashMap;

use crate::{AnyFunctionId, ApplyError, BlockId, LowValue, Module, NodeId, Program};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// Run `body` inside one application frame: bump the nested and total
    /// apply counters, enforce the budgets, and always pop the nested
    /// counter when the body returns.  A panic does not unwind the counter,
    /// matching the previous behaviour (panics abort the run anyway).
    pub(super) fn with_apply_frame(
        &mut self,
        body: impl FnOnce(&mut Self) -> P::Value,
    ) -> P::Value {
        self.apply_depth += 1;
        self.apply_total += 1;
        if self.apply_depth > self.apply_depth_limit {
            panic!(
                "recursion depth exceeded in function application (limit {}) — non-terminating recursion?",
                self.apply_depth_limit
            );
        }
        if self.apply_total > self.apply_total_limit {
            panic!(
                "too many function applications (limit {}) — non-terminating recursion?",
                self.apply_total_limit
            );
        }
        let result = body(self);
        self.apply_depth -= 1;
        result
    }

    /// The post-clone parameter check shared by dynamic and static applies.
    ///
    /// `cloned_param` is the fresh parameter clone the apply walk produced;
    /// `parameter_type_source` is the node whose type slot names the declared
    /// parameter type — the original template parameter for dynamic applies,
    /// the instantiated clone for static materialization (where the template
    /// parameter is not a dynamic [`NodeId`]).
    ///
    /// Returns `true` when the argument failed the parameter check and an
    /// [`ApplyError`] was recorded.  The caller should stop the apply without
    /// evaluating the body.
    pub(super) fn apply_parameter_check(
        &mut self,
        cloned_param: NodeId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        function: AnyFunctionId,
        parameter_type_source: NodeId,
    ) -> bool {
        // Evaluate the argument to the depth the parameter's pattern
        // references, so the unify sees the argument's element values
        // instead of unbound slots; positions the pattern treats as opaque
        // stay lazy.
        self.evaluate_pattern_argument(cloned_param, argument, block);
        let pre_unify_errors = self.unify_errors.len();
        self.unify(cloned_param, argument);
        if self.unify_errors.len() == pre_unify_errors {
            return false;
        }

        // A failed parameter check: the argument does not fit the applied
        // function's declared parameter type.  Record the apply context for
        // attribution (the raw UnifyError leaves drop the two top-level
        // sides), then stop — evaluating the body under a mismatched
        // argument is meaningless and may well panic (e.g. an `Index` over a
        // non-array value).  Deduplicated by apply node, so a later re-read
        // of the same apply does not re-record it.
        let parameter_type = self
            .array_items(parameter_type_source)
            .and_then(|items| items.get(1))
            .map(|item| self.as_dynamic(item.node, block))
            .unwrap_or(parameter_type_source);
        let argument_type = self
            .array_items(argument)
            .and_then(|items| items.get(1))
            .map(|item| self.as_dynamic(item.node, block))
            .unwrap_or(argument);
        if !self.apply_errors.iter().any(|e| e.apply_node == node) {
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

    /// The post-evaluation apply result wiring shared by dynamic and static
    /// applies.
    ///
    /// The apply node caches the return pair and is unified with the cloned
    /// return node, so the classes merge — the apply node *is* the return
    /// pair — and the result cell (the checker's third operand element)
    /// binds to the return type.
    pub(super) fn wire_apply_result(
        &mut self,
        node: NodeId,
        cell: Option<NodeId>,
        result: P::Value,
        applied: NodeId,
        block: BlockId,
    ) -> P::Value {
        match (cell, result.as_enum()) {
            (Some(cell), Some(LowValue::Array(array))) if array.items().len() >= 2 => {
                let items = array.items();
                self.write_node_value(node, Some(result));
                self.unify(node, applied);
                // Resolve the return type before binding the cell: the deep
                // pass resolves the node later but does not replicate to
                // class members, so an unresolved bind would leave the cell
                // unbound.  A lazy return type — a body ending in a call —
                // is an Index read, which already aliased its target cell at
                // evaluation time (see the Index arm), so this unify joins
                // the cell into that class and the binding propagates
                // regardless of when the nested apply runs.  Element 1 is
                // the type slot for a 2-wide pair and for a 3-wide
                // `[value, type, perspective]` pair alike.
                let item = self.as_dynamic(items[1].node, block);
                self.evaluate_node(crate::AnyNodeId::Dynamic(item), Some(block));
                self.unify(cell, item);
                result
            }
            _ => result,
        }
    }
}

/// Group the clones of one apply pass by their template representative, so a
/// pass can re-establish the template's internal class topology among the
/// fresh singleton classes.  The representative function differs for a
/// dynamic template (`disjoint::find` on the live module) and a static
/// template (`static_find` on the immutable solved module); caller supplies
/// it.
pub(super) fn regroup_clones<K, I>(
    remap: I,
    mut find: impl FnMut(K) -> K,
) -> HashMap<K, Vec<NodeId>>
where
    K: Copy + Eq + std::hash::Hash,
    I: IntoIterator<Item = (K, NodeId)>,
{
    let mut groups: HashMap<K, Vec<NodeId>> = HashMap::new();
    for (template, clone) in remap {
        groups.entry(find(template)).or_default().push(clone);
    }
    groups
}
