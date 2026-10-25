//! The function **signature**: the arrow's domain and codomain, and the shape
//! each side takes. See `docs/notes/value-templates.md`.

use lichen_lowlevel::{AnyFunctionId, AnyNodeId, ArrayItem, LowOperator, LowValue, NodeId};

use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind};

use super::Checker;
use crate::program::{HighProgram, TypeOperator, ValueType};

/// What a signature's **domain** states about its parameter.
///
/// # Invariant
/// A scalar domain states a **type**, so it fills the parameter's type slot; a
/// hole and a compound template state a **value**, so they fill the whole
/// parameter. The hole is the inference case and the template the match case.
pub(super) enum Domain {
    /// A scalar or type-valued domain: the parameter's **type slot** takes it.
    TypeValued(NodeId),
    /// A bare open hole: the **whole** parameter takes the hole's expr, so the
    /// actual parameter's value and type are captured.
    Hole(NodeId),
    /// A compound template: the **whole** parameter takes a **clone** of it, so
    /// the match writes into the clone and never into the template.
    Template(ExprId),
}

/// A signature's **codomain**: the body states it, and the return type is a
/// cell.
///
/// # Invariant
/// A hole's and a compound template's type half is their layout, so the
/// signature's return type is that half; a scalar codomain is a type value, so
/// it keeps the whole denotation.
pub(super) struct Codomain {
    /// The codomain's own expr: the signature body's type slot.
    pub(super) denotation: NodeId,
    /// The cell the signature's return type is.
    pub(super) return_type: NodeId,
}

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// The **signature** a type-position `A -> B` lowers to: a real
    /// function; see `docs/notes/function-type-merge.md`.
    ///
    /// # Invariant
    /// `domain` and `codomain` compile before the shell opens, so their nodes
    /// belong to the enclosing template and are cloned per call. A signature is
    /// never applied, only unified against, so its body is the bare return expr.
    pub(super) fn check_signature(
        &mut self,
        e: ExprId,
        domain_expr: ExprId,
        codomain_expr: ExprId,
    ) -> NodeId {
        // Compile both sides in the enclosing scope, ahead of the signature's shell.
        let domain_node = self.check_type_element(domain_expr);
        let codomain_node = self.check_type_element(codomain_expr);
        let domain = self.signature_domain(domain_expr, domain_node);
        let codomain = self.signature_codomain(codomain_expr, codomain_node);
        let (expr, func_node) = self.signature_node(Some(e), domain, codomain);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(func_node);
        self.state[e].ty = Some(expr);
        expr
    }

    /// The graph half of [`Self::check_signature`]; the apply's
    /// function-ness guard needs a signature **pattern** too.
    ///
    /// # Invariant
    /// The pattern is a real function: a function type is the only function
    /// type there is, so there is no second shape for a pattern to wear. A
    /// pattern has no expression, so it registers no `function_of` entry.
    pub(super) fn signature_node(
        &mut self,
        e: Option<ExprId>,
        domain: Domain,
        codomain: Codomain,
    ) -> (NodeId, NodeId) {
        let return_block = self.module.add_block(None);
        let saved = self.current_block;
        self.current_block = return_block;
        let parent = self.current_function();
        let function = self.module.begin_function(return_block, parent);
        if let Some(e) = e {
            self.function_of.insert(e, function);
        }
        self.function_stack.push((function, e));
        // Both value positions are unconstrained: a signature states types, and
        // the wildcard is what an unstated value is.
        let parameter_value = self.fresh_cell();
        let parameter_type = self.fresh_cell();
        let parameter = self.slot_expr(return_block, parameter_value, parameter_type);
        // The domain states the parameter: a type fills the type slot, a hole
        // or a template fills the whole expr.
        match domain {
            Domain::TypeValued(node) => {
                // The domain is unified *into* the parameter's type slot, so a
                // function unified against it binds the two.
                self.module.unify(parameter_type, node);
            }
            Domain::Hole(expr) => {
                // The hole captures the actual parameter's value and type.
                self.module.unify(parameter, expr);
            }
            Domain::Template(expr) => {
                // The clone is the template's instance: the match writes into
                // the clone, and the template keeps its own holes.
                match self.clone_instance(expr) {
                    Some(instance) => {
                        self.module.unify(parameter, instance.root);
                    }
                    None => {
                        // A budget stopped the walk: a partial instance is
                        // worse than none, so the parameter states nothing.
                        let term = self.state[expr].term.unwrap_or(self.type_expr);
                        self.record_guard(term, term, self.loc(expr, 1), DiagKind::Guard, None);
                    }
                }
            }
        }
        let return_value = self.fresh_cell();
        let body = self.slot_expr(return_block, return_value, codomain.denotation);
        self.module.finish_function(function, body, parameter);
        self.module.functions[function].return_type = codomain.return_type;
        let func_node = self.alloc_node(return_block, None, None);
        self.module.write_node_value(
            func_node,
            Some(P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(
                function,
            )))),
        );
        let ty_cell = self.fresh_cell();
        // The paired term `[Function(fid), ↺]`, written once: a value a slot holds is
        // never replaced.
        let expr = self.alloc_node(return_block, None, None);
        let items = [
            ArrayItem::new(AnyNodeId::Dynamic(func_node)),
            ArrayItem::new(AnyNodeId::Dynamic(expr)),
        ];
        self.module.write_node_value(
            expr,
            Some(P::Value::from(LowValue::Array(
                self.module.alloc_array(&items, return_block).into(),
            ))),
        );
        self.module.unify(ty_cell, expr);
        self.function_stack.pop();
        self.current_block = saved;
        (expr, func_node)
    }

    /// The **pattern** the apply's function-ness guard compares with: a
    /// signature whose two cells are open.
    ///
    /// # Invariant
    /// The guard's cells are fresh, so the pattern states nothing: it says
    /// "a function", not "this function".
    pub(super) fn signature_pattern(&mut self, domain: NodeId, codomain: NodeId) -> NodeId {
        let (expr, _) = self.signature_node(
            None,
            Domain::TypeValued(domain),
            Codomain {
                denotation: codomain,
                return_type: codomain,
            },
        );
        expr
    }

    /// The shape a signature's domain states, read after the domain decided.
    ///
    /// # Invariant
    /// The domain is deep-evaluated first: a domain that has not decided is
    /// neither a hole nor a template, and a template that has not decided
    /// cannot be matched.
    fn signature_domain(&mut self, domain: ExprId, denotation: NodeId) -> Domain {
        let term = self.state[domain].term.expect("the domain is compiled");
        self.module.evaluate_node_deep(term, None);
        if self.is_bare_hole(domain) {
            return Domain::Hole(term);
        }
        if self.is_compound_template(domain) {
            return Domain::Template(domain);
        }
        if self.is_undecided_operation(domain) {
            return Domain::Template(domain);
        }
        Domain::TypeValued(denotation)
    }

    /// The shape a signature's codomain states, read after it decided.
    ///
    /// # Invariant
    /// A hole and a compound template are values, so the return type is their
    /// **type half** — the layout — and the denotation stays the body's type
    /// slot. A scalar codomain is a type value and keeps the denotation whole.
    fn signature_codomain(&mut self, codomain: ExprId, denotation: NodeId) -> Codomain {
        let term = self.state[codomain].term.expect("the codomain is compiled");
        self.module.evaluate_node_deep(term, None);
        let layout = self.is_bare_hole(codomain) || self.is_compound_template(codomain);
        if layout {
            let type_half = self.module.expr_type_half(term).unwrap_or(denotation);
            return Codomain {
                denotation,
                return_type: type_half,
            };
        }
        Codomain {
            denotation,
            return_type: denotation,
        }
    }

    /// Whether the expression is a **bare open hole**.
    ///
    /// # Invariant
    /// A hole is a placeholder whose own value cell holds nothing, so an open
    /// hole is one nothing has filled. A `let`-bound hole reads as one: the
    /// binding's own expr is the placeholder.
    fn is_bare_hole(&self, domain: ExprId) -> bool {
        if !matches!(self.ir[domain].kind, ExprKind::Placeholder) {
            return false;
        }
        let Some(term) = self.state[domain].term else {
            return false;
        };
        let Some(value) = self.module.expr_value_half(term) else {
            return false;
        };
        self.module.node_value(AnyNodeId::Dynamic(value)).is_none()
    }

    /// Whether the expression is a **compound template**.
    ///
    /// # Invariant
    /// A compound template's type is one of the shared layouts. The bare
    /// `Struct` constant is not one: it states a type, so it keeps the
    /// ordinary type-slot unify.
    fn is_compound_template(&self, domain: ExprId) -> bool {
        let Some(term) = self.state[domain].term else {
            return false;
        };
        if term == self.tuple_template()
            || term == self.array_template()
            || term == self.struct_template()
        {
            return false;
        }
        let Some(ty) = self.state[domain].ty else {
            return false;
        };
        let ty = self.module.class_root(ty);
        ty == self.module.class_root(self.tuple_template())
            || ty == self.module.class_root(self.array_template())
            || ty == self.module.class_root(self.struct_template())
    }

    /// Whether the expression is an **operation that has not decided**.
    ///
    /// # Invariant
    /// A decided operation is a value, and a value is read by its own kind; an
    /// undecided one may still answer, so the parameter takes a clone of it
    /// and decides with it.
    fn is_undecided_operation(&self, domain: ExprId) -> bool {
        let Some(term) = self.state[domain].term else {
            return false;
        };
        self.module.node_operation(term).is_some()
            && self.module.node_value(AnyNodeId::Dynamic(term)).is_none()
    }
}
