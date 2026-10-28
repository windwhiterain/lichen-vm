//! Materialize a frozen module's graph into fresh clones, then run the tail
//! shared with `Module::function_apply`.

use super::*;
use crate::function::ParameterCheck;
impl<P: Program> Module<P> {
    /// Apply a static function: materialize into clones in `block`, then run the apply tail.
    ///
    /// # Invariant
    /// An apply is not a normalization: the callee's body is cloned into the caller and
    /// the third sanctioned unify runs (the parameter check and the return wiring); no
    /// new callee is minted and the callee's own cells are not settled.
    #[stacksafe]
    pub fn static_function_apply(
        &mut self,
        function: StaticFunctionRef,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<AnyNodeId>,
    ) -> Option<P::Value> {
        // The nesting guard: the apply node's own depth, which a materialization
        // deepens by one.
        if self.depth_exhausted(node) {
            return None;
        }
        let stamp = self.stamp_depth;
        self.stamp_depth = self.node_depth(node) + 1;
        let result = 'frame: {
            let static_module = self.static_module(function.module);
            let (r#return, parameter, assert_count) = {
                let f = &static_module.functions[function.index.0];
                (f.r#return, f.parameter, f.asserts.len())
            };
            let mut ctx = StaticApplyCtx {
                target: block,
                module: static_module,
                remap: HashMap::new(),
                applied: function.index,
                source: function.index,
                enclosing_source: None,
                enclosing_tag: None,
                rule: FrozenClassRule::new(),
                branch_top: None,
                tag: self.nodes[node].function,
                // The caller-side node every clone is attributed through: the apply or
                // the template it was cloned from.
                origin: self.nodes[node].origin.unwrap_or(node),
                minted: HashMap::new(),
                in_parameter: false,
                reconcile_cached_answer: false,
            };
            // Clone the parameter before the return so its fields share one instance.
            ctx.in_parameter = true;
            // The parameter's **image**: the clone, or the shared cell.
            let parameter_image = self.static_node_apply(parameter, &mut ctx);
            ctx.in_parameter = false;
            let applied = self.static_node_apply(r#return, &mut ctx);
            // Each assert instantiates through the shared remap; a baked
            // condition is not re-registered.
            for index in 0..assert_count {
                let condition = ctx.module.functions[function.index.0].asserts[index];
                let baked = !ctx.module.nodes[condition.index].undecided();
                let instantiated = self.static_node_apply(condition, &mut ctx);
                if !baked {
                    self.asserts.push(PendingAssert {
                        condition: instantiated,
                        template: static_ref(&ctx.module, condition),
                    });
                }
            }
            // The parameter unify, as in `function_apply`: regroup clones by their
            // static reps, then unify against the argument.
            if ctx.remap.len() > 1 {
                let groups = crate::apply::regroup_clones(
                    ctx.remap
                        .iter()
                        .map(|(&template, &clone)| (template, clone)),
                    |template| static_find(&ctx.module.nodes, template),
                )
                .into_iter()
                .filter(|(_, clone)| crate::apply::clone_is_a_plain_cell(self, *clone))
                .collect();
                crate::apply::unify_clone_groups(groups, |first, clone| {
                    self.unify(first, clone);
                });
            }
            // The check runs against the image, never the template's cell.
            if self.apply_parameter_check(
                parameter_image,
                argument,
                block,
                node,
                AnyFunctionId::Static(function),
                parameter_image,
            ) {
                break 'frame None;
            }
            let instantiated: Vec<NodeId> = ctx.remap.values().copied().collect();
            self.bind_field_sources(parameter_image, argument, &instantiated);
            let result = self.evaluate_node(Dyn(applied), Some(block));
            // A static callee's type is static too, which the dynamic guard refuses.
            if let Some(cell) = cell
                && let Some(codomain) = self.expr_type_item(applied)
            {
                // A decided codomain stays the frozen ref the clone walk carried; the
                // bind consumes it as a ref.
                self.evaluate_node(codomain, Some(block));
                self.unify_ref(cell, codomain);
            }
            self.wire_apply_result(node, result, applied, block)
        };
        self.stamp_depth = stamp;
        result
    }

    /// The type side: the signature's copies, the check, and `cell` bound.
    #[stacksafe]
    pub(crate) fn static_function_type_apply(
        &mut self,
        function: StaticFunctionRef,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<AnyNodeId>,
        check: ParameterCheck,
    ) -> bool {
        if self.depth_exhausted(node) {
            return false;
        }
        let mut rule = FrozenClassRule::new();
        let Some((parameter, codomain, mut remap)) =
            self.materialize_static_signature_with_classes(function, block, &mut rule)
        else {
            return false;
        };
        let mut minted = HashMap::new();
        // The return expr through the signature's remap, so the graph stays bound.
        let module = self.static_module(function.module);
        let applied = module
            .functions
            .get(function.index.0)
            .map(|entry| entry.r#return);
        if let Some(r#return) = applied {
            // The type side copies the return expr's type and attributes only
            // (`docs/notes/apply-type-side-check.md`).
            let mut ctx = StaticApplyCtx {
                target: block,
                module: module.clone(),
                remap,
                applied: function.index,
                source: function.index,
                enclosing_source: None,
                enclosing_tag: None,
                rule,
                branch_top: None,
                tag: self.nodes[node].function,
                origin: self.nodes[node].origin.unwrap_or(node),
                minted: HashMap::new(),
                in_parameter: false,
                reconcile_cached_answer: false,
            };
            let applied = self.static_return_type_apply(r#return, &mut ctx);
            self.record_applied_expr(node, applied);
            remap = ctx.remap;
            minted = ctx.minted;
            rule = ctx.rule;
        }
        if check != ParameterCheck::Skip {
            let recorded = self.unify_errors.len();
            let failed = self.apply_parameter_check(
                parameter,
                argument,
                block,
                node,
                AnyFunctionId::Static(function),
                parameter,
            );
            // The type side discards only the errors this check records.
            if check == ParameterCheck::Type {
                self.unify_errors.truncate(recorded);
            }
            if failed {
                return false;
            }
            // The asserts re-check on this call's copies (apply-type-side-check §6).
            self.register_static_type_asserts(function, block, node, remap, minted, rule);
        }
        let Some(cell) = cell else {
            return true;
        };
        self.evaluate_node(Dyn(codomain), Some(block));
        self.unify_ref(cell, Dyn(codomain));
        true
    }

    /// The frozen return expr's type and attribute positions; its value is a hole.
    fn static_return_type_apply(
        &mut self,
        r#return: LocalNodeId,
        ctx: &mut StaticApplyCtx<P>,
    ) -> NodeId {
        let Some(value) = ctx.module.read(r#return) else {
            return self.static_node_apply(r#return, ctx);
        };
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return self.static_node_apply(r#return, ctx);
        };
        // SAFETY: the payload lives in the frozen module, pinned by the Arc.
        let source = unsafe { array.items() };
        let mut items = Vec::with_capacity(source.len());
        for (index, item) in source.iter().enumerate() {
            // Position 0 is the expr's value (`docs/notes/compound-element-slots.md`).
            if index == 0 {
                let hole = self.add_node(ctx.target, None, None);
                items.push(ArrayItem::new(AnyNodeId::Dynamic(hole)));
                continue;
            }
            let node = match item.node {
                AnyNodeId::Static(sref) if sref.module == ctx.module.key => {
                    AnyNodeId::Dynamic(self.static_node_apply(sref.index, ctx))
                }
                other => other,
            };
            items.push(ArrayItem { node, ..*item });
        }
        let copied = self.alloc_array(&items, ctx.target);
        let node = self.add_node(
            ctx.target,
            None,
            Some(P::Value::from(LowValue::Array(ArrayValue::with_layout(
                copied,
                array.layout(),
            )))),
        );
        node
    }

    /// The frozen return expr, cloned into `block` and recorded as the applied expr,
    /// together with the parameter's clone.
    #[stacksafe]
    pub(crate) fn materialize_static_return_apply(
        &mut self,
        function: StaticFunctionRef,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
    ) {
        let module = self.static_module(function.module);
        let Some(r#return) = module
            .functions
            .get(function.index.0)
            .map(|entry| entry.r#return)
        else {
            return;
        };
        // One materialization per call: the clone and the return share one remap; a
        // reference names the argument-bound clone.
        let mut rule = FrozenClassRule::new();
        let Some((parameter, _codomain, remap)) =
            self.materialize_static_signature_with_classes(function, block, &mut rule)
        else {
            return;
        };
        // The body graph, not its expr: a frozen reference is a static ref otherwise.
        let mut ctx = StaticApplyCtx {
            target: block,
            module,
            remap,
            applied: function.index,
            source: function.index,
            enclosing_source: None,
            enclosing_tag: None,
            rule,
            branch_top: None,
            tag: self.nodes[node].function,
            origin: self.nodes[node].origin.unwrap_or(node),
            minted: HashMap::new(),
            in_parameter: false,
            reconcile_cached_answer: false,
        };
        // The record states the call's attribute state
        // (`docs/notes/apply-type-side-check.md`).
        let applied = self.static_return_type_apply(r#return, &mut ctx);
        self.applied_parameters.insert(node, parameter);
        // The clone is bound to the argument: a return attribute naming one of
        // its fields reads the argument's field record.
        self.unify_parameter_slots(parameter, argument);
        self.applied_exprs.insert(node, applied);
    }

    /// The frozen asserts, cloned through the signature copies' remap.
    #[stacksafe]
    fn register_static_type_asserts(
        &mut self,
        function: StaticFunctionRef,
        block: BlockId,
        node: NodeId,
        remap: HashMap<LocalNodeId, NodeId>,
        minted: HashMap<StaticFunctionRef, FunctionId>,
        rule: FrozenClassRule,
    ) {
        let module = self.static_module(function.module);
        if module.functions[function.index.0].asserts.is_empty() {
            return;
        }
        let mut ctx = StaticApplyCtx {
            target: block,
            module,
            remap,
            applied: function.index,
            source: function.index,
            enclosing_source: None,
            enclosing_tag: None,
            rule,
            branch_top: None,
            tag: self.nodes[node].function,
            origin: self.nodes[node].origin.unwrap_or(node),
            minted,
            in_parameter: false,
            reconcile_cached_answer: false,
        };
        for index in 0..ctx.module.functions[function.index.0].asserts.len() {
            let condition = ctx.module.functions[function.index.0].asserts[index];
            let baked = !ctx.module.nodes[condition.index].undecided();
            let instantiated = self.static_node_apply(condition, &mut ctx);
            if !baked {
                self.asserts.push(PendingAssert {
                    condition: instantiated,
                    template: static_ref(&ctx.module, condition),
                });
            }
        }
    }

    /// A frozen signature expr copied into `block`.
    #[stacksafe]
    pub fn materialize_static_signature_expr(
        &mut self,
        function: StaticFunctionRef,
        return_expr: bool,
        block: BlockId,
    ) -> Option<NodeId> {
        let module = self.static_module(function.module);
        let entry = module.functions.get(function.index.0)?;
        let local = if return_expr {
            entry.r#return
        } else {
            entry.parameter
        };
        let mut remap: HashMap<LocalNodeId, NodeId> = HashMap::new();
        let mut rule = FrozenClassRule::new();
        Some(self.clone_static_signature_node(
            &module,
            local,
            block,
            &mut remap,
            function.index,
            &mut rule,
        ))
    }

    /// The frozen signature copied into `block`, its frozen classes preserved.
    #[stacksafe]
    fn materialize_static_signature_with_classes(
        &mut self,
        function: StaticFunctionRef,
        block: BlockId,
        rule: &mut FrozenClassRule,
    ) -> Option<(NodeId, NodeId, HashMap<LocalNodeId, NodeId>)> {
        let module = self.static_module(function.module);
        let (parameter, return_type) = {
            let entry = module.functions.get(function.index.0)?;
            (entry.parameter, entry.return_type)
        };
        let mut remap: HashMap<LocalNodeId, NodeId> = HashMap::new();
        let parameter = self.clone_static_signature_node(
            &module,
            parameter,
            block,
            &mut remap,
            function.index,
            rule,
        );
        let codomain = self.clone_static_signature_node(
            &module,
            return_type,
            block,
            &mut remap,
            function.index,
            rule,
        );
        if remap.len() > 1 {
            let groups = crate::apply::regroup_clones(
                remap.iter().map(|(&template, &clone)| (template, clone)),
                |template| static_find(&module.nodes, template),
            )
            .into_iter()
            .filter(|(_, clone)| crate::apply::clone_is_a_plain_cell(self, *clone))
            .collect();
            crate::apply::unify_clone_groups(groups, |first, clone| {
                self.unify(first, clone);
            });
        }
        Some((parameter, codomain, remap))
    }

    /// One node of the frozen signature, copied; an array's items are copied with it.
    ///
    /// # Invariant
    ///
    /// An open cell whose class reaches outside the source scope is **shared**:
    /// it becomes the pass's canonical outer image, one cell per frozen cell,
    /// so the class is not split and a later encounter names the same cell.
    #[stacksafe]
    fn clone_static_signature_node(
        &mut self,
        module: &Arc<StaticModule<P>>,
        local: LocalNodeId,
        block: BlockId,
        remap: &mut HashMap<LocalNodeId, NodeId>,
        source: StaticFunctionId,
        rule: &mut FrozenClassRule,
    ) -> NodeId {
        if let Some(&clone) = remap.get(&local) {
            return clone;
        }
        if let Some(image) = rule.outer_instance(local) {
            return image;
        }
        // The **class** decides: an open cell outside the source scope is
        // shared whole.
        let shared = module.nodes[local.index].undecided() && !rule.inside(module, local, source);
        let clone = self.add_node(block, None, None);
        remap.insert(local, clone);
        if shared {
            // The canonical outer image of this frozen cell: unowned, so its
            // class stays external.
            rule.set_outer_instance(local, clone);
        }
        if let Some(supplied) = module.field_sources.get(&local).copied() {
            let supplied =
                self.clone_static_signature_node(module, supplied, block, remap, source, rule);
            self.field_sources.insert(clone, supplied);
        }
        // A frozen operation is copied with it: a value-only copy loses what it computes.
        let operation = module.nodes[local.index].operation;
        let Some(value) = module.read(local) else {
            if let Some(operation) = operation {
                let operand = operation.operand.map(|operand| {
                    self.clone_static_signature_node(module, operand, block, remap, source, rule)
                });
                self.nodes[clone].operation = Some(Operation {
                    operator: operation.operator,
                    operand,
                });
            }
            if let Some(owner) = module.apply_result_owners.get(&local).copied() {
                let owner =
                    self.clone_static_signature_node(module, owner, block, remap, source, rule);
                self.apply_result_owners.insert(clone, owner);
            }
            return clone;
        };
        let copied = match value.as_enum() {
            Some(LowValue::Array(array)) => {
                // SAFETY: the payload lives in `module`'s arena, pinned by the `Arc`.
                let items = unsafe { array.items() };
                let mut copied = Vec::with_capacity(items.len());
                for item in items {
                    let node = match item.node {
                        AnyNodeId::Static(sref) if sref.module == module.key => {
                            // Every same-module cell is materialized, an open
                            // residual included: its operand uses the same state.
                            Dyn(self.clone_static_signature_node(
                                module, sref.index, block, remap, source, rule,
                            ))
                        }
                        node => node,
                    };
                    copied.push(ArrayItem { node, ..*item });
                }
                P::Value::from(LowValue::Array(ArrayValue::with_layout(
                    self.alloc_array(&copied, block),
                    array.layout(),
                )))
            }
            _ => value,
        };
        if let Some(operation) = operation {
            let operand = operation.operand.map(|operand| {
                self.clone_static_signature_node(module, operand, block, remap, source, rule)
            });
            self.nodes[clone].operation = Some(Operation {
                operator: operation.operator,
                operand,
            });
        }
        self.write_node_value(clone, Some(copied));
        if let Some(owner) = module.apply_result_owners.get(&local).copied() {
            let owner = self.clone_static_signature_node(module, owner, block, remap, source, rule);
            self.apply_result_owners.insert(clone, owner);
        }
        clone
    }

    /// Clone one static node into the dynamic world; `#[stacksafe]` because
    /// static recursion runs through here.
    #[stacksafe]
    fn static_node_apply(&mut self, local: LocalNodeId, ctx: &mut StaticApplyCtx<P>) -> NodeId {
        // The wildcard comes first, as in the dynamic walk: a parameter
        // receives a fresh hole, never cached.
        if (ctx.in_parameter || ctx.reconcile_cached_answer)
            && matches!(
                ctx.module.read(local).and_then(|value| value.as_enum()),
                Some(LowValue::Any)
            )
        {
            return self.add_node(ctx.target, None, None);
        }
        let node = &ctx.module.nodes[local.index];
        // The clone rule's facts, read together so the borrow ends before the walk mutates.
        let (undecided, _, evaluated_deep, template_operation, template_shape) = (
            node.undecided(),
            node.runned,
            node.evaluated_deep,
            node.operation,
            node.low_shape.clone(),
        );
        // The class decides: outside the source is shared; a nested re-home also accepts the enclosing scope.
        let inside = !undecided
            || ctx.rule.inside(&ctx.module, local, ctx.source)
            || ctx
                .enclosing_source
                .is_some_and(|source| ctx.rule.inside(&ctx.module, local, source));
        // The canonical outer instance of a shared cell: one cell per frozen
        // cell, reused on every later encounter.
        if let Some(instance) = ctx.rule.outer_instance(local) {
            // The current verdict decides the owner: a class inside the
            // current scope is promoted out of the unowned state.
            if undecided && inside && self.nodes[instance].function.is_none() {
                self.set_node_owner(instance, ctx.tag);
            }
            return instance;
        }
        if let Some(&clone) = ctx.remap.get(&local) {
            return clone;
        }
        // Reserve the clone id before recursing: diamonds then resolve to one clone,
        // and a value cycle to the clone's own id.
        let clone = self.add_node(ctx.target, None, None);
        // The clone's origin is the caller-side apply; a template's own nodes
        // are not this module's, so it is never the origin.
        self.nodes[clone].origin = Some(ctx.origin);
        ctx.remap.insert(local, clone);
        if undecided && !inside {
            // The canonical outer instance of this frozen cell.
            ctx.rule.set_outer_instance(local, clone);
        }
        // A clone keeps its template's class (`docs/notes/compound-element-slots.md`).
        if let Some(shape) = template_shape {
            self.seed_class_low_type(clone, shape);
        }
        if undecided {
            // A capture cloned for the enclosing call takes that call's tag, never the closure's.
            if inside {
                let owner = if ctx.rule.inside(&ctx.module, local, ctx.source) {
                    ctx.tag
                } else {
                    ctx.enclosing_tag
                };
                self.set_node_owner(clone, owner);
            }
            // Residual: the operation (if any) is kept with its operand
            // walked — the computation re-runs against the argument.
            let operation = template_operation.map(|operation| Operation {
                operator: operation.operator,
                operand: operation
                    .operand
                    .map(|operand| self.static_node_apply(operand, ctx)),
            });
            // The carry rule: the deep pass makes an answer a fact every call shares.
            let carried = operation.is_none() || evaluated_deep.is_some();
            // The mapping runs only for a value that is kept: a dropped answer
            // must not clone a closure into the target block.
            let value = ctx.module.read(local);
            let previous = ctx.reconcile_cached_answer;
            ctx.reconcile_cached_answer |= template_operation.as_ref().is_some_and(|operation| {
                matches!(
                    AsEnum::<crate::LowOperator>::as_enum(&operation.operator),
                    Some(crate::LowOperator::Index)
                )
            }) && carried;
            let mapped = if carried {
                value.map(|value| self.static_remap_value(value, ctx))
            } else {
                None
            };
            ctx.reconcile_cached_answer = previous;
            self.write_node_value(clone, mapped);
            // The claim follows `mapped`, not `carried`: an answer whose own
            // elements are open must not claim the run.
            let claimed = !undecided
                && mapped.is_some_and(|value| {
                    !self.static_answer_elements_are_undecided(value, &ctx.module)
                });
            self.stamp_node_ran_if(clone, claimed);
            self.nodes[clone].operation = operation;
        } else {
            // Baked: the value in place, no payload copy; item refs re-point at
            // per-call clones where the walk made one.
            let mapped = ctx
                .module
                .read(local)
                .map(|value| self.static_remap_value(value, ctx));
            self.write_node_value(clone, mapped);
            let claimed = !undecided
                && mapped.is_some_and(|value| {
                    !self.static_answer_elements_are_undecided(value, &ctx.module)
                });
            self.stamp_node_ran_if(clone, claimed);
        }
        if template_operation.as_ref().is_some_and(|operation| {
            matches!(
                AsEnum::<crate::LowOperator>::as_enum(&operation.operator),
                Some(crate::LowOperator::Apply)
            )
        }) && let Some(value) = self.expr_value_half(clone)
        {
            self.apply_result_owners.insert(value, clone);
        }
        if let Some(owner) = ctx.module.apply_result_owners.get(&local).copied() {
            let owner = self.static_node_apply(owner, ctx);
            self.apply_result_owners.insert(clone, owner);
        }
        if let Some(source) = ctx.module.field_sources.get(&local).copied() {
            let source = self.static_node_apply(source, ctx);
            self.field_sources.insert(clone, source);
            if let Some(operation) = template_operation
                && matches!(
                    AsEnum::<crate::LowOperator>::as_enum(&operation.operator),
                    Some(crate::LowOperator::Index)
                )
                && let Some(operand) = operation.operand
                && let Some(LowValue::Array(operands)) =
                    ctx.module.read(operand).and_then(|value| value.as_enum())
            {
                let operands = unsafe { operands.items() };
                if self.read_position(operands[1].node) == Some(0)
                    && let AnyNodeId::Static(target) = operands[0].node
                    && target.module == ctx.module.key
                {
                    let target = self.static_node_apply(target.index, ctx);
                    self.field_sources.insert(target, source);
                }
            }
        }
        clone
    }
    /// Are the elements of the answer a frozen template just handed this call still open?
    ///
    /// # Invariant
    ///
    /// Each item is read where its own slot lives: a fresh clone through
    /// [`Module::node_value`], a same-module frozen ref through `StaticNode::value`,
    /// and a dependency's ref through the registry, as `node_value`'s static arm does.
    fn static_answer_elements_are_undecided(
        &self,
        value: P::Value,
        module: &StaticModule<P>,
    ) -> bool {
        crate::apply::answer_elements_are_undecided::<P>(value, |node| match node {
            node @ Dyn(_) => self.node_value(node).is_none(),
            AnyNodeId::Static(sref) if sref.module == module.key => {
                module.nodes[sref.index.index].value.is_none()
            }
            node @ AnyNodeId::Static(_) => self.node_value(node).is_none(),
        })
    }

    /// Re-point the items of a value at per-call clones.
    ///
    /// # Invariant
    ///
    /// An item is cloned when the walk already made one or its static node is undecided;
    /// concrete items stay inline absolute refs. Only a ref keyed by `ctx.module` may be
    /// cloned: local indices are per-module, so a dependency's ref — concrete by
    /// construction — stays in place, resolved through the registry.
    fn static_remap_value(&mut self, value: P::Value, ctx: &mut StaticApplyCtx<P>) -> P::Value {
        // A same-module closure with open captures is re-homed: only this call's
        // clones rebind its captured cells.
        if let Some(LowValue::Function(AnyFunctionId::Static(sref))) = value.as_enum() {
            if sref.module == ctx.module.key
                && sref.index != ctx.applied
                && (ctx.module.functions[sref.index.0].open_captures
                    || static_wildcard_constructor(&ctx.module, sref))
            {
                return self.static_clone_function(sref, ctx);
            }
            return value;
        }
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return value;
        };
        // SAFETY: `array` is a payload in `ctx.module`'s arena, which the ctx's
        // `Arc` holds alive for the whole pass.
        let items = unsafe { array.items() };
        let mut changed = false;
        let mut remapped = Vec::with_capacity(items.len());
        for item in items {
            let node = match item.node {
                AnyNodeId::Static(sref)
                    if sref.module == ctx.module.key
                        && (ctx.remap.contains_key(&sref.index)
                            || ctx.module.nodes[sref.index.index].undecided()
                            || (ctx.in_parameter
                                && matches!(
                                    ctx.module
                                        .read(sref.index)
                                        .and_then(|value| value.as_enum()),
                                    Some(LowValue::Any)
                                ))
                            || (ctx.reconcile_cached_answer
                                && matches!(
                                    ctx.module
                                        .read(sref.index)
                                        .and_then(|value| value.as_enum()),
                                    Some(LowValue::Any)
                                ))) =>
                {
                    changed = true;
                    Dyn(self.static_node_apply(sref.index, ctx))
                }
                // A baked closure with open captures: a decided leaf the arm above
                // skips. Re-home it so its cells re-open here.
                AnyNodeId::Static(sref)
                    if sref.module == ctx.module.key
                        && static_node_holds_open_closure(&ctx.module, sref.index) =>
                {
                    changed = true;
                    Dyn(self.static_node_apply(sref.index, ctx))
                }
                node => node,
            };
            remapped.push(ArrayItem { node, ..*item });
        }
        if !changed {
            return value;
        }
        P::Value::from(LowValue::Array(ArrayValue::with_layout(
            self.alloc_array(&remapped, ctx.target),
            array.layout(),
        )))
    }

    /// Re-home a same-module static closure with open captures into a fresh dynamic
    /// [`Function`].
    ///
    /// # Invariant
    ///
    /// The body walks the *shared* remap, so its captured cells are cloned
    /// alongside the applied parameter and re-join its class in the regroup. Its own scope
    /// nodes are re-tagged with the fresh owner and re-instantiated per call; captures,
    /// outside the scope, keep the enclosing owner and are read in place.
    ///
    /// A closure is re-homed **once** per pass ([`StaticApplyCtx::minted`]).
    fn static_clone_function(
        &mut self,
        sref: StaticFunctionRef,
        ctx: &mut StaticApplyCtx<P>,
    ) -> P::Value {
        // The closure this pass already re-homed: its own value node names it again.
        if let Some(&fresh) = ctx.minted.get(&sref) {
            return P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(fresh)));
        }
        let (r#return, parameter, return_type, asserts, scope) = {
            let f = &ctx.module.functions[sref.index.0];
            (
                f.r#return,
                f.parameter,
                f.return_type,
                f.asserts.clone(),
                f.nodes.clone(),
            )
        };
        // Fresh closure homed on the target block; `parent` chains nested re-homes
        // (see `branch_top`).
        let fresh = self.functions.insert(Function {
            nodes: Vec::new(),
            r#return: NodeId::default(),
            parameter: NodeId::default(),
            return_type: NodeId::default(),
            // The materialization's dynamic↔static identity: this closure names
            // the frozen function it came from.
            static_origin: Some(sref),
            template: None,
            asserts: Vec::new(),
            parent: ctx.branch_top,
            scope: None,
            block: ctx.target,
            // An instance, not a template; the `@loop` mark does not travel here.
            looping: None,
        });
        // The re-homed closure's scope hangs under its branch.
        let branch_scope = ctx
            .branch_top
            .and_then(|branch| self.function_scope(branch));
        let fresh_scope = Scope::child(fresh, branch_scope.as_ref());
        self.functions[fresh].scope = Some(fresh_scope);
        // Recorded before the walk: the closure's own value node names it again.
        ctx.minted.insert(sref, fresh);
        // Nested re-homes inside this closure's walk hang under it.
        let outer_top = std::mem::replace(&mut ctx.branch_top, Some(fresh));
        // The **source** of this walk is the closure being re-homed; its
        // cells are the fresh closure's, as in the dynamic walk.
        let outer_source = std::mem::replace(&mut ctx.source, sref.index);
        let outer_tag = std::mem::replace(&mut ctx.tag, Some(fresh));
        let outer_enclosing_source =
            std::mem::replace(&mut ctx.enclosing_source, Some(outer_source));
        let outer_enclosing_tag = std::mem::replace(&mut ctx.enclosing_tag, outer_tag);
        let ret_clone = self.static_node_apply(r#return, ctx);
        let param_clone = self.static_node_apply(parameter, ctx);
        let return_type_clone = self.static_node_apply(return_type, ctx);
        let mut assert_clones = Vec::with_capacity(asserts.len());
        for &condition in &asserts {
            let baked = !ctx.module.nodes[condition.index].undecided();
            let instantiated = self.static_node_apply(condition, ctx);
            if !baked {
                self.asserts.push(PendingAssert {
                    condition: instantiated,
                    template: static_ref(&ctx.module, condition),
                });
            }
            assert_clones.push(instantiated);
        }
        // Re-stamped after the walk: the expr and parameter belong to this closure.
        let mut own = Vec::with_capacity(scope.len());
        for &node in &scope {
            let clone = self.static_node_apply(node, ctx);
            // A class outside this closure's scope is shared, so its cell
            // keeps its owner: the retag must not make it internal.
            if ctx.rule.inside(&ctx.module, node, ctx.source) {
                self.set_node_owner(clone, Some(fresh));
            }
            own.push(clone);
        }
        for (node, clone) in [
            (r#return, ret_clone),
            (parameter, param_clone),
            (return_type, return_type_clone),
        ] {
            if self.nodes.contains_key(clone) && ctx.rule.inside(&ctx.module, node, ctx.source) {
                self.set_node_owner(clone, Some(fresh));
            }
        }
        ctx.branch_top = outer_top;
        ctx.source = outer_source;
        ctx.tag = outer_tag;
        ctx.enclosing_source = outer_enclosing_source;
        ctx.enclosing_tag = outer_enclosing_tag;
        let fresh_function = &mut self.functions[fresh];
        fresh_function.nodes = own;
        fresh_function.r#return = ret_clone;
        fresh_function.parameter = param_clone;
        fresh_function.return_type = return_type_clone;
        fresh_function.asserts = assert_clones;
        self.blocks[ctx.target].functions.push(fresh);
        P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(fresh)))
    }

    /// Every closure this module re-homed from a frozen function, in mint order.
    ///
    /// # Invariant
    ///
    /// A frozen closure is re-homed **once** per materialization pass, so two entries
    /// with the same [`Rehome::origin`] are two passes' closures, never one pass's two.
    pub fn rehomed_functions(&self) -> Vec<Rehome> {
        self.functions
            .iter()
            .filter_map(|(function, entry)| {
                let origin = entry.static_origin?;
                Some(Rehome {
                    function,
                    origin,
                    parent: entry.parent,
                    r#return: entry.r#return,
                    return_owner: self.nodes.get(entry.r#return).and_then(|n| n.function),
                    parameter: entry.parameter,
                    parameter_owner: self.nodes.get(entry.parameter).and_then(|n| n.function),
                })
            })
            .collect()
    }
}

/// A closure this module re-homed from a frozen function.
///
/// # Invariant
///
/// `return_owner` and `parameter_owner` name the closure that owns the shell: a
/// re-homed closure's own shell belongs to the re-home that minted it, so both tags
/// name `function` itself.
#[derive(Debug, Clone, Copy)]
pub struct Rehome {
    /// The fresh dynamic id the re-home minted.
    pub function: FunctionId,
    /// The frozen function the closure came from.
    pub origin: StaticFunctionRef,
    /// The enclosing fresh closure a nested re-home hangs under.
    pub parent: Option<FunctionId>,
    /// The re-homed closure's own return expr.
    pub r#return: NodeId,
    /// The owner tag on that return expr.
    pub return_owner: Option<FunctionId>,
    /// The re-homed closure's own parameter.
    pub parameter: NodeId,
    /// The owner tag on that parameter.
    pub parameter_owner: Option<FunctionId>,
}

/// Whether `node` holds a same-module closure with open captures, inside an
/// array rather than as the value's function.
fn static_node_holds_open_closure<P: Program>(module: &StaticModule<P>, node: LocalNodeId) -> bool {
    let Some(value) = module.nodes[node.index].value else {
        return false;
    };
    match value.as_enum() {
        Some(LowValue::Function(AnyFunctionId::Static(sref))) => {
            sref.module == module.key
                && (module.functions[sref.index.0].open_captures
                    || static_wildcard_constructor(module, sref))
        }
        _ => false,
    }
}

/// The fixed context of one static materialize pass: target block, module, remap,
/// applied function, owner tag.
struct StaticApplyCtx<P: Program> {
    /// The block every cell of this pass lands in.
    ///
    /// # Invariant
    ///
    /// The target never changes within a pass, so a canonical outer instance
    /// lands in the pass's own block however deep the walk reached it: its
    /// lifetime does not depend on the traversal order.
    target: BlockId,
    module: Arc<StaticModule<P>>,
    remap: HashMap<LocalNodeId, NodeId>,
    in_parameter: bool,
    reconcile_cached_answer: bool,
    /// The applied function: its own value node is the recursion self-reference and
    /// must stay baked.
    applied: StaticFunctionId,
    /// The **source** function whose body this walk instantiates.
    ///
    /// # Invariant
    ///
    /// The source is the function the walk reads the body of, not the one the
    /// walk is called from: a nested re-home switches it to the closure being
    /// re-homed, so the class verdicts are made in the scope of the body the
    /// walk is instantiating. It is not the recursion point — that is
    /// [`Self::applied`].
    source: StaticFunctionId,
    /// The enclosing function's source while a closure is re-homed; a capture owned by it is this call's body.
    enclosing_source: Option<StaticFunctionId>,
    /// The owner tag of that enclosing call, which a capture clone keeps.
    enclosing_tag: Option<FunctionId>,
    /// The frozen class rule this pass applies: the verdict per class and the
    /// shared classes' outer instances.
    rule: FrozenClassRule,
    /// The enclosing **fresh dynamic closure** a nested re-home hangs under;
    /// `None` at the top of a static apply.
    ///
    /// # Invariant
    ///
    /// Nested re-homes must chain to the outer fresh id, or that closure's later dynamic
    /// apply cannot see the inner one as a template member — its owner chain reaches no
    /// dynamic ancestor — and hands it on uncloned, so the inner body keeps reading the
    /// *outer* apply's generation of a capture cell, which its unify bound only a clone of.
    branch_top: Option<FunctionId>,
    /// The owner tag on the residual clones; `None` at the top. A baked clone
    /// is untagged — it is referenced in place.
    tag: Option<FunctionId>,
    /// The node the clones record as their origin: the apply node, or its own
    /// template — never a frozen template's node.
    origin: NodeId,
    /// The closure-level counterpart of the node remap: a frozen closure a pass
    /// re-homed, and the fresh closure it became.
    ///
    /// # Invariant
    ///
    /// A frozen closure is re-homed **once** per pass. Its own scope holds its own value
    /// node, so the scope walk reaches it; a second re-home would share the first's
    /// return expr and parameter through the remap, and the owner tag would then name
    /// one of the two closures only — the other would read its own shell in place and
    /// run no parameter check.
    minted: HashMap<StaticFunctionRef, FunctionId>,
}
