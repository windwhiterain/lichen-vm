//! **Which value does this node name?** — the graph facts every consumer of a
//! body was re-deriving for itself.
//!
//! # The line this module is on
//!
//! `low_type.rs` states it for the type side, and it is the same line here:
//!
//! > the graph facts (a transfer per operator, a class per value) live here, the
//! > type facts live with whoever owns the encoding.
//!
//! What this module holds is **what the graph says**: what an operator reads,
//! and what defines a value. What it deliberately does **not** hold is a
//! **control-flow graph**.
//!
//! ## Why not a CFG here
//!
//! This graph is a **DAG of values**. Every node is computed once and cached, and
//! `evaluate_node_deep` answers a `LowValue` for every one it reaches. A CFG says
//! *some of this is not computed once* — a header is entered many times and its
//! blockparams differ each time — so a CFG is a **program** fact, not a graph
//! fact, and the program (a kernel body) belongs to `lichen-kernel-ir` and
//! `lichen-compute`. Putting one here would make a DAG claim to hold something it
//! does not.
//!
//! An earlier draft of this file had exactly that, and it was wrong twice: it
//! exported a `Terminator` beside `lichen-kernel-ir`'s, which is a collision a
//! crate depending on both would hit; and its `Define::Parameter` carried a
//! *slot index*, which is the kernel ABI's arity convention and not the graph's.
//! Both are gone.
//!
//! # Why the graph is not SSA over nodes, which is what these rules are for
//!
//! A `NodeId` is not a value. The apply clone walk **unifies** a substituted
//! parameter with its argument, so two node ids can be one value; and a cell the
//! deep pass left undecided resolves through its equality class to a node
//! that *computes* it, which is **not one of its operands**. An ordered list of
//! nodes therefore cannot say which node defines a value.
//!
//! [`Module::define_in`] is the rule. It takes any node naming a value and
//! answers what defines it — a computation, one of the function's parameter
//! leaves, or nothing computable — and a consumer builds a body over the
//! answers instead of re-deriving them.
//!
//! # Why the instruction list these rules replace was the wrong shape
//!
//! `KernelInstr` was a **stack machine**: instructions named no values, so each
//! backend walked an operand stack and *derived* the form it wanted — `waffle`
//! derives a stack from SSA, `spirv.rs` derives SSA ids from a stack. Both paid
//! to undo the omission, and it was not cosmetic: a stack machine has nowhere to
//! put a value that outlives an expression, so a **loop-carried value had no
//! representation at all** (`docs/notes/loop-conversion.md` §8.5 item 1c).

use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

use crate::ArrayItem;
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

/// What defines the value a node names.
///
/// **A parameter is named by its node, not by an index.** A slot number is the
/// kernel ABI's — the flattening of a domain into arguments — and that
/// convention belongs to whoever lays out a call, not to the graph. The consumer
/// maps the leaf to its own slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Define {
    /// A computation in this body: the node that computes it, whose operands are
    /// themselves resolved.
    Computed(NodeId),
    /// One of the function's parameter leaves — the node naming it.
    Parameter(NodeId),
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

    /// An operand array's elements, in order.
    ///
    /// **The whole array, not the first two** — [`Self::operand_pair`] is the
    /// two-element reading, and a three-element `[function, argument,
    /// result_cell]` has to be readable whole for a caller that wants its third.
    /// A lowering that needs an operand's shape asks for the shape it needs; the
    /// two readings cannot disagree about what the array holds.
    pub fn operand_items(&self, operand: NodeId) -> Result<&'static [ArrayItem], String> {
        // SAFETY: `operand` is a live node of `self`; nothing here releases it.
        unsafe { self.array_items(operand) }.ok_or_else(|| "operand is not an array value".into())
    }

    /// The `[left, right]` of a two-element operand array, all of this module's.
    pub fn operand_pair(&self, operand: NodeId, what: &str) -> Result<Vec<NodeId>, String> {
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
    /// deeper structure would flatten to more than the ABI passes — and the count
    /// is the ABI's convention, which is why this flattens and leaves the number
    /// to the consumer.
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
    /// The deep pass collapses some values to a bare empty cell and
    /// unifies that cell with the defining computation (a kernel call's result, a
    /// `launch` argument, a substituted parameter), so the definition is reached
    /// through the class rather than through an operand.
    ///
    /// **The walk is the class's own member list, not a scan of the module's node
    /// table.** The table holds every kernel's nodes while one kernel is being
    /// compiled, so a scan made codegen quadratic in the number of kernels.
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

    /// What defines the value `node` names, against the **domain the caller
    /// states**.
    ///
    /// `domain` is the function's domain *value* node — not the function, and not
    /// its `[value, type]` parameter pair. The lowlevel has **no type
    /// representation of its own**, so a pair is the only thing a type question can
    /// be answered with, and the cost of decoding one is that the width becomes a
    /// second answer. The caller that already resolved the pair passes what it
    /// knows; see `docs/notes/checker-encoding-instability.md` for the five readers
    /// and the one defect they share.
    ///
    /// **Three answers**, in order:
    ///
    /// 1. **a parameter leaf** — `node` is in the same equality class as one of
    ///    the domain's leaves, so the value *is* that parameter. This is how a
    ///    reduced same-module call's substituted parameter reads: the deep pass
    ///    unified it with the argument.
    /// 2. **a computation** — the node itself, or the class member that computes
    ///    it ([`Self::defining_member`]).
    /// 3. **opaque** — the value has no definition this graph can see.
    /// The leaves of `function`'s **parameter domain** — the pair's value half,
    /// flattened one level.
    ///
    /// **This is where a domain is derived, and deriving it in one place is what
    /// lets every reader agree.** The value half is a decoding of the pair, and a
    /// caller that decoded it separately could decode it differently.
    pub fn parameter_leaves(&self, function: FunctionId) -> Result<Vec<NodeId>, String> {
        let parameter = self.functions[function].parameter;
        match self.pair_value_half(parameter) {
            Some(value) => self.value_leaves(value),
            None => Ok(vec![parameter]),
        }
    }

    /// What defines the value `node` names, against the **function whose domain
    /// it is read in**.
    ///
    /// **The domain is the function's parameter, derived here** rather than handed
    /// in: the value half is a decoding of the pair, and every caller that decoded
    /// it separately could decode it differently. A caller that already holds the
    /// value half reads [`Self::value_leaves`] directly.
    ///
    /// **Three answers**, in order:
    ///
    /// 1. **a parameter leaf** — `node` is in the same equality class as one of
    ///    the domain's leaves, so the value *is* that parameter. This is how a
    ///    reduced same-module call's substituted parameter reads: the deep pass
    ///    unified it with the argument.
    /// 2. **a computation** — the node itself, or the class member that computes
    ///    it ([`Self::defining_member`]).
    /// 3. **opaque** — the value has no definition this graph can see.
    pub fn define_in(&self, function: FunctionId, node: NodeId) -> Define {
        // An `Index` is **usually a view of something already computed** — a
        // `value_of` peel, an element of an array the graph materialised. Only a
        // *selection* computes anything, so resolving the views here is what
        // keeps them out of anything built over the answers.
        if let Some(selection) = self.selection_of(node) {
            return match selection {
                Selection::Computed => Define::Computed(node),
                Selection::Views(view) => self.define_in(function, view),
            };
        }
        for leaf in self.parameter_leaves(function).unwrap_or_default() {
            if self.class_root(node) == self.class_root(leaf) {
                return Define::Parameter(leaf);
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
    /// **One rule for every view, and no guess about encodings.** A *constant*
    /// index names an element: `Index(x, k)` is `x`'s element `k`, whether `x` is a
    /// `[value, type]` pair (a `value_of` peel, `x` a typed slot), a tuple
    /// domain's leaves (`x(1)` on `<Int, Int, Int>`), or an array the graph
    /// materialised (the destructuring a slot read leaves behind). An earlier
    /// draft asked first whether `x` was a pair and inferred that from its
    /// **width** — which a two-leaf domain also has, so it could name the wrong
    /// array. Width is not the question; a constant index is the answer.
    ///
    /// - **an operator target at index 0** — `Index(e, 0)` where `e` computes —
    ///   is a `value_of` peel: the extraction is a view of the computation's
    ///   own result, so the view is the target itself. (A parameter read at a
    ///   constant path is resolved *before* this rule fires, by the consumer
    ///   that knows the parameter — `emit_node`'s parameter-read arm.)
    /// - **a target holding an array value is the container itself** — a
    ///   constant index selects the element (the `[value, type]` peel and a
    ///   tuple's element 0 coincide), and an undecided index into a
    ///   two-element array is the one `Index` that **computes**: the
    ///   language's conditional, which `LowOperator::Index` *is*, because a
    ///   branch is an ordinary lazy index. Its arms are separate values, so a
    ///   consumer reads them by name.
    ///
    /// Anything else — a computed container, a constant index past the end —
    /// is not a view this graph can see through, and [`None`] leaves the node
    /// to be treated as a computation.
    ///
    /// **Nothing here decides which arm runs.** Both arms are values in the body,
    /// which is what leaves a branch available to take.
    pub fn selection_of(&self, node: NodeId) -> Option<Selection> {
        let operation = self.node_operation(node)?;
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Index)
        ) {
            return None;
        }
        let operands = self.operand_items(operation.operand?).ok()?;
        let (Some(target), Some(index)) = (operands.first(), operands.get(1)) else {
            return None;
        };
        // **An operand may be frozen.** The apply clone copies a callee's unchanged
        // subterms by reference, so an index inside a routed body can name a node
        // of the frozen module — and a view is a view whether the node it reads
        // lives here or in the module it came from, so the test reads whatever the
        // operand is rather than insisting it be this module's.
        if let Some(k) = self.usize_value(index.node) {
            if let Some(element) = self.item_of(target.node, k) {
                return Some(Selection::Views(element));
            }
            // **A computed target at index 0 is the peel the checker makes over a
            // call's result** — `value_of` applied to an expression that computes.
            // The operator's result *is* the pair's value, so the peel names the
            // operator rather than anything under it.
            if k == 0
                && let AnyNodeId::Dynamic(node) = target.node
                && self.node_operation(node).is_some()
            {
                return Some(Selection::Views(node));
            }
            return None;
        }
        // SAFETY: every operand here is a live node of this module or of a frozen
        // module that outlives the reference it was read through.
        match unsafe { self.array_items_of(target.node) } {
            Some(items) if items.len() == 2 => Some(Selection::Computed),
            _ => None,
        }
    }

    /// An operand array's items, whether the array belongs to this module or to a
    /// frozen one the caller holds.
    ///
    /// **A frozen node's payload belongs to its own module**, so only this
    /// module's arrays are read here; a frozen *array* has no node in this graph
    /// and is answered `None` rather than by reaching across the boundary.
    ///
    /// # Safety
    ///
    /// The node must be live — this module's, or one of a frozen module that
    /// outlives the reference it was reached through, which is the same obligation
    /// every read of an `AnyNodeId` carries.
    pub unsafe fn array_items_of(&self, node: AnyNodeId) -> Option<&'static [ArrayItem]> {
        match node {
            AnyNodeId::Dynamic(node) => unsafe { self.array_items(node) },
            AnyNodeId::Static(_) => None,
        }
    }

    /// Element `k` of an array node, dynamic entries only.
    fn item_of(&self, array: AnyNodeId, k: usize) -> Option<NodeId> {
        let items = unsafe { self.array_items_of(array) }?;
        match items.get(k)?.node {
            AnyNodeId::Dynamic(node) => Some(node),
            AnyNodeId::Static(_) => None,
        }
    }

    /// The **value** half of a `[value, type]` pair node, or `None` when it is
    /// not a pair.
    ///
    /// **The width is `apply.rs`'s, not a guess.** That module resolves the
    /// apply's return pair with `items[1]` as the type slot and says so for "a
    /// 2-wide pair and for a 3-wide `[value, type, perspective]` pair alike", so
    /// a pair is two **or three** wide and the value is element 0 in both.
    ///
    /// This is the one place that knows an encoding, and it is here because the
    /// lowlevel already owns the convention — `Function` calls its parameter node
    /// a *pair*, the apply resolves its arity, and the evaluator peels
    /// `Index(pair, 0)`. **No shape is derived here**, and the pass in
    /// [`crate::low_type`] still learns no layout.
    pub fn pair_value_half(&self, node: NodeId) -> Option<NodeId> {
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

    /// The **type** half of a `[value, type]` pair node, or `None` when it is
    /// not a pair — the sibling of [`Self::pair_value_half`], reading element 1 by
    /// the same rule (a 2-wide pair and a 3-wide `[value, type, perspective]` pair
    /// agree there).
    ///
    /// **Public for the same reason** as its sibling: the loop reader names a
    /// pair's type when it checks an entering call's argument type, and
    /// duplicating that decoding is what
    /// `docs/notes/checker-encoding-instability.md` is about.
    pub fn pair_type_half(&self, node: NodeId) -> Option<NodeId> {
        // SAFETY: `node` is a live node of `self`.
        let items = unsafe { self.array_items(node) }?;
        if !(2..=3).contains(&items.len()) {
            return None;
        }
        match items[1].node {
            AnyNodeId::Dynamic(r#type) => Some(r#type),
            AnyNodeId::Static(_) => None,
        }
    }

    /// The structural value `node` holds, when it holds one — what a consumer
    /// reads for a literal rather than an operator.
    pub fn structural_value(&self, node: NodeId) -> Option<LowValue> {
        let value = self.node_value(AnyNodeId::Dynamic(node))?;
        AsEnum::<LowValue>::as_enum(&value)
    }

    /// The `n` a node holds, when it holds a plain integer literal — **frozen or
    /// not**, because a literal an apply clone copied across is still a literal.
    pub fn usize_value(&self, node: impl Into<AnyNodeId>) -> Option<usize> {
        let value = self.node_value(node.into())?;
        match AsEnum::<LowValue>::as_enum(&value) {
            Some(LowValue::USize(n)) => Some(n as usize),
            _ => None,
        }
    }
}
