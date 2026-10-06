//! The shared tail of dynamic and static function application.
//!
//! Dynamic functions (`function.rs`) and static function materialization
//! (`static_module.rs`) differ in how they clone the applied template, but
//! they share the same apply bookkeeping: enter/leave the apply budget,
//! unify the cloned parameter against the argument, record a failed
//! parameter check, and wire the returned pair into the apply node/cell.
//! Keeping those pieces here prevents the two apply paths from drifting
//! apart.

use crate::{
    AnyFunctionId, ApplyError, BlockId, BudgetExhausted, LowValue, Module, NodeId, Program,
};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// Run `body` inside one application frame: charge the work counter and
    /// enforce its bound.
    ///
    /// A budget that the frame exceeds is **recorded, not unwound**: the body
    /// is refused, [`Module::budget_exhausted`] takes the budget and its
    /// limit, and the frame returns the undecided marker.
    ///
    /// The frame charges the **work** counter only.  The nesting guard is not
    /// here, because nesting is not a property of the walk: it is read off the
    /// node being applied ([`Module::node_depth`]) before the instantiation
    /// happens, so it holds the same whether the expansion is forced as it is
    /// built or walked later — see [`Module::function_apply`].
    pub(super) fn with_apply_frame(
        &mut self,
        body: impl FnOnce(&mut Self) -> P::Value,
    ) -> P::Value {
        self.apply_total += 1;
        let exhausted =
            (self.apply_total > self.apply_total_limit).then_some(BudgetExhausted::ApplyTotal {
                limit: self.apply_total_limit,
            });
        if let Some(exhausted) = exhausted {
            if self.budget_exhausted.is_none() {
                self.budget_exhausted = Some(exhausted);
            }
            // The body never ran, so nothing was computed — and the answer
            // is *unknown*, not nothing: return the undecided marker, the
            // same refusal `apply_parameter_check` already issues for a body
            // it declined to run.  `LowValue::Error` would instead be cached
            // by the `evaluate_node` postlude as a decided value, letting
            // the deep pass certify this node concrete (its
            // `evaluated_deep.parameterized` derives from the cached value)
            // and every parent array along with it — a proven-concrete
            // claim about a computation that never happened.
            return P::Value::from(LowValue::Parameterized);
        }
        body(self)
    }

    /// Whether applying the function at `node` would instantiate a body deeper
    /// than [`Self::apply_depth_limit`], recording the verdict when it would.
    ///
    /// **The depth is the node's** ([`Self::node_depth`]), so the answer does
    /// not depend on how the graph was built: an expansion's level `k` is
    /// stamped `k` and a converted loop's iterations are all stamped alike,
    /// whatever forces their values and whenever.
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
        // SAFETY: `parameter_type_source` is a node of this module, reachable
        // here — its home block is alive and has not been dropped.
        let parameter_type = unsafe { self.array_items(parameter_type_source) }
            .and_then(|items| items.get(1))
            .map(|item| self.as_dynamic(item.node, block))
            .unwrap_or(parameter_type_source);
        // SAFETY: `argument` is a node of this module, reachable here — its
        // home block is alive and has not been dropped.
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
            // SAFETY: `array` is the payload of `result`, a value this module
            // just evaluated, so its home block is alive and not dropped; the
            // note covers both `items()` calls in this arm.
            (Some(cell), Some(LowValue::Array(array))) if unsafe { array.items() }.len() >= 2 => {
                let items = unsafe { array.items() };
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
///
/// The answer is one `Vec` sorted by representative, which the caller reads as
/// runs.  A `HashMap` of groups costs a table allocation plus one `Vec` per
/// group on **every apply**, and the order it hands the groups back in is
/// arbitrary, so nothing depends on it; the sort is stable, so a group's
/// clones keep the walk's insertion order — the order the unification pairs
/// them in.
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

/// Re-establish one apply pass's template topology among its fresh clones:
/// unify every run [`regroup_clones`] produced.  The grouping caller picks
/// the representative; this half — the unification policy — is the same for
/// the dynamic and the static apply path, so it is stated once.
///
/// The groups are disjoint sets of freshly minted nodes, so the order the
/// runs are visited in cannot affect the result; only a run's own order can,
/// and that is preserved.
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
