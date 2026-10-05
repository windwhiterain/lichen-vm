//! The static-function apply: materializing a solved static module's reachable
//! graph into fresh dynamic clones, then running the apply tail shared with the
//! dynamic path (`Module::function_apply`).

use super::*;
impl<P: Program> Module<P> {
    /// Apply a static function: materialize its graph into fresh dynamic
    /// clones in `block`, then run the standard apply tail (parameter unify,
    /// `ApplyError`, cell wiring) shared with [`Module::function_apply`].
    #[stacksafe]
    pub fn static_function_apply(
        &mut self,
        function: StaticFunctionRef,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> P::Value {
        // The nesting guard, on the same fact as the dynamic path: the apply
        // node's own depth, which a materialization would deepen by one.
        if self.depth_exhausted(node) {
            return P::Value::from(LowValue::Parameterized);
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
                // The caller-side node every clone this pass creates is
                // attributed through ([`Node::origin`]): the apply itself, or
                // — when the apply is a per-call clone too — the template it
                // was cloned from, which is the node the checker built and the
                // only one this module's tables can hold.
                origin: module.nodes[node].origin.unwrap_or(node),
            };
            let applied = module.static_node_apply(r#return, &mut ctx);
            // The parameter is an entry point of the walk, not just a node the
            // return subtree happens to reach: an ignored parameter still must
            // be satisfied, and a parameter read a type annotation pinned is
            // invisible from the return.
            module.static_node_apply(parameter, &mut ctx);
            // The body's asserts are the function's own registry entries: each
            // condition instantiates through the shared remap.  A baked
            // condition is per-call invariant (decided at solve time) and is not
            // re-registered; a cloned one re-checks against the argument.
            // Walked by index rather than over a clone of the list, for the
            // same reason as the dynamic path: instantiating a condition needs
            // `&mut module` while the list lives on the ctx's module.
            for index in 0..assert_count {
                let condition = ctx.module.functions[function.index.0].asserts[index];
                let baked = !ctx.module.nodes[condition.index].parameterized;
                let instantiated = module.static_node_apply(condition, &mut ctx);
                if !baked {
                    module.asserts.push(PendingAssert {
                        condition: instantiated,
                        template: static_ref(&ctx.module, condition),
                    });
                }
            }
            // The parameter unify: same shape as `function_apply` — re-establish
            // the template's internal class topology among the clones (grouped
            // by the solved static reps), evaluate the argument to the pattern's
            // depth, unify, and record an `ApplyError` on failure.
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
                    return P::Value::from(LowValue::Parameterized);
                }
            }
            let result = module.evaluate_node(Dyn(applied), Some(block));
            module.wire_apply_result(node, cell, result, applied, block)
        });
        self.stamp_depth = stamp;
        result
    }

    /// Clone one static node into the dynamic world (see the module docs).
    /// `#[stacksafe]`: static recursion runs through here at one frame per
    /// level, so the apply-depth guard must be able to grow the stack.
    #[stacksafe]
    fn static_node_apply(&mut self, local: LocalNodeId, ctx: &mut StaticApplyCtx<P>) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&local) {
            return clone;
        }
        let node = &ctx.module.nodes[local.index];
        let (parameterized, template_operation) = (node.parameterized, node.operation);
        // Reserve the clone id before recursing so diamonds resolve to one
        // clone and value cycles to the clone's own (still evaluating) id.
        let clone = self.add_node(ctx.target, None, None);
        // The clone's origin: the caller-side apply this materialization
        // answers to, so a runtime failure that names the clone is attributed
        // to the argument of the call that made it ([`Module::node_origin`]).
        // A frozen template's own nodes are not this module's, so the static
        // template cannot be the origin; see [`Node::origin`].
        self.nodes[clone].origin = Some(ctx.origin);
        ctx.remap.insert(local, clone);
        if parameterized {
            // The residual clone joins the template of the code that performed
            // the apply, exactly as a dynamic apply's clones do: a residual the
            // apply materialized may end up inside a *value* of the caller's
            // template (an open parameter's type, for instance), and only a
            // template member is re-instantiated by a later clone of that
            // caller.  An unowned residual is referenced in place forever, so
            // the first call's argument binds its cell for every later call.
            // A baked clone (the `else` arm) is final per call and stays
            // unowned, so a genuinely concrete leaf keeps its fast path.
            self.nodes[clone].function = ctx.tag;
            // Residual: the operation (if any) is kept with its operand
            // walked — the computation re-runs against the argument — and a
            // stale cached value on an operation node is dropped (it was
            // computed against the unbound template parameter).  A
            // parameterized *value* node (no operation — a structural array
            // containing the parameter, or the marker itself) keeps its
            // value, with items re-pointed at the walk's clones, mirroring
            // the dynamic clone rule.
            let operation = template_operation.map(|operation| Operation {
                operator: operation.operator,
                operand: operation
                    .operand
                    .map(|operand| self.static_node_apply(operand, ctx)),
            });
            if operation.is_none() {
                let value = ctx.module.read(local);
                let value = self.static_remap_value(value, ctx);
                self.write_node_value(clone, Some(value));
            }
            self.nodes[clone].operation = operation;
        } else {
            // Baked: the solved value in place (shared payload — no copy),
            // with item refs re-pointed at per-call clones where the walk
            // made one; untouched items stay inline absolute static refs.
            // The residual operation (if any) is dead — the value is final.
            let value = ctx.module.read(local);
            let value = self.static_remap_value(value, ctx);
            self.write_node_value(clone, Some(value));
        }
        clone
    }

    /// Re-point the items of a value at per-call clones: an item is
    /// cloned (walked) when the walk already made one, or when its static
    /// node is itself parameterized — a residual behind a value edge must
    /// re-open against the argument (a condition or branch frozen as
    /// `Parameterized` at solve time reads as unbound forever otherwise).
    /// Concrete items stay inline absolute static refs.  An item naming
    /// *another* module (a frozen dependency the applied function's module
    /// itself imported) is not this template's to clone: local indices are
    /// per-module, so only a ref keyed by `ctx.module` may consult the
    /// remap or the module's parameterized flags — a foreign ref is
    /// concrete by construction (the apply that kept it verbatim proved it)
    /// and stays in place, resolved through the registry.  The item slice is
    /// reallocated only when something changed — the common all-baked case
    /// shares the payload.
    fn static_remap_value(&mut self, value: P::Value, ctx: &mut StaticApplyCtx<P>) -> P::Value {
        // A same-module nested closure with open captures must be re-homed as
        // a dynamic `Function`: its captured cells were bound by whichever
        // application minted the closure, and only this apply's fresh clones
        // can supply this call's values — a baked static template keeps the
        // old binding.  The applied function's *own* value node is the
        // recursion self-reference and stays a frozen static template; a
        // foreign-module ref or a closure whose open cells are all inside its
        // own scope is baked too.
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
        // SAFETY: `array` is a payload of `value`, which lives in a registered
        // static module's arena — `ctx.module`'s, held alive by this apply, or
        // a dependency the registry pins; the walk below releases nothing.
        let items = unsafe { array.items() };
        let mut changed = false;
        let mut remapped = Vec::with_capacity(items.len());
        for item in items {
            let node = match item.node {
                AnyNodeId::Static(sref)
                    if sref.module == ctx.module.key
                        && (ctx.remap.contains_key(&sref.index)
                            || ctx.module.nodes[sref.index.index].parameterized) =>
                {
                    changed = true;
                    Dyn(self.static_node_apply(sref.index, ctx))
                }
                // A baked static closure value inside an array with open
                // captures — not parameterized, so the arm above skips it (a
                // concrete function ref is a decided leaf).  Re-home it
                // explicitly so its captured cells re-open against this call.
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

    /// Re-home a same-module static closure (`sref`) with open captures into
    /// a fresh dynamic [`Function`].  The closure's body is walked through the
    /// *shared* remap, so its captured cells are cloned alongside the applied
    /// parameter and re-join its class in the regroup — the parameter unify
    /// then binds the captures to the argument's value.
    /// The fresh closure's own scope nodes are re-tagged with the fresh
    /// owner, so a later apply of the closure re-instantiates them per call,
    /// while its captures (not in the scope) keep the enclosing owner and are
    /// referenced in place.
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
        // Fresh closure homed on the target block.  Its `parent` is the
        // enclosing fresh dynamic closure when re-homes nest (a wrapper
        // whose body returns a closure that itself returns one), so the
        // enclosing closure's later dynamic apply sees this one's nodes as
        // template members and re-instantiates them per call — the static
        // mirror of the dynamic path's `branch_top`.  [`None`] at the top of
        // a static apply: the enclosing apply is a static function, which
        // has no dynamic id, so a capture is a member of no enclosing
        // dynamic template and is read in place.
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
            // An instance, not a template — see the dynamic clone path's
            // `looping: false` for why the `@loop` mark does not travel here.
            looping: false,
        });
        // Nested re-homes inside this closure's walk hang under it.
        let outer_top = std::mem::replace(&mut ctx.branch_top, Some(fresh));
        let ret_clone = self.static_node_apply(r#return, ctx);
        let param_clone = self.static_node_apply(parameter, ctx);
        let return_type_clone = self.static_node_apply(return_type, ctx);
        let mut assert_clones = Vec::with_capacity(asserts.len());
        for &condition in &asserts {
            let baked = !ctx.module.nodes[condition.index].parameterized;
            let instantiated = self.static_node_apply(condition, ctx);
            if !baked {
                self.asserts.push(PendingAssert {
                    condition: instantiated,
                    template: static_ref(&ctx.module, condition),
                });
            }
            assert_clones.push(instantiated);
        }
        // Re-stamp the closure's own clones with the fresh owner.  Runs after
        // the entry-point walks so a capture the body references (already
        // collided into the shared remap) is not re-owned by the closure.
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

/// Whether static node `node` holds a same-module closure value with open
/// captures — the item-level counterpart of the top-of-value check in
/// [`Module::static_remap_value`], reached when a closure rides inside an
/// array value rather than as the function's direct return.
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

/// The fixed context of one static materialize pass: where the clones land,
/// the module being materialized (an `Arc` clone, so reads never borrow
/// `self` while clones are created), the running remap (static node →
/// its dynamic clone), the static function being applied (its own value
/// node is the recursion self-reference and must stay baked, while any
/// *other* same-module function value with open captures is re-homed as a
/// dynamic closure), and the owner tag the residual clones carry.
struct StaticApplyCtx<P: Program> {
    target: BlockId,
    module: Arc<StaticModule<P>>,
    remap: HashMap<LocalNodeId, NodeId>,
    /// The static function being applied.
    applied: StaticFunctionId,
    /// The enclosing **fresh dynamic closure** a nested re-home hangs under
    /// ([`Function::parent`]) — the static mirror of the dynamic path's
    /// `ApplyCtx::branch_top`.  [`None`] at the top of a static apply (the
    /// enclosing apply is a static function, which has no dynamic id);
    /// [`Some`] inside [`Module::static_clone_function`], so re-homes that
    /// *nest* — a wrapper whose body returns a closure that itself returns
    /// one — wire the inner closure's parent chain to the outer fresh id.
    /// Without the link the outer closure's later dynamic apply cannot see
    /// the inner one as a template member (its owner chain reaches no
    /// dynamic ancestor), hands it on uncloned, and the inner body keeps
    /// reading the *outer* apply's generation of a capture cell — a cell
    /// the outer apply's own unify bound only a clone of.
    branch_top: Option<FunctionId>,
    /// The owner tag stamped on the residual clones this pass creates — the
    /// enclosing template of the code that performed the apply (`Node::function`
    /// of the apply node), the static mirror of the dynamic path's
    /// `ApplyCtx::tag`.  [`None`] for an apply at the top level.  A baked clone
    /// is not tagged: it is final per call and is referenced in place.
    tag: Option<FunctionId>,
    /// The caller-side node the clones this pass creates record as their
    /// origin ([`Node::origin`]): the apply operation node that performed the
    /// materialization, or the template it was itself cloned from when the
    /// apply site is inside a per-call clone.  The static mirror of the
    /// dynamic path's template origin, whose subject here is the *call* — a
    /// frozen template's nodes are not nodes of this module, so the layer
    /// above can only reach the call's own argument edge.
    origin: NodeId,
}
