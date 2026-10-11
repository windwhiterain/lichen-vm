//! The operator rules: the binary operations and the two class conversions.

use lichen_lowlevel::LowShape;

use super::*;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A binary operation `a op b`; see `docs/notes/operator-polymorphism.md` §3
    /// and `docs/notes/floating-point.md` §4.2.
    ///
    /// # Invariant
    /// `Eq`/`Neq` unify the operand types with each other and answer the `0`/`1`
    /// scalar, as do comparisons; `%` and the bitwise trio are `Int`-only. The
    /// scalar operators never mix classes: a concrete `Float` operand selects
    /// `Float` for the whole operation, while two unstated operands share one
    /// open class, gated by a refinement. `@in` pins nothing to a class.
    pub(super) fn check_binop(
        &mut self,
        e: ExprId,
        operator: BinOp,
        left: ExprId,
        right: ExprId,
    ) -> NodeId {
        self.check_expr(left);
        self.check_expr(right);
        let left_ty = self.state[left].ty.unwrap();
        let right_ty = self.state[right].ty.unwrap();
        let mut arithmetic = None;
        match operator {
            // Generalized equality: the operands must be the same type.
            BinOp::Eq | BinOp::Neq => {
                self.check_unify(left_ty, right_ty, self.loc(left, 1), DiagKind::BinOp);
            }
            // The `Int`-only operators: a float operand is refused here.
            BinOp::Rem | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                self.check_unify(left_ty, self.int_type, self.loc(left, 1), DiagKind::BinOp);
                self.check_unify(right_ty, self.int_type, self.loc(right, 1), DiagKind::BinOp);
            }
            // `value @in set` — membership: the right operand is pinned to a fresh
            // set type, and that is the whole check.

            // Invariant: the left operand is unconstrained, not even with the
            // set's element cell (operator-polymorphism.md §2-§3).
            BinOp::In => {
                let element_cell = self.fresh_cell();
                let shape = self.array_node(self.current_block, &[element_cell]);
                let kind = self.kind_expr(self.current_block, self.markers.set_type_marker);
                let set_ty = self.array_node(self.current_block, &[shape, kind]);
                self.check_unify(right_ty, set_ty, self.loc(right, 1), DiagKind::Guard);
            }
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::Lt
            | BinOp::Gt
            | BinOp::Leq
            | BinOp::Geq => {
                arithmetic = Some(self.arithmetic_class(e, left, left_ty, right, right_ty));
            }
        }
        // A comparison yields the `0`/`1` scalar; arithmetic the class it computed
        // over — the shared cell while it is open.
        let result = match operator {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                arithmetic.expect("the arithmetic arm ran")
            }
            _ => self.int_type,
        };
        let operator = P::Operator::from(TypeOperator::from(operator));
        let left = self.value_of(left);
        let right = self.value_of(right);
        let operands = self.array_node(self.current_block, &[left, right]);
        let value = self.op_node(self.current_block, operator, Some(operands));
        let pair = self.pair_of(value, result);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(result);
        pair
    }

    /// The class two arithmetic operands share, and the assert that keeps it a numeric one.
    ///
    /// # Invariant
    /// **One rule for `+ - * /` and for `max`/`min`**: a concrete `Float` operand selects
    /// `Float` for the whole operation, two stated operands must agree, and two unstated ones
    /// share one open class gated by a refinement — so `max 1 1.5` is refused by the same tie
    /// that refuses `1 + 1.5`, and a `Float` maximum is a `Float` rather than a `select`.
    fn arithmetic_class(
        &mut self,
        e: ExprId,
        left: ExprId,
        left_ty: NodeId,
        right: ExprId,
        right_ty: NodeId,
    ) -> NodeId {
        let float = self.names_float_class(left_ty) || self.names_float_class(right_ty);
        // Whether *either* operand has stated a class. When neither has, the two
        // cells become one shared class.
        let stated = self.class_is_stated(left_ty) || self.class_is_stated(right_ty);
        if stated {
            let scalar = if float {
                self.float_type
            } else {
                self.int_type
            };
            self.check_unify(left_ty, scalar, self.loc(left, 1), DiagKind::BinOp);
            self.check_unify(right_ty, scalar, self.loc(right, 1), DiagKind::BinOp);
        } else {
            // The two operands are one class: a use at either class
            // commits the whole operation (`add 1 1.5` stays refused).
            self.check_unify(right_ty, left_ty, self.loc(right, 1), DiagKind::BinOp);
            // Invariant: the shared class must lie in the domain; a
            // refinement, so it stays pending while the class is open.

            // Without the assert a non-numeric use is not refused: `run` answers
            // the lazy marker for a class it cannot compute.

            // The domain is a set's value — the members themselves — so the check
            // reads the same graph a library contract will.
            let (int_type, float_type) = (self.int_type, self.float_type);
            let domain = self.array_node(self.current_block, &[int_type, float_type]);
            let operands = self.array_node(self.current_block, &[left_ty, domain]);
            let condition = self.op_node(
                self.current_block,
                P::Operator::from(TypeOperator::InDomain),
                Some(operands),
            );
            self.register_assert(
                condition,
                self.loc(e, 1),
                true,
                AssertSpelling::Refinement { domain },
            );
        }
        if float {
            self.float_type
        } else if !stated {
            left_ty
        } else {
            self.int_type
        }
    }

    /// Whether a type slot has stated a class rather than an unbound cell.
    ///
    /// # Invariant
    /// The decode is [`crate::shape::low_type_of_slot`], the same authority the
    /// kernel boundary reads a parameter's domain with. It is what tells `x + 1`
    /// (one operand stated, so `x` is pinned to `Int`) from `x + y` (neither
    /// stated, so the operation stays polymorphic).
    fn class_is_stated(&self, ty: NodeId) -> bool {
        crate::shape::low_type_of_slot(&self.module, AnyNodeId::Dynamic(ty)).is_known()
    }

    /// Whether an expression's type slot names the `Float` class.
    ///
    /// # Invariant
    /// The decode is [`crate::shape::low_type_of_slot`]: it answers
    /// [`LowShape::Float`] for a slot naming the float type — directly, or
    /// through the pair an annotated parameter's type cell holds — and
    /// [`LowShape::Unknown`] for a cell that has not bound. So an undecided
    /// operand selects no class.
    fn names_float_class(&self, ty: NodeId) -> bool {
        crate::shape::low_type_of_slot(&self.module, AnyNodeId::Dynamic(ty)) == LowShape::Float
    }

    /// A prefix keyword operator — the two class conversions, `max`/`min` and `exp`.
    ///
    /// # Invariant
    /// One entry point for the whole prefix family, so the operand array's shape and the
    /// result class are stated once. A **conversion** unifies only a class the operand
    /// already states, since a unify binds every cell that class shares; `max`/`min` tie
    /// their operands by [`Self::arithmetic_class`]; `exp` is `Float`'s alone and pins its
    /// operand, having no other class to be.
    pub(super) fn check_prefix(
        &mut self,
        e: ExprId,
        operator: PrefixOp,
        operands: ChildRange,
    ) -> NodeId {
        let ids: Vec<ExprId> =
            self.ir.children[operands.start as usize..operands.end as usize].to_vec();
        for &operand in &ids {
            self.check_expr(operand);
        }
        let types: Vec<NodeId> = ids.iter().map(|&id| self.state[id].ty.unwrap()).collect();
        let (result, operator) = match operator {
            PrefixOp::Int2Float => {
                self.unify_stated_class(ids[0], types[0], self.int_type);
                (self.float_type, TypeOperator::Int2Float)
            }
            PrefixOp::Float2Int => {
                self.unify_stated_class(ids[0], types[0], self.float_type);
                (self.int_type, TypeOperator::Float2Int)
            }
            PrefixOp::Max | PrefixOp::Min => (
                self.arithmetic_class(e, ids[0], types[0], ids[1], types[1]),
                TypeOperator::from(operator),
            ),
            PrefixOp::Exp => {
                self.check_unify(
                    types[0],
                    self.float_type,
                    self.loc(ids[0], 1),
                    DiagKind::BinOp,
                );
                (self.float_type, TypeOperator::Exp)
            }
        };
        // The operand array is the source's own list, in the order it was written.
        let values: Vec<NodeId> = ids.iter().map(|&id| self.value_of(id)).collect();
        let operands = self.array_node(self.current_block, &values);
        let node = self.op_node(
            self.current_block,
            P::Operator::from(operator),
            Some(operands),
        );
        let pair = self.pair_of(node, result);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(node);
        self.state[e].ty = Some(result);
        pair
    }

    /// Unify a slot against `class` **only when the slot already states a class**.
    ///
    /// # Invariant
    /// A conversion's own rule: a unify binds every cell the operand's class shares, and an
    /// undecided operand's cell may be one a kernel body's other positions read, so pinning
    /// it here would refuse the program those positions describe.
    fn unify_stated_class(&mut self, operand: ExprId, ty: NodeId, class: NodeId) {
        if crate::shape::low_type_of_slot(&self.module, AnyNodeId::Dynamic(ty)).is_known() {
            self.check_unify(ty, class, self.loc(operand, 1), DiagKind::Conv);
        }
    }
}
