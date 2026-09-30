use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, BlockId, BudgetExhausted, EvaluatedDeep,
    LowOperator, LowValue, Module, NodeId, OperatorExt as _, Program, table::KeyState,
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
    /// Run one evaluation attempt with the mark held.
    fn run(self, body: impl FnOnce(&mut Module<P>, NodeId) -> P::Value) -> P::Value {
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

    /// The mark of one evaluation attempt, held until it drops.
    ///
    /// Invariant: an evaluation attempt owns `Node::visiting` for exactly its
    /// own frame and clears it on every exit — a cached answer, a lazy
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
                                                    self.evaluate_node(Dyn(element), Some(block))
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
                    // A nullary operator (e.g. `TypeOperator::Fresh`) has no
                    // operand node: the honest stand-in is the computed-nothing
                    // value — never the `None` unit value, which a program can
                    // genuinely produce.  (`OperatorExt::run` takes the operand
                    // by value, so the absence is spelled as a value.)
                    None => P::Value::from(LowValue::Void),
                };
                operator.run(operand, block, self)
            }
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
                            // A non-`Function` target is a *callable program
                            // value* (e.g. a compute `Kernel`) — not a
                            // structural apply.  The compute layer compiles it;
                            // here it stays lazy so a body's deep pass and the
                            // JIT can read the graph instead of panicking on a
                            // legitimate (checker-validated) kernel apply.  A
                            // genuinely non-callable target is caught by the
                            // checker's unification before the deep pass runs,
                            // so reaching this arm is never a real error.
                            _ => P::Value::from(LowValue::Parameterized),
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
                                        let mut path = Vec::new();
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
        if !matches!(value.as_enum(), Some(LowValue::Parameterized)) {
            // The single write API caches the result and, if the node is a
            // member of a unified class, replicates a concrete value to the
            // class's unbound pure-cell members — so a late-arriving value
            // (e.g. a lazy host-operator result) reaches the cells and the
            // representative bound before it was concrete.  Without this,
            // `bind` (which reads only the representative's value) could not
            // see the member's concrete value when the class later merges.
            self.write_node_value(node, Some(value));
        }
        value
    }

    /// Run [`Self::evaluate_node`] for all nodes in the reachable subtree of `id`.
    #[stacksafe]
    pub fn evaluate_node_deep(&mut self, node: NodeId, current: Option<BlockId>) -> P::Value {
        self.evaluate_node_deep_inner(Dyn(node), current, true, false)
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
        self.evaluate_node_deep_inner(Dyn(node), current, false, true)
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
    #[stacksafe]
    fn evaluate_node_deep_inner(
        &mut self,
        node: AnyNodeId,
        current: Option<BlockId>,
        skip_shallow: bool,
        force_operand: bool,
    ) -> P::Value {
        // A static ref is a decided leaf: the module solved it, so there is
        // nothing to evaluate, descend, or mark — read its value.
        // Even a forced pass gains nothing from a solved subtree (residuals
        // never re-run), so the leaf rule is unconditional.
        if let AnyNodeId::Static(sref) = node {
            return self.static_read(sref);
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
            // `evaluated_deep` stays `None`, "never ran".  The nested counter
            // deliberately stays inflated, as it did when the guard unwound.
            self.deep_depth -= 1;
            return P::Value::from(LowValue::Void);
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
            self.evaluate_node_deep_inner(Dyn(operand), Some(block), false, true);
        }
        let value = self.evaluate_node(Dyn(node), current);
        if let Some(LowValue::Array(array)) = value.as_enum() {
            // The descent below may reach `node` again through the array's own
            // items (a self-referential value), so it is marked for the
            // duration: the same structural-cycle cut as the entry above.
            self.nodes[node].visiting = true;
            let block = self.nodes[node].block;
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
                self.evaluate_node_deep_inner(item.node, Some(block), skip_shallow, force_operand);
            }
            self.nodes[node].visiting = false;
        }
        // A table's entries are edges like array items: its keys were
        // already forced concrete at build, but its values are lazy refs —
        // both must be proven (or disproven) concrete by the descent.
        if let Some(LowValue::Table(table)) = value.as_enum() {
            self.nodes[node].visiting = true;
            let block = self.nodes[node].block;
            // SAFETY: `table` is the value this module just evaluated for
            // `node`.  The descent below mutates the module but never releases
            // a block — `drop_block` is called only from `garbage_collect` —
            // so the payload's arena stays alive for the whole loop.
            for item in unsafe { table.items() } {
                self.evaluate_node_deep_inner(item.key, Some(block), skip_shallow, force_operand);
                self.evaluate_node_deep_inner(item.value, Some(block), skip_shallow, force_operand);
            }
            self.nodes[node].visiting = false;
        }
        // An array is unproven while any position resolved to the lazy
        // marker, or any position at all sits behind a shallow mark.  A
        // static position's concreteness is the module's solved flag — it
        // was already decided by the deep pass that solved the module.
        let parameterized = matches!(value.as_enum(), Some(LowValue::Parameterized))
            || matches!(
                value.as_enum(),
                Some(LowValue::Array(array))
                    // An array holding a shallow position can never be
                    // proven concrete — its marked subtree was deliberately
                    // not evaluated, and even an assert's forced pass that
                    // cached values in it leaves it unproven by this flag,
                    // so it is never referenced in place across applies.
                    // SAFETY: `array` is the value this module just evaluated
                    // for `node`; its home block is alive.  The note covers the
                    // two `items()` calls in this arm.
                    if unsafe { array.items() }.iter().any(|item| item.shallow)
                        || unsafe { array.items() }.iter().any(|item| match item.node {
                            Dyn(node) => self.nodes[node]
                                .evaluated_deep
                                .is_some_and(|e| e.parameterized),
                            AnyNodeId::Static(sref) => self.static_module(sref.module).nodes
                                [sref.index.index]
                                .parameterized,
                        })
            )
            || matches!(
                value.as_enum(),
                Some(LowValue::Table(table))
                    // SAFETY: `table` is the value this module just evaluated
                    // for `node`; its home block is alive.  The note covers the
                    // two `items()` calls in this arm.
                    if unsafe { table.items() }.iter().any(|item| match item.key {
                        Dyn(node) => self.nodes[node]
                            .evaluated_deep
                            .is_some_and(|e| e.parameterized),
                        AnyNodeId::Static(sref) => self.static_module(sref.module).nodes
                            [sref.index.index]
                            .parameterized,
                    }) || unsafe { table.items() }.iter().any(|item| match item.value {
                        Dyn(node) => self.nodes[node]
                            .evaluated_deep
                            .is_some_and(|e| e.parameterized),
                        AnyNodeId::Static(sref) => self.static_module(sref.module).nodes
                            [sref.index.index]
                            .parameterized,
                    })
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
