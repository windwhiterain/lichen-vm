//! Materialize a frozen module's graph into fresh clones, then run the tail
//! shared with `Module::function_apply`.

use super::*;
use crate::function::ParameterCheck;
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
                branch_top: None,
                tag: self.nodes[node].function,
                // The caller-side node every clone is attributed through: the apply or
                // the template it was cloned from.
                origin: self.nodes[node].origin.unwrap_or(node),
                minted: HashMap::new(),
            };
            let applied = self.static_node_apply(r#return, &mut ctx);
            // The parameter is a walk entry point: an ignored one is still satisfied.
            self.static_node_apply(parameter, &mut ctx);
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
            if let Some(&cloned_param) = ctx.remap.get(&parameter) {
                if ctx.remap.len() > 1 {
                    let groups = crate::apply::regroup_clones(
                        ctx.remap
                            .iter()
                            .map(|(&template, &clone)| (template, clone)),
                        |template| static_find(&ctx.module.nodes, template),
                    );
                    crate::apply::unify_clone_groups(groups, |first, clone| {
                        self.unify(first, clone);
                    });
                }
                if self.apply_parameter_check(
                    cloned_param,
                    argument,
                    block,
                    node,
                    AnyFunctionId::Static(function),
                    cloned_param,
                ) {
                    break 'frame None;
                }
            }
            let result = self.evaluate_node(Dyn(applied), Some(block));
            // A static callee's type is static too, which the dynamic guard refuses.
            if let Some(cell) = cell
                && let Some(codomain) = self.expr_type_item(applied)
            {
                // A decided codomain stays the frozen ref the clone walk carried; a
                // leaf in this block gives the bind its value.
                let codomain = self.as_dynamic(codomain, block);
                self.evaluate_node(Dyn(codomain), Some(block));
                self.unify(cell, codomain);
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
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
        check: ParameterCheck,
    ) -> bool {
        if self.depth_exhausted(node) {
            return false;
        }
        let Some((parameter, codomain, mut remap)) =
            self.materialize_static_signature_with_classes(function, block)
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
                branch_top: None,
                tag: self.nodes[node].function,
                origin: self.nodes[node].origin.unwrap_or(node),
                minted: HashMap::new(),
            };
            let applied = self.static_return_type_apply(r#return, &mut ctx);
            self.applied_exprs.insert(node, applied);
            remap = ctx.remap;
            minted = ctx.minted;
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
            self.register_static_type_asserts(function, block, node, remap, minted);
        }
        let Some(cell) = cell else {
            return true;
        };
        self.evaluate_node(Dyn(codomain), Some(block));
        self.unify(cell, codomain);
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
        argument: NodeId,
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
        let Some((parameter, _codomain, remap)) =
            self.materialize_static_signature_with_classes(function, block)
        else {
            return;
        };
        // The body graph, not its expr: a frozen reference is a static ref otherwise.
        let mut ctx = StaticApplyCtx {
            target: block,
            module,
            remap,
            applied: function.index,
            branch_top: None,
            tag: self.nodes[node].function,
            origin: self.nodes[node].origin.unwrap_or(node),
            minted: HashMap::new(),
        };
        // The record states the call's attribute state
        // (`docs/notes/apply-type-side-check.md`).
        let applied = self.static_return_type_apply(r#return, &mut ctx);
        self.applied_parameters.insert(node, parameter);
        // The clone is bound to the argument: a return attribute naming one of
        // its fields reads the argument's field record.
        self.unify_parameter_slots(parameter, argument, block);
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
            branch_top: None,
            tag: self.nodes[node].function,
            origin: self.nodes[node].origin.unwrap_or(node),
            minted,
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
        Some(self.clone_static_signature_node(&module, local, block, &mut remap))
    }

    /// The frozen signature copied into `block`, its frozen classes preserved.
    #[stacksafe]
    fn materialize_static_signature_with_classes(
        &mut self,
        function: StaticFunctionRef,
        block: BlockId,
    ) -> Option<(NodeId, NodeId, HashMap<LocalNodeId, NodeId>)> {
        let module = self.static_module(function.module);
        let (parameter, return_type) = {
            let entry = module.functions.get(function.index.0)?;
            (entry.parameter, entry.return_type)
        };
        let mut remap: HashMap<LocalNodeId, NodeId> = HashMap::new();
        let parameter = self.clone_static_signature_node(&module, parameter, block, &mut remap);
        let codomain = self.clone_static_signature_node(&module, return_type, block, &mut remap);
        if remap.len() > 1 {
            let groups = crate::apply::regroup_clones(
                remap.iter().map(|(&template, &clone)| (template, clone)),
                |template| static_find(&module.nodes, template),
            );
            crate::apply::unify_clone_groups(groups, |first, clone| {
                self.unify(first, clone);
            });
        }
        Some((parameter, codomain, remap))
    }

    /// Whether a frozen node is still **open**: it holds nothing, or a position its
    /// value names does.
    ///
    /// # Invariant
    /// An open frozen position is the template's own cell, so no unify can bind it. A
    /// value copied into this module must not name one: the copy would hand the caller
    /// a cell nothing can decide (`docs/notes/kernel-parameter-class.md`). A cycle is
    /// not open by itself — the universe `K = [Type, ↺]` names only itself.
    fn frozen_ref_is_open(&self, sref: StaticNodeId) -> bool {
        let mut seen = std::collections::HashSet::new();
        self.frozen_ref_is_open_with(sref, &mut seen)
    }

    fn frozen_ref_is_open_with(
        &self,
        sref: StaticNodeId,
        seen: &mut std::collections::HashSet<StaticNodeId>,
    ) -> bool {
        if !seen.insert(sref) {
            return false;
        }
        let module = self.static_module(sref.module);
        let node = &module.nodes[sref.index.index];
        let Some(value) = node.value else {
            // A cell with neither a value nor an operation is the template's own hole;
            // a residual operation keeps its frozen reference, which its reader resolves.
            return node.operation.is_none();
        };
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return false;
        };
        // SAFETY: the payload lives in the frozen arena, pinned by the `Arc`.
        let items = unsafe { array.items() };
        items.iter().any(|item| match item.node {
            AnyNodeId::Static(inner) => self.frozen_ref_is_open_with(inner, seen),
            AnyNodeId::Dynamic(_) => false,
        })
    }

    /// Copy a frozen value so that every **open** position it names is this call's own
    /// cell; a decided ref stays the artifact's, which every reader resolves in place.
    #[stacksafe]
    pub(crate) fn copy_frozen_value(
        &mut self,
        value: P::Value,
        block: BlockId,
        remap: &mut HashMap<StaticNodeId, NodeId>,
    ) -> P::Value {
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return value;
        };
        // SAFETY: the payload lives in the frozen arena, pinned by the `Arc`.
        let items = unsafe { array.items() };
        let mut copied = Vec::with_capacity(items.len());
        let mut changed = false;
        for item in items {
            let node = match item.node {
                AnyNodeId::Static(sref) if self.frozen_ref_is_open(sref) => {
                    changed = true;
                    Dyn(self.materialize_open_ref(sref, block, remap))
                }
                node => node,
            };
            copied.push(ArrayItem { node, ..*item });
        }
        if !changed {
            return value;
        }
        P::Value::from(LowValue::Array(ArrayValue::with_layout(
            self.alloc_array(&copied, block),
            array.layout(),
        )))
    }

    /// This call's cell for an **open** frozen node: a decided value is copied, an open
    /// position becomes a fresh cell the caller's inference binds.
    #[stacksafe]
    pub(crate) fn materialize_open_ref(
        &mut self,
        sref: StaticNodeId,
        block: BlockId,
        remap: &mut HashMap<StaticNodeId, NodeId>,
    ) -> NodeId {
        if let Some(&clone) = remap.get(&sref) {
            return clone;
        }
        let clone = self.add_node(block, None, None);
        remap.insert(sref, clone);
        let module = self.static_module(sref.module);
        // A frozen operation is copied with it: a value-only copy loses what it computes.
        if let Some(operation) = module.nodes[sref.index.index].operation {
            let operand = operation.operand.map(|operand| {
                self.materialize_open_ref(
                    StaticNodeId {
                        module: sref.module,
                        index: operand,
                    },
                    block,
                    remap,
                )
            });
            self.nodes[clone].operation = Some(Operation {
                operator: operation.operator,
                operand,
            });
        }
        let copied = module
            .read(sref.index)
            .map(|value| self.copy_frozen_value(value, block, remap));
        self.write_node_value(clone, copied);
        clone
    }

    /// One node of the frozen signature, copied; an array's items are copied with it.
    #[stacksafe]
    fn clone_static_signature_node(
        &mut self,
        module: &Arc<StaticModule<P>>,
        local: LocalNodeId,
        block: BlockId,
        remap: &mut HashMap<LocalNodeId, NodeId>,
    ) -> NodeId {
        if let Some(&clone) = remap.get(&local) {
            return clone;
        }
        let clone = self.add_node(block, None, None);
        remap.insert(local, clone);
        // A frozen operation is copied with it: a value-only copy loses what it computes.
        let operation = module.nodes[local.index].operation;
        let Some(value) = module.read(local) else {
            if let Some(operation) = operation {
                let operand = operation
                    .operand
                    .map(|operand| self.clone_static_signature_node(module, operand, block, remap));
                self.nodes[clone].operation = Some(Operation {
                    operator: operation.operator,
                    operand,
                });
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
                            // A value is copied; so is a per-call cell with no value and no
                            // operation — the parameter's own positions.

                            // A residual operation keeps its frozen reference, which the reader
                            // resolves in that module.
                            if module.read(sref.index).is_some()
                                || module.nodes[sref.index.index].operation.is_none()
                            {
                                Dyn(self
                                    .clone_static_signature_node(module, sref.index, block, remap))
                            } else {
                                item.node
                            }
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
            let operand = operation
                .operand
                .map(|operand| self.clone_static_signature_node(module, operand, block, remap));
            self.nodes[clone].operation = Some(Operation {
                operator: operation.operator,
                operand,
            });
        }
        self.write_node_value(clone, Some(copied));
        clone
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
        let (undecided, _runned, evaluated_deep, template_operation, template_shape) = (
            node.undecided(),
            node.runned,
            node.evaluated_deep,
            node.operation,
            node.low_shape.clone(),
        );
        // Reserve the clone id before recursing: diamonds then resolve to one clone,
        // and a value cycle to the clone's own id.
        let clone = self.add_node(ctx.target, None, None);
        // The clone's origin is the caller-side apply; a template's own nodes
        // are not this module's, so it is never the origin.
        self.nodes[clone].origin = Some(ctx.origin);
        ctx.remap.insert(local, clone);
        // A clone keeps its template's class (`docs/notes/compound-element-slots.md`).
        if let Some(shape) = template_shape {
            self.seed_class_low_type(clone, shape);
        }
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
            // The carry rule: the deep pass makes an answer a fact every call shares.
            let carried = operation.is_none() || evaluated_deep.is_some();
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
            block: ctx.target,
            // An instance, not a template; the `@loop` mark does not travel here.
            looping: None,
        });
        // Recorded before the walk: the closure's own value node names it again.
        ctx.minted.insert(sref, fresh);
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
        // Re-stamped after the walk: the expr and parameter belong to this closure.
        let mut own = Vec::with_capacity(scope.len());
        for &node in &scope {
            let clone = self.static_node_apply(node, ctx);
            self.nodes[clone].function = Some(fresh);
            own.push(clone);
        }
        for clone in [ret_clone, param_clone, return_type_clone] {
            if self.nodes.contains_key(clone) {
                self.nodes[clone].function = Some(fresh);
            }
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
