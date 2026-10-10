//! Which value does this node name? The graph facts a body consumer needs.
//! See docs/notes/lowlevel-vm.md.

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
/// # Invariant
///
/// A parameter is named by its node, never by a slot index: an index is the
/// kernel ABI's flattening of a domain into arguments, and the consumer maps
/// the leaf to its own slot.
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
    /// # Invariant
    ///
    /// The three structural operators name their operands; a program's own
    /// operator does not, so this returns the operand array itself — the array
    /// is a value like any other, ordered before its operator, and its own
    /// items are reached through the array's own definition. A node with no
    /// operation reads nothing: it is a value.
    pub fn operands_of(&self, node: NodeId) -> Result<Vec<NodeId>, String> {
        let Some(operation) = self.node_operation(node) else {
            return Ok(Vec::new());
        };
        let Some(operand) = operation.operand else {
            return Ok(Vec::new());
        };
        match operation.operator.as_enum() {
            // The checker's apply operands are `[function, argument,
            // result_cell]`, and the result cell is not codegen.
            Some(LowOperator::Apply) => self.operand_pair(operand, "Apply"),
            Some(LowOperator::Index) | Some(LowOperator::TableGet) => {
                self.operand_pair(operand, "operator")
            }
            // A program's own operator: its operand array, whole.
            _ => Ok(vec![operand]),
        }
    }

    /// An operand array's elements, in order — the whole array, not the first
    /// two.
    ///
    /// # Invariant
    ///
    /// [`Self::operand_pair`] is the two-element reading; a three-element
    /// `[function, argument, result_cell]` has to be readable whole for a
    /// caller that wants its third, and the two readings cannot disagree
    /// about what the array holds.
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

    /// The scalar leaves of a value: itself, or an array's items, one level
    /// deep.
    ///
    /// # Invariant
    ///
    /// One level, because a kernel's domain is at most a tuple of scalars; a
    /// deeper structure would flatten past the ABI, and the count is the
    /// consumer's convention.
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

    /// The member of `node`'s equality class that **defines** its value: a
    /// class member carrying a computational operator.
    ///
    /// # Invariant
    ///
    /// A `value_of` index extraction is not a definition — it is a view of a
    /// `[value, type]` pair. The deep pass collapses some values to a bare
    /// empty cell and unifies that cell with the defining computation, so the
    /// definition is reached through the class rather than an operand. The
    /// walk is the class's own member list, not the module's node table.
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

    /// The leaves of `function`'s **parameter domain** — the pair's value
    /// half, flattened one level.
    ///
    /// # Invariant
    ///
    /// The value half is decoded here and nowhere else, so every reader agrees:
    /// a caller that decoded it separately could decode it differently.
    pub fn parameter_leaves(&self, function: FunctionId) -> Result<Vec<NodeId>, String> {
        let parameter = self.functions[function].parameter;
        match self.pair_value_half(parameter) {
            Some(value) => self.value_leaves(value),
            None => Ok(vec![parameter]),
        }
    }

    /// What defines the value `node` names, against the **function whose
    /// domain it is read in**.
    ///
    /// # Invariant
    ///
    /// The domain is the function's parameter, derived here rather than
    /// handed in, so every reader decodes the pair the same way. Three
    /// answers, in order: a parameter leaf — `node` is in one domain leaf's
    /// equality class, how a reduced call's substituted parameter reads; a
    /// computation — the node or the class member that computes it; or opaque
    /// — no definition this graph can see.
    pub fn define_in(&self, function: FunctionId, node: NodeId) -> Define {
        // An `Index` is usually a view of something already computed — a
        // `value_of` peel, a materialised array's element.
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
    /// # Invariant
    ///
    /// A *constant* index names an element, whether the target is a
    /// `[value, type]` pair, a tuple domain's leaves, or a materialised
    /// array. An earlier draft inferred a pair from the **width**, which a
    /// two-leaf domain also has. `Index(e, 0)` over a computation is a
    /// `value_of` peel; an undecided index into a two-element array is the
    /// conditional, the one `Index` that computes.
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
        // **An operand may be frozen**, by reference from the apply clone. A
        // view is a view wherever the node it reads lives.
        if let Some(k) = self.usize_value(index.node) {
            if let Some(element) = self.item_of(target.node, k) {
                return Some(Selection::Views(element));
            }
            // **A computed target at index 0** is a `value_of` peel: the
            // result *is* the pair's value, so it names the operator.
            if k == 0
                && let AnyNodeId::Dynamic(node) = target.node
                && self.node_operation(node).is_some()
            {
                return Some(Selection::Views(node));
            }
            return None;
        }
        // SAFETY: each operand is a live node of this module, or of a
        // frozen module outliving the reference it was read through.
        match unsafe { self.array_items_of(target.node) } {
            Some(items) if items.len() == 2 => Some(Selection::Computed),
            _ => None,
        }
    }

    /// An operand array's items, whether the array belongs to this module or
    /// to a frozen one the caller holds.
    ///
    /// # Invariant
    ///
    /// A frozen node's payload belongs to its own module, so only this
    /// module's arrays are read here; a frozen *array* has no node in this
    /// graph and is answered `None`.
    ///
    /// # Safety
    ///
    /// The node must be live — this module's, or one of a frozen module that
    /// outlives the reference it was reached through, which is the same
    /// obligation every read of an `AnyNodeId` carries.
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

    /// The **value** half of a `[value, type]` pair node, or `None` when it
    /// is not a pair.
    ///
    /// # Invariant
    ///
    /// The width is `apply.rs`'s, not a guess: it resolves the apply's return
    /// pair with `items[1]` as the type slot for a 2-wide and a 3-wide
    /// `[value, type, perspective]` pair alike, so the value is element 0 in
    /// both. This is the one place that knows an encoding, and the pass in
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

    /// Whether `node`'s owner chain reaches `function` — the membership test
    /// the apply clone walk uses.
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
    /// not a pair.
    ///
    /// # Invariant
    ///
    /// The sibling of [`Self::pair_value_half`], reading element 1 by the same
    /// rule, so the two agree on what a pair is. It is public because the loop
    /// reader names a pair's type when it checks an entering call's argument
    /// type, and duplicating that decoding is what
    /// docs/notes/checker-encoding-instability.md is about.
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

    /// The `n` a node holds, when it holds a plain integer literal — frozen
    /// or not.
    pub fn usize_value(&self, node: impl Into<AnyNodeId>) -> Option<usize> {
        let value = self.node_value(node.into())?;
        match AsEnum::<LowValue>::as_enum(&value) {
            Some(LowValue::USize(n)) => Some(n as usize),
            _ => None,
        }
    }
}
