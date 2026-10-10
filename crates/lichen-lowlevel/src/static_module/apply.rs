//! Materialize a frozen module's graph into fresh clones, then run the tail
//! shared with `Module::function_apply`.

use super::*;
impl<P: Program> Module<P> {
    /// Apply a static function: materialize into clones in `block`, then run the apply tail.
    #[stacksafe]
    pub fn static_function_apply(
        &mut self,
        function: StaticFunctionRef,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> Option<P::Value> {
        // The nesting guard: the apply node's own depth, which a materialization
        // deepens by one.
        if self.depth_exhausted(node) {
            return None;
        }
        let stamp = self.stamp_depth;
        self.stamp_depth = self.node_depth(node) + 1;
        let result = self.with_apply_frame(|module| {
            let static_module = module.static_module(function.module);
            let (r#return, parameter, assert_count) = {
                let f = &static_module.functions[function.index.0];
                (f.r#return, f.parameter, f.asserts.len())
            };
            let mut ctx = StaticApplyCtx {
                target: block,
                module: static_module,
                remap: HashMap::new(),
                applied: function.index,
                branch_top: None,
                tag: module.nodes[node].function,
                // The caller-side node every clone is attributed through: the apply or
                // the template it was cloned from.
                origin: module.nodes[node].origin.unwrap_or(node),
            };
            let applied = module.static_node_apply(r#return, &mut ctx);
            // The parameter is a walk entry point: an ignored one is still satisfied.
            module.static_node_apply(parameter, &mut ctx);
            // Each assert instantiates through the shared remap; a baked
            // condition is not re-registered.
            for index in 0..assert_count {
                let condition = ctx.module.functions[function.index.0].asserts[index];
                let baked = !ctx.module.nodes[condition.index].undecided();
                let instantiated = module.static_node_apply(condition, &mut ctx);
                if !baked {
                    module.asserts.push(PendingAssert {
                        condition: instantiated,
                        template: static_ref(&ctx.module, condition),
                    });
                }
            }
            // The parameter unify, as in `function_apply`: regroup clones by their
            // static reps, then unify against the argument.
            if let Some(&cloned_param) = ctx.remap.get(&parameter) {
                if ctx.remap.len() > 1 {
                    let groups = crate::apply::regroup_clones(
                        ctx.remap
                            .iter()
                            .map(|(&template, &clone)| (template, clone)),
                        |template| static_find(&ctx.module.nodes, template),
                    );
                    crate::apply::unify_clone_groups(groups, |first, clone| {
                        module.unify(first, clone);
                    });
                }
                if module.apply_parameter_check(
                    cloned_param,
                    argument,
                    block,
                    node,
                    AnyFunctionId::Static(function),
                    cloned_param,
                ) {
                    return None;
                }
            }
            let result = module.evaluate_node(Dyn(applied), Some(block));
            module.wire_apply_result(node, cell, result, applied, block)
        });
        self.stamp_depth = stamp;
        result
    }

    /// Clone one static node into the dynamic world; `#[stacksafe]` because
    /// static recursion runs through here.
    #[stacksafe]
    fn static_node_apply(&mut self, local: LocalNodeId, ctx: &mut StaticApplyCtx<P>) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&local) {
            return clone;
        }
        let node = &ctx.module.nodes[local.index];
        // The clone rule's facts, read together so the borrow ends before the walk mutates.
        let (undecided, runned, evaluated_deep, template_operation) = (
            node.undecided(),
            node.runned,
            node.evaluated_deep,
            node.operation,
        );
        // Reserve the clone id before recursing: diamonds then resolve to one clone,
        // and a value cycle to the clone's own id.
        let clone = self.add_node(ctx.target, None, None);
        // The clone's origin is the caller-side apply; a template's own nodes
        // are not this module's, so it is never the origin.
        self.nodes[clone].origin = Some(ctx.origin);
        ctx.remap.insert(local, clone);
        if undecided {
            // A residual clone joins the caller's template; unowned it is read
            // in place forever (`apply-clone-ownership.md` §1).
            self.nodes[clone].function = ctx.tag;
            // Residual: the operation (if any) is kept with its operand
            // walked — the computation re-runs against the argument.
            let operation = template_operation.map(|operation| Operation {
                operator: operation.operator,
                operand: operation
                    .operand
                    .map(|operand| self.static_node_apply(operand, ctx)),
            });
            // The carry rule (`Module::node_apply`): only a template fact travels.
            let template_answer = runned && evaluated_deep.is_some();
            let carried = operation.is_none() || template_answer;
            // The mapping runs only for a value that is kept: a dropped answer
            // must not clone a closure into the target block.
            let value = ctx.module.read(local);
            let mapped = if carried {
                value.map(|value| self.static_remap_value(value, ctx))
            } else {
                None
            };
            self.write_node_value(clone, mapped);
            // The claim follows `mapped`, not `carried`: an answer whose own
            // elements are open must not claim the run.
            self.nodes[clone].runned = mapped.is_some_and(|value| {
                !self.static_answer_elements_are_undecided(value, &ctx.module)
            });
            self.nodes[clone].operation = operation;
        } else {
            // Baked: the value in place, no payload copy; item refs re-point at
            // per-call clones where the walk made one.
            let mapped = ctx
                .module
                .read(local)
                .map(|value| self.static_remap_value(value, ctx));
            self.write_node_value(clone, mapped);
            self.nodes[clone].runned = mapped.is_some_and(|value| {
                !self.static_answer_elements_are_undecided(value, &ctx.module)
            });
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
                && ctx.module.functions[sref.index.0].open_captures
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
                            || ctx.module.nodes[sref.index.index].undecided()) =>
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
        P::Value::from(LowValue::Array(self.alloc_array(&remapped, ctx.target)))
    }

    /// Re-home a same-module static closure with open captures into a fresh dynamic
    /// [`Function`].
    ///
    /// # Invariant
    ///
    /// The body is walked through the *shared* remap, so its captured cells are cloned
    /// alongside the applied parameter and re-join its class in the regroup. Its own scope
    /// nodes are re-tagged with the fresh owner and re-instantiated per call; captures,
    /// outside the scope, keep the enclosing owner and are read in place.
    fn static_clone_function(
        &mut self,
        sref: StaticFunctionRef,
        ctx: &mut StaticApplyCtx<P>,
    ) -> P::Value {
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
            asserts: Vec::new(),
            parent: ctx.branch_top,
            block: ctx.target,
            // An instance, not a template; the `@loop` mark does not travel here.
            looping: false,
        });
        // Nested re-homes inside this closure's walk hang under it.
        let outer_top = std::mem::replace(&mut ctx.branch_top, Some(fresh));
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
        // Re-stamp the closure's clones after the entry walks, so a capture is
        // not re-owned by the closure.
        let mut own = Vec::with_capacity(scope.len());
        for &node in &scope {
            let clone = self.static_node_apply(node, ctx);
            self.nodes[clone].function = Some(fresh);
            own.push(clone);
        }
        ctx.branch_top = outer_top;
        let fresh_function = &mut self.functions[fresh];
        fresh_function.nodes = own;
        fresh_function.r#return = ret_clone;
        fresh_function.parameter = param_clone;
        fresh_function.return_type = return_type_clone;
        fresh_function.asserts = assert_clones;
        self.blocks[ctx.target].functions.push(fresh);
        P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(fresh)))
    }
}

/// Whether `node` holds a same-module closure with open captures, inside an
/// array rather than as the value's function.
fn static_node_holds_open_closure<P: Program>(module: &StaticModule<P>, node: LocalNodeId) -> bool {
    let Some(value) = module.nodes[node.index].value else {
        return false;
    };
    match value.as_enum() {
        Some(LowValue::Function(AnyFunctionId::Static(sref))) => {
            sref.module == module.key && module.functions[sref.index.0].open_captures
        }
        _ => false,
    }
}

/// The fixed context of one static materialize pass: target block, module, remap,
/// applied function, owner tag.
struct StaticApplyCtx<P: Program> {
    target: BlockId,
    module: Arc<StaticModule<P>>,
    remap: HashMap<LocalNodeId, NodeId>,
    /// The applied function: its own value node is the recursion self-reference and
    /// must stay baked.
    applied: StaticFunctionId,
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
}
