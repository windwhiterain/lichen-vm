//! The template operations: the clone and the unify.
//!
//! # Invariant
//! A clone is a **new instance**: the template's own holes stay open in it, so
//! a unify against the clone writes into the clone and never into the
//! template. The boundary is what the two sides share, and it is stated in
//! [`Checker::clone_boundary`].

use std::collections::HashSet;

use lichen_lowlevel::{NodeId, TemplateBoundary, TemplateInstance};

use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind};

use super::Checker;
use crate::program::{HighProgram, TypeOperator, ValueType};

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<lichen_lowlevel::LowOperator> + From<TypeOperator>,
{
    /// `@clone e` — a fresh instantiation of `e`'s value graph.
    ///
    /// # Invariant
    /// Every call builds its **own** instance: two clones of one template share
    /// nothing but the cells the boundary names, so a hole filled through one
    /// clone is not filled in the other.
    pub(super) fn check_clone(&mut self, e: ExprId, value: ExprId) -> NodeId {
        self.check_expr(value);
        let Some(instance) = self.clone_instance(value) else {
            // A budget stopped the walk: a partial instance is worse than none.
            let term = self.state[value].term.unwrap_or(self.type_expr);
            self.record_guard(term, term, self.loc(value, 0), DiagKind::Guard, None);
            return self.refused_expr(e);
        };
        let term = instance.root;
        let val = self.state[value].val.map(|node| instance.node_of(node));
        let ty = self.state[value].ty.map(|node| instance.node_of(node));
        self.state[e].term = Some(term);
        self.state[e].val = val;
        self.state[e].ty = ty;
        term
    }

    /// `a = b` — the unify of the **actual** with the **expected**.
    ///
    /// # Invariant
    /// A non-function template unifies whole terms, so a value is matched
    /// against its layout. A function template unifies the actual function's
    /// type with the cloned signature, preserving the actual body for calls.
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
        self.compute_operands(actual_term, expected_term);
        // Constrain a function's signature, preserving its body; other templates
        // constrain the whole value layout.
        let found = if self.module.is_function_type(expected_ty) {
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

    /// The instance of `value`'s value graph, or `None` when a budget stopped
    /// the walk.
    ///
    /// # Invariant
    /// The instance is **fresh**: the walk clones the graph it reaches and
    /// shares only the cells the boundary names, so the template's own cells
    /// are the template's.
    pub(super) fn clone_instance(&mut self, value: ExprId) -> Option<TemplateInstance> {
        let root = self.state[value].term?;
        let boundary = self.clone_boundary(value);
        self.module
            .clone_template(root, self.current_block, self.current_function(), &boundary)
    }

    /// The instance of a template's **value** graph, or `None` when a budget
    /// stopped the walk.
    ///
    /// # Invariant
    /// The instance is **fresh**: the walk clones the graph it reaches and
    /// shares only the cells the boundary names, so the template's own cells
    /// are the template's. The root is the value itself, so a callee whose
    /// value is a call's answer is read where it is decided.
    pub(super) fn clone_instance_value(&mut self, value: ExprId) -> Option<TemplateInstance> {
        let root = self.value_of(value);
        let boundary = self.clone_boundary(value);
        self.module
            .clone_template(root, self.current_block, self.current_function(), &boundary)
    }

    /// The cells a clone of `value` **shares** with the caller.
    ///
    /// # Invariant
    /// Only the enclosing **lambda parameters** are shared: their cells are the
    /// caller's, and a template that depends on one keeps the dependency.
    ///
    /// A `let` binding is **not** shared: its cell is the template's own, so
    /// sharing it would let a unify write into the template's hole.
    ///
    /// The limit is deliberate: this slice does not decide a nested lambda's
    /// binding ownership, so it shares none.
    pub(super) fn clone_boundary(&self, value: ExprId) -> TemplateBoundary {
        let mut boundary = TemplateBoundary::new();
        for parameter in self.reachable_parameters(value) {
            if let Some(term) = self.state[parameter].term {
                boundary.share(term);
            }
        }
        boundary
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
