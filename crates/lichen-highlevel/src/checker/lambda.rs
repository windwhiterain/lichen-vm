//! Lambda and apply checking: the function shell, argument expr,
//! function-ness guard and the apply-time attribute check.

use std::collections::HashMap;

use lichen_lowlevel::{AnyFunctionId, AnyNodeId, LowOperator, LowValue, NodeId};

use crate::attr::{AttrSet, slot_value_node};
use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind};
use crate::program::{HighProgram, TypeOperator, ValueType};
use lichen_utils::extend::AsEnum;

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
        // **A rebuild names its template**: a re-check keeps the earlier function.
        if let Some(template) = self.function_of.get(&e).copied() {
            self.module.set_template(function, template);
        }
        self.function_of.insert(e, function);
        self.function_expr.insert(function, e);
        self.function_stack.push((function, Some(e)));
        // The `@loop` mark crosses into the graph here, where the shell exists and
        // the body does not (loop-conversion.md §8.6).
        if looping {
            // The mark states the loop state's parameter slot
            // (`docs/notes/loop-conversion.md`).
            self.module
                .mark_looping(function, crate::shape::EXPR_VALUE_SLOT);
        }
        // Predeclare each formal leaf's slot so body reads resolve to it.
        let formals = self.formal_leaves(parameter);
        // **A formal's value states its type**: the loop state's element resolves
        // through the value node.
        for formal in &formals {
            if let (Some(value), Some(r#type)) = (self.state[*formal].val, self.state[*formal].ty) {
                self.module.record_element_type(value, r#type.into());
            }
        }
        let mut scope: HashMap<ExprId, Binding> = HashMap::new();
        for formal in &formals {
            // The formal's value position is unconstrained: the call supplies it.
            let value_cell = self.fresh_cell();
            let type_cell = self.fresh_cell();
            // The formal *is* the expr `[value, type]`, its cells in scope so the
            // clone yields fresh ones per call.
            let slot = self.slot_expr(return_block, value_cell, type_cell);
            scope.insert(
                *formal,
                Binding {
                    term: slot,
                    ty: type_cell,
                },
            );
            self.state[*formal].term = Some(slot);
            self.state[*formal].ty = Some(type_cell);
            // **The parameter's value cell is the body's read-chain root**.
            self.module.parameter_values.insert(function, value_cell);
        }
        // Push one frame even when the side declares no formal.
        self.scopes.push(scope);
        // Check the arbitrary parameter expression; its value is the domain.
        self.check_expr(parameter);
        let domain = self.state[parameter]
            .term
            .expect("the parameter side is compiled");
        // Register the domain even when it names an outer-scope value.
        self.module.register_in_function(function, domain);
        self.module.finish_function(function, domain, domain);
        let domain_value = self.module.expr_value_half(domain).unwrap_or(domain);
        // Propagate a root attribute only when the domain is that formal itself.
        for formal in &formals {
            if self.state[*formal]
                .term
                .and_then(|term| self.module.expr_value_half(term))
                == Some(domain_value)
            {
                self.state[*formal].attr = self.state[parameter].attr;
                if let Some(marker) = self.attribute_markers.get(&parameter).copied() {
                    self.attribute_markers.insert(*formal, marker);
                }
            }
        }
        // The expr is registered before the body, so a self-reference
        // resolves to it instead of re-entering this check.

        // The function's own type is the shared `Function` type.
        let func_node = self.alloc_node(return_block, None, None);
        let ty_cell = self.fresh_cell();
        let function_type = self.function_template();
        // One expr, `[value, type, attrs…]`: a type node beside it would
        // collide under the clone's topology re-establishment.
        let expr = self.slot_expr(return_block, func_node, function_type);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(func_node);
        self.state[e].ty = Some(ty_cell);
        self.current_parameters.push(formals);

        let ret = self.check_expr(r#return);

        // Complete only an unasserted identity return whose class is inside this function's scope.
        let scope = self.module.function_scope(function);
        // The identity is the **domain**: a body that returns the parameter
        // hands the call the argument.
        let identity_return = self.module.expr_value_half(ret) == Some(domain_value);
        for cell in [domain_value] {
            let inside = scope
                .as_ref()
                .is_some_and(|scope| self.module.class_inside_scope(cell, scope));
            if inside
                && identity_return
                && domain == ret
                && self.module.functions[function].asserts.is_empty()
                && self.module.class_value(cell).is_none()
            {
                self.module
                    .write_node_value(cell, Some(P::Value::from(LowValue::Any)));
            }
        }
        self.current_parameters.pop();
        self.scopes.pop();
        self.current_block = saved;
        // The value node pre-exists — the self-reference applied it during the
        // body — and fills in now with the function id.
        self.module.functions[function].r#return = ret;
        self.module.functions[function].parameter = domain;
        // The return's type cell, stored so a type-level clone-on-unify reads
        // the codomain without forcing `r#return`.
        self.module.functions[function].return_type = self.state[r#return].ty.unwrap();
        // The body claims what it returns; a hole claims its reference.
        let hole = self.state[r#return]
            .attr
            .is_some_and(|slot| self.module.class_value(slot).is_none());
        let reference = hole
            .then(|| {
                self.reference_claims
                    .get(&function)
                    .and_then(|claims| claims.last())
                    .map(|&(index, _slot, term)| (index, term))
            })
            .flatten();
        if let Some(claim) = reference {
            self.module.functions[function].return_attribute = Some(claim);
        } else if let Some(slot) = self.state[r#return].attr
            && let Some(marker) = self.attribute_markers.get(&r#return).copied()
        {
            let slot = self.module.class_operation_owner(slot).unwrap_or(slot);
            self.module.functions[function].return_attribute =
                Some((marker.order_index() as u32, slot));
        } else if let Some(claim) = self.body_claims.get(&function).copied() {
            self.module.functions[function].return_attribute = Some(claim);
        }
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
        // The placeholder type cell joins the `Function` type's class, so a body
        // reference to it now reads the function type.
        self.module.unify(ty_cell, function_type);
        self.function_stack.pop();
        self.state[e].term = Some(expr);
        self.state[e].val = Some(func_node);
        self.state[e].ty = Some(function_type);
        expr
    }

    /// The formal parameter leaves the parameter side declares.
    ///
    /// # Invariant
    ///
    /// Only the **own** leaves are candidates: a nested function's parameter
    /// side belongs to that function, so the walk stops at a `Function` node,
    /// and an **outer** formal the side captures is already declared by the
    /// lambda that owns it, so only a fresh leaf is this one's. One name is
    /// one leaf, so a repeated formal is listed once.
    pub(super) fn formal_leaves(&self, parameter: ExprId) -> Vec<ExprId> {
        let mut seen: std::collections::HashSet<ExprId> = std::collections::HashSet::new();
        let mut found: Vec<ExprId> = Vec::new();
        let mut stack = vec![parameter];
        while let Some(current) = stack.pop() {
            if !seen.insert(current) {
                continue;
            }
            match self.ir[current].kind {
                // A nested function's own parameter side and body are its own.
                ExprKind::Function { .. } => continue,
                // An already-declared outer formal is not this lambda's leaf.
                ExprKind::Formal => {
                    if self.state[current].term.is_none() {
                        found.push(current);
                    }
                    continue;
                }
                _ => {}
            }
            stack.extend(self.ir.children(current));
        }
        // The leaves in the order the parameter side names them.
        found.sort_unstable_by_key(|id| id.0);
        found
    }

    /// The argument's field a certificate names: an `Index` of the bound
    /// parameter clone carries the field position.
    ///
    /// # Invariant
    /// The certificate may be a vocabulary call over the reference
    /// (`perspective_of x.to`); the reference is that call's argument, so the walk
    /// looks through an `Apply` first (spec §3, "Every attribute is handled like
    /// the type").
    fn certificate_field(
        &mut self,
        certificate: NodeId,
        parameter_clone: NodeId,
        argument: ExprId,
    ) -> Option<ExprId> {
        let operation = self.module.node_operation(certificate)?;
        let operator = AsEnum::<LowOperator>::as_enum(&operation.operator);
        if matches!(operator, Some(LowOperator::Apply)) {
            let operand = operation.operand;
            let called = match operand {
                // SAFETY: `operand` is a live node of this module.
                Some(operand) => unsafe { self.module.array_items(operand) }
                    .and_then(|items| items.get(1).map(|item| item.node))
                    .and_then(|node| node.dynamic()),
                None => None,
            };
            // The reference's own field read is the call's argument; its position
            // names the argument's field.
            let called = match called {
                Some(node) => node,
                None => return None,
            };
            // The reference's argument is an **expr**: a frozen one is copied whole,
            // so the read's operation is its value half's.
            let called = match self.module.node_operation(called) {
                Some(_) => called,
                None => slot_value_node(&*self, called),
            };
            let Some(field_operation) = self.module.node_operation(called) else {
                return None;
            };
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&field_operation.operator),
                Some(LowOperator::Index)
            ) {
                return None;
            }
            let operand = field_operation.operand;
            let position = operand.and_then(|operand| {
                // SAFETY: `operand` is a live node of this module.
                unsafe { self.module.array_items(operand) }
                    .and_then(|items| items.get(1).map(|item| item.node))
                    .and_then(|node| node.dynamic())
            });
            let Some(position) = position else {
                return None;
            };
            self.module.evaluate_node_deep(position, None);
            let value = self.module.node_value(AnyNodeId::Dynamic(position))?;
            let LowValue::USize(position) = AsEnum::<LowValue>::as_enum(&value)? else {
                return None;
            };
            return self.argument_field(argument, position);
        }
        if !matches!(operator, Some(LowOperator::Index)) {
            return None;
        }
        let operand = operation.operand?;
        // SAFETY: `operand` is a live node of this module.
        let items = unsafe { self.module.array_items(operand) }?;
        let container = items.first()?.node.dynamic()?;
        // The clone's index container is the parameter: bound, or still an
        // undecided clone of it.
        let parameter_value = unsafe { self.module.array_items(parameter_clone) }
            .and_then(|items| items.first().map(|item| item.node))
            .and_then(|node| node.dynamic());
        let class = self.module.equality_representative(container);
        let argument_value = self.value_of(argument);
        let kindred = [Some(parameter_clone), parameter_value, Some(argument_value)]
            .iter()
            .flatten()
            .any(|node| self.module.equality_representative(*node) == class);
        let undecided =
            self.node_value(container).is_none() && self.module.node_operation(container).is_none();
        if !kindred && !undecided {
            return None;
        }
        let position = items.get(1)?.node.dynamic()?;
        self.module.evaluate_node_deep(position, None);
        let value = self.module.node_value(AnyNodeId::Dynamic(position))?;
        let LowValue::USize(position) = AsEnum::<LowValue>::as_enum(&value)? else {
            return None;
        };
        self.argument_field(argument, position)
    }

    /// The **value** a stated attribute slot answers, following the expr encoding.
    ///
    /// # Invariant
    /// A slot is `[value, type]` and a call holds the applied return expr, so the chain
    /// is followed until a value that is not an expr; an undecided position is a hole.
    fn attribute_value(&mut self, slot: NodeId) -> Option<P::Value> {
        let mut node = AnyNodeId::Dynamic(slot);
        for _ in 0..8 {
            // The walk reads each position where it lives; a frozen one is not copied.
            self.module.evaluate_node_deep_ref(node, None);
            // A class member may hold no slot value of its own: the class answers for it.
            let value = self.module.class_value(node.dynamic()?)?;
            if let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(&value)
                && array.layout() != 0
            {
                // SAFETY: the node's own payload, alive for this read.
                let next = unsafe { array.items() }.first().map(|item| item.node);
                if let Some(next) = next {
                    node = next;
                    continue;
                }
            }
            return Some(value);
        }
        None
    }

    /// The argument's field at `position` of its struct type's order, following
    /// the written names when they reorder.
    fn argument_field(&mut self, argument: ExprId, position: usize) -> Option<ExprId> {
        let (mut value, mut names_start, mut names_end) = (argument, 0usize, 0usize);
        loop {
            match self.ir[value].kind {
                ExprKind::Instantiate {
                    value: inner,
                    names,
                    ..
                }
                | ExprKind::Record {
                    value: inner,
                    names,
                } => {
                    names_start = names.start as usize;
                    names_end = names.end as usize;
                    value = inner;
                }
                ExprKind::Apply {
                    argument: inner, ..
                } => value = inner,
                ExprKind::Tuple(_) => break,
                _ => return None,
            }
        }
        let arguments = self.range_children(value);
        let names = self.ir.struct_names[names_start..names_end].to_vec();
        let written = if names.iter().any(|name| name.is_some()) {
            // The name table is the instance value's own first slot: the layout
            // lives in the value.
            let value = self.value_of(argument);
            let table = self.struct_name_table(value)?;
            let field = self.module.name_table_name(table, position)?;
            names.iter().position(|name| *name == Some(field))
        } else {
            Some(position)
        };
        written.and_then(|index| arguments.get(index)).copied()
    }

    /// The function a **curried partial application** returns.
    ///
    /// # Invariant
    /// A partial application's value is a runtime cell, so the function is read from the
    /// IR: the callee's function, its return expression, and that expression's function.
    pub(super) fn curried_callee(&self, function: ExprId) -> Option<AnyFunctionId> {
        let ExprKind::Apply {
            function: inner, ..
        } = self.ir[function].kind
        else {
            return None;
        };
        // The inner application's own function: a decided value, else one step down.
        let inner_function = self.state[inner]
            .term
            .and_then(|term| self.module.function_apply_function(term))
            .or_else(|| self.curried_callee(inner))?;
        let AnyFunctionId::Dynamic(inner_function) = inner_function else {
            return None;
        };
        let callee_expr = *self.function_expr.get(&inner_function)?;
        let ExprKind::Function { r#return, .. } = self.ir[callee_expr].kind else {
            return None;
        };
        self.function_of
            .get(&r#return)
            .copied()
            .map(AnyFunctionId::Dynamic)
    }

    pub(super) fn check_app(&mut self, e: ExprId, function: ExprId, argument: ExprId) -> NodeId {
        self.check_expr(function);
        // The call's expected type may decide a placeholder callee, so the
        // callee is pinned before the argument is checked.
        self.pin_placeholder_callee(function, argument);
        self.check_expr(argument);
        // The function slot is its *value*, not the expr; the argument slot
        // is the expr, so the apply compares type to type.
        let function_value = self.value_of(function);
        // The argument operand matches the parameter's arity: value@0, type@1,
        // and attribute@2 for an attributed parameter.

        // The callee's canonical attribute positions, source or frozen: the
        // position is the marker.
        let function_ty = self.state[function].ty.unwrap();
        // The callee's **expr** names the function: its value half holds it.
        let callee_expr = self.state[function].term.unwrap();
        let callee = self
            .module
            .function_apply_function(callee_expr)
            .or_else(|| {
                // A frozen binding states its function structurally, so the callee is
                // resolved without evaluating it.
                self.module
                    .expr_value_half(callee_expr)
                    .and_then(|half| self.module.static_function_of_callee(half))
                    .map(AnyFunctionId::Static)
            })
            .or_else(|| self.curried_callee(function));
        // The callee's **parameter expr** states its requirement: the same read as
        // its type slot (`docs/notes/attributes.md`).
        let parameter_expr = match callee {
            Some(AnyFunctionId::Dynamic(fid)) => Some(self.module.functions[fid].parameter),
            Some(AnyFunctionId::Static(sref)) => {
                self.module
                    .materialize_static_signature_expr(sref, false, self.current_block)
            }
            None => None,
        };
        let (parameter_marker, requirement) = parameter_expr
            .map(|expr| self.stated_attributes(expr))
            .unwrap_or_default()
            .first()
            .map(|(attribute, value)| (Some(*attribute), Some(*value)))
            .unwrap_or((None, None));
        // The positional unify compares slot to slot; attribute equality is
        // checked below, the clone resetting its own cells.
        let argument_value = self.value_of(argument);
        let argument_type = self.state[argument].ty.unwrap();
        // A runtime failure naming the argument resolves to the argument's use.
        self.node_edges
            .insert(argument_value, self.loc(argument, 0));
        // The expr's attribute position is the argument's **provided** slot
        // (`docs/notes/array-attribute-view.md`).
        let caller_marker = match parameter_marker {
            Some(marker) => Some(marker),
            None => self
                .merged_tails
                .get(&argument)
                .and_then(|tail| tail.first().copied()),
        };
        let argument_attr = match caller_marker {
            Some(marker) => self
                .attribute_extension(&marker)
                .map(|ext| self.provided_slot(argument, &marker, ext)),
            None => None,
        };
        let expr_marker = caller_marker.filter(|_| argument_attr.is_some());
        let value_argument_expr = match (expr_marker, argument_attr) {
            (Some(marker), Some(attr)) => self.attribute_expr(
                self.current_block,
                argument_value,
                argument_type,
                &marker,
                attr,
            ),
            _ => self.slot_expr(self.current_block, argument_value, argument_type),
        };
        // The type side's argument is the value side's expr: a return attribute
        // naming a parameter field resolves through it.
        let type_argument_expr = value_argument_expr;
        // A decided callee states the result type now; the runtime binds the same cell.
        let c = self.fresh_cell();
        // The result type is the **instanced** return type, not the declaration.
        let declared_return = self
            .applied_return_type(callee_expr)
            .map(AnyNodeId::Dynamic)
            .or_else(|| {
                match callee {
                    // A frozen callee states its signature, not the call's own type cell.
                    Some(AnyFunctionId::Static(_)) => None,
                    Some(AnyFunctionId::Dynamic(fid)) => {
                        let declared = self.module.functions[fid].return_type;
                        // Only a decided return type is a fact the call can state
                        // now; an open one belongs to the runtime apply.
                        let decided = self
                            .module
                            .node_value(AnyNodeId::Dynamic(declared))
                            .is_some()
                            || self.module.class_low_type(declared).is_some();
                        decided.then_some(AnyNodeId::Dynamic(declared))
                    }
                    None => None,
                }
            });
        if let Some(declared) = declared_return {
            self.module.unify_ref(AnyNodeId::Dynamic(c), declared);
        }
        // The callee's **expr** rides with the apply: the type side reads the
        // function from its value half (apply §7).
        let static_callee = match callee {
            Some(AnyFunctionId::Static(sref)) => Some(sref),
            _ => None,
        };
        // A resolved callee is a function; an unresolved non-function one is refused.
        let callable = callee.is_some() || self.is_function_type(function_ty);
        if !callable && self.type_is_concrete(function_ty) {
            let fn_ty = self.function_template();
            self.check_unify(function_ty, fn_ty, self.loc(e, 1), DiagKind::Guard);
        }
        // **The body's own requirements are replayed here**, where the caller's slot
        // is (`docs/notes/array-attribute-view.md`).
        if let Some(AnyFunctionId::Dynamic(fid)) = callee
            && let Some(required) = self.deferred_requirements.get(&fid).cloned()
        {
            for (marker, declared) in required {
                self.unify_declared_value(argument, &marker, declared, self.loc(e, 2));
            }
        }
        // The apply's attribute equality: the signature expr's stated requirement
        // against the argument's provided slot.
        if let (Some(declared), Some(marker)) = (requirement, parameter_marker) {
            // `#_` states nothing (attributes.md); unifying would pin the template.
            let declared = crate::attr::slot_value_node(&*self, declared);
            self.module.evaluate_node_deep(declared, None);
            self.unify_declared_value(argument, &marker, declared, self.loc(e, 2));
        }
        // **One apply per call**: this node's check pass states the signature and its
        // run computes the value.
        let operands = self.array_node(
            self.current_block,
            &[function_value, value_argument_expr, c, callee_expr],
        );
        let apply = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Apply),
            Some(operands),
        );
        self.module.apply_result_owners.insert(apply, apply);
        self.call_applies.insert(e, apply);
        // The call's type application at check; a function under check is skipped.
        if callable {
            let in_progress = matches!(
                callee,
                Some(AnyFunctionId::Dynamic(function))
                    if self.function_stack.iter().any(|&(checked, _)| checked == function)
            );
            if !in_progress {
                let report = self.module.report_type_side_failures;
                // The check's failure is attributed to the argument: the edge is this
                // call's.
                self.apply_edges.insert(
                    apply,
                    ApplyEdge {
                        argument_expr: argument,
                        apply_expr: e,
                    },
                );
                // A frozen callee's refinement reads the value (apply-type-side-check §6).
                let type_slot_argument = if report && static_callee.is_none() {
                    type_argument_expr
                } else {
                    value_argument_expr
                };
                // The parameter is this call's clone; the template's type side is skipped.
                let returned = self.applied_parameter(callee_expr).is_some();
                let applied = if returned {
                    true
                } else {
                    match callee {
                        Some(callee) => self.module.apply_function_type(
                            callee,
                            AnyNodeId::Dynamic(type_slot_argument),
                            self.current_block,
                            apply,
                            AnyNodeId::Dynamic(c),
                            report,
                        ),
                        None => false,
                    }
                };
                // The call decides the read: the argument's value states the field.
                if let Some(AnyFunctionId::Dynamic(function)) = callee
                    && let Some(reads) = self.deferred_field_reads.remove(&function)
                {
                    let argument_value = self.value_of(argument);
                    let mut resolved: std::collections::HashMap<ExprId, (NodeId, NodeId)> =
                        std::collections::HashMap::new();
                    let mut remaining = Vec::new();
                    for read in reads {
                        let container_value = if read.direct {
                            Some(argument_value)
                        } else {
                            resolved.get(&read.container).map(|&(_, value)| value)
                        };
                        let Some(container_value) = container_value else {
                            remaining.push(read);
                            continue;
                        };
                        if let Some((field_ty, field_value)) =
                            self.resolved_field(container_value, read.name)
                        {
                            self.check_unify(field_ty, read.ty, read.read_loc, DiagKind::Guard);
                            resolved.insert(read.expr, (field_ty, field_value));
                        } else {
                            remaining.push(read);
                        }
                    }
                    if !remaining.is_empty() {
                        self.deferred_field_reads.insert(function, remaining);
                    }
                }
                // A Check failure settles its node: the shallow pass walks it again.
                if report && !applied {
                    self.module
                        .write_node_value(apply, Some(P::Value::from(LowValue::Error)));
                }
            }
        }
        let node = apply;
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
        // A body that returns its parameter hands the call the argument: the
        // parameter's attribute slot is the caller's own.
        let returns_parameter = match callee {
            Some(AnyFunctionId::Dynamic(fid)) => {
                self.module.functions[fid].parameter == self.module.functions[fid].r#return
            }
            Some(AnyFunctionId::Static(sref)) => {
                self.module.static_function_returns_parameter(sref)
            }
            None => false,
        };
        // The callee declaration states the call attribute: read it, not the answer.
        let declared_certificate = match callee {
            Some(AnyFunctionId::Dynamic(fid)) => self.module.functions[fid].return_attribute,
            Some(AnyFunctionId::Static(_sref)) => self.module.applied_claims.get(&apply).copied(),
            None => None,
        };
        let certificate = match returns_parameter {
            true => None,
            false => declared_certificate
                .map(|(index, slot)| (slot, index))
                .or_else(|| {
                    self.module
                        .applied_exprs
                        .get(&apply)
                        .copied()
                        .and_then(|expr| self.stated_attributes(expr).first().copied())
                        .map(|(attribute, slot)| (slot, attribute.order_index() as u32))
                }),
        };
        if let Some((slot, index)) = certificate
            && let Some(marker) = P::Attr::ORDER.get(index as usize).copied()
            && let Some(ext) = self.attribute_extension(&marker)
        {
            let value = slot_value_node(&*self, slot);
            // A claim naming a parameter field is the **caller's** field (spec §3).
            let field = self
                .module
                .applied_parameters
                .get(&apply)
                .copied()
                .and_then(|parameter| {
                    self.certificate_field(slot, parameter, argument)
                        .or_else(|| self.certificate_field(value, parameter, argument))
                });
            // A claim naming no field answers with its own slot.
            let decided = self
                .attribute_value(slot)
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value));
            let provided = match (field, decided) {
                (Some(field), _) => self.provided_slot(field, &marker, ext),
                (None, None) => self.provided_slot(argument, &marker, ext),
                (None, Some(decided)) => {
                    let fresh_value = self.value_node(P::Value::from(decided));
                    let fresh = self.expr_of(fresh_value, self.int_type);
                    self.node_edges.insert(fresh, self.loc(e, 2));
                    fresh
                }
            };
            self.state[e].attr = Some(provided);
            self.merged_tails.insert(e, vec![marker]);
        }
        node
    }

    /// Whether a type cell is the **`Function` type** — the shared
    /// `[FunctionType, Type]` expression.
    ///
    /// # Invariant
    /// The predicate reads the **marker**, never a function: the type names no
    /// function, so a callee that is a function is recognised by its type and a
    /// callee's identity is read from its value.
    pub(super) fn is_function_type(&mut self, ty: NodeId) -> bool {
        crate::shape::is_function_type_any(&mut self.module, self.type_expr, AnyNodeId::Dynamic(ty))
    }
}
