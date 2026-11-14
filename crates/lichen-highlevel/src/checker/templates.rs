//! Template unification and dependent-shape analysis.

use std::collections::HashSet;

use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, NodeId};
use lichen_utils::extend::AsEnum;

use crate::attr::AttrSet;
use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind};

use super::Checker;
use crate::program::{HighProgram, TypeOperator, ValueType};

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<lichen_lowlevel::LowOperator> + From<TypeOperator>,
{
    pub(super) fn is_compound_type(&mut self, ty: NodeId) -> bool {
        self.is_struct_layout(ty)
            || crate::shape::is_array_type_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(ty),
            )
            || crate::shape::is_tuple_type_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(ty),
            )
            || crate::shape::is_table_type_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(ty),
            )
            || crate::shape::is_set_type_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(ty),
            )
    }

    /// `a = b` — the unify of the **actual** with the **expected**.
    ///
    /// # Invariant
    /// A non-function template unifies whole terms, so a value is matched
    /// against its layout. A function template unifies the actual **function**
    /// with the signature, preserving the actual body for calls.
    /// The direction is fixed: `actual` is found and `expected` the requirement.
    /// The `:` annotation does **not** come through here: it states a type, and a type is unified as a type.
    pub(super) fn check_unify_expr(
        &mut self,
        e: ExprId,
        actual: ExprId,
        expected: ExprId,
    ) -> NodeId {
        self.check_expr(actual);
        self.check_expr(expected);
        let actual_term = self.state[actual].term.expect("the actual is compiled");
        let expected_term = self.state[expected].term.expect("the expected is compiled");
        let actual_ty = self.state[actual].ty.expect("the actual has a type");
        let expected_ty = self.state[expected].ty.expect("the expected has a type");
        // A formal's unify reads its slot identity directly; other values keep
        // the lazy read required by the lowering contract.
        let direct = self.formal_actual(actual);
        let actual_value = if direct {
            self.module
                .expr_value_half(actual_term)
                .expect("a formal's expr has a value half")
        } else {
            self.value_of(actual)
        };
        self.record_template_field_sources(expected, actual, actual_value);
        // A template against an undecided actual: the template is the cell's
        // **requirement** — a link, never a write.
        if self.is_function_type(expected_ty) && !self.actual_value_is_decided(actual_value) {
            self.check_unify(
                actual_ty,
                expected_ty,
                self.loc(actual, 0),
                DiagKind::Annotation,
            );
            self.state[e].term = Some(actual_term);
            self.state[e].val = direct.then_some(actual_value).or(self.state[actual].val);
            self.state[e].ty = self.state[actual].ty;
            return actual_term;
        }

        self.compute_operands(actual_term, expected_term);
        // A statement states the actual's attributes, so a call site reads them.
        self.adopt_stated_attributes(actual_term, expected_term);
        // A known function is unified through its expr, so the constraint
        // compares two signatures.
        let found = if self.is_function_type(expected_ty)
            && self.actual_value_is_decided(actual_value)
            && self.module.function_expr_function(actual_term).is_none()
        {
            actual_ty
        } else {
            actual_term
        };
        self.check_unify(
            found,
            expected_term,
            self.loc(actual, 0),
            DiagKind::Annotation,
        );
        // The unify's own value is the actual's: the expected side states what
        // the actual must be, not what it is.
        self.state[e].term = Some(actual_term);
        self.state[e].val = direct.then_some(actual_value).or(self.state[actual].val);
        self.state[e].ty = self.state[actual].ty;
        // A statement states the actual attribute: its annotated side names it.
        if self.state[actual].attr.is_none()
            && let Some(stated) = self.state[expected].attr
        {
            self.state[e].attr = Some(stated);
            if let Some(marker) = self.attribute_markers.get(&expected).copied() {
                self.attribute_markers.insert(e, marker);
                // The function whose body is being checked claims it, whatever shape
                // the block takes (`Checker::body_claims`).
                if let Some(function) = self.current_function() {
                    self.body_claims
                        .insert(function, (marker.order_index() as u32, stated));
                }
            }
        }
        actual_term
    }

    /// Whether an expression's value is already decided.
    pub(super) fn actual_value_is_decided(&self, value: NodeId) -> bool {
        self.module.node_value(AnyNodeId::Dynamic(value)).is_some()
    }

    /// The nearest binding of a parameter, with its term replaced.
    ///
    /// # Invariant
    /// The **nearest** scope wins: a shadowing parameter's own binding is the
    /// one a use reads. The function's parameter record is not touched, so the
    /// argument is still bound before the body runs.
    fn update_binding_term(&mut self, parameter: ExprId, term: NodeId) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(binding) = scope.get_mut(&parameter) {
                binding.term = term;
                return;
            }
        }
    }

    /// Whether the template's **length** depends on an enclosing lambda
    /// parameter.
    ///
    /// # Invariant
    /// Only an enclosing lambda's parameter counts: a `let` binding is the
    /// template's own cell, and a hole is nothing at all. A template that is
    /// not an array template states no length of its own, so its length is its
    /// value's own and depends on the parameter through the template's own
    /// graph.
    pub(super) fn length_depends_on_parameter(&self, template: ExprId) -> bool {
        match self.ir[template].kind {
            ExprKind::Array { length, .. } => !self.reachable_parameters(length).is_empty(),
            _ => !self.reachable_parameters(template).is_empty(),
        }
    }

    /// The lambda parameters the operand reaches.
    ///
    /// # Invariant
    /// Only the **enclosing** lambda's parameters are candidates: a parameter
    /// of a lambda *inside* the operand is that lambda's own cell and is cloned
    /// with it.
    pub(super) fn reachable_parameters(&self, value: ExprId) -> Vec<ExprId> {
        let mut seen: HashSet<ExprId> = HashSet::new();
        let mut found = Vec::new();
        let mut stack = vec![value];
        while let Some(current) = stack.pop() {
            if !seen.insert(current) {
                continue;
            }
            if matches!(self.ir[current].kind, ExprKind::Formal)
                && self
                    .current_parameters
                    .iter()
                    .any(|formals| formals.contains(&current))
            {
                found.push(current);
            }
            stack.extend(self.ir.children(current));
        }
        found
    }
}
