use stacksafe::stacksafe;

use crate::{BlockId, Module, NodeId, Operator, OperatorExt as _, Program, Value};

/// A runtime evaluation failure — the only one today is an out-of-bounds
/// [`Operator::Index`].  Structured facts (the index node, the index value,
/// the container length) so the highlevel layer can attribute a span and
/// render a message without walking the module graph.
#[derive(Debug, Clone, Copy)]
pub struct EvalError {
    /// The index operand node — its source span attributes the diagnostic.
    pub index: NodeId,
    pub index_value: usize,
    pub length: usize,
}

impl<P: Program> Module<P> {
    /// If `id` lives in a child of `referer`, it is a block root, and
    /// `Self::evaluate_block` is called on it.
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
                                        // An out-of-bounds index is a user error,
                                        // not an invariant violation: record it
                                        // and yield no value instead of panicking
                                        // in raw slice indexing.
                                        if index < array.len() {
                                            // A read of a pure cell is a
                                            // reference, not a snapshot:
                                            // joining the reader to the
                                            // cell's class lets a later bind
                                            // reach it through replication,
                                            // independent of evaluation
                                            // order.  A non-cell element
                                            // (a concrete value, another
                                            // computation) reads as before.
                                            self.alias_read(node, array[index]);
                                            self.evaluate_node(array[index], Some(block))
                                        } else {
                                            self.eval_errors.push(EvalError {
                                                index: operands[1],
                                                index_value: index,
                                                length: array.len(),
                                            });
                                            Value::None
                                        }
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
                ext.run(operand, block, self)
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
                                // Element 2 is the checker-wired result
                                // cell, when present — the lowlevel tests
                                // build bare 2-element operands.
                                self.function_apply(
                                    function,
                                    operands[1],
                                    block,
                                    node,
                                    operands.get(2).copied(),
                                )
                            }
                            _ => unreachable!("Apply target must be a function value"),
                        }
                    }
                    _ => unreachable!("Apply operand must be an array of [function, argument]"),
                }
            }
        };
        // A transient marker is not a final answer: an operation whose
        // operands were unbound at evaluation time re-runs on the next read,
        // so a later binding is observed regardless of evaluation order
        // (concrete results are memoized as usual).  Cells never reach this
        // postlude — they return their cached marker from the top.
        if !matches!(value, Value::Parameterized) {
            self.nodes[node].value = Some(value);
        }
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
