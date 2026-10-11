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
            vals.push(self.value_of(el));
            let ty = self.state[el].ty;
            slots.push(self.element_slot(ty, None));
        }
        // A tuple: `[values, [[slot…], [TupleType, Type]]]`.
        let value = self.array_node(self.current_block, &vals);
        let shape = self.array_node(self.current_block, &slots);
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
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
            self.check_expr(el);
            let denotation = self.type_denotation(el, None);
            let attrs = self.element_attr_slots(el);
            slots.push(self.element_slot(Some(denotation), Some(attrs)));
        }
        let shape = self.array_node(self.current_block, &slots);
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(shape);
        self.state[e].ty = Some(kind);
        pair
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
        ty: Option<NodeId>,
        attrs: Option<Vec<NodeId>>,
    ) -> NodeId {
        let any = self.any();
        let mut items: Vec<NodeId> = Vec::with_capacity(shape::slot_width::<P>());
        items.push(any);
        items.push(ty.unwrap_or_else(|| self.fresh_cell()));
        match attrs {
            Some(attrs) => items.extend(attrs),
            None => items.extend(std::iter::repeat_n(any, P::Attr::ORDER.len())),
        }
        let slot = self.array_node(self.current_block, &items);
        // A concrete slot is what a clone references in place.
        self.module.evaluate_node_deep(slot, None);
        slot
    }

    /// An element expression's **dense** attribute positions: its own slots where
    /// spelled, the wildcard elsewhere.
    ///
    /// # Invariant
    /// The term's tail is in the canonical order, so tail slot `i` sits at
    /// [`shape::attr_slot`]`(i)` of the term and belongs at its marker's
    /// `order_index`.
    fn element_attr_slots(&mut self, el: ExprId) -> Vec<NodeId> {
        let any = self.any();
        let mut slots = vec![any; P::Attr::ORDER.len()];
        let tail = self.schema_tail(el).to_vec();
        if tail.is_empty() {
            return slots;
        }
        let Some(term) = self.state[el].term else {
            return slots;
        };
        // SAFETY: `term` is a live node of this module; nothing in this crate
        // calls `Module::drop_block`.
        let Some(items) = (unsafe { shape::array_items(&self.module, AnyNodeId::Dynamic(term)) })
        else {
            return slots;
        };
        for (i, marker) in tail.iter().enumerate() {
            if let Some(item) = items.get(shape::attr_slot(i))
                && let AnyNodeId::Dynamic(node) = item.node
            {
                slots[marker.order_index()] = node;
            }
        }
        slots
    }

    /// A tuple shape's **element types**, in order: each element slot's type
    /// position, as a fresh shape array.
    ///
    /// # Invariant
    /// The shape is a tuple type's own slot list, so every element is a slot; a
    /// static ref is materialized into the current block, as the caller's own
    /// shape handling does.
    pub(super) fn tuple_element_types(&mut self, shape: NodeId) -> NodeId {
        // SAFETY: `shape` is a live node of this module; nothing in this crate
        // calls `Module::drop_block`.
        let nodes: Vec<AnyNodeId> = unsafe { self.module.array_items(shape) }
            .map(|items| items.iter().map(|item| item.node).collect())
            .unwrap_or_default();
        if nodes.is_empty() {
            return shape;
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
    /// unifies with its type: `struct<.f Int, .g b>` fails, `struct<.f Int, .g B>`
    /// works. An expression that carries attributes denotes the annotated value's
    /// term instead; see `docs/notes/operator-polymorphism.md` §3.
    pub(super) fn check_type_element(&mut self, el: ExprId) -> NodeId {
        self.check_expr(el);
        self.type_denotation(el, None)
    }
}
