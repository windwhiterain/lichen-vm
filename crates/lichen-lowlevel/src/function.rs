use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{ArrayRef, BlockId, Function, FunctionId, LowValue, Module, NodeId, Operation, Program};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

/// The fixed context of one clone pass: where the clones land, the
/// template's membership set, and the running node-id remap (template node
/// to its clone).
struct ApplyCtx<'a> {
    target: BlockId,
    members: &'a HashSet<NodeId>,
    /// The scope of a nested function value being cloned (see
    /// [`Module::value_apply`]): its own nodes join the membership, so
    /// references to the outer members inside the closure — a captured
    /// parameter — are rewritten to the fresh clones instead of pointing at
    /// the never-bound template cells.
    extra: Option<&'a HashSet<NodeId>>,
    /// The function being applied: its own value node is the recursion
    /// self-reference (referenced in place when proven concrete), while any
    /// *other* function value in the scope is a nested closure and must be
    /// cloned per call — its captures bind to this call's clones, which a
    /// concreteness proof of the value node cannot see.
    applied: FunctionId,
    /// The applied function's parameter pair, always cloned: the parameter
    /// check runs against the fresh clone, and every call must bind its own
    /// cells (recursion re-applies the template per level).
    parameter: NodeId,
    remap: &'a mut HashMap<NodeId, NodeId>,
}

impl<P: Program> Module<P> {
    /// `#[stacksafe]`: application recursion runs through here (and
    /// [`Module::evaluate_node`]) at one frame per level, so the depth guard
    /// must be able to grow the stack — otherwise a deep recursion overflows
    /// the native stack before the guard panics.
    #[stacksafe]
    pub(super) fn function_apply(
        &mut self,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> P::Value {
        self.apply_depth += 1;
        self.apply_total += 1;
        if self.apply_depth > self.apply_depth_limit {
            panic!(
                "recursion depth exceeded in function application (limit {}) — non-terminating function application?",
                self.apply_depth_limit
            );
        }
        if self.apply_total > self.apply_total_limit {
            panic!(
                "too many function applications (limit {}) — non-terminating recursion?",
                self.apply_total_limit
            );
        }
        let (r#return, parameter) = {
            let function = &self.functions[function];
            (function.r#return, function.parameter)
        };
        debug_assert!(self.functions[function].nodes.contains(&r#return));
        debug_assert!(self.functions[function].nodes.contains(&parameter));
        let members = self.functions[function].nodes.clone();
        let mut remap = HashMap::new();
        let mut ctx = ApplyCtx {
            target: block,
            members: &members,
            extra: None,
            applied: function,
            parameter,
            remap: &mut remap,
        };
        let applied = self.node_apply(r#return, &mut ctx);
        // The scope's assert points are side nodes — nothing in the return's
        // subtree references them, so the return clone cannot carry them.
        // Clone each one through the shared remap (its condition rewrites to
        // this call's clones, so a body's assert that could not resolve at
        // normalize re-checks the instantiated condition against the
        // argument); the clone registers itself, so the checker's pass sees
        // it.
        let scope_asserts: Vec<NodeId> = members
            .iter()
            .filter(|&&id| self.nodes[id].assert.is_some())
            .copied()
            .collect();
        for &point in &scope_asserts {
            self.node_apply(point, &mut ctx);
        }
        // The parameter is cloned like any parameterized node, and the clone
        // is unified with the argument instead of being replaced by it: the
        // class binding propagates the argument's value to every reference
        // to the parameter in the body.
        if let Some(&cloned_param) = ctx.remap.get(&parameter) {
            // The clones are fresh singleton classes; re-establish the
            // template's internal class topology among them, so template
            // nodes unified at definition time (e.g. the elements of a
            // homogeneous array pattern) stay unified after cloning — the
            // elementwise unify below then forces the argument to satisfy
            // the pattern's internal constraints.  A single clone (just the
            // parameter) has no topology to re-establish.
            if ctx.remap.len() > 1 {
                let mut groups: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
                for (&template, &clone) in ctx.remap.iter() {
                    groups
                        .entry(disjoint::find(&mut self.nodes, template))
                        .or_default()
                        .push(clone);
                }
                for clones in groups.values() {
                    let first = clones[0];
                    for &clone in &clones[1..] {
                        self.unify(first, clone);
                    }
                }
            }
            // Evaluate the argument to the depth the parameter's pattern
            // references, so the unify sees the argument's element values
            // instead of unbound slots; positions the pattern treats as
            // opaque stay lazy.
            self.evaluate_pattern_argument(cloned_param, argument, block);
            self.unify(cloned_param, argument);
        }
        let result = self.evaluate_node(applied, Some(block));
        self.apply_depth -= 1;
        // The apply's result is the function's return pair `[value, type]`
        // (the checker encodes every expression as such a pair).  The apply
        // node caches that pair and is unified with the return node, so the
        // classes merge — the apply node *is* the return pair — and the
        // result cell (the checker's third operand element) binds to the
        // return type.  Unifying the two equal pairs never conflicts (their
        // elements are the same nodes); a lazy result — a polymorphic
        // template — leaves the cell bound to an unresolved class, which
        // reads as unbound until the deep pass resolves it.  An apply
        // without a wired cell (a hand-built lowlevel graph) is unchanged.
        match (cell, result.as_enum()) {
            (Some(cell), Some(LowValue::Array(array))) if array.ids().len() == 2 => {
                let ids = array.ids();
                self.nodes[node].value = Some(result);
                self.unify(node, applied);
                // Resolve the return type before binding the cell: the deep
                // pass resolves the node later but does not replicate to
                // class members, so an unresolved bind would leave the cell
                // unbound.  A lazy return type — a body ending in a call —
                // is an Index read, which already aliased its target cell at
                // evaluation time (see the Index arm), so this unify joins
                // the cell into that class and the binding propagates
                // regardless of when the nested apply runs.
                self.evaluate_node(ids[1], Some(block));
                self.unify(cell, ids[1]);
                result
            }
            _ => result,
        }
    }

    /// Evaluate `argument` to the structural depth `pattern` (the cloned
    /// parameter) references, so the apply's unify sees the argument's
    /// element values instead of unbound slots.  Only array positions in
    /// the pattern recurse; sub-values the pattern treats as opaque stay
    /// unevaluated.  `seen` holds the `(pattern, argument)` pairs on the
    /// current recursion, so a structural cycle (the `Type : Type` universe,
    /// which a typed pattern's spine reaches twice) is walked once instead
    /// of looping.
    #[stacksafe]
    fn evaluate_pattern_argument(&mut self, pattern: NodeId, argument: NodeId, block: BlockId) {
        self.evaluate_pattern_argument_inner(pattern, argument, block, &mut HashSet::new());
    }

    #[stacksafe]
    fn evaluate_pattern_argument_inner(
        &mut self,
        pattern: NodeId,
        argument: NodeId,
        block: BlockId,
        seen: &mut HashSet<(NodeId, NodeId)>,
    ) {
        if !seen.insert((pattern, argument)) {
            return;
        }
        self.evaluate_node(argument, Some(block));
        let (Some(Some(LowValue::Array(pattern))), Some(Some(LowValue::Array(argument)))) = (
            self.nodes[pattern].value.map(|value| value.as_enum()),
            self.nodes[argument].value.map(|value| value.as_enum()),
        ) else {
            return;
        };
        for (i, (&pattern_id, &argument_id)) in pattern
            .ids()
            .iter()
            .zip(argument.ids().iter())
            .enumerate()
        {
            // A shallow position on either side is opaque — its subtree
            // stays lazy, so the apply's argument evaluation does not force
            // what the marker deliberately left unevaluated.
            if pattern.is_shallow(i) || argument.is_shallow(i) {
                continue;
            }
            self.evaluate_pattern_argument_inner(pattern_id, argument_id, block, seen);
        }
    }

    #[stacksafe]
    fn node_apply(&mut self, node: NodeId, ctx: &mut ApplyCtx<'_>) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&node) {
            return clone;
        }
        if !ctx.members.contains(&node) && !ctx.extra.is_some_and(|extra| extra.contains(&node)) {
            return node; // outside the template scope — reference as-is
        }
        // The body always exists, so only the parts that depend on the
        // parameter need fresh nodes: unevaluated operation nodes (their
        // operand edges must be rewritten) and nodes not proven concrete —
        // flagged parameterized nodes, plus nodes whose dependence was
        // never resolved (the Index/Apply arms read operands shallowly, so
        // those flags may still be `None`).  A node whose subtree evaluated
        // to a concrete value (`Some(false)`) is baked — reference it in
        // place.  A *function value* is never baked by that proof: its
        // body's dependence on this call is invisible to the deep pass, so
        // any function value other than the applied function's own
        // self-reference (the recursion point) is cloned per call — a
        // nested closure's captures must rebind to this call's clones.  The
        // same goes for a proven-concrete structure *containing* such a
        // function value (a function's pair, a tuple of closures): the
        // proof cannot see through the function's body either.
        let (value, operation, parameterized_deep) = {
            let source = &self.nodes[node];
            (source.value, source.operation, source.parameterized_deep)
        };
        // An operation node always clones: its cached value may have been
        // derived from the parameter (a read of a pinned parameter cell
        // evaluates to the pinned value, looking concrete), so it must
        // recompute against the remapped operands instead of being
        // referenced in place.
        let depends_on_parameter = node == ctx.parameter
            || operation.is_some()
            || parameterized_deep != Some(false)
            || value.is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    Some(LowValue::Function(function)) if function != ctx.applied
                )
            })
            || (parameterized_deep == Some(false)
                && self.value_contains_foreign_function(value, ctx.applied));
        if !depends_on_parameter {
            return node;
        }
        // Reserve the clone id before recursing so diamonds resolve to one
        // clone and value cycles to the clone's own (still evaluating) id.
        let clone = self.add_node(ctx.target, None, None);
        ctx.remap.insert(node, clone);
        // A cached value on an operation node was computed against the
        // body's parameter and is stale once the argument is mapped in, so
        // such clones are left unevaluated — the kept operand chain
        // recomputes against the argument.  Constant nodes (no operation)
        // carry their remapped value.
        let value = if operation.is_some() {
            None
        } else {
            value.map(|value| self.value_apply(value, ctx))
        };
        let operation = operation.map(|operation| Operation {
            operand: operation
                .operand
                .map(|operand| self.node_apply(operand, ctx)),
            ..operation
        });
        // An assert point clones with its condition remapped like any other
        // edge: an in-body assert that could not resolve at normalize (its
        // condition reads the unbound parameter) re-checks the instantiated
        // condition against this call's argument.
        let assert = self.nodes[node].assert.map(|condition| self.node_apply(condition, ctx));
        self.nodes[clone].value = value;
        self.nodes[clone].operation = operation;
        self.nodes[clone].assert = assert;
        if self.nodes[clone].assert.is_some() {
            // The clone is a fresh assertion — register it so the check
            // pass sees the instantiated constraint, not just the template's.
            self.asserts.push(clone);
        }
        clone
    }

    #[stacksafe]
    fn value_apply(&mut self, value: P::Value, ctx: &mut ApplyCtx<'_>) -> P::Value {
        match value.as_enum() {
            Some(LowValue::Array(array)) => {
                // The mask travels with the ids: each call's clone honors
                // its own markers.
                let nodes: Vec<NodeId> = array
                    .ids()
                    .iter()
                    .map(|&id| self.node_apply(id, ctx))
                    .collect();
                let mask: Vec<bool> = array.mask().to_vec();
                P::Value::from(LowValue::Array(ArrayRef {
                    ids: self.copy_nodes(&nodes, ctx.target),
                    shallow: self.copy_mask(&mask, ctx.target),
                }))
            }
            Some(LowValue::Function(function)) => {
                // A cloned function's scope is mapped like an array: every
                // member and both entry points are cloned into the target,
                // and the result is a fresh function homed on the target
                // block, so it is dropped with it.
                let (scope, r#return, parameter) = {
                    let function = &self.functions[function];
                    (
                        function.nodes.clone(),
                        function.r#return,
                        function.parameter,
                    )
                };
                // The nested function's own scope joins the clone's
                // template: its body may capture the applied function's
                // members (an outer parameter), and those references must
                // be rewritten to the fresh clones — the ones the apply's
                // parameter unify binds.  The remap is shared, so a member
                // reached from the outer body and from here is one clone.
                // `applied` switches to the function being instantiated, so
                // its own self-reference (if recursive) stays in place.
                let target = ctx.target;
                let mut inner = ApplyCtx {
                    target,
                    members: ctx.members,
                    extra: Some(&scope),
                    applied: function,
                    parameter: self.functions[function].parameter,
                    remap: ctx.remap,
                };
                let nodes: HashSet<NodeId> = scope
                    .iter()
                    .map(|&id| self.node_apply(id, &mut inner))
                    .collect();
                let r#return = self.node_apply(r#return, &mut inner);
                let parameter = self.node_apply(parameter, &mut inner);
                let function = self.functions.insert(Function {
                    nodes,
                    r#return,
                    parameter,
                    block: target,
                });
                self.blocks[target].functions.push(function);
                P::Value::from(LowValue::Function(function))
            }
            // A program-specific value may carry a handle into an arena —
            // relocate it into the target block like any other payload.
            None => Self::copy_ext(self, value, ctx.target),
            _ => value,
        }
    }

    /// Whether `value`'s array tree contains a function value other than
    /// `applied` — a nested closure whose captures must rebind to this
    /// call.  A concreteness proof ([`Node::parameterized_deep`]) cannot see
    /// a function's body, so a proven-concrete structure that contains one
    /// must still be cloned, never referenced in place.
    fn value_contains_foreign_function(
        &self,
        value: Option<P::Value>,
        applied: FunctionId,
    ) -> bool {
        let Some(value) = value else {
            return false;
        };
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return false;
        };
        let mut stack: Vec<NodeId> = array.ids().to_vec();
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            match self.nodes[node].value.and_then(|value| value.as_enum()) {
                Some(LowValue::Function(function)) if function != applied => return true,
                Some(LowValue::Array(array)) => stack.extend(array.ids().iter().copied()),
                _ => {}
            }
        }
        false
    }
}
