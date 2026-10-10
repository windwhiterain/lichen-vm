//! Tuple terms, tuple type expressions, and the type-position helper.

use super::*;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    pub(super) fn check_tuple_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut vals = Vec::new();
        let mut tys = Vec::new();
        for &el in &elements {
            self.check_expr(el);
            vals.push(self.value_of(el));
            tys.push(self.state[el].ty.unwrap());
        }
        // A tuple: `[values, [[element types], [TupleType, Type]]]`.
        let value = self.array_node(self.current_block, &vals);
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// A tuple type expression: `[[element types], [TupleType, Type]]`.
    pub(super) fn check_tuple_type(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut tys = Vec::new();
        for &el in &elements {
            tys.push(self.check_type_element(el));
        }
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(shape);
        self.state[e].ty = Some(kind);
        pair
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
