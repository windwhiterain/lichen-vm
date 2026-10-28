//! Template unification and dependent-shape analysis.

use std::collections::HashSet;

use lichen_lowlevel::{AnyNodeId, NodeId};

use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind};

use super::Checker;
use crate::program::{HighProgram, TypeOperator, ValueType};

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<lichen_lowlevel::LowOperator> + From<TypeOperator>,
{
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
        // Constrain a function's signature, preserving its body; other templates
        // constrain the whole value layout.
        let actual_value = self.value_of(actual);
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
            if let Some(function) = self.module.function_apply_function(expected_term) {
                self.module.requirements.insert(actual_value, function);
                if let Some(value) = self.module.expr_value_half(actual_term) {
                    self.module.requirements.entry(value).or_insert(function);
                }
            }
            self.state[e].term = Some(actual_term);
            self.state[e].val = self.state[actual].val;
            self.state[e].ty = self.state[actual].ty;
            return actual_term;
        }

        self.compute_operands(actual_term, expected_term);
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
        self.state[e].val = self.state[actual].val;
        self.state[e].ty = self.state[actual].ty;
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
            if matches!(self.ir[current].kind, ExprKind::Parameter)
                && self.current_parameters.contains(&current)
            {
                found.push(current);
            }
            stack.extend(self.ir.children(current));
        }
        found
    }
}
