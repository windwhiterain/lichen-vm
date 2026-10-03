//! Lambda and apply checking: the rules that compile a function expression —
//! its parameter pair, its function shell, its arrow type — and wire an
//! application: the slot-aligned argument pair the lowlevel apply unifies, the
//! function-ness guard, and the apply-time parameter-attribute check.

use std::collections::HashMap;

use lichen_lowlevel::{AnyFunctionId, LowOperator, LowValue, NodeId};

use crate::diagnostic::DiagKind;
use crate::ir::ExprId;
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

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
    ) -> NodeId {
        let return_block = self.module.add_block(None);
        let saved = self.current_block;
        self.current_block = return_block;
        // The function shell exists *before* any of its nodes, and the stack
        // entry goes in with it: the allocation helper tags and registers
        // every node against the function currently being built, so the
        // parameter's cells and pair are this template's from the start —
        // nothing has to be moved, re-tagged, or overwritten afterwards.
        //
        // `parent` is the enclosing function: a nested closure's nodes then
        // read as members of the enclosing template too, while a sibling's do
        // not (the mutual-recursion invariant).  The link itself is the
        // frontend's [`ExprKind::Function::parent`], resolved here through
        // [`Checker::function_of`] — which function that *is* is decided where
        // the lambda's syntax was compiled, not here; see the frontend's
        // `fn_parents` invariant for the sibling rule it encodes.
        let parent = parent_expr.and_then(|p| self.function_of.get(&p).copied());
        let function = self.module.begin_function(return_block, parent);
        self.function_of.insert(e, function);
        self.function_stack.push((function, Some(e)));
        let value_cell = self.fresh_cell();
        let type_cell = self.fresh_cell();
        // The parameter *is* the pair `[value, type]`; the cells live in the
        // function's scope so the apply's clone yields fresh cells per call
        // (that is what makes a polymorphic value usable at several types).
        // A `x # n` parameter carries a third attribute slot — itself a
        // `[value, type]` term pair (two fresh cells), the uniform slot shape
        // — so the pair becomes the schema-shaped
        // `[value, type, [attribute value, attribute type]]`.
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
        // The return and parameter slots are named once the pair exists; both
        // are already registered in this function's own scope by the helper
        // that allocated them (the apply clone walk requires `parameter` to be
        // a member, and `finish_function` asserts it).
        self.module.finish_function(function, param, param);
        self.state[parameter].term = Some(param);
        self.state[parameter].ty = Some(type_cell);
        // A self- or mutually-recursive binding (`fib = n => e`): the IR is a
        // cycle — the body references the function's own `ExprId`.  Register
        // the function's pair *before* the body compiles, so the reference
        // resolves to the pre-registered pair (whose value node is the
        // function's own) instead of re-entering this check.  Every lambda
        // takes this path (a non-referencing lambda's pre-registration is
        // overwritten identically below); the pair's type slot is a cell,
        // bound to the arrow below once the return type is known.
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
        // The annotated parameter's type is compiled in scope — it may
        // reference the parameter itself (`x : x -> Int`) — and unified
        // against the parameter's type slot *before* the body compiles, so
        // in-body readers see the annotated kind statically (an array
        // annotation's length, a function annotation's arrow) and the
        // generated constraints fire at normalize.  At each apply the
        // argument still checks against the very same slot — the unify
        // differs from the outer `(x => e) : (T -> _)` annotation only in
        // when it happens, not in what it binds.
        if let Some(parameter_type) = parameter_type {
            self.check_expr(parameter_type);
            // The type the annotation **names**: a type expression's term is the
            // type value itself, unless the expression carries attributes — a
            // refinement written *on a type*, `x : (_ ! in_num) => e` — in which
            // case the term is the `[type, …, attribute]` pair the attribute
            // lives in and the parameter's slot takes the *denotation*
            // ([`Checker::type_denotation`]).  The attribute is enforced where it
            // was written (the type expression's own assert, on this function).
            let denotation = self.type_denotation(parameter_type);
            self.check_unify(
                type_cell,
                denotation,
                self.loc(parameter_type, 1),
                DiagKind::Annotation,
            );
        }
        // The annotated parameter's attribute `x # n`, compiled in body scope
        // like the type so `n` may reference the parameter itself.  The
        // declared perspective is a *template* constraint: the apply's check
        // compares each argument's attribute against this declared value node
        // (via [`Checker::function_param_attr`], then the attribute's
        // [`AttrExt::unify_slots`]).  The live attribute cell in the parameter
        // pair is deliberately left **unbound** — binding it to the declared
        // value here would let the deep pass *bake* it (it is a concrete
        // value), so the per-apply clone would reference the template's cell
        // instead of resetting it, and the lowlevel apply's positional unify
        // would then enforce the declared perspective (equality) against the
        // argument, defeating the attribute's subtype relaxation.  Kept
        // unbound, it is a fresh per-apply clone that binds the argument's
        // actual perspective, exactly like the value/type cells — so the
        // body's return reads the caller's perspective and `f (5 # 4)` yields
        // `5 # 4`.  The declared value itself stays only in
        // [`Checker::function_param_attr`].
        if let Some(parameter_attribute) = parameter_attribute {
            self.check_expr(parameter_attribute);
            // The declared value is the annotation expression's `[value, type]`
            // term pair — the uniform slot shape the apply's check compares
            // against the argument's slot.
            let declared = self.state[parameter_attribute]
                .term
                .expect("an attribute expr is compiled");
            // The parameter's schema tail[0] names the attribute; the apply's
            // check resolves its `AttrExt` from this marker.
            let marker = self.ir.schema(parameter).tail[0];
            // A parameter annotation is the other site that reads an
            // attribute: a build with no attribute extension reports it here,
            // so the apply's check can simply decline to run (the guard has
            // already failed the build).
            if self.attribute_extension(&marker).is_none() {
                self.no_attr_ext_guard(self.loc(e, 2));
            }
            self.function_param_attr.insert(e, (marker, declared));
        }
        let ret = self.check_expr(r#return);
        self.scopes.pop();
        self.current_block = saved;
        // The shell fills in: the return and parameter entry points.  The
        // body's asserts were registered into [`Function::asserts`] as they
        // were compiled, and the scope grew node by node.  The value node
        // pre-exists (the self-reference applied it during the body); it
        // fills in now with the function id.
        self.module.functions[function].r#return = ret;
        self.module.functions[function].parameter = param;
        // The return may live in another function's scope — a body ending
        // in a variable reference to a nested closure's pair (owned by that
        // closure, its chain reaching here through the parent link).  The
        // entry point still belongs to this function's scope: the apply
        // walk starts from it.
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
        // The function's own type: the arrow shape `[parameter type, return
        // type]` kinded as a function — `[[in, out], [FunctionType, Type]]`.
        // Built while the current function is still the shell, so these
        // nodes join its scope like the rest of the body.
        let (shape, _kind, arrow) =
            self.arrow_parts(return_block, type_cell, self.state[r#return].ty.unwrap());
        // The printer needs the arrow's *shape* — an anonymous `[dom, codom]`
        // pair is indistinguishable from a tuple type without it, so only a
        // registered shape renders as `dom -> codom`.
        self.arrows.insert(shape);
        // The self-reference's type cell now carries the arrow, so the
        // in-body applications see the function's real type.
        self.module.unify(ty_cell, arrow);
        let pair = self.array_node(return_block, &[func_node, arrow]);
        self.function_stack.pop();
        self.state[e].term = Some(pair);
        self.state[e].val = Some(func_node);
        self.state[e].ty = Some(arrow);
        pair
    }

    pub(super) fn check_app(&mut self, e: ExprId, function: ExprId, argument: ExprId) -> NodeId {
        self.check_expr(function);
        self.check_expr(argument);
        // The function slot is the function's *value* (the runtime apply
        // needs a `HighProgramValue::Function`, not the pair); the argument slot is the
        // full pair, so the apply's unify compares type cell to type cell.
        let function_value = self.value_of(function);
        // The apply's argument operand is normalized to the function's
        // declared *parameter* arity, slot-aligned: value@0, type@1, and — for
        // a parameter carrying the attribute — the argument's attribute@2
        // (the `[value, type]` slot; read the missing `[0, int]` when absent).
        // The lowlevel apply's positional unify then compares value-to-value
        // and type-to-type, matching the parameter pair's shape.  The attribute
        // equality is checked separately below (the per-apply parameter clone
        // resets its own attribute cells, so it cannot enforce the template's
        // declared value).
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
        // Function-ness guard: catch *concretely* non-function types
        // statically (applying a literal is an error, not a runtime panic).
        // Concrete function types and unbound types (parameters, lambdas,
        // call results) are left to the runtime apply — unifying the shared
        // cell here would chain the type cells of every use of a polymorphic
        // value.  A failed unify never merges classes, so this cannot chain
        // either.
        let function_ty = self.state[function].ty.unwrap();
        let concrete = self.type_is_concrete(function_ty);
        if concrete && !shape::is_function_type(&mut self.module, self.type_expr, function_ty) {
            let d = self.fresh_cell();
            let c = self.fresh_cell();
            // A unification *pattern*, not a source arrow: it must stay out
            // of `arrows` (see [`Checker::arrow`]), or every guard site would
            // print as `?d -> ?c`.
            let fn_ty = self.arrow(self.current_block, d, c);
            self.check_unify(function_ty, fn_ty, self.loc(e, 1), DiagKind::Guard);
        }
        // The apply's attribute equality check: the function's declared
        // parameter attribute (or its `missing` for an unannotated parameter)
        // against the argument's attribute (or `missing`).  This is what
        // rejects `id (5 # 4)` (declared missing vs `4`) and `f 5` for
        // `f = x # 4 => x` (declared `4` vs missing).  Routed through the
        // attribute's `AttrExt::unify_slots`; a program with no attribute
        // extension reaches neither branch (no schema carries an attribute).
        //
        // A build with no attribute extension records the guard once, where
        // the attribute is first read — an annotation or a parameter
        // annotation — and it is the argument pair's own missing slot
        // ([`Checker::missing_slot_of`]) that reports it when no other site
        // has.  So this check is skipped, not re-reported.
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
                // attribute's missing slot — a `[0, int]` term pair, the
                // uniform slot shape.
                let missing = self.missing_slot_of(&marker, self.loc(e, 2));
                let loc2 = self.loc(e, 2);
                ext.unify_slots(self, found_attr, missing, loc2);
            }
        }
        // The result's type cell: unbound unless the apply's evaluation
        // syncs it.  The cell rides in the apply's operand; the runtime
        // apply unifies the return pair with the apply node — the apply
        // node *is* the return pair — and binds the cell to the return
        // type: a concrete result syncs its type, a polymorphic template's
        // lazy result leaves it unbound.
        let c = self.fresh_cell();
        let operands = self.array_node(self.current_block, &[function_value, argument_pair, c]);
        let node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Apply),
            Some(operands),
        );
        // Record the argument edge: the checker is the only place that knows
        // this application's argument structure (its expression's source
        // span), and the *edge* (this apply op node -> the argument) is unique
        // per application even when the argument node itself is shared — so a
        // runtime parameter-check failure can be attributed to the argument's
        // span regardless of node sharing.  The lowlevel records only the
        // apply node on failure; the diagnostics read this edge.
        self.apply_edges.insert(
            node,
            ApplyEdge {
                argument_expr: argument,
                apply_expr: e,
            },
        );
        self.state[e].term = Some(node);
        self.state[e].val = None;
        self.state[e].ty = Some(c);
        node
    }
}
