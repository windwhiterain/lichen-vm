//! What an operator **reads**, and which node **defines** a value — the two
//! facts a control-flow graph over this graph needs, and the two each consumer
//! used to re-derive for itself.
//!
//! # Why this is here rather than in a consumer
//!
//! Each backend read its own operands out of each operator's operand array:
//! `lichen-compute` had `operand_pair`, `apply_pair` and `operand_items`
//! decoding three shapes between them, and `lichen-compute-gpu` had its own.
//! **Two consumers, one fact, and no way for them to disagree loudly** — they
//! could only disagree by being wrong in one file. The same is true of the
//! aliasing below, which `emit_node` re-derived on every bare cell it reached.
//!
//! # The graph is not SSA over nodes, and that is the whole of this module
//!
//! A `NodeId` is not a value. The apply clone walk **unifies** a substituted
//! parameter with its argument, so two node ids can be one value; and a cell the
//! deep pass left `Parameterized` resolves through its equality class to a node
//! that *computes* it, which is not one of its operands. An ordered list of
//! nodes therefore cannot say which node defines a value.
//!
//! [`Module::define_in`] is the rule: it takes any node naming a value and
//! answers **what defines it** — a computation in this body, one of the
//! function's parameter leaves, or nothing computable at all. A
//! [control-flow graph](Body) is then SSA over the answers, and a consumer reads
//! names instead of re-deriving them.
//!
//! # Why the instruction list this replaces was the wrong shape
//!
//! `KernelInstr` was a **stack machine**: instructions named no values, so both
//! consumers walked an operand stack and each *derived* the form it wanted —
//! `waffle` derives a stack from SSA, `spirv.rs` derives SSA ids from a stack.
//! Both paid to undo what the IR omitted, and the omission was not cosmetic: a
//! stack machine has nowhere to put a value that outlives an expression, so a
//! **loop-carried value had no representation at all**
//! (`docs/notes/loop-conversion.md` §8.5 item 1c).

use std::collections::HashSet;

use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

use crate::{AnyNodeId, FunctionId, LowOperator, LowValue, Module, NodeId, Program};

/// What an `Index` node turned out to be. See [`Module::selection_of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// A `[then, else]` pair indexed by an undecided value: the language's
    /// conditional, and the one `Index` that computes.
    Computed,
    /// A view: the value the index actually names.
    Views(NodeId),
}

/// A function's control-flow graph: **SSA over the values this module defines**.
#[derive(Debug, Clone)]
pub struct Body {
    /// The function this is the body of.
    pub function: FunctionId,
    /// The blocks, in no particular order — [`Self::entry`] is the start.
    pub blocks: Vec<BasicBlock>,
    /// Which block control starts in.
    pub entry: usize,
}

/// One basic block: the values it receives, the values it computes, and where
/// it goes.
///
/// **Not [`Block`](crate::Block).** That is a garbage-collection arena — a
/// `Bump` with a parent/child chain — and the two arrived independently: this one
/// is control flow, that one is memory. Naming them apart is not tidiness, because
/// a caller holding both is holding two different things that once shared a word.
#[derive(Debug, Clone)]
pub struct BasicBlock {
    /// The values this block **receives**.
    ///
    /// **The entry block's are the function's parameters**, flattened to their
    /// scalar leaves — so a body reads a parameter by name here, the same way a
    /// loop body reads its carried state. **One rule for both is the point**: it
    /// is what makes "read the loop's state" an ordinary read rather than an
    /// instruction that exists only inside a loop.
    pub params: Vec<NodeId>,
    /// The values this block computes, **in an order that respects operands** —
    /// a value appears after everything it reads. A consumer may emit straight
    /// through, with no ordering to discover and no stack to track.
    pub instrs: Vec<NodeId>,
    pub terminator: Terminator,
}

/// Where a block goes when its instructions have run.
#[derive(Debug, Clone)]
pub enum Terminator {
    /// Leave the function, carrying these values.
    ///
    /// **A list, because a codomain is.** A kernel may return one leaf or a
    /// tuple of them, and a tuple codomain's values are one `Return` — the same
    /// one wasm's multi-value result is and the same one SPIR-V's `OpReturn` is.
    Return { values: Vec<NodeId> },
    /// Arrive at `target`, handing it `args` as its parameters.
    Br(Br),
    /// Two-way branch on `cond`. Whether a consumer has to narrow the condition
    /// to its own width is the consumer's fact, not this module's.
    CondBr {
        cond: NodeId,
        if_true: Br,
        if_false: Br,
    },
}

/// A branch and the values it hands over.
///
/// **`args` is the whole of a block's incoming state.** A backedge's args are
/// the next iteration's carried tuple, which is why a loop needs no second
/// mechanism for a phi.
#[derive(Debug, Clone)]
pub struct Br {
    pub target: usize,
    pub args: Vec<NodeId>,
}

/// What defines the value `node` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Define {
    /// A computation in this body: the node that computes it, and whose
    /// operands are themselves resolved.
    Computed(NodeId),
    /// The function's `slot`-th parameter leaf — `leaf` is the node naming it.
    Parameter { slot: usize, leaf: NodeId },
    /// The value has no definition this graph can see.
    Opaque,
}

impl<P: Program> Module<P> {
    /// Every node whose value `node` reads, in operand order.
    ///
    /// **One authority for "what does this operator read"**, beside the
    /// [`LowOperator`] enum that documents each shape. The three structural
    /// operators name their operands; a program's own operator does not, so this
    /// returns the **operand array** itself — the array is a value like any
    /// other, is ordered before its operator, and its own items are reached
    /// through the array's own definition.
    ///
    /// A node with no operation reads nothing: it is a value (a parameter, a
    /// literal, a resolved cell).
    pub fn operands_of(&self, node: NodeId) -> Result<Vec<NodeId>, String> {
        let Some(operation) = self.node_operation(node) else {
            return Ok(Vec::new());
        };
        let Some(operand) = operation.operand else {
            return Ok(Vec::new());
        };
        match operation.operator.as_enum() {
            // The checker's apply operands are `[function, argument, result_cell]`
            // and the result cell is not codegen, so this takes the first two and
            // tolerates the rest.
            Some(LowOperator::Apply) => self.operand_pair(operand, "Apply"),
            Some(LowOperator::Index) | Some(LowOperator::TableGet) => {
                self.operand_pair(operand, "operator")
            }
            // A program's own operator: its operand array, whole.
            _ => Ok(vec![operand]),
        }
    }

    /// The `[left, right]` of a two-element operand array, all of this module's.
    fn operand_pair(&self, operand: NodeId, what: &str) -> Result<Vec<NodeId>, String> {
        // SAFETY: `operand` is a live node of `self`; nothing here releases it.
        let items = unsafe { self.array_items(operand) }
            .ok_or_else(|| format!("{what} operand is not an array value"))?;
        if items.len() < 2 {
            return Err(format!(
                "{what} operand array must have two elements, and has {}",
                items.len()
            ));
        }
        items[..2]
            .iter()
            .map(|item| match item.node {
                AnyNodeId::Dynamic(node) => Ok(node),
                // A static element is a reference into a frozen module, which has
                // no node here; a body that reads one cannot be lowered.
                AnyNodeId::Static(_) => Err(format!(
                    "{what} operand element is a static reference into a frozen module, which has \
                     no node in this graph"
                )),
            })
            .collect()
    }

    /// The scalar leaves of a value: itself, or an array's items, one level deep.
    ///
    /// **One level, because a kernel's domain is at most a tuple of scalars.** A
    /// deeper structure would flatten to more than the ABI passes.
    pub fn value_leaves(&self, value: NodeId) -> Result<Vec<NodeId>, String> {
        // SAFETY: `value` is a live node of `self`.
        match unsafe { self.array_items(value) } {
            Some(items) => items
                .iter()
                .map(|item| match item.node {
                    AnyNodeId::Dynamic(node) => Ok(node),
                    AnyNodeId::Static(_) => Err(
                        "a body reads a static reference into a frozen module, which has no node in \
                         this graph"
                            .into(),
                    ),
                })
                .collect(),
            None => Ok(vec![value]),
        }
    }

    /// The member of `node`'s equality class that **defines** its value: a class
    /// member carrying a computational operator, and not a `value_of` index
    /// extraction — that is a view of a `[value, type]` pair rather than the
    /// computation itself.
    ///
    /// The deep pass collapses some values to a bare `Parameterized` cell and
    /// unifies that cell with the defining computation (a kernel call's result, a
    /// `launch` argument, a substituted parameter), so the definition is reached
    /// through the class rather than through an operand.
    ///
    /// **The walk is the class's own member list, not a scan of the module's
    /// node table.** The table holds every kernel's nodes while one kernel is
    /// being compiled, so a scan made codegen quadratic in the number of
    /// kernels.
    pub fn defining_member(&self, node: NodeId) -> Option<NodeId> {
        let root = self.class_root(node);
        for member in disjoint::members(&self.nodes, root) {
            if let Some(operation) = self.node_operation(member)
                && !matches!(
                    AsEnum::<LowOperator>::as_enum(&operation.operator),
                    Some(LowOperator::Index)
                )
            {
                return Some(member);
            }
        }
        None
    }

    /// What defines the value `node` names, read as a member of `function`'s
    /// body.
    ///
    /// **This is the rule that makes the graph SSA.** Three answers, in order:
    ///
    /// 1. **a parameter leaf** — `node` is in the same equality class as one of
    ///    the function's domain leaves, so the value *is* that parameter. This is
    ///    how a reduced same-module call's substituted parameter reads: the deep
    ///    pass unified it with the argument.
    /// 2. **a computation** — the node itself, or the class member that computes
    ///    it ([`Self::defining_member`]).
    /// 3. **opaque** — the value has no definition this graph can see.
    pub fn define_in(&self, function: FunctionId, node: NodeId) -> Define {
        // An `Index` is **usually a view of something already computed** — a
        // parameter read at a path, a `value_of` peel, an element of an array the
        // graph materialised. Only a *selection* computes anything, so resolving
        // the views here is what keeps them out of a block's `instrs`.
        if let Some(selection) = self.selection_of(node) {
            return match selection {
                Selection::Computed => Define::Computed(node),
                Selection::Views(view) => self.define_in(function, view),
            };
        }
        for (slot, leaf) in self
            .parameter_leaves(function)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            if self.class_root(node) == self.class_root(leaf) {
                return Define::Parameter { slot, leaf };
            }
        }
        if self.node_operation(node).is_some() {
            return Define::Computed(node);
        }
        match self.defining_member(node) {
            Some(computation) => Define::Computed(computation),
            None => Define::Opaque,
        }
    }

    /// What an `Index` node is: a **selection** (it computes a value) or a
    /// **view** of something else (it names one).
    ///
    /// Three views, in the order they are tried, and one selection:
    ///
    /// - **a `value_of` peel** — `Index(pair, 0)` where `pair` is a
    ///   `[value, type]` pair. The extraction is a view of the pair's *value*.
    /// - **an element of a materialised array** — the destructuring a slot read
    ///   leaves behind (`read [a, b]` becomes `x(0)`, `x(1)`).
    /// - **a parameter read at a constant path** — `x(0)` on a tuple domain,
    ///   which resolves through the same peel.
    ///
    /// And the one that computes: **a `[then, else]` pair indexed by a value the
    /// graph cannot decide** — the language's conditional, which `LowOperator::
    /// Index` is because a branch is an ordinary lazy index. Its arms are separate
    /// values, so a consumer reads them by name and emits one select.
    ///
    /// **Nothing here decides which arm runs.** Both arms are values in the body,
    /// which is what lets a backend build a real branch rather than a select, and
    /// what the loop conversion finally takes.
    pub fn selection_of(&self, node: NodeId) -> Option<Selection> {
        let operation = self.node_operation(node)?;
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Index)
        ) {
            return None;
        }
        let operands = self.operand_pair(operation.operand?, "Index").ok()?;
        let (target, index) = (operands[0], operands[1]);
        let constant = self.usize_value(index);
        let array = self.value_half(target);
        if let (Some(0), Some(array)) = (constant, array) {
            // SAFETY: `array` is a live node of `self`.
            if let Some(element) = unsafe { self.array_items(array) }
                .and_then(|items| items.first())
                .and_then(|item| match item.node {
                    AnyNodeId::Dynamic(node) => Some(node),
                    AnyNodeId::Static(_) => None,
                })
            {
                return Some(Selection::Views(element));
            }
        }
        if let (Some(k), Some(array)) = (constant, array) {
            // SAFETY: `array` is a live node of `self`.
            if let Some(element) = unsafe { self.array_items(array) }
                .and_then(|items| items.get(k))
                .and_then(|item| match item.node {
                    AnyNodeId::Dynamic(node) => Some(node),
                    AnyNodeId::Static(_) => None,
                })
            {
                return Some(Selection::Views(element));
            }
        }
        if constant.is_some() {
            return None;
        }
        // SAFETY: `array` is a live node of `self`.
        match array {
            Some(array) => {
                // SAFETY: `array` is a live node of `self`.
                let arms = unsafe { self.array_items(array) }.map_or(0, |items| items.len());
                (arms == 2).then_some(Selection::Computed)
            }
            None => None,
        }
    }

    /// The **value** half of a `[value, type]` pair node, or `None` when it is
    /// not a pair. See [`Module::pair_value_half`] for the width rule, which is
    /// `apply.rs`'s and not this file's.
    fn value_half(&self, node: NodeId) -> Option<NodeId> {
        self.pair_value_half(node)
    }

    /// The `n` a node holds, when it holds a plain integer literal.
    fn usize_value(&self, node: NodeId) -> Option<usize> {
        match self.structural_value(node) {
            Some(LowValue::USize(n)) => Some(n as usize),
            _ => None,
        }
    }

    /// The function's parameter leaves: the **value** half of its `[value, type]`
    /// pair, flattened. A function whose parameter node is not a pair (the
    /// checker leaves a direct kernel-apply's codomain unbound and stores the
    /// value node directly) is the one leaf.
    pub fn parameter_leaves(&self, function: FunctionId) -> Result<Vec<NodeId>, String> {
        let parameter = self.functions[function].parameter;
        match self.array_element(parameter, 0) {
            Ok(value) => self.value_leaves(value),
            // Not a pair: the whole parameter node is the one leaf.
            Err(_) => Ok(vec![parameter]),
        }
    }

    /// Element `at` of an array node.
    fn array_element(&self, array: NodeId, at: usize) -> Result<NodeId, String> {
        // SAFETY: `array` is a live node of `self`.
        let items = unsafe { self.array_items(array) }
            .ok_or_else(|| format!("node {array:?} is not an array value"))?;
        match items.get(at).map(|item| item.node) {
            Some(AnyNodeId::Dynamic(node)) => Ok(node),
            Some(AnyNodeId::Static(_)) => Err(format!(
                "element {at} of node {array:?} is a static reference into a frozen module"
            )),
            None => Err(format!(
                "node {array:?} has no element {at}: it has {} element(s)",
                items.len()
            )),
        }
    }

    /// The control-flow graph of `function`.
    ///
    /// **Straight-line only.** A body with no branch is one block, and that is
    /// every body the compute surface lowers today. A `@loop`-marked function
    /// is refused **by name**, because its recursion is converted from the
    /// templates and the nest is not built yet
    /// ([`docs/notes/loop-conversion.md`](../../docs/notes/loop-conversion.md)
    /// §8.6).
    pub fn control_flow(&self, function: FunctionId) -> Result<Body, String> {
        if self.function_is_looping(function) {
            return Err(format!(
                "function {function:?} is a `@loop` binding and no loop has been built for it yet: \
                 its recursion is converted from the templates, which this control-flow graph \
                 cannot express until the loop nest lands \
                 (`docs/notes/loop-conversion.md` §8.6)"
            ));
        }
        let leaves = self.parameter_leaves(function)?;
        let values = self.function_values(function)?;
        let params: Vec<NodeId> = leaves
            .iter()
            .map(|leaf| self.define_in(function, *leaf))
            .filter_map(|define| match define {
                Define::Computed(node) => Some(node),
                Define::Parameter { leaf, .. } => Some(leaf),
                Define::Opaque => None,
            })
            .collect();

        // Post-order from the returned values: a definition is emitted when
        // everything it reads is, which is a dependency order without a sort. The
        // traversal is an explicit stack rather than a recursion, so a deep body
        // costs heap rather than the native stack.
        let mut instrs = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut on_stack: Vec<(NodeId, usize)> = values.iter().map(|&node| (node, 0)).collect();
        on_stack.reverse();
        while let Some((node, at)) = on_stack.last().copied() {
            let Define::Computed(definition) = self.define_in(function, node) else {
                on_stack.pop();
                seen.insert(node);
                continue;
            };
            let operands = self.operands_of(definition)?;
            if at < operands.len() {
                on_stack.last_mut().expect("non-empty").1 += 1;
                let operand = operands[at];
                // A node owned by another template is a boundary, not an
                // operand: a nested closure's body is its own function's.
                if !seen.contains(&operand) && self.belongs_to(operand, function)? {
                    on_stack.push((operand, 0));
                }
                continue;
            }
            on_stack.pop();
            if seen.insert(definition) {
                instrs.push(definition);
            }
        }
        // Post-order already puts each value's dependencies first. **The returned
        // values lead**, in their own order: a consumer emits the block and hands
        // the terminator's list out, and the two must agree about which is which.
        let leading = values
            .iter()
            .copied()
            .filter_map(|value| match self.define_in(function, value) {
                Define::Computed(node) => Some(node),
                Define::Parameter { leaf, .. } => Some(leaf),
                Define::Opaque => None,
            })
            .collect::<Vec<_>>();
        instrs.retain(|node| !leading.contains(node));
        let mut ordered = leading.clone();
        ordered.extend(instrs);

        Ok(Body {
            function,
            blocks: vec![BasicBlock {
                params,
                instrs: ordered,
                terminator: Terminator::Return { values: leading },
            }],
            entry: 0,
        })
    }

    /// The values the function returns, flattened: the **value** half of its
    /// `[value, type]` pair when that half is a pair itself (a tuple codomain),
    /// one value when it is a scalar, and the return node itself when the
    /// checker stored a bare value (a direct kernel-apply's codomain is left
    /// unbound, so its return is the value node).
    fn function_values(&self, function: FunctionId) -> Result<Vec<NodeId>, String> {
        let r#return = self.functions[function].r#return;
        match self.pair_value_half(r#return) {
            // A scalar codomain's value half is a leaf, and an array of leaves is
            // a tuple codomain — the same one-level rule `value_leaves` states.
            Some(value) => self.value_leaves(value),
            None => Ok(vec![r#return]),
        }
    }

    /// The **value** half of a `[value, type]` pair node, or `None` when the node
    /// is not one.
    ///
    /// **The width is `apply.rs`'s, not a guess.** That module resolves the
    /// apply's return pair with `items[1]` as the type slot and says so for "a
    /// 2-wide pair and for a 3-wide `[value, type, perspective]` pair alike", so
    /// a pair here is two **or three** wide and the value is element 0 in both.
    /// Reading it as two-wide alone would drop the value half of every
    /// perspective-bearing pair.
    ///
    /// This is the one place in this module that knows an encoding, and it is
    /// here because the lowlevel already owns the convention — `Function` calls
    /// its parameter node a *pair*, the apply resolves its arity, and the
    /// evaluator peels `Index(pair, 0)`. It is **not** the type layer's business
    /// to be told again: no shape is derived here, and the pass in
    /// [`crate::low_type`] still learns no layout.
    fn pair_value_half(&self, node: NodeId) -> Option<NodeId> {
        // SAFETY: `node` is a live node of `self`.
        let items = unsafe { self.array_items(node) }?;
        if !(2..=3).contains(&items.len()) {
            return None;
        }
        match items[0].node {
            AnyNodeId::Dynamic(value) => Some(value),
            AnyNodeId::Static(_) => None,
        }
    }

    /// Whether `node`'s owner chain reaches `function` — the same membership
    /// test the apply clone walk uses, read here so the two cannot disagree.
    pub fn belongs_to(&self, node: NodeId, function: FunctionId) -> Result<bool, String> {
        let mut owner = self.nodes[node].function;
        while let Some(current) = owner {
            if current == function {
                return Ok(true);
            }
            owner = self.functions[current].parent;
        }
        Ok(false)
    }

    /// The structural value `node` holds, when it holds one — what a consumer
    /// reads for a literal rather than an operator.
    pub fn structural_value(&self, node: NodeId) -> Option<LowValue> {
        let value = self.node_value(AnyNodeId::Dynamic(node))?;
        AsEnum::<LowValue>::as_enum(&value)
    }
}
