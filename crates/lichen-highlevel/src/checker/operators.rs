//! The operator rules: the binary operations and the two class conversions.

use lichen_lowlevel::LowShape;

use super::*;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A binary operation `a op b`.
    ///
    /// `Eq`/`Neq` are the generalized equality over any two *same-typed*
    /// values: their operands are unified with **each other**, and the result is
    /// the `0`/`1` scalar that drives an `if` (the language has no `Bool`).
    ///
    /// `+ - * /` and the four order comparisons compute over the language's two
    /// scalar classes, `Int` and `Float`, and never mix them: a concretely
    /// `Float` operand selects `Float` for the whole operation, so a
    /// cross-class expression (`1.5 + 1`) fails an operand unify and the
    /// diagnostic names the class it expected — a refusal, never a conversion
    /// (`docs/notes/floating-point.md` §4.2).  An operand whose class is not
    /// decided yet names neither, so the operation keeps its historical
    /// default — `Int` — and pins both operands there.
    ///
    /// `%` and the bitwise trio are `Int`-only: a float has no remainder and no
    /// bit pattern in this language, so a float operand is a check error for
    /// them and for them alone.
    ///
    /// `@in` is the one operator that is not arithmetic at all: it is set
    /// **membership**, so its operands are a value and a set of values rather
    /// than two members of one class, and it consults the set rather than a
    /// class (`docs/notes/operator-polymorphism.md` §3).  Its arm below is the
    /// only one that pins nothing to a class.
    ///
    /// An operand's type is unified against the class the operation computes
    /// over — a concretely wrong operand is a check error, and an unbound
    /// operand (a parameter) is *pinned* to that class, so a later apply at the
    /// other class is a runtime failure in the argument unify, not a panic
    /// inside the operator.
    ///
    /// The result's type is the operands' own class for the four arithmetic
    /// operators, and the machine scalar for every comparison: that is what
    /// makes `1.5 + 1.5` type as a `Float` and `1.5 < 2.5` drive an `if`.
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
        // Whether this operation computes over floats: a concretely `Float`
        // operand says so, and any other operand — an `Int`, a not-yet-decided
        // parameter, a non-scalar — leaves the operation on the `Int` default
        // it had before floats existed.
        let float = self.names_float_class(left_ty) || self.names_float_class(right_ty);
        // Whether *either* operand has stated a class yet.  When one has, the
        // operation computes over that class and the other operand is pinned to
        // it, exactly as before.  When neither has, the operation is
        // **polymorphic**: pinning both to `Int` here is what used to refuse
        // `add 1.5 2.5`, so the two cells are made one class instead and each
        // use commits the whole operation to a single class.  Nothing is lost
        // in the reported cases: a stated operand is what the diagnostics name,
        // and the undecided pair is precisely the case with no operand to name.
        let stated = self.class_is_stated(left_ty) || self.class_is_stated(right_ty);
        match operator {
            // The generalized equality: the operands must be the same type, so
            // a type value (`: Type`) can be compared with a type constant and
            // a cross-class comparison is the operand unify's refusal.
            BinOp::Eq | BinOp::Neq => {
                self.check_unify(left_ty, right_ty, self.loc(left, 1), DiagKind::BinOp)
            }
            // The `Int`-only operators: a float operand is refused here, by the
            // same unify that refuses every other non-`Int`.  Their domain is a
            // single class, so there is no polymorphism to keep open.
            BinOp::Rem | BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                self.check_unify(left_ty, self.int_type, self.loc(left, 1), DiagKind::BinOp);
                self.check_unify(right_ty, self.int_type, self.loc(right, 1), DiagKind::BinOp);
            }
            // `value @in set` — membership, the one binary operator that is not
            // arithmetic: it pins nothing to a class and it ties nothing to the
            // other operand's type, because the two sides are deliberately of
            // different shapes — the left is a value and the right is a **set**
            // of such values.
            //
            // The right operand is pinned to a fresh set type (the same
            // container pin `check_index` and `check_table_find` apply, with the
            // same `Guard` diagnostic: "this operand must be a set"), and that is
            // the whole check.  **The left operand is deliberately
            // unconstrained** — not even unified with the set's element cell —
            // and that is the load-bearing negative, not an omission.
            //
            // A membership test is a fact about a **value**, so it is answered
            // by evaluating it ([`crate::set::contains`]), never by reconciling
            // types ("the contract is a fact about values — 'this operand's value
            // is one of the numeric classes' — and a fact about a value is
            // checked by evaluating it, not by unifying types":
            // `docs/notes/operator-polymorphism.md` §2–§3).  Unifying the left
            // against the set's element cell *would* look like a friendlier
            // diagnostic and would break the very case this operator exists for:
            // `in_num = v => type_of v @in Num` must stay polymorphic, and
            // `type_of`'s result cell is the argument's own type cell, so the
            // unify would write the argument's class — refusing `in_num 1.5`
            // with "expected Int, found Float" after `in_num 1` had bound it.
            // The predicate must constrain nothing, exactly like a user-written
            // `v => v > 3`.
            //
            // What that costs is the *static* refusal of a mismatched left
            // (`5 @in Num` is accepted and answers `0`, since the value `5` is
            // not the type `Int`); what it buys is that the operator works where
            // a set is consulted about a value nobody has decided yet.
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
                if stated {
                    let scalar = if float {
                        self.float_type
                    } else {
                        self.int_type
                    };
                    self.check_unify(left_ty, scalar, self.loc(left, 1), DiagKind::BinOp);
                    self.check_unify(right_ty, scalar, self.loc(right, 1), DiagKind::BinOp);
                } else {
                    // The two operands are one class, so a use at either class
                    // is a use for the whole operation: `add 1 1.5` stays a
                    // refusal because `1` commits the shared cell first — the
                    // apply clone preserves the class, so the arguments of one
                    // application meet.
                    self.check_unify(right_ty, left_ty, self.loc(right, 1), DiagKind::BinOp);
                    // That shared class must lie in the operation's **domain**,
                    // and the check is a refinement rather than a unify: the
                    // condition `left_ty ∈ {Int, Float}` is registered as an
                    // assert, so it stays *pending* while the class is open and
                    // the apply clone re-checks it per call
                    // (`docs/notes/operator-polymorphism.md` §3).  Without it a
                    // non-numeric use is not refused at all — `run` answers the
                    // lazy marker for a class it cannot compute, so `add "a" "b"`
                    // would yield an undecided value.
                    //
                    // The domain is a **set's value**: the members themselves
                    // ([`crate::set`]), which is exactly what `Num = set{Int,
                    // Float}` lowers to, so the check reads the same graph a
                    // library-written contract will.  Its first member is the
                    // class a reader that must commit picks — `Int` here, the
                    // operators' historical default.
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
            }
        }
        // A comparison yields the `0`/`1` scalar whatever its operands are; the
        // four arithmetic operators yield the class they computed over — the
        // operands' own shared cell while it is still open.
        let result = match operator {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div if float => self.float_type,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div if !stated => left_ty,
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

    /// Whether a type slot has **stated a class** — anything the low type
    /// vocabulary can name, as opposed to an inference cell that has not bound
    /// yet.
    ///
    /// This is the decode [`crate::shape::low_type_of_slot`], the same authority
    /// the kernel boundary reads a parameter's domain with.  It is what tells
    /// `x + 1` (one operand stated, so `x` is pinned to `Int`) from `x + y`
    /// (neither stated, so the operation stays polymorphic).
    fn class_is_stated(&self, ty: NodeId) -> bool {
        crate::shape::low_type_of_slot(&self.module, AnyNodeId::Dynamic(ty)).is_known()
    }

    /// Whether an expression's type slot names the `Float` class — the one
    /// operand shape that selects float arithmetic and the float order
    /// comparisons.
    ///
    /// The decode is [`crate::shape::low_type_of_slot`], the same authority the
    /// kernel boundary reads a parameter's domain with: it answers
    /// [`LowShape::Float`] for a slot naming the float type — directly, or
    /// through the pair an annotated parameter's type cell holds — and
    /// [`LowShape::Unknown`] for a cell that has not bound yet.  So an
    /// undecided operand selects no class.
    fn names_float_class(&self, ty: NodeId) -> bool {
        crate::shape::low_type_of_slot(&self.module, AnyNodeId::Dynamic(ty)) == LowShape::Float
    }

    /// A prefix class conversion `int2float e` / `float2int e` — the one place
    /// the language's two scalar classes meet.
    ///
    /// The operand is checked against the direction's **source** class and the
    /// result's type is its **target**, so this is the only expression form
    /// whose type is not its operand's.  A wrong-class operand is the same
    /// refusal every other operator issues — the diagnostic names the class it
    /// expected, and nothing here silently converts
    /// (`docs/notes/floating-point.md` §4.2 owns the rule that no *other*
    /// construct crosses).
    ///
    /// **Only a class the operand already states is unified.**  A unify binds
    /// every cell the operand's class shares, and the operand's class may be one
    /// a kernel body's other cells read: pinning the index a float is written
    /// beside to `Int` here would refuse the very program these two words exist
    /// to write.  So an undecided operand stays undecided, and the value that
    /// arrives at the other class answers the lazy marker in
    /// [`crate::program::TypeOperator::run`] rather than a guess here — a weaker
    /// message than a parameter pinned at its apply, paid for by the conversion
    /// being usable where the classes are not yet decided.
    ///
    /// `float2int`'s partiality is not checked here: in range is a fact about
    /// the value, not about its type, so the interpreter records
    /// [`crate::program::OUT_OF_RANGE`] and answers the lazy marker.
    pub(super) fn check_convert(&mut self, e: ExprId, operator: ConvOp, value: ExprId) -> NodeId {
        self.check_expr(value);
        let (source, target) = match operator {
            ConvOp::Int2Float => (self.int_type, self.float_type),
            ConvOp::Float2Int => (self.float_type, self.int_type),
        };
        let operand_ty = self.state[value].ty.unwrap();
        if crate::shape::low_type_of_slot(&self.module, AnyNodeId::Dynamic(operand_ty)).is_known() {
            self.check_unify(operand_ty, source, self.loc(value, 1), DiagKind::Conv);
        }
        // A unary operator's operand array is the binary one with its second
        // slot absent — the shape [`crate::program::TypeOperator`]'s `run`
        // reads `operands[0]` from.
        let operand = self.value_of(value);
        let operands = self.array_node(self.current_block, &[operand]);
        let operator = P::Operator::from(TypeOperator::from(operator));
        let node = self.op_node(self.current_block, operator, Some(operands));
        let pair = self.pair_of(node, target);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(node);
        self.state[e].ty = Some(target);
        pair
    }
}
