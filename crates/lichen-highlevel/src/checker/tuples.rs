//! Tuple terms and the element slot algebra the set and the table types use.

use super::*;
use crate::shape;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A tuple value `(e1, …, en)`.
    ///
    /// # Invariant
    /// The value is the elements' **own exprs** and the type is the shared
    /// `Tuple` template: the layout lives in the value, so a read takes an
    /// element's type and attributes from the element itself.
    pub(super) fn check_tuple_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut vals = Vec::with_capacity(elements.len());
        for &el in &elements {
            self.check_expr(el);
            // An element is a complete slot: the term is the value expr.
            vals.push(self.state[el].term.expect("an element is compiled"));
        }
        let value = self.array_node(self.current_block, &vals);
        let ty_node = self.tuple_template();
        let expr = self.expr_of(value, ty_node);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        expr
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
                self.slot_attribute(AnyNodeId::Dynamic(term), marker.order_index() as u32)
            {
                slots[marker.order_index()] = node;
            }
        }
        slots
    }
}
