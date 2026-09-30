//! The binary-operator rule.

use super::*;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A binary integer operation `a op b`: both operands must be `Int`, and
    /// the result is `Int` (a comparison yields `0/1` to drive an `if`'s
    /// lazy `Index` branch).  Each operand's type is unified against the int
    /// type expression — a concretely non-`Int` operand is a check error,
    /// and an unbound operand (a parameter) is *pinned* to `Int`, so a
    /// later apply at a non-`Int` argument is a runtime failure in the
    /// argument unify, not a panic inside the operator.
    pub(super) fn check_binop(
        &mut self,
        e: ExprId,
        operator: BinOp,
        left: ExprId,
        right: ExprId,
    ) -> NodeId {
        self.check_expr(left);
        self.check_expr(right);
        match operator {
            // `==` compares two *same-typed* values and yields 0/1: the Int
            // equalities (`s.a == 1`, `x == y`) and the type-value equalities
            // (`S::a == Int`) — the operands' types must be equal, so a type
            // value (`: Type`) can be compared with a type constant.
            BinOp::Eq => self.check_unify(
                self.state[left].ty.unwrap(),
                self.state[right].ty.unwrap(),
                self.loc(left, 1),
                DiagKind::BinOp,
            ),
            BinOp::Add | BinOp::Sub | BinOp::Leq => {
                self.check_unify(
                    self.state[left].ty.unwrap(),
                    self.int_type,
                    self.loc(left, 1),
                    DiagKind::BinOp,
                );
                self.check_unify(
                    self.state[right].ty.unwrap(),
                    self.int_type,
                    self.loc(right, 1),
                    DiagKind::BinOp,
                );
            }
        }
        let operator = P::Operator::from(TypeOperator::from(operator));
        let left = self.value_of(left);
        let right = self.value_of(right);
        let operands = self.array_node(self.current_block, &[left, right]);
        let value = self.op_node(self.current_block, operator, Some(operands));
        let pair = self.pair_of(value, self.int_type);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(self.int_type);
        pair
    }
}
