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
    /// Whether a requirement's value carries an open slot an instance must fill.
    ///
    /// # Invariant
    /// Only a compound type's value positions are inspected. A type constant such
    /// as `Type` or `Float` is a value whose own interior is not a template, and a
    /// decided instance's fields are actual values even when one of them is a type.
    pub(super) fn requirement_is_open(&mut self, value: NodeId, ty: NodeId, depth: usize) -> bool {
        if depth > 64 {
            return false;
        }
        let Some(held) = self
            .module
            .class_value(value)
            .and_then(|value| value.as_enum())
        else {
            return true;
        };
        match held {
            LowValue::Any => true,
            LowValue::Array(array) => {
                // SAFETY: the handle belongs to this module, which outlives the walk.
                let items = unsafe { array.items() };
                // **A frozen array is a type-level template**: a static item counts as open.
                if items.iter().any(|item| item.node.dynamic().is_none()) {
                    return true;
                }
                if !self.is_compound_type(ty) {
                    return false;
                }
                items
                    .iter()
                    .any(|item| self.requirement_is_open_any(item.node, depth + 1))
            }
            _ => false,
        }
    }

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

    fn requirement_is_open_any(&mut self, node: AnyNodeId, depth: usize) -> bool {
        let Some(expr) = node.dynamic() else {
            return false;
        };
        let Some(items) = crate::shape::value_items(&self.module, node) else {
            return false;
        };
        let Some(value) = items.first().and_then(|item| item.node.dynamic()) else {
            return false;
        };
        let ty = items.get(1).and_then(|item| item.node.dynamic());
        let _ = expr;
        match ty {
            Some(ty) => self.requirement_is_open(value, ty, depth),
            None => true,
        }
    }

    /// A requirement copy with its open value slots emptied.
    ///
    /// # Invariant
    /// Only value positions are fresh; types, attributes, name table and
    /// nominal id are shared.
    fn instantiate_requirement(&mut self, expr: NodeId, depth: usize) -> NodeId {
        if depth > 64 {
            return expr;
        }
        let Some(items) = crate::shape::value_items(&self.module, AnyNodeId::Dynamic(expr)) else {
            return expr;
        };
        let Some(value) = items.first().and_then(|item| item.node.dynamic()) else {
            return expr;
        };
        let Some(ty) = items.get(1).map(|item| item.node) else {
            return expr;
        };
        let source = value;
        let value = self.instantiate_requirement_value(value, ty, depth + 1);
        // **The instance inherits the template's frozen reference**: the fresh cell is the
        // same frozen value, by reference.
        if value != source
            && let Some(frozen) = self
                .module
                .frozen_reads
                .get(&source)
                .or_else(|| {
                    self.module
                        .frozen_reads
                        .get(&self.module.class_root(source))
                })
                .copied()
        {
            self.module.frozen_reads.insert(value, frozen);
        }
        let mut items = items.to_vec();
        items[0].node = AnyNodeId::Dynamic(value);
        let layout = self
            .module
            .node_value(AnyNodeId::Dynamic(expr))
            .and_then(|value| value.as_enum())
            .and_then(|value| match value {
                LowValue::Array(array) => Some(array.layout()),
                _ => None,
            })
            .unwrap_or(0);
        self.array_value_node(self.current_block, items, layout)
    }

    pub(super) fn instantiate_requirement_value(
        &mut self,
        value: NodeId,
        ty: AnyNodeId,
        depth: usize,
    ) -> NodeId {
        if depth > 64 {
            return value;
        }
        // A frozen type half names a value outside every applying function: it is
        // referenced, not copied.
        if matches!(ty, AnyNodeId::Static(_)) {
            return value;
        }
        // A value read through `Index` has no class value until it runs.
        self.module.evaluate_node(AnyNodeId::Dynamic(value), None);
        let Some(held) = self
            .module
            .class_value(value)
            .and_then(|value| value.as_enum())
        else {
            // An unrun operation still states its shape: keep it. Only a
            // genuinely empty slot is fresh.
            if self.module.node_operation(value).is_some() {
                return value;
            }
            return self.fresh_cell();
        };
        let LowValue::Array(array) = held else {
            return match held {
                LowValue::Any => self.fresh_cell(),
                _ => value,
            };
        };
        // SAFETY: the handle belongs to this module, which outlives the copy.
        let source: Vec<ArrayItem> = unsafe { array.items() }.to_vec();
        let items: Vec<ArrayItem> = if self.is_struct_layout_any(ty) {
            source
                .iter()
                .take(2)
                .copied()
                .chain(source.iter().skip(2).map(|item| {
                    ArrayItem {
                        node: item
                            .node
                            .dynamic()
                            .map(|field| {
                                AnyNodeId::Dynamic(self.instantiate_requirement(field, depth + 1))
                            })
                            .unwrap_or(item.node),
                        ..*item
                    }
                }))
                .collect()
        } else if crate::shape::is_array_type_any(&mut self.module, self.type_expr, ty) {
            source
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    if index == 0 {
                        *item
                    } else {
                        ArrayItem {
                            node: item
                                .node
                                .dynamic()
                                .map(|hole| {
                                    AnyNodeId::Dynamic(
                                        self.instantiate_requirement(hole, depth + 1),
                                    )
                                })
                                .unwrap_or(item.node),
                            ..*item
                        }
                    }
                })
                .collect()
        } else if crate::shape::is_tuple_type_any(&mut self.module, self.type_expr, ty)
            || crate::shape::is_table_type_any(&mut self.module, self.type_expr, ty)
            || crate::shape::is_set_type_any(&mut self.module, self.type_expr, ty)
        {
            source
                .iter()
                .map(|item| ArrayItem {
                    node: item
                        .node
                        .dynamic()
                        .map(|element| {
                            AnyNodeId::Dynamic(self.instantiate_requirement(element, depth + 1))
                        })
                        .unwrap_or(item.node),
                    ..*item
                })
                .collect()
        } else {
            // A plain array such as `Type` is not a template; a frozen one leaves the slot fresh.
            if source.iter().any(|item| item.node.dynamic().is_none()) {
                return self.fresh_cell();
            }
            return value;
        };
        self.array_value_node(self.current_block, items, array.layout())
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
        // A direct template is copied per use; an explicit constructor
        // application is already fresh.
        let expected_term = if matches!(self.ir[expected].kind, ExprKind::Apply { .. }) {
            // An explicit constructor application is already fresh.
            expected_term
        } else {
            self.module
                .expr_value_half(expected_term)
                .filter(|value| self.requirement_is_open(*value, expected_ty, 0))
                .map(|_| self.instantiate_requirement(expected_term, 0))
                .unwrap_or(expected_term)
        };
        // A formal's unify reads its slot identity directly; other values keep
        // the lazy read required by the lowering contract.
        let direct = self.formal_actual(actual);
        // **A parameter annotation's value is the declared type**: kernel-body reads
        // resolve against it.
        if direct
            && let Some(&(function, _)) = self.function_stack.last()
            && let Some(value) = self.module.expr_value_half(expected_term)
        {
            self.module
                .parameter_annotations
                .entry(function)
                .or_insert(value);
        }
        let actual_value = if direct {
            self.module
                .expr_value_half(actual_term)
                .expect("a formal's expr has a value half")
        } else {
            self.value_of(actual)
        };
        let actual_value_is_decided = self.actual_value_is_decided(actual_value);
        let expected_states_requirement =
            self.expected_states_requirement(expected_term, expected_ty);
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
            self.state[e].val = direct.then_some(actual_value).or(self.state[actual].val);
            self.state[e].ty = self.state[actual].ty;
            return actual_term;
        }

        self.compute_operands(actual_term, expected_term);
        // A statement states the actual's attributes, so a call site reads them.
        self.adopt_stated_attributes(actual_term, expected_term);
        // A requirement matched into an undecided place: a read of it stays lazy
        // (`docs/notes/class-carries-the-call.md`).
        if !actual_value_is_decided && expected_states_requirement {
            self.module.requirement_nodes.insert(actual_value);
        }
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

    /// Whether the expected side states a requirement: a template whose open slot the
    /// actual fills.
    ///
    /// # Invariant
    /// The predicate is [`Self::requirement_is_open`]'s, not raw openness: an ordinary
    /// type-level field is a value, and only a template's open slot makes a requirement.
    fn expected_states_requirement(&mut self, expected: NodeId, expected_ty: NodeId) -> bool {
        let value = self.module.expr_value_half(expected).unwrap_or(expected);
        self.requirement_is_open(value, expected_ty, 0)
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
