use stacksafe::stacksafe;

use crate::{BlockId, EvaluatedDeep, LowOperator, LowValue, Module, NodeId, OperatorExt as _, Program};
use lichen_utils::extend::AsEnum;

/// A runtime evaluation failure — the only one today is an out-of-bounds
/// [`LowOperator::Index`].  Structured facts (the index node, the index value,
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
    /// `Self::evaluate_block` is called on it.  `#[stacksafe]`: application
    /// recursion runs through here (and [`Module::function_apply`]) at one
    /// frame per level, so the depth guards must be able to grow the stack —
    /// otherwise a deep recursion overflows the native stack before the
    /// guard panics.
    #[stacksafe]
    pub fn evaluate_node(&mut self, node: NodeId, referer: Option<BlockId>) -> P::Value {
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
        let operator = operation.operator;
        let value = match operator.as_enum() {
            Some(LowOperator::Index) => {
                let Some(operands) = operation.operand else {
                    unreachable!("Index expects an operand array node")
                };
                // A marker anywhere in the operand chain means the index
                // can't be resolved yet — stay lazy so the definition pass
                // can flag the node.
                match self.evaluate_node(operands, Some(block)).as_enum() {
                    Some(LowValue::Parameterized) => P::Value::from(LowValue::Parameterized),
                    Some(LowValue::Array(array)) => {
                        let operands = array.items();
                        match self.evaluate_node(operands[1].node, Some(block)).as_enum() {
                            Some(LowValue::Parameterized) => {
                                P::Value::from(LowValue::Parameterized)
                            }
                            Some(LowValue::USize(index)) => {
                                match self.evaluate_node(operands[0].node, Some(block)).as_enum() {
                                    Some(LowValue::Parameterized) => {
                                        P::Value::from(LowValue::Parameterized)
                                    }
                                    Some(LowValue::Array(array)) => {
                                        let array = array.items();
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
                                            let element = array[index].node;
                                            self.alias_read(node, element);
                                            self.evaluate_node(element, Some(block))
                                        } else {
                                            self.eval_errors.push(EvalError {
                                                index: operands[1].node,
                                                index_value: index,
                                                length: array.len(),
                                            });
                                            P::Value::from(LowValue::None)
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
            None => {
                let operand = match operation.operand {
                    Some(operand) => {
                        let value = self.evaluate_node_deep(operand, Some(block));
                        if self.nodes[operand].evaluated_deep.unwrap().parameterized {
                            P::Value::from(LowValue::Parameterized)
                        } else {
                            value
                        }
                    }
                    None => P::Value::from(LowValue::None),
                };
                operator.run(operand, block, self)
            }
            Some(LowOperator::Apply) => {
                let Some(operands) = operation.operand else {
                    unreachable!("Apply expects an operand array node")
                };
                // A marker target — the body's own parameter during the
                // definition pass — stays lazy instead of panicking.
                match self.evaluate_node(operands, Some(block)).as_enum() {
                    Some(LowValue::Parameterized) => P::Value::from(LowValue::Parameterized),
                    Some(LowValue::Array(array)) => {
                        let operands = array.items();
                        match self.evaluate_node(operands[0].node, Some(block)).as_enum() {
                            Some(LowValue::Parameterized) => {
                                P::Value::from(LowValue::Parameterized)
                            }
                            Some(LowValue::Function(function)) => {
                                // Element 2 is the checker-wired result
                                // cell, when present — the lowlevel tests
                                // build bare 2-element operands.
                                self.function_apply(
                                    function,
                                    operands[1].node,
                                    block,
                                    node,
                                    operands.get(2).map(|item| item.node),
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
        if !matches!(value.as_enum(), Some(LowValue::Parameterized)) {
            self.nodes[node].value = Some(value);
        }
        self.nodes[node].visiting = false;
        value
    }

    /// Run [`Self::evaluate_node`] for all nodes in the reachable subtree of `id`.
    #[stacksafe]
    pub fn evaluate_node_deep(&mut self, node: NodeId, current: Option<BlockId>) -> P::Value {
        self.evaluate_node_deep_inner(node, current, true, false)
    }

    /// Deep-evaluate `id` *ignoring laziness*: unlike
    /// [`Self::evaluate_node_deep`], every array position is descended into
    /// (the shallow mask does not hold a subtree back) and an unevaluated
    /// operation's operand chain is forced before the operation itself runs,
    /// so the whole reachable subtree — values and operand edges — is
    /// evaluated.  The assert check ([`Module::check_asserts`]) uses this: an
    /// asserted condition must be fully evaluated whatever its markers.
    ///
    /// Forcing caches concrete values inside shallow regions but does *not*
    /// upgrade their concreteness proofs: an array with a shallow mark stays
    /// flagged unproven by [`Node::evaluated_deep`] and keeps cloning
    /// per apply, which preserves the deep pass's laziness invariants at the
    /// cost of redundant clones.
    #[stacksafe]
    pub fn evaluate_node_forced(&mut self, node: NodeId, current: Option<BlockId>) -> P::Value {
        self.evaluate_node_deep_inner(node, current, false, true)
    }

    /// Shared core of the deep and forced passes.  `skip_shallow` keeps the
    /// deep pass's laziness (a marked position's subtree is not descended
    /// into); `force_operand` runs an unevaluated operation's operand chain
    /// first, so a forced evaluation resolves the operation against fully
    /// evaluated operands instead of the parameterized gate keeping it lazy.
    #[stacksafe]
    fn evaluate_node_deep_inner(
        &mut self,
        node: NodeId,
        current: Option<BlockId>,
        skip_shallow: bool,
        force_operand: bool,
    ) -> P::Value {
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
        // A forced evaluation forces the operand edge of an unevaluated
        // operation before the operation itself runs.  The operand is a
        // static graph edge, not value-reachable, so the deep pass only
        // propagates flags through it; the forced pass runs the computation
        // behind it — shallow markers included — so a masked operand
        // resolves instead of gating the operation lazy.
        if force_operand
            && self.nodes[node].value.is_none()
            && let Some(operand) = self.nodes[node].operation.and_then(|op| op.operand)
        {
            let block = self.nodes[node].block;
            self.evaluate_node_deep_inner(operand, Some(block), false, true);
        }
        let value = self.evaluate_node(node, current);
        if let Some(LowValue::Array(array)) = value.as_enum() {
            self.nodes[node].visiting = true;
            let block = self.nodes[node].block;
            for item in array.items() {
                // A shallow position is a lazy region: its whole subtree
                // stays unevaluated (never proven concrete), and a read
                // forces the single element on demand through `Index` —
                // unless the forced pass is running, which descends into it
                // like any other position.
                if skip_shallow && item.shallow {
                    continue;
                }
                self.evaluate_node_deep_inner(item.node, Some(block), skip_shallow, force_operand);
            }
            self.nodes[node].visiting = false;
        }
        // An array is unproven while any position resolved to the lazy
        // marker, or any position at all sits behind a shallow mark.
        let parameterized = matches!(value.as_enum(), Some(LowValue::Parameterized))
            || matches!(
                value.as_enum(),
                Some(LowValue::Array(array))
                    // An array holding a shallow position can never be
                    // proven concrete — its marked subtree was deliberately
                    // not evaluated, and even an assert's forced pass that
                    // cached values in it leaves it unproven by this flag,
                    // so it is never referenced in place across applies.
                    if array.items().iter().any(|item| item.shallow)
                        || array.items().iter().any(|item| self.nodes[item.node].evaluated_deep.is_some_and(|e| e.parameterized))
            )
            || self.nodes[node].operation.is_some_and(|op| {
                op.operand.is_some_and(|operand| {
                    // Operands are static graph edges, not value-reachable, so a
                    // nested block release may have dropped the node by now.
                    self.nodes
                        .get(operand)
                        .is_some_and(|node| node.evaluated_deep.is_some_and(|e| e.parameterized))
                })
            });
        self.nodes[node].evaluated_deep = Some(EvaluatedDeep { parameterized });
        self.deep_depth -= 1;
        value
    }

    fn evaluate_block(&mut self, root: NodeId) -> P::Value {
        self.evaluate_node_deep(root, None);
        self.garbage_collect(root).expect("evaluated return node")
    }
}
