//! Lambda and apply checking: the function shell, argument pair,
//! function-ness guard and the apply-time attribute check.

use std::collections::HashMap;

use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, ArrayItem, LowOperator, LowValue, NodeId, StaticFunctionRef,
};

use crate::attr::{AttrExt, AttrSet, slot_value_node};
use crate::diagnostic::DiagKind;
use crate::ir::ExprId;
use crate::program::{HighProgram, TypeOperator, ValueType};

use super::{ApplyEdge, Binding, Checker};

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    pub(super) fn check_lam(
        &mut self,
        e: ExprId,
        parent_expr: Option<ExprId>,
        parameter_type: Option<ExprId>,
        parameter_attribute: Option<ExprId>,
        parameter: ExprId,
        r#return: ExprId,
        looping: bool,
    ) -> NodeId {
        let return_block = self.module.add_block(None);
        let saved = self.current_block;
        self.current_block = return_block;
        // The shell exists before its nodes, so the parameter's cells are
        // this template's from the start.

        // `parent` is the enclosing function, so a nested closure's nodes read
        // as its members and a sibling's do not.
        let parent = parent_expr.and_then(|p| self.function_of.get(&p).copied());
        let function = self.module.begin_function(return_block, parent);
        self.function_of.insert(e, function);
        self.function_stack.push((function, Some(e)));
        // The `@loop` mark crosses into the graph here, where the shell exists and
        // the body does not (loop-conversion.md §8.6).
        if looping {
            self.module.mark_looping(function);
        }
        // The parameter's value position is unconstrained: the call supplies it.
        let value_cell = self.fresh_cell();
        let type_cell = self.fresh_cell();
        // The parameter *is* the pair `[value, type]`, its cells in scope so the
        // clone yields fresh ones per call.

        // A `x # n` parameter adds a third slot, itself a `[value, type]` pair.
        let attr_cell = parameter_attribute.is_some().then(|| {
            let value_cell = self.fresh_cell();
            let type_cell = self.fresh_cell();
            (
                value_cell,
                type_cell,
                self.array_node(return_block, &[value_cell, type_cell]),
            )
        });
        let param = match attr_cell {
            Some((_, _, attr)) => {
                self.state[parameter].attr = Some(attr);
                self.array_node(return_block, &[value_cell, type_cell, attr])
            }
            None => self.array_node(return_block, &[value_cell, type_cell]),
        };
        // Both entry points are already registered in this function's scope;
        // the clone walk requires `parameter` to be one.
        self.module.finish_function(function, param, param);
        self.state[parameter].term = Some(param);
        self.state[parameter].ty = Some(type_cell);
        // The pair is registered before the body, so a self-reference
        // resolves to it instead of re-entering this check.

        // The term is its own type (`f : f : f …`), stated once, before the body.
        let func_node = self.alloc_node(return_block, None, None);
        let ty_cell = self.fresh_cell();
        let pair = self.alloc_node(return_block, None, None);
        // One node, not a type node beside the pair: a distinct one collides
        // under the clone's topology re-establishment.
        let items = [
            ArrayItem::new(AnyNodeId::Dynamic(func_node)),
            ArrayItem::new(AnyNodeId::Dynamic(pair)), // the self-reference
        ];
        self.module.write_node_value(
            pair,
            Some(P::Value::from(LowValue::Array(
                self.module.alloc_array(&items, return_block),
            ))),
        );
        self.state[e].term = Some(pair);
        self.state[e].val = Some(func_node);
        self.state[e].ty = Some(ty_cell);
        self.scopes.push(HashMap::from([(
            parameter,
            Binding {
                term: param,
                ty: type_cell,
            },
        )]));
        // The parameter's annotated type is unified into its type slot
        // before the body, so an in-body reader sees it.
        if let Some(parameter_type) = parameter_type {
            self.check_expr(parameter_type);
            // The type the annotation names: a type expression's term,
            // unless it carries attributes — then its denotation.
            let denotation = self.type_denotation(parameter_type, Some(parameter));
            // A written type is decided before the body, so a body's read can
            // state its requirement from it.
            self.module.evaluate_node_deep(denotation, None);
            self.check_unify(
                type_cell,
                denotation,
                self.loc(parameter_type, 1),
                DiagKind::Annotation,
            );
        }
        // Invariant: the attribute cell stays undecided — a bound declared
        // value would be baked by the deep pass.
        let mut parameter_requirement = None;
        if let Some(parameter_attribute) = parameter_attribute {
            // The attribute is compiled in body scope, like the type, so `n` may
            // reference the parameter itself.
            self.check_expr(parameter_attribute);
            // The declared value: the annotation expression's `[value, type]`
            // term pair.
            let declared = self.state[parameter_attribute]
                .term
                .expect("an attribute expr is compiled");
            // The parameter's schema tail[0] names the attribute; the apply's
            // check resolves its `AttrExt` from this marker.
            let marker = self.ir.schema(parameter).tail[0];
            // A parameter annotation reads an attribute too: a build with no
            // extension reports it here, so the apply declines.
            if self.attribute_extension(&marker).is_none() {
                self.no_attr_ext_guard(self.loc(e, 2));
            }
            parameter_requirement = Some(declared);
            self.function_param_attr.insert(e, (marker, declared));
        }
        let ret = self.check_expr(r#return);
        // The requirement rides its own slot, so a body's annotation of the parameter
        // cannot overwrite what the template states.
        self.module.functions[function].parameter_requirement = parameter_requirement;
        // The return's certificate, recorded for a call site (attributes.md).
        if let Some(slot) = self.state[r#return].attr
            && self.merged_tails.contains_key(&r#return)
        {
            let tail = self.schema_tail(r#return).to_vec();
            let marker = tail
                .iter()
                .rev()
                .find(|marker| {
                    self.attribute_extension(*marker)
                        .is_some_and(|ext| !ext.is_label())
                })
                .copied();
            if let Some(marker) = marker {
                self.function_return_attr.insert(e, (marker, slot, tail));
            }
        }
        self.scopes.pop();
        self.current_block = saved;
        // The value node pre-exists — the self-reference applied it during the
        // body — and fills in now with the function id.
        self.module.functions[function].r#return = ret;
        self.module.functions[function].parameter = param;
        // The return's type cell, stored so a type-level clone-on-unify reads
        // the codomain without forcing `r#return`.
        self.module.functions[function].return_type = self.state[r#return].ty.unwrap();
        // The return may belong to a nested closure's scope, but the entry
        // point must be this function's member.
        if !self.module.functions[function].nodes.contains(&ret) {
            self.module.functions[function].nodes.push(ret);
        }
        self.module.write_node_value(
            func_node,
            Some(P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(
                function,
            )))),
        );
        self.lambda_value_nodes.push(func_node);
        // The placeholder type cell joins the pair's class, so a body reference
        // to it now reads the function type.
        self.module.unify(ty_cell, pair);
        self.function_stack.pop();
        self.state[e].term = Some(pair);
        self.state[e].val = Some(func_node);
        self.state[e].ty = Some(pair);
        pair
    }

    /// The **signature** a type-position `A -> B` lowers to: a real
    /// function; see `docs/notes/function-type-merge.md`.
    ///
    /// # Invariant
    /// `domain` and `codomain` compile before the shell opens, so their nodes
    /// belong to the enclosing template and are cloned per call. A signature is
    /// never applied, only unified against, so its body is the bare return pair.
    pub(super) fn check_signature(
        &mut self,
        e: ExprId,
        domain: NodeId,
        codomain: NodeId,
    ) -> NodeId {
        let (pair, func_node) = self.signature_node(Some(e), domain, codomain);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(func_node);
        self.state[e].ty = Some(pair);
        pair
    }

    /// The graph half of [`Self::check_signature`]; the apply's
    /// function-ness guard needs a signature **pattern** too.
    ///
    /// # Invariant
    /// The pattern is a real function: a function type is the only function
    /// type there is, so there is no second shape for a pattern to wear. A
    /// pattern has no expression, so it registers no `function_of` entry.
    fn signature_node(
        &mut self,
        e: Option<ExprId>,
        domain: NodeId,
        codomain: NodeId,
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
        let parameter = self.array_node(return_block, &[parameter_value, parameter_type]);
        // The domain is unified *into* the parameter's type slot, so a function
        // unified against it binds the two.
        self.module.unify(parameter_type, domain);
        let return_value = self.fresh_cell();
        let body = self.array_node(return_block, &[return_value, codomain]);
        self.module.finish_function(function, body, parameter);
        self.module.functions[function].return_type = codomain;
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
        let pair = self.alloc_node(return_block, None, None);
        let items = [
            ArrayItem::new(AnyNodeId::Dynamic(func_node)),
            ArrayItem::new(AnyNodeId::Dynamic(pair)),
        ];
        self.module.write_node_value(
            pair,
            Some(P::Value::from(LowValue::Array(
                self.module.alloc_array(&items, return_block),
            ))),
        );
        self.module.unify(ty_cell, pair);
        self.function_stack.pop();
        self.current_block = saved;
        (pair, func_node)
    }

    /// A frozen callee's parameter requirement: the artifact's parameter tail.
    ///
    /// # Invariant
    /// A copied slot whose value is undecided is a wildcard and states nothing; a slot
    /// the extensions do not recognise is not a constraint this build checks.
    fn check_static_parameter(&mut self, function: StaticFunctionRef, argument: ExprId, e: ExprId) {
        let Some(slot) = self
            .module
            .materialize_static_parameter_requirement(function, self.current_block)
        else {
            return;
        };
        let Some((marker, ext)) = self.static_tail_owner(slot) else {
            return;
        };
        let declared = slot_value_node(&*self, slot);
        self.module.evaluate_node_deep(declared, None);
        if self.node_value(declared).is_none() {
            return;
        }
        let found = self.provided_slot(argument, &marker, ext);
        let loc = self.loc(e, 2);
        ext.unify_slots(self, found, slot, loc);
    }

    /// The attribute that owns a copied frozen tail slot, with its behaviour.
    fn static_tail_owner(&mut self, slot: NodeId) -> Option<(P::Attr, &'static dyn AttrExt<P>)> {
        let registry = self.attr_ext.as_deref()?;
        P::Attr::ORDER.iter().copied().find_map(|marker| {
            let ext = self.attribute_extension(&marker)?;
            (!ext.is_label() && ext.render(&self.module, slot, registry).is_some())
                .then_some((marker, ext))
        })
    }

    pub(super) fn check_app(&mut self, e: ExprId, function: ExprId, argument: ExprId) -> NodeId {
        self.check_expr(function);
        self.check_expr(argument);
        // The function slot is its *value*, not the pair; the argument slot
        // is the pair, so the apply compares type to type.
        let function_value = self.value_of(function);
        // The argument operand matches the parameter's arity: value@0, type@1,
        // and attribute@2 for an attributed parameter.

        // The positional unify compares slot to slot; attribute equality is
        // checked below, the clone resetting its own cells.
        let param_persp = self.function_param_attr.get(&function).copied();
        let argument_value = self.value_of(argument);
        // A runtime failure naming the argument resolves to the argument's use.
        self.node_edges
            .insert(argument_value, self.loc(argument, 0));
        let argument_type = self.state[argument].ty.unwrap();
        let argument_attr = param_persp
            .as_ref()
            .map(|(marker, _)| self.attr_or_missing(argument, marker));
        let value_argument_pair = match argument_attr {
            Some(attr) => {
                self.array_node(self.current_block, &[argument_value, argument_type, attr])
            }
            None => self.array_node(self.current_block, &[argument_value, argument_type]),
        };
        // The type side's argument: the same slots with an undecided value, so
        // its check never evaluates the argument's value.
        let type_value = self.fresh_cell();
        let type_argument_pair = match argument_attr {
            Some(attr) => self.array_node(self.current_block, &[type_value, argument_type, attr]),
            None => self.array_node(self.current_block, &[type_value, argument_type]),
        };
        // The result's type cell rides in the apply's operand; the runtime apply
        // binds it to the return pair.
        let c = self.fresh_cell();
        // The callee's **type** rides with the apply: `f : f`, so an apply
        // whose value is not concrete applies the type. §7.
        let function_ty = self.state[function].ty.unwrap();
        // A frozen callee's requirements live in the artifact, not in this build's
        // tables (attributes.md).
        let static_callee = match self.module.function_type_function(function_ty) {
            Some(AnyFunctionId::Static(sref)) => Some(sref),
            _ => None,
        };
        // A **decided** non-function callee is refused here; an
        // undecided one waits for the runtime apply.
        if self.type_is_concrete(function_ty) && !self.module.is_function_type(function_ty) {
            let d = self.fresh_cell();
            let c = self.fresh_cell();
            let (fn_ty, _) = self.signature_node(None, d, c);
            self.check_unify(function_ty, fn_ty, self.loc(e, 1), DiagKind::Guard);
        }
        // The apply's attribute equality: the declared parameter attribute (or
        // its `missing`) against the argument's.

        // A build with no extension already failed the guard where the
        // attribute was first read, so this check is skipped.
        if let Some((param_marker, param_slot)) = param_persp {
            if let Some(ext) = self.attribute_extension(&param_marker) {
                // `#_` states nothing (attributes.md); unifying would pin the template.
                let declared = crate::attr::slot_value_node(&*self, param_slot);
                self.module.evaluate_node_deep(declared, None);
                if self.node_value(declared).is_some() {
                    let found = self.provided_slot(argument, &param_marker, ext);
                    let loc2 = self.loc(e, 2);
                    ext.unify_slots(self, found, param_slot, loc2);
                }
            }
        } else if let Some(sref) = static_callee {
            self.check_static_parameter(sref, argument, e);
        }
        // Both slots' applications carry the four elements the apply arm reads.
        let value_operands = self.array_node(
            self.current_block,
            &[function_value, value_argument_pair, c, function_ty],
        );
        let value_apply = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Apply),
            Some(value_operands),
        );
        // The same four elements over an undecided callee: the type applies, the body does not.
        let undecided_callee = self.fresh_cell();
        let type_operands = self.array_node(
            self.current_block,
            &[undecided_callee, type_argument_pair, c, function_ty],
        );
        let type_apply = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Apply),
            Some(type_operands),
        );
        self.type_applies.insert(e, type_apply);
        // The call's type application at check; a function under check is skipped.
        if self.module.is_function_type(function_ty) {
            let in_progress = matches!(
                self.module.function_type_function(function_ty),
                Some(AnyFunctionId::Dynamic(function))
                    if self.function_stack.iter().any(|&(checked, _)| checked == function)
            );
            if !in_progress {
                let report = self.module.report_type_side_failures;
                // The check's failure is attributed to the argument, and the type
                // apply has its own node: it needs the same edge.
                self.apply_edges.insert(
                    type_apply,
                    ApplyEdge {
                        argument_expr: argument,
                        apply_expr: e,
                    },
                );
                // A frozen callee's refinement reads the value (apply-type-side-check §6).
                let type_slot_argument = if report && static_callee.is_none() {
                    type_argument_pair
                } else {
                    value_argument_pair
                };
                let applied = self.module.apply_function_type(
                    function_ty,
                    type_slot_argument,
                    self.current_block,
                    type_apply,
                    c,
                    report,
                );
                // A Check failure settles its node: the shallow pass walks it again.
                if report && !applied {
                    self.module
                        .write_node_value(type_apply, Some(P::Value::from(LowValue::Error)));
                }
            }
        }
        let node = value_apply;
        // The checker alone knows this application's argument span; the
        // edge is per-application even when the node is shared.
        self.apply_edges.insert(
            node,
            ApplyEdge {
                argument_expr: argument,
                apply_expr: e,
            },
        );
        // A marked recursion's site is not classified here: emitting a loop
        // is each reader's fact (loop-conversion.md §8.6).
        self.state[e].term = Some(node);
        self.state[e].val = None;
        self.state[e].ty = Some(c);
        // **The callee's return certificate is the call's** (attributes.md).
        if let Some((_marker, slot, tail)) = self.function_return_attr.get(&function).cloned() {
            let value = slot_value_node(&*self, slot);
            if self.module.node_operation(value).is_none()
                && let Some(decided) = self.node_value(value)
            {
                let fresh_value = self.value_node(decided);
                let fresh = self.pair_of(fresh_value, self.int_type);
                self.state[e].attr = Some(fresh);
                self.merged_tails.insert(e, tail);
                self.node_edges.insert(fresh, self.loc(e, 2));
            }
        }
        // A frozen callee's return certificate is the call's too (attributes.md).
        if let Some(sref) = static_callee {
            let tails = self
                .module
                .materialize_static_return_tail(sref, self.current_block);
            for slot in tails {
                if let Some((marker, _ext)) = self.static_tail_owner(slot) {
                    self.state[e].attr = Some(slot);
                    self.merged_tails.insert(e, vec![marker]);
                    self.node_edges.insert(slot, self.loc(e, 2));
                }
            }
        }
        node
    }
}
