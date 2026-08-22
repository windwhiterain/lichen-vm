use stacksafe::stacksafe;

use crate::lowlevel::{BlockId, Module, NodeId, Operator, OperatorExt as _, Program, Value};

impl<P: Program> Module<P> {
    /// If `id` lives in a child of `referer`, its a block root, [`Self::evaluate_block`] will be called on it.
    pub fn evaluate_node(&mut self, node: NodeId, referer: Option<BlockId>) -> Value<P> {
        let block = self.nodes[node].block;
        debug_assert!(
            self.blocks.contains_key(block),
            "node {node:?} references released block {block:?}"
        );
        if let Some(referer) = referer
            && self.blocks[block].parent == Some(referer)
        {
            return self.evaluate_block(node);
        }
        if let Some(value) = self.nodes[node].value {
            return value;
        }
        if self.nodes[node].visiting {
            unreachable!("cycle detected: node {node:?} is being evaluated");
        }
        self.nodes[node].visiting = true;
        let operation = self.nodes[node].operation.unwrap();
        let value = match operation.operator {
            Operator::Index => {
                let Some(operands) = operation.operand else {
                    unreachable!("Index expects an operand array node")
                };
                // A marker anywhere in the operand chain means the index
                // can't be resolved yet — stay lazy so the definition pass
                // can flag the node.
                match self.evaluate_node(operands, Some(block)) {
                    Value::Parameterized => Value::Parameterized,
                    Value::Array(ptr) => {
                        let operands = unsafe { &*ptr };
                        match self.evaluate_node(operands[1], Some(block)) {
                            Value::Parameterized => Value::Parameterized,
                            Value::USize(index) => {
                                match self.evaluate_node(operands[0], Some(block)) {
                                    Value::Parameterized => Value::Parameterized,
                                    Value::Array(ptr) => {
                                        let array = unsafe { &*ptr };
                                        self.evaluate_node(array[index], Some(block))
                                    }
                                    _ => unreachable!("Index target must be an array"),
                                }
                            }
                            _ => unreachable!("Index needs a USize index node"),
                        }
                    }
                    _ => unreachable!("Index operand must be an array of [array, index]"),
                }
            }
            Operator::Ext(ext) => {
                let operand = match operation.operand {
                    Some(operand) => {
                        let value = self.evaluate_node_deep(operand, Some(block));
                        if self.nodes[operand].parameterized_deep.unwrap() {
                            Value::Parameterized
                        } else {
                            value
                        }
                    }
                    None => Value::None,
                };
                let Module { nodes, blocks, .. } = self;
                ext.run(operand, &mut blocks[block], nodes)
            }
            Operator::Apply => {
                let Some(operands) = operation.operand else {
                    unreachable!("Apply expects an operand array node")
                };
                // A marker target — the body's own parameter during the
                // definition pass — stays lazy instead of panicking.
                match self.evaluate_node(operands, Some(block)) {
                    Value::Parameterized => Value::Parameterized,
                    Value::Array(ptr) => {
                        let operands = unsafe { &*ptr };
                        match self.evaluate_node(operands[0], Some(block)) {
                            Value::Parameterized => Value::Parameterized,
                            Value::Function(function) => {
                                self.function_apply(function, operands[1], block)
                            }
                            _ => unreachable!("Apply target must be a function value"),
                        }
                    }
                    _ => unreachable!("Apply operand must be an array of [function, argument]"),
                }
            }
        };
        self.nodes[node].value = Some(value);
        self.nodes[node].visiting = false;
        value
    }

    /// Run [`Self::evaluate_node`] for all nodes in the reachable subtree of `id`.
    #[stacksafe]
    pub fn evaluate_node_deep(&mut self, node: NodeId, current: Option<BlockId>) -> Value<P> {
        // A structural cycle (e.g. the `Type : Type` universe `[Type, ↺]`,
        // which every type spine in the recursive-pair encoding reaches) is
        // cut here: the node is being deep-evaluated by an outer frame and
        // already holds its cached value, so re-entering it would only loop.
        // A node marked visiting with no cached value is an *operation* cycle
        // mid-computation — it falls through so [`Self::evaluate_node`]'s
        // own guard panics, as before.
        if self.nodes[node].visiting
            && let Some(value) = self.nodes[node].value
        {
            return value;
        }
        self.deep_depth += 1;
        if self.deep_depth > self.evaluate_depth_limit {
            panic!(
                "recursion depth exceeded in deep evaluation (limit {}) — non-terminating evaluation?",
                self.evaluate_depth_limit
            );
        }
        let value = self.evaluate_node(node, current);
        if let Value::Array(array) = value {
            self.nodes[node].visiting = true;
            let block = self.nodes[node].block;
            for &id in unsafe { &*array } {
                self.evaluate_node_deep(id, Some(block));
            }
            self.nodes[node].visiting = false;
        }
        let parameterized = matches!(value, Value::Parameterized)
            || matches!(
                value,
                Value::Array(array)
                    if unsafe { &*array }.iter().any(|&id| self.nodes[id].parameterized_deep == Some(true))
            )
            || self.nodes[node].operation.is_some_and(|op| {
                op.operand.is_some_and(|operand| {
                    // Operands are static graph edges, not value-reachable, so a
                    // nested block release may have dropped the node by now.
                    self.nodes
                        .get(operand)
                        .is_some_and(|node| node.parameterized_deep == Some(true))
                })
            });
        self.nodes[node].parameterized_deep = Some(parameterized);
        self.deep_depth -= 1;
        value
    }

    fn evaluate_block(&mut self, root: NodeId) -> Value<P> {
        self.evaluate_node_deep(root, None);
        self.garbage_collect(root).expect("evaluated return node")
    }
}
