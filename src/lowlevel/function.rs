use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::lowlevel::{BlockId, Function, FunctionId, Module, NodeId, Operation, Program, Value};

/// The fixed context of one clone pass: where the clones land, what the
/// parameter maps onto, and the template's membership set plus the running
/// node-id remap.
struct ApplyCtx<'a> {
    target: BlockId,
    param: NodeId,
    argument: NodeId,
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
            param: parameter,
            argument,
            members: &members,
            remap: &mut remap,
        };
        let applied = self.node_apply(r#return, &mut ctx);
        let result = self.evaluate_node(applied, Some(block));
        self.apply_depth -= 1;
        result
    }

    #[stacksafe]
    fn node_apply(&mut self, node: NodeId, ctx: &mut ApplyCtx<'_>) -> NodeId {
        if node == ctx.param {
            return ctx.argument;
        }
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
