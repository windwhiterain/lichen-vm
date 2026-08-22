use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{BlockId, Function, FunctionId, Module, NodeId, Operation, Program, Value};
use lichen_utils::disjoint;

/// The fixed context of one clone pass: where the clones land, the
/// template's membership set, and the running node-id remap (template node
/// to its clone).
struct ApplyCtx<'a> {
    target: BlockId,
    members: &'a HashSet<NodeId>,
    remap: &'a mut HashMap<NodeId, NodeId>,
}

impl<P: Program> Module<P> {
    pub(super) fn function_apply(
        &mut self,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
    ) -> Value<P> {
        self.apply_depth += 1;
        if self.apply_depth > self.apply_depth_limit {
            panic!(
                "recursion depth exceeded in function application (limit {}) — non-terminating function application?",
                self.apply_depth_limit
            );
        }
        let (r#return, parameter) = {
            let function = &self.functions[function];
            (function.r#return, function.parameter)
        };
        debug_assert!(self.functions[function].nodes.contains(&r#return));
        debug_assert!(self.functions[function].nodes.contains(&parameter));
        let members: HashSet<NodeId> = self.functions[function].nodes.iter().copied().collect();
        let mut remap = HashMap::new();
        let mut ctx = ApplyCtx {
            target: block,
            members: &members,
            remap: &mut remap,
        };
        let applied = self.node_apply(r#return, &mut ctx);
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
        result
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
        let (Some(Value::Array(pattern_ids)), Some(Value::Array(argument_ids))) =
            (self.nodes[pattern].value, self.nodes[argument].value)
        else {
            return;
        };
        for (&pattern_id, &argument_id) in
            unsafe { &*pattern_ids }.iter().zip(unsafe { &*argument_ids }.iter())
        {
            self.evaluate_pattern_argument_inner(pattern_id, argument_id, block, seen);
        }
    }

    #[stacksafe]
    fn node_apply(&mut self, node: NodeId, ctx: &mut ApplyCtx<'_>) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&node) {
            return clone;
        }
        if !ctx.members.contains(&node) {
            return node; // outside the template scope — reference as-is
        }
        // The body always exists, so only the parts that depend on the
        // parameter need fresh nodes: unevaluated operation nodes (their
        // operand edges must be rewritten) and nodes not proven concrete —
        // flagged parameterized nodes, plus nodes whose dependence was
        // never resolved (the Index/Apply arms read operands shallowly, so
        // those flags may still be `None`).  A node whose subtree evaluated
        // to a concrete value (`Some(false)`) is baked — reference it in
        // place.
        let (value, operation, parameterized_deep) = {
            let source = &self.nodes[node];
            (source.value, source.operation, source.parameterized_deep)
        };
        let depends_on_parameter =
            (value.is_none() && operation.is_some()) || parameterized_deep != Some(false);
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
        self.nodes[clone].value = value;
        self.nodes[clone].operation = operation;
        clone
    }

    #[stacksafe]
    fn value_apply(&mut self, value: Value<P>, ctx: &mut ApplyCtx<'_>) -> Value<P> {
        match value {
            Value::Array(array) => {
                let nodes: Vec<NodeId> = unsafe { &*array }
                    .iter()
                    .map(|&id| self.node_apply(id, ctx))
                    .collect();
                Value::Array(self.copy_nodes(&nodes, ctx.target))
            }
            Value::Function(function) => {
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
                let nodes: Vec<NodeId> = scope.iter().map(|&id| self.node_apply(id, ctx)).collect();
                let r#return = self.node_apply(r#return, ctx);
                let parameter = self.node_apply(parameter, ctx);
                let function = self.functions.insert(Function {
                    nodes,
                    r#return,
                    parameter,
                    block: ctx.target,
                });
                self.blocks[ctx.target].functions.push(function);
                Value::Function(function)
            }
            Value::Ext(ext) => Self::copy_ext(self, ext, ctx.target),
            value => value,
        }
    }
}
