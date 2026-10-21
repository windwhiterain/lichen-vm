//! Tuple terms, tuple type expressions, and the type-position helper.

use super::*;
use crate::shape;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    pub(super) fn check_tuple_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut vals = Vec::new();
        let mut slots = Vec::new();
        for &el in &elements {
            self.check_expr(el);
            // An element is a complete slot: the term is the value pair.
            vals.push(self.state[el].term.expect("an element is compiled"));
            let ty = self.state[el].ty;
            let attrs = self.element_attr_slots(el);
            slots.push(self.element_slot(None, ty, Some(attrs)));
        }
        // A tuple: `[values, [[slot…], [TupleType, Type]]]`.
        let value = self.array_node(self.current_block, &vals);
        let shape = self.array_node(self.current_block, &slots);
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let ty_node = self.slot_pair(self.current_block, shape, kind);
        let pair = self.pair_of(value, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// A tuple type expression: each element's slot is a full term.
    pub(super) fn check_tuple_type(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut slots = Vec::new();
        for &el in &elements {
            slots.push(self.element_slot_of(el));
        }
        let shape = self.array_node(self.current_block, &slots);
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let pair = self.slot_pair(self.current_block, shape, kind);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(shape);
        self.state[e].ty = Some(kind);
        pair
    }

    /// An element's slot, read from the expression that writes it.
    ///
    /// # Invariant
    /// A top-level `:` spells the value position; an element without one is a type
    /// expression whose value position is the wildcard. Every compound
    /// construction site reads its shape elements through this one rule, so a
    /// struct field and a tuple element state the same thing.
    /// See docs/notes/compound-element-slots.md.
    pub(super) fn element_slot_of(&mut self, el: ExprId) -> NodeId {
        self.check_expr(el);
        let (value, type_expr) = match self.ir[el].kind {
            ExprKind::Annotation {
                value,
                r#type: Some(type_expr),
                ..
            } => (Some(value), type_expr),
            _ => (None, el),
        };
        let denotation = self.type_denotation(type_expr, None);
        // A slot's attributes are the **element's**: `5 : Int # 2` is one
        // annotation, so the type child holds none of them.
        let attrs = self.element_attr_slots(el);
        let value = value.map(|value| self.value_of(value));
        self.element_slot(value, Some(denotation), Some(attrs))
    }

    /// A compound element's slot: `[value, type, attrs…]`, the same width for
    /// every element of every compound kind.
    ///
    /// # Invariant
    /// The width is [`shape::slot_width`] whatever an element spells, so a
    /// written type's slots and a value's unify whatever either one writes; an
    /// unconstrained position is the wildcard, which neither binds nor
    /// propagates.
    pub(super) fn element_slot(
        &mut self,
        value: Option<NodeId>,
        ty: Option<NodeId>,
        attrs: Option<Vec<NodeId>>,
    ) -> NodeId {
        let any = self.any();
        let mut items: Vec<NodeId> = Vec::with_capacity(shape::slot_width::<P>());
        items.push(value.unwrap_or(any));
        items.push(ty.unwrap_or_else(|| self.fresh_cell()));
        match attrs {
            Some(attrs) => items.extend(attrs),
            None => items.extend(std::iter::repeat_n(any, P::Attr::ORDER.len())),
        }
        let layout = self.full_width_layout();
        // The second position is the value's shape authority
        // (`docs/notes/compound-element-slots.md`).
        if let (Some(value), Some(ty)) = (value, ty)
            && value != ty
        {
            self.module
                .record_element_type(value, AnyNodeId::Dynamic(ty));
        }
        self.array_node_with_layout(self.current_block, &items, layout)
    }

    /// An element expression's **dense** attribute positions: its own slots where
    /// spelled, the wildcard elsewhere.
    ///
    /// # Invariant
    /// The term's tail is in the canonical order, so tail slot `i` sits at
    /// [`shape::attr_slot`]`(i)` of the term and belongs at its marker's
    /// `order_index`.
    pub(super) fn element_attr_slots(&mut self, el: ExprId) -> Vec<NodeId> {
        let any = self.any();
        let mut slots = vec![any; P::Attr::ORDER.len()];
        let tail = self.schema_tail(el).to_vec();
        if tail.is_empty() {
            return slots;
        }
        let Some(term) = self.state[el].term else {
            return slots;
        };
        for marker in &tail {
            if let Some(AnyNodeId::Dynamic(node)) =
                self.slot_attribute(term, marker.order_index() as u32)
            {
                slots[marker.order_index()] = node;
            }
        }
        slots
    }

    /// A compound shape's **element types**, in order: each slot's type position,
    /// as a fresh array.
    ///
    /// # Invariant
    /// A shape element is a slot, so a positional field-type list is each slot's
    /// type. An undecided shape has no items to read, so the list is built lazily
    /// — one `Index(Index(shape, i), 1)` per position, `arity` of them — and a
    /// static ref is materialized into the current block.
    pub(super) fn slot_types_of(&mut self, shape: NodeId, arity: usize) -> NodeId {
        // SAFETY: `shape` is a live node of this module; nothing in this crate
        // calls `Module::drop_block`.
        let nodes: Vec<AnyNodeId> = unsafe { self.module.array_items(shape) }
            .map(|items| items.iter().map(|item| item.node).collect())
            .unwrap_or_default();
        if nodes.is_empty() {
            let mut tys = Vec::with_capacity(arity);
            for i in 0..arity {
                let index = self.usize_node(i);
                let ops = self.array_node(self.current_block, &[shape, index]);
                let element = self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::Index),
                    Some(ops),
                );
                let type_ops = self.array_node(self.current_block, &[element, self.one()]);
                let ty = self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::Index),
                    Some(type_ops),
                );
                tys.push(ty);
            }
            return self.array_node(self.current_block, &tys);
        }
        let mut tys = Vec::with_capacity(nodes.len());
        for node in nodes {
            let ty = shape::slot_type(&self.module, node).unwrap_or(node);
            tys.push(self.module.as_dynamic(ty, self.current_block));
        }
        self.array_node(self.current_block, &tys)
    }

    /// The type an expression contributes in a type position — a struct field, a
    /// tuple-type element, a function-type side.
    ///
    /// # Invariant
    /// A term put in a type position unifies as its own value pair, which never
    /// unifies with its type: `struct{.f Int, .g b}` fails, `struct{.f Int, .g B}`
    /// works. An expression that carries attributes denotes the annotated value's
    /// term instead; see `docs/notes/operator-polymorphism.md` §3.
    pub(super) fn check_type_element(&mut self, el: ExprId) -> NodeId {
        self.check_expr(el);
        self.type_denotation(el, None)
    }
}
