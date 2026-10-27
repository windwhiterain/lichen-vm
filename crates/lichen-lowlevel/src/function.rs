use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, ArrayValue, BlockId, Function,
    FunctionId, LowValue, Module, NodeId, Operation, PendingAssert, Program, Scope, TableItem,
};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

/// How an instantiation treats the parameter check.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParameterCheck {
    /// Clone the signature only: a call under check binds its codomain.
    Skip,
    /// Run the check and discard its errors: the type side detects a mismatch.
    Type,
    /// Run the check and report its errors: the value side's binding.
    Value,
    /// Run the check and keep its errors: a `Check` build's type side.
    Report,
}

/// A failed apply-time parameter check's context: the key back into [`Module::unify_errors`].
///
/// # Invariant
/// `parameter_type` and `argument_type` are the two top-level sides of the failing
/// `unify`, which the raw [`UnifyError`] (deep conflict leaves) discards, so this is
/// the only record of them; `apply_node` is the edge the checker keyed its structure by.
#[derive(Debug, Clone, Copy)]
pub struct ApplyError {
    /// The applied function — a static ref when the apply materialized a static
    /// dependency.
    pub function: AnyFunctionId,
    pub parameter_type: NodeId,
    pub argument_type: NodeId,
    pub argument: NodeId,
    pub apply_node: NodeId,
    pub error_index: usize,
}

/// The fixed context of one clone pass: where clones land, the anchor, the remap, and
/// the owner tag.
struct ApplyCtx<'a> {
    target: BlockId,
    /// Each template block's image in this pass; a parentless one is the
    /// apply's own target (`docs/notes/gc-block.md`).
    ///
    /// # Invariant
    /// One entry per template block per pass, so every clone of one scope
    /// shares one block.
    blocks: HashMap<BlockId, BlockId>,
    /// The membership anchor: the applied function.  A node belongs to it iff its
    /// [`Function::parent`] chain reaches it.
    anchor: FunctionId,
    /// The scope the walk's decisions are made in: the scope of the function whose
    /// body this walk instantiates.
    ///
    /// # Invariant
    /// The scope names the function the walk instantiates, so a class is cloned
    /// only when **every** member of it lies inside that scope. See
    /// `docs/notes/apply-clone-ownership.md` §4.
    scope_root: Option<Arc<Scope<FunctionId>>>,
    /// The scope verdict of each class this walk has judged.
    ///
    /// # Invariant
    /// One entry per class judged, so every member of one class reads one answer
    /// and a clone never splits a class.
    scope_verdicts: HashMap<NodeId, bool>,
    /// The branch stack top: the id a freshly cloned closure hangs under
    /// ([`Function::parent`]).
    ///
    /// # Invariant
    /// Each closure clone re-anchors it at its own fresh id, so every fresh template's
    /// chain runs back through the enclosing instances to [`Self::anchor`].
    branch_top: FunctionId,
    /// The scope of the closure being cloned, when inside a closure branch.
    ///
    /// # Invariant
    /// Membership is the chain test **or** a scope hit: a closure whose value arrived
    /// through a unification is walked under the enclosing anchor, and without the scope
    /// hit its fresh scope would be shared across calls.
    closure_scope: Option<&'a [NodeId]>,
    /// The function being applied — the recursion point, whose own value node stays in place.
    applied: FunctionId,
    /// The applied function's parameter expr, always cloned: the check runs per call.
    parameter: NodeId,
    /// The parameter expr's value slot: re-minted per call, and never carried into.
    parameter_slot: Option<NodeId>,
    /// Whether this walk is instantiating the parameter; wildcard cells become holes only here.
    in_parameter: bool,
    /// Template keys a minted walk inserted; the enclosing pass drops them with the
    /// mint, so it groups only its own clones.
    minted_keys: Option<&'a mut Vec<NodeId>>,
    /// The owner tag stamped on the clones this pass creates: the apply node's owner, or
    /// a closure's fresh id.
    tag: Option<FunctionId>,
    /// The closure memo: a cell's own closure, and the reach it is on.
    ///
    /// # Invariant
    /// One entry per cell the pass judges, so the proof runs once per cell and
    /// a cycle among decided cells is settled by the first verdict.
    closure: HashMap<NodeId, Option<bool>>,
    remap: &'a mut HashMap<NodeId, NodeId>,
    /// A source [`FunctionId`] → the fresh closure this walk minted for it.
    ///
    /// # Invariant
    /// One closure per call: the walk reaches a template closure from several nodes, and
    /// minting per reach gives a single call two closures where its body means one.
    minted: &'a mut HashMap<FunctionId, FunctionId>,
}

/// One **instantiation** of a function's template: the return expr's clone and the
/// template→clone map.
pub(super) struct Instantiation {
    /// The clone of the function's return expr — what an apply evaluates.
    pub applied: NodeId,
    /// Template node → its node of this instantiation; read through
    /// [`Instantiation::node_of`].
    ///
    /// # Invariant
    /// A template node the walk referenced in place — a concrete or per-call invariant
    /// value — is absent from the map and *is* its own clone.
    remap: HashMap<NodeId, NodeId>,
}

impl Instantiation {
    /// The node of this instantiation standing for the template node
    /// `template`.
    pub fn node_of(&self, template: NodeId) -> NodeId {
        self.remap.get(&template).copied().unwrap_or(template)
    }
}

impl<P: Program> Module<P> {
    /// `#[stacksafe]`: apply recursion is one frame per level, so the depth guard must be
    /// able to grow the stack.
    #[stacksafe]
    pub(super) fn function_apply(
        &mut self,
        function: FunctionId,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<AnyNodeId>,
    ) -> Option<P::Value> {
        // The nesting guard, first: `node`'s depth bounds the expansion, so a loop never nests.
        if self.depth_exhausted(node) {
            return None;
        }
        // A marked recursion whose shape converts runs as a loop — one instantiation per
        // iteration, not one per nested level.
        if self.function_is_looping(function)
            && let Ok(conversion) = self.loop_conversion(function)
        {
            return self.apply_loop(&conversion, function, argument, block, node, cell);
        }
        let template = self.functions[function].template.unwrap_or(function);
        if self.report_type_side_failures && self.value_templates.contains(&template) {
            return None;
        }
        self.value_templates.insert(template);
        let result = (|| {
            let Some(instantiation) =
                self.instantiate(function, argument, block, node, ParameterCheck::Value)
            else {
                // A failed parameter check leaves the apply's result unknown:
                // the body must not run under a mismatched argument.
                return None;
            };
            let result = self.evaluate_node(Dyn(instantiation.applied), Some(block));
            // The call's own result type is the applied return's type cell.
            if let Some(cell) = cell
                && let Some(codomain) = self.expr_type_item(instantiation.applied)
            {
                self.evaluate_node(codomain, Some(block));
                self.unify_ref(cell, codomain);
            }
            self.wire_apply_result(node, result, instantiation.applied, block)
        })();
        self.value_templates.remove(&template);
        result
    }

    /// Apply a function's **signature** where its value is not concrete yet.
    ///
    /// # Invariant
    /// The function is named by the caller, never by a type: a function's type
    /// is the shared `FunctionType` marker and carries no signature, so the
    /// apply states the function it means. The domain and codomain are this
    /// application's own clones ([`Self::instantiate`]), so a call site never
    /// monomorphises a polymorphic template; the body is not evaluated, so
    /// only the signature is applied.
    pub(super) fn apply_type(
        &mut self,
        function: FunctionId,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<AnyNodeId>,
    ) -> bool {
        self.apply_type_with(function, argument, block, node, cell, ParameterCheck::Type)
    }

    /// The shared body of the two type applications; the check decides the bind.
    fn apply_type_with(
        &mut self,
        function: FunctionId,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<AnyNodeId>,
        check: ParameterCheck,
    ) -> bool {
        let Some(instantiation) = self.instantiate(function, argument, block, node, check) else {
            return false;
        };
        // The result is the clone's return type, resolved before the bind; the
        // deep pass replicates to no class member.
        let applied = instantiation.applied;
        // The clone's return expr: the applied call's own expr, which states the
        // attribute as the source return does.
        self.record_applied_expr(node, applied);
        let parameter = instantiation.node_of(self.functions[function].parameter);
        self.applied_parameters.insert(node, parameter);
        // Only a clone whose expr is absent is evaluated; forcing one that answers costs an
        // apply the deep budget counts.
        if self.expr_type_item(applied).is_none() {
            self.evaluate_node(Dyn(applied), Some(block));
        }
        let Some(codomain) = self.expr_type_item(applied) else {
            return false;
        };
        self.evaluate_node(codomain, Some(block));
        // A decided codomain stays the ref the clone walk carried; the bind
        // consumes it as a ref.
        if let Some(cell) = cell {
            self.unify_ref(cell, codomain);
        }
        true
    }

    /// Apply a function **type**'s signature for one call, binding `cell` to
    /// that call's instantiated codomain.
    ///
    /// # Invariant
    /// The domain and codomain are this call's own clones ([`Self::instantiate`]),
    /// so a call site never monomorphises a polymorphic template; a type that
    /// names no function binds nothing. `report` runs the parameter check at
    /// the type side and keeps its errors; otherwise only the signature clones.
    /// Answers whether the type side applied (the cloned codomain was bound).
    ///
    /// # Invariant
    /// The **function** is named, never a type: a function's type is the shared
    /// `FunctionType` marker and carries no signature, so the caller states the
    /// function it means.
    pub fn apply_function_type(
        &mut self,
        function: AnyFunctionId,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        cell: AnyNodeId,
        report: bool,
    ) -> bool {
        let check = if report {
            ParameterCheck::Report
        } else {
            ParameterCheck::Skip
        };
        // A frozen callee's signature applies too (`static_function_type_apply`).
        match function {
            AnyFunctionId::Dynamic(function) => {
                self.apply_type_with(function, argument, block, node, Some(cell), check)
            }
            // The value side owns a static callee's signature when nothing reports:
            // it binds the cell when it materializes the body.
            AnyFunctionId::Static(function) if report => {
                self.static_function_type_apply(function, argument, block, node, Some(cell), check)
            }
            AnyFunctionId::Static(function) => {
                // Nothing reports here, but the applied return graph still records the
                // call's own attribute state.
                self.materialize_static_return_apply(function, argument, block, node);
                false
            }
        }
    }

    /// Resolve a wildcard constructor to the implementation inferred for this instantiation.
    pub fn resolve_constructor(&self, function: AnyFunctionId) -> AnyFunctionId {
        let mut current = function;
        for _ in 0..64 {
            let Some(next) = self.constructor_bindings.get(&current).copied() else {
                break;
            };
            if next == current {
                break;
            }
            current = next;
        }
        current
    }

    /// Whether `function` is `_x => _`: an open result independent of its parameter.
    pub(crate) fn is_wildcard_constructor(&self, function: AnyFunctionId) -> bool {
        let function = match function {
            AnyFunctionId::Dynamic(function) => function,
            AnyFunctionId::Static(function) => {
                let module = self.static_module(function.module);
                return crate::static_module::static_wildcard_constructor(&module, function);
            }
        };
        let Some(function) = self.functions.get(function) else {
            return false;
        };
        let function = function;
        if !self.nodes.contains_key(function.r#return)
            || !self.nodes.contains_key(function.parameter)
            || !self.nodes.contains_key(function.return_type)
        {
            return false;
        }
        let Some(return_value) = self.expr_value_half(function.r#return) else {
            return false;
        };
        let Some(parameter_value) = self.expr_value_half(function.parameter) else {
            return false;
        };
        let Some(parameter_type) = self.expr_type_half(function.parameter) else {
            return false;
        };
        let return_type = function.return_type;
        if !self.nodes.contains_key(return_type) {
            return false;
        }
        let open = |node| self.nodes[node].operation.is_none() && self.class_value(node).is_none();
        open(return_value)
            && open(return_type)
            && self.class_root(return_value) != self.class_root(parameter_value)
            && self.class_root(return_type) != self.class_root(parameter_type)
    }

    /// Instantiate `function`'s template for `argument`: clone the return expr, then
    /// apply the check the mode names.
    ///
    /// # Invariant
    /// Every node created is stamped with the apply node's depth plus one, so a node's
    /// depth is a fact about the graph; a nested instantiation restores the previous stamp.
    #[stacksafe]
    pub(super) fn instantiate(
        &mut self,
        function: FunctionId,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        check: ParameterCheck,
    ) -> Option<Instantiation> {
        let stamp = self.stamp_depth;
        self.stamp_depth = self.node_depth(node) + 1;
        let instantiation = self.instantiate_stamped(function, argument, block, node, check);
        self.stamp_depth = stamp;
        instantiation
    }

    #[stacksafe]
    fn instantiate_stamped(
        &mut self,
        function: FunctionId,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        check: ParameterCheck,
    ) -> Option<Instantiation> {
        let (r#return, parameter, assert_count) = {
            let function = &self.functions[function];
            (
                function.r#return,
                function.parameter,
                function.asserts.len(),
            )
        };
        debug_assert!(
            self.functions[function].nodes.contains(&r#return),
            "function {function:?} (block {:?}, parent {:?}) return {return:?} not in scope {:?}",
            self.functions[function].block,
            self.functions[function].parent,
            self.functions[function].nodes
        );
        debug_assert!(self.functions[function].nodes.contains(&parameter));
        let mut remap = HashMap::new();
        let mut minted = HashMap::new();
        let parameter_slot = unsafe { self.array_items_of(AnyNodeId::Dynamic(parameter)) }
            .and_then(|items| items.first().map(|item| item.node))
            .and_then(|node| node.dynamic());
        let mut ctx = ApplyCtx {
            target: block,
            blocks: HashMap::new(),
            // The scope is the applied function's own: the body this walk
            // instantiates is that function's.
            anchor: function,
            scope_root: self.function_scope(function),
            scope_verdicts: HashMap::new(),
            branch_top: function,
            closure_scope: None,
            applied: function,
            parameter,
            parameter_slot,
            in_parameter: false,
            minted_keys: None,
            tag: self.nodes[node].function,
            closure: HashMap::new(),
            remap: &mut remap,
            minted: &mut minted,
        };
        // The parameter is a walk entry point: its unify fires even when the body
        // never references it.  Idempotent.
        ctx.in_parameter = true;
        // The parameter's image: the clone, or the template's own cell.
        let parameter_image = self.node_apply(parameter, &mut ctx);
        ctx.in_parameter = false;
        let applied = self.node_apply(r#return, &mut ctx);
        // The body's asserts come from the function's own registry, instantiated through the
        // remap; only a real clone registers.

        // A check that keeps its errors registers them too: `Check` reports.
        for index in 0..assert_count * usize::from(check != ParameterCheck::Skip) {
            let condition = self.functions[function].asserts[index];
            let instantiated = self.node_apply(condition, &mut ctx);
            if instantiated != condition {
                self.asserts.push(PendingAssert {
                    condition: instantiated,
                    template: Dyn(condition),
                });
            }
        }
        // The clone is unified with the argument, not replaced by it, so the binding propagates.
        if ctx.remap.contains_key(&parameter) {
            // Re-establish the template's class topology among the fresh singleton
            // clones — among the plain cells only.
            if ctx.remap.len() > 1 {
                let groups = crate::apply::regroup_clones(
                    ctx.remap
                        .iter()
                        .map(|(&template, &clone)| (template, clone)),
                    |template| disjoint::find(&mut self.nodes, template),
                )
                .into_iter()
                .filter(|(_, clone)| crate::apply::clone_is_a_plain_cell(self, *clone))
                .collect();
                crate::apply::unify_clone_groups(groups, |first, clone| {
                    self.unify(first, clone);
                });
            }
        }
        // A signature clone binds nothing; the other modes probe the parameter.
        if check != ParameterCheck::Skip {
            let recorded = self.unify_errors.len();
            let failed = self.apply_parameter_check(
                parameter_image,
                argument,
                block,
                node,
                AnyFunctionId::Dynamic(function),
                parameter,
            );
            // The type side discards only the errors this check records.
            if check == ParameterCheck::Type {
                self.unify_errors.truncate(recorded);
            }
            if failed {
                return None;
            }
        }
        // A clone's open cells reach the value of the call that made them through the
        // replay, not through the template.
        self.link_replay(function, argument, block, node, &remap);
        Some(Instantiation { applied, remap })
    }

    /// Link a clone's open cells to a replay of the apply that made them.
    ///
    /// # Invariant
    /// One replay per apply: every open clone cell the applied function's scope owns is
    /// unified with the cell of the replay's own instantiation standing for the same
    /// template node. The replay is stamped run, so the scope that built it leaves the
    /// replay to a later clone of that scope.
    fn link_replay(
        &mut self,
        function: FunctionId,
        argument: AnyNodeId,
        block: BlockId,
        node: NodeId,
        remap: &HashMap<NodeId, NodeId>,
    ) {
        if self.replaying {
            return;
        }
        // Only a template's own hole is what a call has yet to fill.
        let Some(return_value) = self.expr_value_half(self.functions[function].r#return) else {
            return;
        };
        if self.nodes[return_value].operation.is_some() || self.class_value(return_value).is_some()
        {
            return;
        }
        let mut open: Vec<(NodeId, NodeId)> = remap
            .iter()
            .filter(|&(&template, &clone)| {
                self.nodes[clone].operation.is_none()
                    && self.class_value(clone).is_none()
                    && self.function_descends_from(self.nodes[template].function, function)
            })
            .map(|(&template, &clone)| (template, clone))
            .collect();
        if open.is_empty() {
            return;
        }
        // A map's order is not the graph's, and a class's representative depends on order.
        open.sort_by_key(|&(template, _)| template);
        // The apply's own operands: the callee the caller named and the argument it supplied.
        let Ok(operands) = self.operands_of(node) else {
            return;
        };
        let Some((callee, supplied)) = operands.first().zip(operands.get(1)) else {
            return;
        };
        let items = [ArrayItem::new(Dyn(*callee)), ArrayItem::new(Dyn(*supplied))];
        let array = self.alloc_array(&items, block);
        let operand = self.add_node(
            block,
            None,
            Some(P::Value::from(LowValue::Array(ArrayValue::from(array)))),
        );
        let replay = self.add_node(
            block,
            Some(Operation {
                operator: P::Operator::from(crate::LowOperator::Apply),
                operand: Some(operand),
            }),
            None,
        );
        // The run stamp is whether the operator runs; this scope has not run it.
        self.stamp_node_ran_if(replay, true);
        // The correspondence: one instantiation of the same template, whose cells stand for
        // the replay's result at every depth.
        self.replaying = true;
        let replay = self.instantiate(function, argument, block, replay, ParameterCheck::Skip);
        self.replaying = false;
        let Some(replay) = replay else {
            return;
        };
        for (template, clone) in open {
            // A node the replay referenced in place is decided: never unify that with an open
            // cell.
            let Some(&cell) = replay.remap.get(&template) else {
                continue;
            };
            if cell == clone || self.class_value(cell).is_some() {
                continue;
            }
            self.unify(clone, cell);
        }
    }

    /// Evaluate `argument` as deep as `pattern` references it, so the unify sees element
    /// values instead of undecided slots.
    ///
    /// # Invariant
    /// Only array positions recurse; a sub-value the pattern treats as opaque stays
    /// unevaluated.
    #[stacksafe]
    pub(crate) fn evaluate_pattern_argument(
        &mut self,
        pattern: NodeId,
        argument: AnyNodeId,
        block: BlockId,
    ) {
        // The visited set is the module's, so a nested check pops its own.
        let mut seen = self.pattern_seen.pop().unwrap_or_default();
        seen.clear();
        self.evaluate_pattern_argument_inner(Dyn(pattern), argument, block, &mut seen);
        seen.clear();
        self.pattern_seen.push(seen);
    }

    #[stacksafe]
    fn evaluate_pattern_argument_inner(
        &mut self,
        pattern: AnyNodeId,
        argument: AnyNodeId,
        block: BlockId,
        seen: &mut HashSet<(AnyNodeId, AnyNodeId)>,
    ) {
        if !seen.insert((pattern, argument)) {
            return;
        }
        // One force answers both reads.
        let forced = self.evaluate_node(argument, Some(block));
        let pattern_value = match pattern {
            Dyn(pattern) => self.nodes[pattern].value.and_then(|value| value.as_enum()),
            AnyNodeId::Static(pattern) => {
                self.static_read(pattern).and_then(|value| value.as_enum())
            }
        };
        let (Some(LowValue::Array(pattern)), Some(LowValue::Array(argument))) =
            (pattern_value, forced.and_then(|value| value.as_enum()))
        else {
            return;
        };
        // SAFETY: both are live payloads of reachable nodes; the descent releases no block.
        for (pattern_item, argument_item) in unsafe { pattern.items() }
            .iter()
            .zip(unsafe { argument.items() }.iter())
        {
            // A shallow position is opaque: the apply must not force what the marker left lazy.
            if pattern_item.shallow || argument_item.shallow {
                continue;
            }
            self.evaluate_pattern_argument_inner(
                pattern_item.node,
                argument_item.node,
                block,
                seen,
            );
        }
    }

    /// The image of `source`'s block under this pass.
    ///
    /// # Invariant
    /// A parentless block (the root, or a lambda body that outlives its
    /// enclosing scope) maps to the apply's own target; a scope maps to a fresh
    /// child of its parent's image, so each instantiation collects its own.
    fn clone_target(&mut self, source: BlockId, ctx: &mut ApplyCtx<'_>) -> BlockId {
        if let Some(&image) = ctx.blocks.get(&source) {
            return image;
        }
        let image = match self.blocks[source].parent {
            // Seed the parent before this one, so a nested scope nests.
            Some(parent) => {
                let parent_image = self.clone_target(parent, ctx);
                self.add_block(Some(parent_image))
            }
            None => ctx.target,
        };
        ctx.blocks.insert(source, image);
        image
    }

    /// The template root a clone instantiates: the top of its origin chain.
    ///
    /// # Invariant
    /// The chain stops at a released origin: the origin edge is not a keep-alive
    /// edge, so the collector may take the node it names, and the last live node
    /// of the chain is the template root then. A live node's own block outlives
    /// it, so the root's block is a live block.
    fn template_root(&self, node: NodeId, origin: Option<NodeId>) -> NodeId {
        let mut root = node;
        let mut next = origin;
        let mut hops = 0;
        while let Some(origin) = next {
            // The chain stops where the collector took the node.
            if !self.nodes.contains_key(origin) {
                break;
            }
            root = origin;
            hops += 1;
            if hops > 64 {
                break;
            }
            next = self.nodes[root].origin;
        }
        root
    }

    #[stacksafe]
    fn node_apply(&mut self, node: NodeId, ctx: &mut ApplyCtx<'_>) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&node) {
            return clone;
        }
        // One read of the node's own facts: this walk asks for them several times, and each
        // ask is a slot-table lookup.
        let (value, operation, evaluated_deep, function, block, origin) = {
            let source = &self.nodes[node];
            (
                source.value,
                source.operation,
                source.evaluated_deep,
                source.function,
                source.block,
                source.origin,
            )
        };
        // A wildcard is carried into ordinary values; a parameter template
        // receives a fresh undecided cell instead.
        if matches!(value.and_then(|value| value.as_enum()), Some(LowValue::Any)) {
            let fresh = self.add_node(ctx.target, None, None);
            if !ctx.in_parameter {
                self.write_node_value(fresh, value);
            }
            return fresh;
        }
        // Only the parts whose value could differ per call get fresh clones — see
        // `docs/notes/apply-clone-ownership.md`.

        // The clone computes the same thing, so the source's class low type seeds it.
        let low_shape = self.class_low_shape(node);
        // The template root a clone instantiates, whose block may still be a scope.
        let template_root = self.template_root(node, origin);
        // Only a template's own fact is proven; a computed value is that call's.
        let template_decided = template_root == node
            || (self.nodes[template_root].value.is_some()
                && self.nodes[template_root]
                    .evaluated_deep
                    .is_some_and(|deep| !deep.undecided));
        let proven_concrete = evaluated_deep.is_some_and(|e| !e.undecided)
            && operation.is_none()
            && value.is_none_or(|value| !self.answer_elements_are_undecided(value))
            && template_decided;
        // A **closed** cell is shared: decided content, closed all the way down.
        let rehomed = matches!(
            value.and_then(|value| value.as_enum()),
            Some(LowValue::Function(_))
        );
        let closed = !rehomed && self.template_content_is_closed(node, None, &mut ctx.closure);
        // A scope's content is this call's; a clone of a scope's node is scoped too.
        let scoped = self.blocks[block].parent.is_some()
            || self.blocks[self.nodes[template_root].block]
                .parent
                .is_some();
        // Judged against the call's anchor, so one call gives a node one answer.
        let depends_on_parameter = ctx.in_parameter
            || node == ctx.parameter
            || Some(node) == ctx.parameter_slot
            || (!proven_concrete && !closed)
            || scoped
            || self.value_holds_foreign_function(value, ctx.anchor);
        if !depends_on_parameter {
            return node;
        }
        // The scope rule: the class decides, so a clone never splits one.
        if !self.node_class_inside_scope(node, ctx) {
            return node; // outside the scope — reference as-is
        }
        // Reserve the clone id before recursing, so a diamond or a cycle resolves to it.
        let scope = block;
        let target = self.clone_target(scope, ctx);
        let clone = self.add_node(target, None, None);
        // A scope ends at its root, so the clone ends the image it landed in:
        // each instantiation collects its own.
        if self.blocks[scope].scope_root == Some(node) {
            self.blocks[target].scope_root = Some(clone);
        }
        // The clone's origin is the node it instantiates, so a reader holding the clone can
        // reach the source node.
        self.nodes[clone].origin = Some(node);
        // The owner tag: an apply's clones take the apply node's owner; a closure's own scope
        // takes the fresh id.
        let owner = match ctx.closure_scope {
            Some(scope) if scope.contains(&node) => ctx.tag,
            Some(_) => function,
            None => ctx.tag,
        };
        self.set_node_owner(clone, owner);
        ctx.remap.insert(node, clone);
        if let Some(keys) = ctx.minted_keys.as_deref_mut() {
            keys.push(node);
        }
        // The run stamp is whether the operator runs; `evaluated_deep` is the other axis
        // (`apply-clone-ownership.md`).
        let operation = operation.map(|operation| Operation {
            operand: operation
                .operand
                .map(|operand| self.node_apply(operand, ctx)),
            ..operation
        });
        // The deep pass makes an answer a fact every call shares; `runned` is another axis.
        let template_answer = evaluated_deep.is_some();
        // A wildcard answer is never carried: the clone's own
        // cell is its replacement.
        let wildcard_answer = value.is_some_and(|value| value.as_enum() == Some(LowValue::Any));
        let carried = match &operation {
            None => !wildcard_answer,
            Some(_) => {
                // An answer holding a foreign closure is no more carriable than a bare one: mapping
                // cannot reach inside a function body.
                template_answer
                    && !wildcard_answer
                    && !self.value_holds_foreign_function(value, ctx.applied)
            }
        };
        // A mapping runs only for a kept value: a dropped answer must not clone a closure.
        let mapped = if carried {
            value.map(|value| self.value_apply(value, target, ctx))
        } else {
            None
        };
        self.write_node_value(clone, mapped);
        // Only a proved answer is a run: an undecided value is a seed.
        let carried_answer = evaluated_deep.is_some_and(|deep| !deep.undecided)
            && mapped.is_some_and(|value| !self.answer_elements_are_undecided(value));
        self.stamp_node_ran_if(clone, carried_answer);
        self.nodes[clone].operation = operation;
        if self.nodes[clone]
            .operation
            .as_ref()
            .is_some_and(|operation| {
                matches!(
                    AsEnum::<crate::LowOperator>::as_enum(&operation.operator),
                    Some(crate::LowOperator::Apply)
                )
            })
            && let Some(value) = self.expr_value_half(clone)
        {
            self.apply_result_owners.insert(value, clone);
        }
        if let Some(owner) = self.apply_result_owners.get(&node).copied() {
            let owner = self.node_apply(owner, ctx);
            self.apply_result_owners.insert(clone, owner);
        }
        // A clone is a singleton class here, so the slot write *is* the class write.
        self.nodes[clone].low_shape = low_shape;
        // A mark is **not** copied onto the clone: the anchor handoff is
        // `Module::marks_of`, which reads the clone's origin.
        clone
    }

    /// Map one template value onto the clones `target` holds; every payload
    /// moves into that block.
    #[stacksafe]
    fn value_apply(
        &mut self,
        value: P::Value,
        target: BlockId,
        ctx: &mut ApplyCtx<'_>,
    ) -> P::Value {
        match value.as_enum() {
            Some(LowValue::Array(array)) => {
                // Each element rides with its shallow flag; a baked static ref is
                // referenced in place.
                let mut items = self.item_clone_scratch.pop().unwrap_or_default();
                items.clear();
                // SAFETY: `array` is `value`'s live payload and this pass releases no block.
                for &item in unsafe { array.items() } {
                    items.push(ArrayItem {
                        node: match item.node {
                            AnyNodeId::Static(_) => item.node,
                            Dyn(node) => Dyn(self.node_apply(node, ctx)),
                        },
                        ..item
                    });
                }
                // The layout travels with the payload (`docs/notes/array-attribute-view.md`).
                let items_addressed =
                    ArrayValue::with_layout(self.alloc_array(&items, target), array.layout());
                items.clear();
                self.item_clone_scratch.push(items);
                P::Value::from(LowValue::Array(items_addressed))
            }
            Some(LowValue::Table(table)) => {
                // A remap that changes no entry preserves the table's identity.
                let mut changed = false;
                // SAFETY: `table` is `value`'s live payload and this pass releases no block.
                let items: Vec<TableItem> = unsafe { table.items() }
                    .iter()
                    .map(|&item| {
                        let key = match item.key {
                            AnyNodeId::Static(_) => item.key,
                            Dyn(node) => Dyn(self.node_apply(node, ctx)),
                        };
                        let value = match item.value {
                            AnyNodeId::Static(_) => item.value,
                            Dyn(node) => Dyn(self.node_apply(node, ctx)),
                        };
                        let key_is_unchanged = matches!(
                            (self.node_value(key), self.node_value(item.key)),
                            (Some(mapped), Some(original)) if mapped == original
                        );
                        let value_is_unchanged = matches!(
                            (self.node_value(value), self.node_value(item.value)),
                            (Some(mapped), Some(original)) if mapped == original
                        );
                        changed |= !key_is_unchanged || !value_is_unchanged;
                        TableItem {
                            key,
                            value,
                            hash: item.hash,
                        }
                    })
                    .collect();
                if !changed {
                    return value;
                }
                P::Value::from(LowValue::Table(self.alloc_table(&items, target)))
            }
            // A static function value is frozen: its captures are static, so it is referenced in
            // place.
            Some(LowValue::Function(AnyFunctionId::Static(_))) => value,
            Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
                // One closure per call: the first reach mints, the rest read that one back.
                if let Some(minted) = ctx.minted.get(&function).copied() {
                    return P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(minted)));
                }
                // A cloned function's scope is mapped like an array and homed on the target block.
                let (scope, r#return, parameter, return_type, static_origin, asserts) = {
                    let function = &self.functions[function];
                    (
                        function.nodes.clone(),
                        function.r#return,
                        function.parameter,
                        function.return_type,
                        function.static_origin,
                        function.asserts.clone(),
                    )
                };
                // The fresh closure's id is reserved before the walk, so its template reads as members
                // of that id.
                let fresh = self.functions.insert(Function {
                    nodes: Vec::new(),
                    r#return,
                    parameter,
                    return_type: NodeId::default(),
                    // A clone of a materialized static closure is still that
                    // logical function, so the static origin travels with it.
                    static_origin,
                    // The original template: a clone of a clone still answers to it.
                    template: Some(self.functions[function].template.unwrap_or(function)),
                    asserts: Vec::new(),
                    parent: Some(ctx.branch_top),
                    scope: None,
                    block: target,
                    // A clone does not carry its template's `@loop` mark: an instance is expanded like
                    // any other call.
                    looping: None,
                });
                // The fresh closure's scope hangs under the branch it was minted
                // in, so it names the same lexical place its parent does.
                let branch_scope = self.function_scope(ctx.branch_top);
                let fresh_scope = Scope::child(fresh, branch_scope.as_ref());
                self.functions[fresh].scope = Some(fresh_scope);
                // The fresh id is recorded before the scope walk; a scope can name it again.
                ctx.minted.insert(function, fresh);
                // The nested function's scope joins the clone's template, so its captures
                // rewrite to the fresh clones.
                let parameter_slot = unsafe { self.array_items_of(AnyNodeId::Dynamic(parameter)) }
                    .and_then(|items| items.first().map(|item| item.node))
                    .and_then(|node| node.dynamic());
                // The minted template owns its parameter, not the enclosing call's clone.
                let template_parameter = parameter;
                let saved_parameter = ctx.remap.remove(&template_parameter);
                // The nested scope's clones are that call's image; the mint states its own.
                let saved_scope: Vec<(NodeId, NodeId)> = scope
                    .iter()
                    .filter_map(|node| ctx.remap.remove_entry(node))
                    .collect();
                let mut minted_keys = Vec::new();
                let minted = &mut *ctx.minted;
                let mut inner = ApplyCtx {
                    target,
                    blocks: HashMap::new(),
                    anchor: ctx.anchor,
                    // The closure's own scope: a capture is outside it.
                    scope_root: self.function_scope(function),
                    scope_verdicts: HashMap::new(),
                    branch_top: fresh,
                    closure_scope: Some(&scope),
                    applied: function,
                    parameter,
                    parameter_slot,
                    in_parameter: false,
                    minted_keys: Some(&mut minted_keys),
                    tag: Some(fresh),
                    closure: HashMap::new(),
                    remap: ctx.remap,
                    minted,
                };
                let nodes: Vec<NodeId> = scope
                    .iter()
                    .map(|&id| self.node_apply(id, &mut inner))
                    .collect();
                let r#return = self.node_apply(r#return, &mut inner);
                let parameter = self.node_apply(template_parameter, &mut inner);
                // The enclosing pass clones its own parameter when it reaches it.
                inner.remap.remove(&template_parameter);
                // A hand-built function (a lowlevel test) may leave
                // `return_type` unset — clone it only when it names a node.
                let return_type = if self.nodes.contains_key(return_type) {
                    self.node_apply(return_type, &mut inner)
                } else {
                    return_type
                };
                // The fresh closure's asserts instantiate with its scope: a concrete condition stays
                // referenced in place.
                let mut fresh_asserts = Vec::with_capacity(asserts.len());
                for &condition in &asserts {
                    let instantiated = self.node_apply(condition, &mut inner);
                    if instantiated != condition {
                        self.asserts.push(PendingAssert {
                            condition: instantiated,
                            template: Dyn(condition),
                        });
                    }
                    fresh_asserts.push(instantiated);
                }
                drop(inner);
                // The mint keeps its scope's clones; a capture stays shared with the
                // enclosing pass so both walks name one outer clone.
                let scope_members: std::collections::HashSet<NodeId> =
                    scope.iter().copied().collect();
                for key in minted_keys {
                    if scope_members.contains(&key) {
                        ctx.remap.remove(&key);
                    }
                }
                // The enclosing pass's own parameter clone, when it made one before the mint.
                if let Some(saved) = saved_parameter {
                    ctx.remap.insert(template_parameter, saved);
                }
                // The enclosing pass keeps its image of the nested scope.
                for (template, clone) in saved_scope {
                    ctx.remap.insert(template, clone);
                }
                // Nodes cloned into the target are re-stamped to the fresh id, so the fresh template
                // re-instantiates per call.
                for &id in &nodes {
                    if self.nodes[id].block == target {
                        self.set_node_owner(id, Some(fresh));
                    }
                }
                let fresh_function = &mut self.functions[fresh];
                fresh_function.nodes = nodes;
                fresh_function.r#return = r#return;
                fresh_function.parameter = parameter;
                fresh_function.return_type = return_type;
                fresh_function.asserts = fresh_asserts;
                self.blocks[target].functions.push(fresh);
                P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(fresh)))
            }
            // A program-specific value may carry a handle into an arena —
            // relocate it into the target block like any other payload.
            None => Self::copy_ext(self, value, target),
            _ => value,
        }
    }

    /// Whether the class `node` belongs to lies inside the walk's scope.
    ///
    /// # Invariant
    /// The verdict is a fact of the class: every member of one class reads the
    /// same answer, so a clone never splits a class and a class reaching outside
    /// the scope is referenced whole.  A class inside the **applied function's**
    /// scope is cloned too: a closure minted there captures it into the enclosing
    /// call's remap, and the body reads that one image.
    fn node_class_inside_scope(&mut self, node: NodeId, ctx: &mut ApplyCtx<'_>) -> bool {
        let Some(scope) = ctx.scope_root.clone() else {
            return false; // no scope: nothing is inside it
        };
        let representative = self.class_root(node);
        if let Some(&verdict) = ctx.scope_verdicts.get(&representative) {
            return verdict;
        }
        // A minted closure's captures are this call's body: clone them in the anchor.
        let verdict = self.class_inside_scope(node, &scope)
            || (ctx.closure_scope.is_some()
                && self
                    .function_scope(ctx.anchor)
                    .is_some_and(|anchor| self.class_inside_scope(node, &anchor)));
        ctx.scope_verdicts.insert(representative, verdict);
        verdict
    }

    /// Whether `function`'s scope is under `anchor`'s — the lexical membership test.
    ///
    /// # Invariant
    /// A function with no scope — one that was never created through
    /// [`Module::begin_function`], or one whose record is gone — is a member of
    /// nothing, so the answer is `false` rather than a panic.
    pub(crate) fn function_descends_from(
        &self,
        function: Option<FunctionId>,
        anchor: FunctionId,
    ) -> bool {
        let Some(function) = function else {
            return false;
        };
        let (Some(function), Some(anchor)) =
            (self.function_scope(function), self.function_scope(anchor))
        else {
            return false;
        };
        anchor.is_ancestor_or_self(&function)
    }

    /// Whether an answer's own slots are undecided — the dynamic read of the shared
    /// policy.
    ///
    /// # Invariant
    /// One level deep by design: a structure whose elements are decided is a fact a clone
    /// may answer with, however open its interior is.
    fn answer_elements_are_undecided(&self, value: P::Value) -> bool {
        crate::apply::answer_elements_are_undecided::<P>(value, |node| {
            self.node_value(node).is_none()
        })
    }

    /// Whether `value`'s tree holds a closure foreign to `against`.
    ///
    /// # Invariant
    /// Attribution belongs here, where the scope being instantiated is known: `against`'s
    /// own self-reference and an enclosing function stay in place, and a static function is
    /// frozen and never foreign. Callers pass the call's anchor, so one call judges a node
    /// once however deep the walk is.
    fn value_holds_foreign_function(&self, value: Option<P::Value>, against: FunctionId) -> bool {
        let foreign = |function: FunctionId| {
            function != against && !self.function_descends_from(Some(against), function)
        };
        let held = |node: AnyNodeId| match node {
            AnyNodeId::Static(_) => None,
            Dyn(node) => self.nodes[node].value.and_then(|value| value.as_enum()),
        };
        let mut stack: Vec<AnyNodeId> = match value.and_then(|value| value.as_enum()) {
            Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
                return foreign(function);
            }
            // SAFETY: `array`/`table` are payloads the caller holds reachable; this only reads.
            Some(LowValue::Array(array)) => unsafe { array.items() }
                .iter()
                .map(|item| item.node)
                .collect(),
            Some(LowValue::Table(table)) => unsafe { table.items() }
                .iter()
                .flat_map(|item| [item.key, item.value])
                .collect(),
            _ => return false,
        };
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            match held(node) {
                Some(LowValue::Function(AnyFunctionId::Dynamic(function))) if foreign(function) => {
                    return true;
                }
                Some(LowValue::Array(array)) => {
                    // SAFETY: `array` is a live node's payload; this only reads.
                    stack.extend(unsafe { array.items() }.iter().map(|item| item.node))
                }
                Some(LowValue::Table(table)) => {
                    // SAFETY: `table` is a live node's payload; this only reads.
                    for item in unsafe { table.items() } {
                        stack.push(item.key);
                        stack.push(item.value);
                    }
                }
                _ => {}
            }
        }
        false
    }
}
