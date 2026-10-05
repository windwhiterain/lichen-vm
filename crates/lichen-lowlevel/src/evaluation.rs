use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, BlockId, BudgetExhausted, EvaluatedDeep,
    LowOperator, LowValue, Module, NodeId, OperatorExt, Program, StaticModuleCache,
    ancestors::AncestorPairs, table::KeyState,
};
use lichen_utils::extend::AsEnum;

/// The evaluation-attempt mark of one node — see [`Module::retain_node`] for
/// the invariant it upholds.  Retained on construction, released on drop,
/// including on unwind.  It holds the whole module, so the attempt's body is
/// reached *through* it ([`Self::module`]) rather than through the borrow it
/// was built from.
struct VisitGuard<'a, P: Program> {
    module: &'a mut Module<P>,
    node: NodeId,
}

impl<P: Program> VisitGuard<'_, P> {
    /// Run `body` with the mark held: an evaluation attempt of the marked
    /// node, or one descent of the deep pass that is cut against it.
    fn run<R>(self, body: impl FnOnce(&mut Module<P>, NodeId) -> R) -> R {
        let node = self.node;
        body(self.module, node)
    }
}

impl<P: Program> Drop for VisitGuard<'_, P> {
    fn drop(&mut self) {
        self.module.nodes[self.node].visiting = false;
    }
}

/// A runtime evaluation failure — an out-of-bounds [`LowOperator::Index`], a
/// table read that misses or whose key is not concrete.  Structured facts
/// (the offending nodes and values) so the highlevel layer can attribute a
/// span and render a message without walking the module graph.  The nodes
/// are [`AnyNodeId`]s: a static one (a solved constant of a plugged
/// dependency) has no importer span.
#[derive(Debug, Clone, Copy)]
pub enum EvalError {
    /// An out-of-bounds array read.  `index` is the index operand node —
    /// its source span attributes the diagnostic.
    Index {
        index: AnyNodeId,
        index_value: usize,
        length: usize,
    },
    /// A [`LowOperator::TableGet`] that found no entry for the key, or
    /// whose key is still unbound (a not-yet-concrete key can match nothing
    /// — the table's stored keys are all concrete), or whose target or key
    /// is a computed nothing (a [`LowValue::Void`] never matches: it is the
    /// residue of an already-recorded failure, not a key).  `table` is the
    /// container operand node, `key` the key node.
    TableMiss { table: AnyNodeId, key: AnyNodeId },
    /// A table build dropped an entry whose key could not be forced to a
    /// concrete value (its subtree holds an unbound cell or a parameterized
    /// computation) — hashing needs the key's decided content.
    TableKeyUnbound { key: AnyNodeId },
    /// A [`LowOperator::Index`] whose target is not an array at all — a read
    /// of a scalar, a function, a table, or a type-level node.  Reachable from
    /// source (a field read applied to something that is not a container), so
    /// it is a recorded failure and a computed nothing, never an internal
    /// error: `target` is the container operand node, so the highlevel can
    /// attribute the diagnostic to the expression that was indexed.
    IndexTarget { target: AnyNodeId },
    /// A [`LowOperator::Index`] whose **subscript** is not an index at all —
    /// a read through a string, a tuple, a function, a table.  Reachable from
    /// source (`a[i]` with `i : string`, and the same through a parameter), so
    /// it is a recorded failure and a computed nothing: `subscript` is the
    /// index operand node, so the highlevel can attribute the diagnostic to
    /// the expression that was used as a subscript.
    IndexSubscript { subscript: AnyNodeId },
    /// A [`LowOperator::Apply`] whose **target** is a structural value that
    /// cannot be applied — a scalar, a string, a table, or the unit value.
    /// Reachable from source (`g 1` with `g : int`), including through a
    /// deferred callee whose unbound type the checker's function-ness guard
    /// cannot see, so it is a recorded failure and a computed nothing:
    /// `function` is the callee operand node, so the highlevel can attribute
    /// the diagnostic to the expression that was applied.
    ApplyTarget { function: AnyNodeId },
}

impl<P: Program> Module<P> {
    /// If `id` lives in a child of `referer`, it is a block root, and
    /// `Self::evaluate_block` is called on it.  `#[stacksafe]`: application
    /// recursion runs through here (and [`Module::function_apply`]) at one
    /// frame per level, so the depth guards must be able to grow the stack —
    /// otherwise a deep recursion overflows the native stack before the
    /// guard panics.
    #[stacksafe]
    pub fn evaluate_node(&mut self, node: AnyNodeId, referer: Option<BlockId>) -> P::Value {
        // A static ref is a decided leaf: read its solved value (absolute
        // refs, shared arena) and return.  Nothing is evaluated, cached
        // into importer nodes, or marked — the static module already solved
        // it.
        match node {
            Dyn(node) => self.evaluate_node_body(node, referer),
            AnyNodeId::Static(sref) => self.static_read(sref),
        }
    }

    /// Take the mark of the node an evaluation attempt — or a deep-pass
    /// descent that must be cut against it — is computing, held until the
    /// returned guard drops.
    ///
    /// Invariant: a frame owns `Node::visiting` for exactly its own scope, and
    /// the guard clears it on every exit — a cached answer, a lazy
    /// (`Parameterized`) answer, and an unwinding panic alike.  A node is
    /// therefore never left flagged visiting once no frame is computing it,
    /// because the next evaluation of that node must not read the stale flag
    /// as a cycle: a node the postlude deliberately declined to cache (a
    /// `Parameterized` answer) is evaluated again by a later pass, and before
    /// this invariant that second attempt saw `visiting == true` and panicked.
    /// [`Drop`] is what makes the invariant hold on the unwind path, so a
    /// future internal panic inside an attempt costs one node instead of
    /// poisoning the module for the rest of the build.
    fn retain_node(&mut self, node: NodeId) -> VisitGuard<'_, P> {
        self.nodes[node].visiting = true;
        VisitGuard { module: self, node }
    }

    /// The body of [`Self::evaluate_node`] — everything after the static-ref
    /// leaf rule, which [`Self::evaluate_node`] handles before the mark is
    /// taken: a static node has no visit mark to own.
    #[stacksafe]
    fn evaluate_node_body(&mut self, node: NodeId, referer: Option<BlockId>) -> P::Value {
        let block = self.nodes[node].block;
        debug_assert!(
            self.blocks.contains_key(block),
            "node {node:?} references released block {block:?}"
        );
        // A child-block root delegates whole: [`Self::evaluate_block`] runs
        // the deep pass, which takes its own marks.  Returning before this
        // frame takes one keeps ownership flat — one attempt, one mark.
        if let Some(referer) = referer
            && self.blocks[block].parent == Some(referer)
        {
            return self.evaluate_block(node);
        }
        if let Some(value) = self.nodes[node].value {
            // An operator that has not run must run: a value in the slot that
            // *this node's* operator did not produce ([`Module::has_no_result_yet`])
            // was written by a unification — an assertion, not a computation —
            // so the operator still owes its own answer.
            if self.nodes[node].operation.is_some()
                && !self.nodes[node].runned
                && !self.nodes[node].visiting
            {
                let guard = self.retain_node(node);
                guard.run(|module, node| module.evaluate_node_operation(node));
                return self.nodes[node].value.unwrap_or(value);
            }
            return value;
        }
        // A node flagged visiting with no cached value is being computed by an
        // inner frame, so this read is genuinely cyclic.  The flag is cleared
        // on every exit from the attempt that set it (see the invariant on
        // [`Self::retain_node`]), so it always means an active frame rather
        // than a leaked one.
        if self.nodes[node].visiting {
            unreachable!("cycle detected: node {node:?} is being evaluated");
        }
        let guard = self.retain_node(node);
        guard.run(|module, node| module.evaluate_node_operation(node))
    }

    /// The operation dispatch of [`Self::evaluate_node`]: compute this node's
    /// operation, then apply the postlude that decides whether the answer is
    /// cached.  Runs while the caller holds the node's [`VisitGuard`], so an
    /// early return here cannot leak the mark.
    #[stacksafe]
    fn evaluate_node_operation(&mut self, node: NodeId) -> P::Value {
        let block = self.nodes[node].block;
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
                match self.evaluate_node(Dyn(operands), Some(block)).as_enum() {
                    Some(LowValue::Parameterized) => P::Value::from(LowValue::Parameterized),
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is the value the module just
                        // evaluated for the operand node; its home block is
                        // alive and not dropped.
                        let operands = unsafe { array.items() };
                        match self.evaluate_node(operands[1].node, Some(block)).as_enum() {
                            Some(LowValue::Parameterized) => {
                                P::Value::from(LowValue::Parameterized)
                            }
                            // A computed-nothing index propagates: the
                            // read's own failure was recorded where the
                            // `Void` was produced.
                            Some(LowValue::Void) => P::Value::from(LowValue::Void),
                            Some(LowValue::USize(index)) => {
                                match self.evaluate_node(operands[0].node, Some(block)).as_enum() {
                                    Some(LowValue::Parameterized) => {
                                        P::Value::from(LowValue::Parameterized)
                                    }
                                    // A computed-nothing target propagates
                                    // the same way — no second diagnostic.
                                    Some(LowValue::Void) => P::Value::from(LowValue::Void),
                                    Some(LowValue::Array(array)) => {
                                        // SAFETY: `array` is the value the
                                        // module just evaluated for the target
                                        // node; its home block is alive.
                                        let array = unsafe { array.items() };
                                        // An out-of-bounds index is a user error,
                                        // not an invariant violation: record it
                                        // and yield a computed nothing (`Void`)
                                        // instead of panicking in raw slice
                                        // indexing.
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
                                            match element {
                                                Dyn(element) => {
                                                    self.alias_read(node, element);
                                                    // A read whose element is
                                                    // its own class — the
                                                    // literal self-read of a
                                                    // lazy type slot, or a
                                                    // class `alias_read` just
                                                    // joined — never re-enters
                                                    // the evaluation: the
                                                    // class's committed value
                                                    // answers it, and an
                                                    // undecided class stays
                                                    // lazy (the read resolves
                                                    // through replication when
                                                    // the class binds).
                                                    if self.equality_representative(element)
                                                        == self.equality_representative(node)
                                                    {
                                                        let rep =
                                                            self.equality_representative(node);
                                                        self.class_committed_value(rep)
                                                            .unwrap_or_else(|| {
                                                                P::Value::from(
                                                                    LowValue::Parameterized,
                                                                )
                                                            })
                                                    } else {
                                                        self.evaluate_node(
                                                            Dyn(element),
                                                            Some(block),
                                                        )
                                                    }
                                                }
                                                // A static element is
                                                // immutable — no class to
                                                // join — and its value is
                                                // absolute, so the read
                                                // result caches into
                                                // this node like any other.
                                                AnyNodeId::Static(sref) => self.static_read(sref),
                                            }
                                        } else {
                                            self.eval_errors.push(EvalError::Index {
                                                index: operands[1].node,
                                                index_value: index,
                                                length: array.len(),
                                            });
                                            P::Value::from(LowValue::Void)
                                        }
                                    }
                                    // The read's operands are a *pair*: every
                                    // failure mode of reading is a user error
                                    // (a field read applied to something that is
                                    // not a container, of an element that does
                                    // not exist, or through a subscript that is
                                    // not an index), never an invariant
                                    // violation — record it and yield a computed
                                    // nothing.  A late binding still reaches this
                                    // position through the `Void`/`Parameterized`
                                    // arms, so nothing that could resolve is lost.
                                    _ => {
                                        self.eval_errors.push(EvalError::IndexTarget {
                                            target: operands[0].node,
                                        });
                                        P::Value::from(LowValue::Void)
                                    }
                                }
                            }
                            // A subscript that is concretely not an index —
                            // a string, a tuple, a function — is the same
                            // class of user error as a non-container target
                            // (neither is expressible in the type encoding,
                            // so the checker cannot reject either one
                            // statically): recorded, with the subscript node
                            // carrying the fact, and a computed nothing.
                            _ => {
                                self.eval_errors.push(EvalError::IndexSubscript {
                                    subscript: operands[1].node,
                                });
                                P::Value::from(LowValue::Void)
                            }
                        }
                    }
                    _ => unreachable!("Index operand must be an array of [array, index]"),
                }
            }
            None => operator.run_deferred(operation.operand, block, self),
            Some(LowOperator::Apply) => {
                let Some(operands) = operation.operand else {
                    unreachable!("Apply expects an operand array node")
                };
                // A marker target — the body's own parameter during the
                // definition pass — stays lazy instead of panicking.
                match self.evaluate_node(Dyn(operands), Some(block)).as_enum() {
                    Some(LowValue::Parameterized) => P::Value::from(LowValue::Parameterized),
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is the value the module just
                        // evaluated for the Apply operand node; its home block
                        // is alive and not dropped.
                        let operands = unsafe { array.items() };
                        match self.evaluate_node(operands[0].node, Some(block)).as_enum() {
                            Some(LowValue::Parameterized) => {
                                P::Value::from(LowValue::Parameterized)
                            }
                            Some(LowValue::Function(function)) => {
                                // Element 2 is the checker-wired result
                                // cell, when present — the lowlevel tests
                                // build bare 2-element operands.  A static
                                // argument or cell ref is materialized into
                                // a leaf node first: the apply tail (unify,
                                // pattern walk) operates on dynamic nodes.
                                let argument = self.as_dynamic(operands[1].node, block);
                                let cell = operands
                                    .get(2)
                                    .map(|item| self.as_dynamic(item.node, block));
                                match function {
                                    AnyFunctionId::Dynamic(function) => {
                                        self.function_apply(function, argument, block, node, cell)
                                    }
                                    AnyFunctionId::Static(sref) => self
                                        .static_function_apply(sref, argument, block, node, cell),
                                }
                            }
                            // A scalar, a string, a table, or the unit value
                            // can never be a function — and never a callable
                            // program value — so applying one is a user error.
                            // The checker's function-ness guard cannot see a
                            // deferred callee's unbound type, so this is where
                            // the apply is refused: recorded, with the callee
                            // operand node carrying the fact, and a computed
                            // nothing.
                            Some(
                                LowValue::USize(_)
                                | LowValue::Float(_)
                                | LowValue::Str(_)
                                | LowValue::Table(_)
                                | LowValue::None,
                            ) => {
                                self.eval_errors.push(EvalError::ApplyTarget {
                                    function: operands[0].node,
                                });
                                P::Value::from(LowValue::Void)
                            }
                            // A computed nothing is the residue of an
                            // already-recorded failure: propagate it without
                            // recording a second one.
                            Some(LowValue::Void) => P::Value::from(LowValue::Void),
                            // A structural array and the program's own value
                            // both reach here, and neither is provably a
                            // function: the program's value variant is opaque
                            // to the lowlevel, and a compute `Kernel` is a
                            // struct — structurally an array — that the
                            // compute layer compiles into a cross-kernel call.
                            // The program answers through its operator
                            // dispatch (`OperatorExt::is_callable`), whose
                            // default refuses; a callee it disclaims is a user
                            // error recorded exactly as a scalar is, and a
                            // callee it claims stays lazy so a body's deep pass
                            // and the JIT keep the graph readable.
                            _ => {
                                if <P::Operator as OperatorExt<P>>::is_callable(
                                    self,
                                    operands[0].node,
                                ) {
                                    P::Value::from(LowValue::Parameterized)
                                } else {
                                    self.eval_errors.push(EvalError::ApplyTarget {
                                        function: operands[0].node,
                                    });
                                    P::Value::from(LowValue::Void)
                                }
                            }
                        }
                    }
                    _ => unreachable!("Apply operand must be an array of [function, argument]"),
                }
            }
            Some(LowOperator::TableGet) => {
                let Some(operands) = operation.operand else {
                    unreachable!("TableGet expects an operand array node")
                };
                match self.evaluate_node(Dyn(operands), Some(block)).as_enum() {
                    Some(LowValue::Parameterized) => P::Value::from(LowValue::Parameterized),
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is the value the module just
                        // evaluated for the TableGet operand node; its home
                        // block is alive and not dropped.
                        let operands = unsafe { array.items() };
                        let table = operands[0].node;
                        let key = operands[1].node;
                        match self.evaluate_node(table, Some(block)).as_enum() {
                            Some(LowValue::Parameterized) => {
                                P::Value::from(LowValue::Parameterized)
                            }
                            Some(LowValue::Table(payload)) => {
                                // The key is force-evaluated and
                                // deep-content-hashed; a key that is
                                // decided-and-absent misses like any other
                                // absent key.  A key that is not *decided
                                // yet* (a lambda parameter mid-apply, a lazy
                                // computation with unbound operands) is not a
                                // miss: the lookup has not happened yet, so
                                // the read stays lazy and a later pass, with
                                // the key bound, decides it.
                                match self.key_state(key) {
                                    KeyState::Undecided => {
                                        return P::Value::from(LowValue::Parameterized);
                                    }
                                    KeyState::Unhashable => {
                                        self.eval_errors.push(EvalError::TableMiss { table, key });
                                        return P::Value::from(LowValue::Void);
                                    }
                                    KeyState::Hashed(hash) => {
                                        // SAFETY: `payload` is the evaluated
                                        // table value of a live node of this
                                        // module; its home block is alive.
                                        let items = unsafe { payload.items() };
                                        let start = items.partition_point(|item| item.hash < hash);
                                        let mut path = AncestorPairs::new();
                                        let mut found = None;
                                        for item in &items[start..] {
                                            if item.hash != hash {
                                                break;
                                            }
                                            if self.key_eq(key, item.key, &mut path) {
                                                found = Some(item.value);
                                                break;
                                            }
                                        }
                                        self.finish_table_get(node, found, block)
                                    }
                                }
                            }
                            // A computed-nothing target (e.g. the anonymous
                            // struct's "no name table" marker behind a lazy
                            // named read) is a miss like any other: recorded,
                            // never a panic.
                            Some(LowValue::Void) => {
                                self.eval_errors.push(EvalError::TableMiss { table, key });
                                P::Value::from(LowValue::Void)
                            }
                            _ => unreachable!("TableGet target must be a table"),
                        }
                    }
                    _ => unreachable!("TableGet operand must be an array of [table, key]"),
                }
            }
        };
        // A transient marker is not a final answer: an operation whose
        // operands were unbound at evaluation time re-runs on the next read,
        // so a later binding is observed regardless of evaluation order
        // (concrete results are memoized as usual).  Cells never reach this
        // postlude — they return their cached marker from the top.
        //
        // Note what is *not* claimed here: an operation node **may** compute
        // this marker (a probe measured `Eq` over an unbound operand doing
        // exactly that).  What is declined is caching it — so an operation
        // node's slot holds a *decided* answer or nothing, which is what makes
        // `Module::has_no_result_yet`'s "has an operation and no cached value"
        // the same question as "has not produced an answer yet".
        if matches!(value.as_enum(), Some(LowValue::Parameterized)) {
            self.nodes[node].runned = true;
            return value;
        }
        // The computation produced an answer: it is committed through the one
        // value-write path, so the class's value reaches the representative
        // rather than sitting on this member beside it, and it is reconciled
        // with what the class already held — a class may hold a value a
        // unification put there while this node was undecided (the unifier
        // writes, it does not compute), and a disagreement is exactly the
        // conflict the unify deferred to here.
        self.write_node_answer(node, value);
        value
    }

    /// Run [`Self::evaluate_node`] for all nodes in the reachable subtree of `id`.
    #[stacksafe]
    pub fn evaluate_node_deep(&mut self, node: NodeId, current: Option<BlockId>) -> P::Value {
        let mut cache = StaticModuleCache::new();
        self.evaluate_node_deep_inner(Dyn(node), current, true, false, &mut cache)
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
        let mut cache = StaticModuleCache::new();
        self.evaluate_node_deep_inner(Dyn(node), current, false, true, &mut cache)
    }

    /// The tail of a [`LowOperator::TableGet`] once the entry is located: a
    /// found element is read by reference (joining the reader to the cell's
    /// class lets a later bind reach it through replication, like an array
    /// `Index` read), and an absent one is a miss — recorded, never a panic.
    fn finish_table_get(
        &mut self,
        node: NodeId,
        found: Option<AnyNodeId>,
        block: BlockId,
    ) -> P::Value {
        match found {
            Some(Dyn(element)) => {
                self.alias_read(node, element);
                self.evaluate_node(Dyn(element), Some(block))
            }
            // A static element is immutable — no class to join — and its value
            // is absolute, so the read result caches into this node like any
            // other.
            Some(AnyNodeId::Static(sref)) => self.static_read(sref),
            None => {
                let (table, key) = self.table_get_operands(node);
                self.eval_errors.push(EvalError::TableMiss { table, key });
                P::Value::from(LowValue::Void)
            }
        }
    }

    /// The `[table, key]` operand nodes of a `TableGet`, for attributing a
    /// miss.  The read has already proved the operand is a 2-element array, so
    /// the shape holds here.
    fn table_get_operands(&self, node: NodeId) -> (AnyNodeId, AnyNodeId) {
        let operand = self.nodes[node]
            .operation
            .and_then(|op| op.operand)
            .expect("a TableGet node reached the miss path carries its operand");
        match self
            .node_value(Dyn(operand))
            .and_then(|value| value.as_enum())
        {
            Some(LowValue::Array(array)) => {
                // SAFETY: `array` is the payload of `node`'s operand, a live
                // node of this module, so its home block has not been dropped.
                let items = unsafe { array.items() };
                (items[0].node, items[1].node)
            }
            _ => unreachable!("a TableGet operand is the [table, key] array"),
        }
    }

    /// Shared core of the deep and forced passes.  `skip_shallow` keeps the
    /// deep pass's laziness (a marked position's subtree is not descended
    /// into); `force_operand` runs an unevaluated operation's operand chain
    /// first, so a forced evaluation resolves the operation against fully
    /// evaluated operands instead of the parameterized gate keeping it lazy.
    /// `cache` is the walk's one-entry static-module resolution cache (see
    /// [`StaticModuleCache`]) — one lookup per module per walk, not per ref.
    #[stacksafe]
    fn evaluate_node_deep_inner(
        &mut self,
        node: AnyNodeId,
        current: Option<BlockId>,
        skip_shallow: bool,
        force_operand: bool,
        cache: &mut StaticModuleCache<P>,
    ) -> P::Value {
        // A static ref is a decided leaf: the module solved it, so there is
        // nothing to evaluate, descend, or mark — read its value.
        // Even a forced pass gains nothing from a solved subtree (residuals
        // never re-run), so the leaf rule is unconditional.
        if let AnyNodeId::Static(sref) = node {
            return cache.read(self, sref);
        }
        let node = match node {
            Dyn(node) => node,
            AnyNodeId::Static(_) => unreachable!(),
        };
        // A structural cycle (e.g. the `Type : Type` universe `[Type, ↺]`,
        // which every type spine in the recursive-pair encoding reaches) is
        // cut here: the node is being deep-evaluated by an outer frame and
        // already holds its cached value, so re-entering it would only loop.
        // A node marked visiting with no cached value is an *operation* cycle
        // mid-computation — it falls through so [`Self::evaluate_node`]'s
        // own guard panics, as before.  The flag's lifetime is exactly one
        // evaluation attempt (see the invariant on `retain_node`), so a
        // visiting node with no value is always one an outer frame is
        // computing right now.
        if self.nodes[node].visiting
            && let Some(value) = self.nodes[node].value
        {
            // The cut **assumes this node concrete** for the readers that reach
            // it while its own frame is still computing it — the coinductive
            // step a cyclic value needs.  It is a fact about the node rather
            // than about the reader, so a one-level self-reference (the
            // universe's `[Type, ↺]`) and a longer cycle read alike, and it is
            // cleared where the real verdict is written below.
            self.nodes[node].assumed_concrete = true;
            return value;
        }
        self.deep_depth += 1;
        if self.deep_depth > self.evaluate_depth_limit {
            if self.budget_exhausted.is_none() {
                self.budget_exhausted = Some(BudgetExhausted::EvaluateDepth {
                    limit: self.evaluate_depth_limit,
                });
            }
            // Nothing was computed, and nothing here can ever compute it:
            // return the computed-nothing value — the same shape the VM gives
            // a read it declined to perform.  The undecided marker would
            // promise "try again later", which nothing downstream can honour:
            // this frame already owns the budget verdict, and a later read
            // reaches the same refusal.  Unlike the apply frame's refusal
            // this value is never cached onto a node (this returns before
            // `evaluate_node`, whose postlude does the writing), so it cannot
            // be mistaken for a decided proven answer — the node's
            // `evaluated_deep` stays `None`, "never ran".  The counter is
            // decremented on the way out because `deep_depth` is a *nesting*
            // counter, not a cumulative budget: it is incremented at entry and
            // restored on every exit.  Left inflated, the refusal would be
            // permanent — nothing else restores the counter except
            // `reset_apply_budget`, so every later `evaluate_node_deep` in the
            // process would start already past the limit and refuse too.
            // Restoring it scopes the refusal to the subtree that is too deep:
            // shallow siblings still walk and are decided, and only the nodes
            // past the limit yield `Void`.
            self.deep_depth -= 1;
            return P::Value::from(LowValue::Void);
        }
        // A forced evaluation forces the operand edge of an unevaluated
        // operation before the operation itself runs.  The operand is a
        // static graph edge, not value-reachable, so the deep pass only
        // propagates flags through it; the forced pass runs the computation
        // behind it — shallow markers included — so a masked operand
        // resolves instead of gating the operation lazy.
        //
        // An **unproven** node needs that walk even when it already holds a
        // value: its concreteness is derived partly from its operand's verdict
        // ([`Self::value_is_parameterized`]), so an operation whose operand
        // chain was walked while it was still waiting keeps reading as
        // unproven after the chain resolves — while the value it computed is
        // decided.  A reader that takes the verdict for "this is not decided
        // yet" (`Module::key_state`, which gates a `TableGet` on it) would then
        // never see that value.  Re-forcing the operand is what makes the
        // verdict fresh; it is the same walk an unevaluated operation gets.
        // Read only under `force_operand`, so the deep pass pays nothing.
        let unproven = force_operand
            && self.nodes[node]
                .evaluated_deep
                .is_some_and(|deep| deep.parameterized);
        if force_operand
            && (self.nodes[node].value.is_none() || unproven)
            && let Some(operand) = self.nodes[node].operation.and_then(|op| op.operand)
        {
            let block = self.nodes[node].block;
            self.evaluate_node_deep_inner(Dyn(operand), Some(block), false, true, cache);
        }
        let value = self.evaluate_node(Dyn(node), current);
        if let Some(LowValue::Array(array)) = value.as_enum() {
            // The descent below may reach `node` again through the array's own
            // items (a self-referential value), so it is marked for the
            // duration: the same structural-cycle cut as the entry above.
            let guard = self.retain_node(node);
            guard.run(|module, node| {
                let block = module.nodes[node].block;
                // SAFETY: `array` is the value this module just evaluated for
                // `node`.  The descent below mutates the module but never releases
                // a block — `drop_block` is called only from `garbage_collect` —
                // so the payload's arena stays alive for the whole loop.
                for item in unsafe { array.items() } {
                    // A shallow position is a lazy region: its whole subtree
                    // stays unevaluated (never proven concrete), and a read
                    // forces the single element on demand through `Index` —
                    // unless the forced pass is running, which descends into it
                    // like any other position.
                    if skip_shallow && item.shallow {
                        continue;
                    }
                    module.evaluate_node_deep_inner(
                        item.node,
                        Some(block),
                        skip_shallow,
                        force_operand,
                        cache,
                    );
                }
            });
        }
        // A table's entries are edges like array items: its keys were
        // already forced concrete at build, but its values are lazy refs —
        // both must be proven (or disproven) concrete by the descent.
        if let Some(LowValue::Table(table)) = value.as_enum() {
            let guard = self.retain_node(node);
            guard.run(|module, node| {
                let block = module.nodes[node].block;
                // SAFETY: `table` is the value this module just evaluated for
                // `node`.  The descent below mutates the module but never releases
                // a block — `drop_block` is called only from `garbage_collect` —
                // so the payload's arena stays alive for the whole loop.
                for item in unsafe { table.items() } {
                    module.evaluate_node_deep_inner(
                        item.key,
                        Some(block),
                        skip_shallow,
                        force_operand,
                        cache,
                    );
                    module.evaluate_node_deep_inner(
                        item.value,
                        Some(block),
                        skip_shallow,
                        force_operand,
                        cache,
                    );
                }
            });
        }
        // An array is unproven while any position resolved to the lazy
        // marker, or any position at all sits behind a shallow mark.  A
        // static position's concreteness is the module's solved flag — it
        // was already decided by the deep pass that solved the module.
        let parameterized = self.value_is_parameterized(cache, value, node);
        self.nodes[node].evaluated_deep = Some(EvaluatedDeep { parameterized });
        // The real verdict supersedes any cycle-cut assumption: the node is no
        // longer in progress, so the mark must not outlive the frame.
        self.nodes[node].assumed_concrete = false;
        self.deep_depth -= 1;
        value
    }

    /// The concreteness of one **ref** inside the verdict computation: the
    /// three-state read of [`Module::node_evaluated_deep`].
    ///
    /// A verdict is written only when the pass *finishes* a node, so a node a
    /// cycle cut re-entered while its own frame is still computing it has no
    /// verdict yet at the moment an enclosing frame decides its parent's.  Such
    /// a node is assumed **concrete** — that coinductive step is what lets a
    /// cyclic value be proven at all, and the canonical universe `[Type, ↺]` is
    /// the case that needs it (its own descent re-enters it, and the checker's
    /// note records that cloning the universe per apply is a unification
    /// conflict, not a slowdown).
    ///
    /// A node the pass **never ran on** is a different fact, and the contract on
    /// `node_evaluated_deep` fixes its reading: for a node no frame is
    /// computing, `None` must never mean "proven concrete", so it reads
    /// parameterized.  Conflating the two was the defect `P1-31`.
    ///
    /// The assumption **fills a missing verdict; it never overrides one** — a
    /// node that already wrote its answer keeps it, even if a later re-entrant
    /// pass cuts on it again.
    fn ref_is_parameterized(&self, cache: &mut StaticModuleCache<P>, id: AnyNodeId) -> bool {
        match id {
            Dyn(node) => {
                let entry = &self.nodes[node];
                match entry.evaluated_deep {
                    Some(deep) => deep.parameterized,
                    None => !entry.assumed_concrete,
                }
            }
            AnyNodeId::Static(sref) => cache.node_parameterized(self, sref),
        }
    }

    /// Whether `value` — the value this module just evaluated for `node` — is
    /// unproven: the lazy marker itself, an array or table with a shallow
    /// position or a parameterized element, or an operation whose operand is
    /// parameterized.  `cache` is the walk's static-module resolution cache,
    /// so a static element's solved flag costs one lookup per module for the
    /// whole walk rather than one per element.
    ///
    /// Every position's own verdict is read through
    /// [`Self::ref_is_parameterized`], so an **in-progress** position is
    /// assumed concrete (the coinductive step) while one the pass never ran on
    /// is not.
    fn value_is_parameterized(
        &self,
        cache: &mut StaticModuleCache<P>,
        value: P::Value,
        node: NodeId,
    ) -> bool {
        // The value's extension view is taken once: every arm below tests the
        // same value, and taking the view clones the extension leaf out of the
        // composed union, so re-taking it per arm is work already done.
        let view = value.as_enum();
        matches!(view, Some(LowValue::Parameterized))
            || matches!(
                view,
                Some(LowValue::Array(array))
                    // An array holding a shallow position can never be
                    // proven concrete — its marked subtree was deliberately
                    // not evaluated, and even an assert's forced pass that
                    // cached values in it leaves it unproven by this flag,
                    // so it is never referenced in place across applies.
                    // SAFETY: `array` is the payload of `value`, the value this
                    // module just evaluated for `node`, so its home block is
                    // alive.  The note covers the two `items()` calls in this
                    // arm.
                    if unsafe { array.items() }.iter().any(|item| item.shallow)
                        || unsafe { array.items() }
                            .iter()
                            .any(|item| self.ref_is_parameterized(cache, item.node))
            )
            || matches!(
                view,
                Some(LowValue::Table(table))
                    // SAFETY: `table` is the payload of `value`, the value this
                    // module just evaluated for `node`, so its home block is
                    // alive.  The note covers the two `items()` calls in this
                    // arm.
                    if unsafe { table.items() }
                        .iter()
                        .any(|item| self.ref_is_parameterized(cache, item.key))
                        || unsafe { table.items() }
                            .iter()
                            .any(|item| self.ref_is_parameterized(cache, item.value))
            )
            || self.nodes[node].operation.is_some_and(|op| {
                op.operand.is_some_and(|operand| {
                    // Deliberately **not** read through `ref_is_parameterized`.
                    // A core operator's operand is the argument array a layer
                    // above synthesized for it, and the deep pass descends
                    // value-reachable edges only, so "this operand was never
                    // walked" is the normal case rather than an anomaly —
                    // reading it as unproven would flip every pair read
                    // (`Index(pair, 0)`) in a template and clone it per apply.
                    // The operand's effect on the node's *value* is already
                    // decided where the value is: the value arm above for a
                    // structural operator (a read of an unbound element yields
                    // the marker), and the `Parameterized` gate the operation
                    // postlude applies to an extension operator's operand
                    // (`Self::evaluate_node_operation`'s `None` arm) for the
                    // rest.  Operands are static graph edges, not
                    // value-reachable, so a nested block release may have
                    // dropped the node by now.
                    self.nodes
                        .get(operand)
                        .is_some_and(|node| node.evaluated_deep.is_some_and(|e| e.parameterized))
                })
            })
    }

    fn evaluate_block(&mut self, root: NodeId) -> P::Value {
        let value = self.evaluate_node_deep(root, None);
        // The deep pass answers without caching the root in two legitimate
        // cases, so the compaction below may have no moved value to return:
        // a budget refusal returns before `evaluate_node`, and a
        // `Parameterized` answer is deliberately left uncached by the
        // postlude.  Both are leaf markers owned by no arena, so the pass's
        // own answer is the block's value verbatim — `Void` for a refusal,
        // whose budget verdict is already recorded, so this propagates the
        // refusal rather than reporting it a second time.
        self.garbage_collect(root).unwrap_or(value)
    }
}
