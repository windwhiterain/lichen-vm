//! Which value does this node name? The graph facts a body consumer needs.
//! See docs/notes/lowlevel-vm.md.

use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

use crate::ArrayItem;
use crate::{
    AnyFunctionId, AnyNodeId, FunctionId, LowOperator, LowValue, Module, NodeId, Program,
    StaticFunctionRef,
};

/// What an `Index` node turned out to be. See [`Module::selection_of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// A `[then, else]` expr indexed by an undecided value: the language's
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
            // result_cell, callee_type]`; the last two are not codegen.
            Some(LowOperator::Apply) => self.operand_expr(operand, "Apply"),
            Some(LowOperator::Index) | Some(LowOperator::TableGet) => {
                self.operand_expr(operand, "operator")
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
    /// [`Self::operand_expr`] is the two-element reading; a four-element
    /// `[function, argument, result_cell, callee_type]` has to be readable whole
    /// for a caller that wants its later ones, and the two readings cannot
    /// disagree about what the array holds.
    pub fn operand_items(&self, operand: NodeId) -> Result<&'static [ArrayItem], String> {
        // SAFETY: `operand` is a live node of `self`; nothing here releases it.
        unsafe { self.array_items(operand) }.ok_or_else(|| "operand is not an array value".into())
    }

    /// The `[left, right]` of a two-element operand array, all of this module's.
    pub fn operand_expr(&self, operand: NodeId, what: &str) -> Result<Vec<NodeId>, String> {
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
                    // An element is a complete slot: its value is the position read.
                    AnyNodeId::Dynamic(_) => match self.element_value_node(item.node) {
                        Some(AnyNodeId::Dynamic(node)) => Ok(node),
                        _ => Err(
                            "a body reads a static reference into a frozen module, which has no                              node in this graph"
                                .into(),
                        ),
                    },
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

    /// Whether an apply's callee operand **states** its function; a type-side
    /// apply's callee is undecided by construction.
    fn apply_callee_is_stated(&self, apply: NodeId) -> bool {
        let Some(operation) = self.node_operation(apply) else {
            return false;
        };
        let Some(operand) = operation.operand else {
            return false;
        };
        let Ok(items) = self.operand_items(operand) else {
            return false;
        };
        let Some(callee) = items.first().and_then(|item| item.node.dynamic()) else {
            return false;
        };
        self.node_value(callee.into()).is_some() || self.node_operation(callee).is_some()
    }

    /// The member of `node`'s equality class that **defines** its value: a
    /// class member carrying a computational operator.
    ///
    /// # Invariant
    ///
    /// A `value_of` index extraction is not a definition — it is a view of a
    /// `[value, type]` expr. The deep pass collapses some values to a bare
    /// empty cell and unifies that cell with the defining computation, so the
    /// definition is reached through the class rather than an operand. The
    /// walk is the class's own member list, not the module's node table.
    pub fn defining_member(&self, node: NodeId) -> Option<NodeId> {
        let root = self.class_root(node);
        let members: Vec<NodeId> = disjoint::members(&self.nodes, root).collect();
        // A call defines the value it produces, and only a stated callee can
        // (`docs/notes/lowlevel-low-types.md`).
        for member in &members {
            let Some(operation) = self.node_operation(*member) else {
                continue;
            };
            if matches!(
                AsEnum::<LowOperator>::as_enum(&operation.operator),
                Some(LowOperator::Apply)
            ) && self.apply_callee_is_stated(*member)
            {
                return Some(*member);
            }
        }

        members.into_iter().find(|member| {
            self.node_operation(*member).is_some_and(|operation| {
                !matches!(
                    AsEnum::<LowOperator>::as_enum(&operation.operator),
                    Some(LowOperator::Index)
                )
            })
        })
    }

    /// The leaves of `function`'s **parameter domain** — the expr's value
    /// half, flattened one level.
    ///
    /// # Invariant
    ///
    /// The value half is decoded here and nowhere else, so every reader agrees:
    /// a caller that decoded it separately could decode it differently.
    pub fn parameter_leaves(&self, function: FunctionId) -> Result<Vec<NodeId>, String> {
        let parameter = self.functions[function].parameter;
        match self.expr_value_half(parameter) {
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
    /// handed in, so every reader decodes the expr the same way. Three
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
    /// `[value, type]` expr, a tuple domain's leaves, or a materialised
    /// array. An earlier draft inferred an expr from the **width**, which a
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
        if let Some(k) = self.read_position(index.node) {
            if let Some(element) = self.item_of(target.node, k) {
                return Some(Selection::Views(element));
            }
            // **A computed target at index 0** is a `value_of` peel: the
            // result *is* the expr's value, so it names the operator.
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
            Some(items) if items.len() >= 2 => Some(Selection::Computed),
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

    /// The **frozen function a callee names**, through the projection an
    /// imported binding arrives as.
    ///
    /// # Invariant
    /// The callee is `Index(<a frozen module's export array>, k)` at a constant
    /// `k`, and element `k` is a frozen function — a binding nothing evaluated
    /// still names the function it is (`docs/notes/loop-conversion.md` §8.5).
    pub fn static_function_of_callee(&self, callee: NodeId) -> Option<StaticFunctionRef> {
        let item = self.resolve_read(callee, 0)?;
        match item {
            // A frozen element: its value is read through the reference.
            AnyNodeId::Static(function) => match self
                .static_read(function)
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
            {
                Some(LowValue::Function(AnyFunctionId::Static(function))) => Some(function),
                _ => None,
            },
            // A materialized leaf of the same fact.
            AnyNodeId::Dynamic(function) => match self
                .node_value(AnyNodeId::Dynamic(function))
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
            {
                Some(LowValue::Function(AnyFunctionId::Static(function))) => Some(function),
                _ => None,
            },
        }
    }

    /// The node a read chain names (`docs/notes/compound-element-slots.md`).
    fn resolve_read(&self, node: NodeId, depth: u32) -> Option<AnyNodeId> {
        if depth > 8 {
            return None;
        }
        let Some(operation) = self.node_operation(node) else {
            return Some(AnyNodeId::Dynamic(node));
        };
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Index)
        ) {
            return Some(AnyNodeId::Dynamic(node));
        }
        let operands = self.operand_items(operation.operand?).ok()?;
        let position = self.usize_value(operands.get(1)?.node)?;
        let items = self.read_items(operands.first()?.node, depth + 1)?;
        Some(items.get(position)?.node)
    }

    /// An `Index`'s target items, frozen or stated.
    fn read_items(&self, target: AnyNodeId, depth: u32) -> Option<&'static [ArrayItem]> {
        if depth > 8 {
            return None;
        }
        match target {
            AnyNodeId::Static(sref) => match self.static_read(sref)?.as_enum()? {
                LowValue::Array(array) => Some(unsafe { array.items() }),
                _ => None,
            },
            AnyNodeId::Dynamic(node) => {
                if self.node_operation(node).is_some() {
                    let item = self.resolve_read(node, depth + 1)?;
                    return self.read_items(item, depth + 1);
                }
                // SAFETY: the node's stated array is the payload of a live node.
                unsafe { self.array_items_of(AnyNodeId::Dynamic(node)) }
            }
        }
    }

    /// The **value** half of a `[value, type]` expr node, or `None` when it
    /// is not an expr.
    ///
    /// # Invariant
    ///
    /// The width is `apply.rs`'s, not a guess: it resolves the apply's return
    /// expr with `items[1]` as the type slot for a 2-wide and a 3-wide
    /// `[value, type, perspective]` expr alike, so the value is element 0 in
    /// both. This is the one place that knows an encoding, and the pass in
    /// [`crate::low_type`] still learns no layout.
    pub fn expr_value_half(&self, node: NodeId) -> Option<NodeId> {
        // SAFETY: `node` is a live node of `self`.
        let items = unsafe { self.array_items(node) }?;
        if items.len() < 2 {
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
            owner = self
                .functions
                .get(current)
                .and_then(|function| function.parent);
        }
        Ok(false)
    }

    /// The **type** half of a `[value, type]` expr node, or `None` when it is
    /// not an expr.
    ///
    /// # Invariant
    ///
    /// The sibling of [`Self::expr_value_half`], reading element 1 by the same
    /// rule, so the two agree on what an expr is. It is public because the loop
    /// reader names an expr's type when it checks an entering call's argument
    /// type, and duplicating that decoding is what
    /// docs/notes/checker-encoding-instability.md is about.
    pub fn expr_type_half(&self, node: NodeId) -> Option<NodeId> {
        match self.expr_type_item(node)? {
            AnyNodeId::Dynamic(r#type) => Some(r#type),
            AnyNodeId::Static(_) => None,
        }
    }

    /// Whether references share an equality class; equal content is not enough
    /// across dynamic and frozen references.
    pub fn same_class(&self, a: AnyNodeId, b: AnyNodeId) -> bool {
        match (a, b) {
            (AnyNodeId::Dynamic(a), AnyNodeId::Dynamic(b)) => {
                self.class_root(a) == self.class_root(b)
            }
            (AnyNodeId::Static(a), AnyNodeId::Static(b)) if a.module == b.module => {
                let module = self.static_module(a.module);
                crate::static_module::static_find(&module.nodes, a.index)
                    == crate::static_module::static_find(&module.nodes, b.index)
            }
            _ => false,
        }
    }

    /// The **type** half of a `[value, type]` expr node, as the expr's own reference.
    pub fn expr_type_item(&self, node: NodeId) -> Option<AnyNodeId> {
        // SAFETY: `node` is a live node of `self`.
        let items = unsafe { self.array_items(node) }?;
        if items.len() < 2 {
            return None;
        }
        Some(items[1].node)
    }

    /// The **array items** of a ref: a dynamic node's payload, or a frozen node's.
    ///
    /// # Invariant
    /// A frozen payload is read where it lives; a leaf copy would hand its open
    /// positions refs nothing can write.
    ///
    /// # Safety
    /// As [`Self::array_items_of`]: the dynamic node must be live.
    pub unsafe fn array_items_ref(&self, node: AnyNodeId) -> Option<&'static [ArrayItem]> {
        match node {
            AnyNodeId::Dynamic(node) => unsafe { self.array_items(node) },
            AnyNodeId::Static(sref) => {
                let value = self.static_read(sref)?;
                let LowValue::Array(array) = value.as_enum()? else {
                    return None;
                };
                // SAFETY: the payload lives in the registered static module's arena.
                Some(unsafe { array.items() })
            }
        }
    }

    /// [`Self::expr_type_item`] over a ref.
    ///
    /// # Safety
    /// As [`Self::array_items_ref`]: a dynamic ref must be live.
    pub unsafe fn expr_type_item_ref(&self, node: AnyNodeId) -> Option<AnyNodeId> {
        // SAFETY: the caller's obligation.
        let items = unsafe { self.array_items_ref(node) }?;
        items.get(1).map(|item| item.node)
    }

    /// The value an element slot states: its first position, or the node itself.
    pub fn element_value_node(&self, node: AnyNodeId) -> Option<AnyNodeId> {
        let Some(value) = self.node_value(node) else {
            return Some(node);
        };
        match AsEnum::<LowValue>::as_enum(&value) {
            Some(LowValue::Array(array)) if array.layout() != 0 => {
                // SAFETY: `array` is the payload of a value of this module, whose
                // block has not been dropped.
                let items = unsafe { array.items() };
                items.first().map(|item| item.node)
            }
            _ => Some(node),
        }
    }

    /// State the program's **full-width layout**.
    ///
    /// # Invariant
    /// One layout per program, because the attribute order is the program's
    /// (`docs/notes/compound-element-slots.md`).
    pub fn record_full_width_layout(&mut self, layout: u32) {
        self.full_width_layout.get_or_insert(layout);
    }

    /// The program's full-width layout, once a compound has stated it.
    pub fn full_width_layout(&self) -> Option<u32> {
        self.full_width_layout
    }

    /// State the **type node** of an element's value.
    ///
    /// # Invariant
    /// The type node is the element slot's second position
    /// (`docs/notes/compound-element-slots.md`), so the value node has a shape authority
    /// without a class shape being copied onto it; one statement per value node.
    pub fn record_element_type(&mut self, value: NodeId, r#type: AnyNodeId) {
        self.element_types.entry(value).or_insert(r#type);
    }

    /// The type node an element value states, read by node and then by class.
    ///
    /// # Invariant
    /// A class holds one value, so a merged value node answers its class's statement;
    /// a node that states none answers [`None`].
    pub fn element_type_of(&self, node: NodeId) -> Option<AnyNodeId> {
        let mut cursor = node;
        // A read is defined by its template node, so the statement is read there too.
        for _ in 0..ELEMENT_TYPE_DEPTH {
            if let Some(r#type) = self
                .element_types
                .get(&cursor)
                .or_else(|| self.element_types.get(&self.class_root(cursor)))
            {
                return Some(*r#type);
            }
            let Some(origin) = self.node_origin(cursor) else {
                return None;
            };
            cursor = origin;
        }
        None
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

/// The deepest origin chain an element type is looked up along.
const ELEMENT_TYPE_DEPTH: usize = 8;
