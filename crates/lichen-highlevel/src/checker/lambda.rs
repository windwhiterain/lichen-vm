//! Lambda and apply checking: the function shell, argument pair,
//! function-ness guard and the apply-time attribute check.

use std::collections::HashMap;

use lichen_lowlevel::{AnyFunctionId, AnyNodeId, ArrayItem, LowOperator, LowValue, NodeId};

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

        // Every lambda takes this path: a non-referencing lambda's
        // pre-registration is overwritten identically below.
        let func_node = self.alloc_node(return_block, None, None);
        let ty_cell = self.fresh_cell();
        let pair = self.array_node(return_block, &[func_node, ty_cell]);
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
            self.check_unify(
                type_cell,
                denotation,
                self.loc(parameter_type, 1),
                DiagKind::Annotation,
            );
        }
        // Invariant: the attribute cell stays undecided — a bound declared
        // value would be baked by the deep pass.
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
            self.function_param_attr.insert(e, (marker, declared));
        }
        let ret = self.check_expr(r#return);
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
        // The term becomes its own type (`f : f : f …`): the pre-body pair is
        // mutated in place into `[Function(fid), ↺]`.

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
        let pair = self.array_node(return_block, &[func_node, ty_cell]);
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
        let argument_type = self.state[argument].ty.unwrap();
        let argument_pair = match &param_persp {
            Some((marker, _)) => {
                let argument_persp = self.attr_or_missing(argument, marker);
                self.array_node(
                    self.current_block,
                    &[argument_value, argument_type, argument_persp],
                )
            }
            None => self.array_node(self.current_block, &[argument_value, argument_type]),
        };
        // Function-ness guard: catch concretely non-function types statically;
        // undecided ones wait for the runtime apply.
        let function_ty = self.state[function].ty.unwrap();
        let concrete = self.type_is_concrete(function_ty);
        if concrete && !self.module.is_function_type(function_ty) {
            let d = self.fresh_cell();
            let c = self.fresh_cell();
            // The guard binds the callee's type against two fresh cells of a
            // real signature (`signature_node`).
            let (fn_ty, _) = self.signature_node(None, d, c);
            self.check_unify(function_ty, fn_ty, self.loc(e, 1), DiagKind::Guard);
        }
        // The apply's attribute equality: the declared parameter attribute (or
        // its `missing`) against the argument's.

        // A build with no extension already failed the guard where the
        // attribute was first read, so this check is skipped.
        if let Some((param_marker, param_slot)) = param_persp {
            if let Some(ext) = self.attribute_extension(&param_marker) {
                let arg_missing = self.attr_or_missing(argument, &param_marker);
                let loc2 = self.loc(e, 2);
                ext.unify_slots(self, arg_missing, param_slot, loc2);
            }
        } else if self.state[argument].attr.is_some() {
            let marker = self.schema_tail(argument)[0];
            if let Some(ext) = self.attribute_extension(&marker) {
                let found_attr = self.state[argument].attr.unwrap();
                // The declared side of an unannotated parameter is the
                // attribute's missing slot, a `[0, int]` term pair.
                let missing = self.missing_slot_of(&marker, self.loc(e, 2));
                let loc2 = self.loc(e, 2);
                ext.unify_slots(self, found_attr, missing, loc2);
            }
        }
        // The result's type cell rides in the apply's operand; the runtime
        // apply binds it to the return pair.
        let c = self.fresh_cell();
        let operands = self.array_node(self.current_block, &[function_value, argument_pair, c]);
        let node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Apply),
            Some(operands),
        );
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
        node
    }
}
