use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, ArrayValue, BlockId, Function,
    FunctionId, LowValue, Module, NodeId, Operation, PendingAssert, Program, TableItem,
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
    /// The applied function's parameter pair, always cloned: the check runs per call.
    parameter: NodeId,
    /// The parameter pair's value slot: re-minted per call, and never carried into.
    parameter_slot: Option<NodeId>,
    /// Template keys a minted walk inserted; the enclosing pass drops them with the
    /// mint, so it groups only its own clones.
    minted_keys: Option<&'a mut Vec<NodeId>>,
    /// The owner tag stamped on the clones this pass creates: the apply node's owner, or
    /// a closure's fresh id.
    tag: Option<FunctionId>,
    remap: &'a mut HashMap<NodeId, NodeId>,
    /// A source [`FunctionId`] → the fresh closure this walk minted for it.
    ///
    /// # Invariant
    /// One closure per call: the walk reaches a template closure from several nodes, and
    /// minting per reach gives a single call two closures where its body means one.
    minted: &'a mut HashMap<FunctionId, FunctionId>,
}

/// One **instantiation** of a function's template: the return pair's clone and the
/// template→clone map.
pub(super) struct Instantiation {
    /// The clone of the function's return pair — what an apply evaluates.
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
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
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
        let Some(instantiation) =
            self.instantiate(function, argument, block, node, ParameterCheck::Value)
        else {
            // A failed parameter check leaves the apply's result unknown:
            // the body must not run under a mismatched argument.
            return None;
        };
        let result = self.evaluate_node(Dyn(instantiation.applied), Some(block));
        // A closure's type cell is the call's own; the pair sync below owns the verdict.
        if let Some(cell) = cell
            && let Some(codomain) = self.pair_type_item(instantiation.applied)
        {
            let codomain = self.as_dynamic(codomain, block);
            self.evaluate_node(Dyn(codomain), Some(block));
            if !self.is_function_type(codomain) {
                let (_, recorded) = self.try_unify(cell, codomain);
                self.unify_errors.truncate(recorded.start);
            }
        }
        self.wire_apply_result(node, result, instantiation.applied, block)
    }

    /// Apply a function's **type** where its value is not concrete yet.
    ///
    /// # Invariant
    /// `f : f` — the type is the function — so an apply of an unknown value still
    /// applies the type. The domain and codomain are this application's own
    /// clones ([`Self::instantiate`]), so a call site never monomorphises a
    /// polymorphic template; the body is not evaluated, so only the signature is
    /// applied.
    pub(super) fn apply_type(
        &mut self,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> bool {
        self.apply_type_with(function, argument, block, node, cell, ParameterCheck::Type)
    }

    /// The shared body of the two type applications; the check decides the bind.
    fn apply_type_with(
        &mut self,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
        check: ParameterCheck,
    ) -> bool {
        let Some(instantiation) = self.instantiate(function, argument, block, node, check) else {
            return false;
        };
        // The result is the clone's return type, resolved before the bind; the
        // deep pass replicates to no class member.
        let applied = instantiation.applied;
        // The clone's return pair: the applied call's own pair, which states the
        // attribute as the source return does.
        self.applied_pairs.insert(node, applied);
        let parameter = instantiation.node_of(self.functions[function].parameter);
        self.applied_parameters.insert(node, parameter);
        // Only a clone whose pair is absent is evaluated; forcing one that answers costs an
        // apply the deep budget counts.
        if self.pair_type_item(applied).is_none() {
            self.evaluate_node(Dyn(applied), Some(block));
        }
        let Some(codomain) = self.pair_type_item(applied) else {
            return false;
        };
        let codomain = self.as_dynamic(codomain, block);
        self.evaluate_node(Dyn(codomain), Some(block));
        // A decided codomain stays the frozen ref the clone walk carried; a leaf in
        // this block gives the bind its value.
        if let Some(cell) = cell {
            self.unify(cell, codomain);
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
    pub fn apply_function_type(
        &mut self,
        type_node: NodeId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: NodeId,
        report: bool,
    ) -> bool {
        let Some(function) = self.function_type_function(type_node) else {
            return false;
        };
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
                self.materialize_static_return_apply(function, block, node);
                false
            }
        }
    }

    /// Instantiate `function`'s template for `argument`: clone the return pair, then
    /// apply the check the mode names.
    ///
    /// # Invariant
    /// Every node created is stamped with the apply node's depth plus one, so a node's
    /// depth is a fact about the graph; a nested instantiation restores the previous stamp.
    #[stacksafe]
    pub(super) fn instantiate(
        &mut self,
        function: FunctionId,
        argument: NodeId,
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
        argument: NodeId,
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
            // Membership is the chain test, not a scope snapshot.
            anchor: function,
            branch_top: function,
            closure_scope: None,
            applied: function,
            parameter,
            parameter_slot,
            minted_keys: None,
            tag: self.nodes[node].function,
            remap: &mut remap,
            minted: &mut minted,
        };
        // The parameter is a walk entry point, so the unify fires even when the body never
        // references it.  Idempotent.  It is walked **before** the return pair, so a nested
        // closure the body mints captures this call's parameter clone and not the parameter
        // node the call is about to clone away from.
        let traced = self.functions[function].static_origin.is_some();
        if traced {
            eprintln!(
                "TRACE instantiate function={function:?} parameter={parameter:?} return={:?} apply_node={node:?} origin={:?} static_origin={:?} template={:?} parent={:?}",
                r#return,
                self.nodes[node].origin,
                self.functions[function].static_origin,
                self.functions[function].template,
                self.functions[function].parent
            );
        }
        let param_walk = self.node_apply(parameter, &mut ctx);
        let applied = self.node_apply(r#return, &mut ctx);
        if traced {
            eprintln!(
                "TRACE   parameter walked in_place={} owner={:?} chain={:?}",
                param_walk == parameter,
                self.nodes[param_walk].function,
                self.function_chain(param_walk)
            );
            eprintln!(
                "TRACE   return    walked in_place={} owner={:?} chain={:?}",
                applied == r#return,
                self.nodes[applied].function,
                self.function_chain(applied)
            );
            eprintln!(
                "TRACE   remap has parameter={} cloned_param={:?}",
                ctx.remap.contains_key(&parameter),
                ctx.remap.get(&parameter)
            );
        }
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
        if let Some(&cloned_param) = ctx.remap.get(&parameter) {
            // Re-establish the template's class topology among the fresh singleton clones.
            if ctx.remap.len() > 1 {
                let groups = crate::apply::regroup_clones(
                    ctx.remap
                        .iter()
                        .map(|(&template, &clone)| (template, clone)),
                    |template| disjoint::find(&mut self.nodes, template),
                );
                crate::apply::unify_clone_groups(groups, |first, clone| {
                    self.unify(first, clone);
                });
            }
            // A signature clone binds nothing; the other modes probe the parameter.
            if check != ParameterCheck::Skip {
                let recorded = self.unify_errors.len();
                let failed = self.apply_parameter_check(
                    cloned_param,
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
        }
        Some(Instantiation { applied, remap })
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
        argument: NodeId,
        block: BlockId,
    ) {
        self.evaluate_pattern_argument_inner(
            Dyn(pattern),
            Dyn(argument),
            block,
            &mut HashSet::new(),
        );
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
        // The argument goes through `evaluate_node` (a static ref resolves); the pattern is a
        // dynamic clone, read raw.
        self.evaluate_node(argument, Some(block));
        let pattern_value = match pattern {
            Dyn(pattern) => self.nodes[pattern].value.and_then(|value| value.as_enum()),
            AnyNodeId::Static(pattern) => {
                self.static_read(pattern).and_then(|value| value.as_enum())
            }
        };
        let (Some(LowValue::Array(pattern)), Some(LowValue::Array(argument))) = (
            pattern_value,
            self.evaluate_node(argument, Some(block))
                .and_then(|value| value.as_enum()),
        ) else {
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

    #[stacksafe]
    fn node_apply(&mut self, node: NodeId, ctx: &mut ApplyCtx<'_>) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&node) {
            return clone;
        }
        // One shared node: each reach mints its own undecided cell, never a cached one.
        let wildcard = self.nodes[node].value.or_else(|| self.nodes[node].value);
        if matches!(
            wildcard.and_then(|value| value.as_enum()),
            Some(LowValue::Any)
        ) {
            return self.add_node(ctx.target, None, None);
        }
        // Membership is the anchor chain test or a closure-scope hit; else in place.
        // A closure is instantiated from its **template**, so a node owned by a function
        // nested in that template is reached through the template as well: the clone's own
        // id is not in any lexical chain, and a body node left outside the walk would keep
        // reading the template's cells instead of this call's.
        let anchor_template = self.functions[ctx.anchor].template;
        let member = self.function_descends_from(self.nodes[node].function, ctx.anchor)
            || anchor_template.is_some_and(|template| {
                self.function_descends_from(self.nodes[node].function, template)
            })
            || ctx.closure_scope.is_some_and(|scope| scope.contains(&node));
        if !member {
            return node; // outside the template scope — reference as-is
        }
        // Only the parts whose value could differ per call get fresh clones — see
        // `docs/notes/apply-clone-ownership.md`.

        // The clone computes the same thing, so the source's class low type seeds it.
        let low_shape = self.class_low_type(node).cloned();
        let (value, operation, evaluated_deep) = {
            let source = &self.nodes[node];
            (source.value, source.operation, source.evaluated_deep)
        };
        // The template root a clone instantiates, whose block may still be a scope.
        let template_root = {
            let mut root = node;
            let mut hops = 0;
            while let Some(origin) = self.nodes[root].origin {
                root = origin;
                hops += 1;
                if hops > 64 {
                    break;
                }
            }
            root
        };
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
        // A scope's content is this call's; a clone of a scope's node is scoped too.
        let scoped = self.blocks[self.nodes[node].block].parent.is_some()
            || self.blocks[self.nodes[template_root].block]
                .parent
                .is_some();
        // Judged against the call's anchor, so one call gives a node one answer.
        let depends_on_parameter = node == ctx.parameter
            || Some(node) == ctx.parameter_slot
            || !proven_concrete
            || scoped
            || self.value_holds_foreign_function(value, ctx.anchor);
        if !depends_on_parameter {
            return node;
        }
        // Reserve the clone id before recursing, so a diamond or a cycle resolves to it.
        let scope = self.nodes[node].block;
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
        self.nodes[clone].function = match ctx.closure_scope {
            Some(scope) if scope.contains(&node) => ctx.tag,
            Some(_) => self.nodes[node].function,
            None => ctx.tag,
        };
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
        // A clone is a singleton class here, so the slot write *is* the class write.
        if let Some(low_shape) = low_shape {
            self.nodes[clone].low_shape = Some(low_shape);
        }
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
                // Each element rides with its shallow flag; a baked static ref is referenced in place.

                // SAFETY: `array` is `value`'s live payload and this pass releases no block.
                let items: Vec<ArrayItem> = unsafe { array.items() }
                    .iter()
                    .map(|&item| ArrayItem {
                        node: match item.node {
                            AnyNodeId::Static(_) => item.node,
                            Dyn(node) => Dyn(self.node_apply(node, ctx)),
                        },
                        ..item
                    })
                    .collect();
                // The layout travels with the payload (`docs/notes/array-attribute-view.md`).
                P::Value::from(LowValue::Array(ArrayValue::with_layout(
                    self.alloc_array(&items, target),
                    array.layout(),
                )))
            }
            Some(LowValue::Table(table)) => {
                // The entry nodes clone like array items; the stored hash travels verbatim.

                // SAFETY: `table` is `value`'s live payload and this pass releases no block.
                let items: Vec<TableItem> = unsafe { table.items() }
                    .iter()
                    .map(|&item| TableItem {
                        key: match item.key {
                            AnyNodeId::Static(_) => item.key,
                            Dyn(node) => Dyn(self.node_apply(node, ctx)),
                        },
                        value: match item.value {
                            AnyNodeId::Static(_) => item.value,
                            Dyn(node) => Dyn(self.node_apply(node, ctx)),
                        },
                        hash: item.hash,
                    })
                    .collect();
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
                    return_marker: None,
                    return_field: None,
                    // A clone of a materialized static closure is still that
                    // logical function, so the static origin travels with it.
                    static_origin,
                    // The template is the **original** one: a closure cloned from a closure
                    // still answers to the template it was built from, or the membership test
                    // would read a per-call id no lexical chain names.
                    template: Some(self.functions[function].template.unwrap_or(function)),
                    asserts: Vec::new(),
                    parent: Some(ctx.branch_top),
                    block: target,
                    // A clone does not carry its template's `@loop` mark: an instance is expanded like
                    // any other call.
                    looping: None,
                });
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
                    branch_top: fresh,
                    closure_scope: Some(&scope),
                    applied: function,
                    parameter,
                    parameter_slot,
                    minted_keys: Some(&mut minted_keys),
                    tag: Some(fresh),
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
                        self.nodes[id].function = Some(fresh);
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

    /// Whether `function`'s chain of lexical parents reaches `anchor` — the template
    /// membership test.
    ///
    /// # Invariant
    /// Walks with [`SlotMap::get`], so a dangling parent reads as non-membership rather
    /// than panicking.
    pub(crate) fn function_descends_from(
        &self,
        function: Option<FunctionId>,
        anchor: FunctionId,
    ) -> bool {
        let mut current = function;
        while let Some(f) = current {
            if f == anchor {
                return true;
            }
            current = self.functions.get(f).and_then(|f| f.parent);
        }
        false
    }

    /// The owner chain of a node, for the trace.
    pub(crate) fn function_chain(&self, node: NodeId) -> Vec<FunctionId> {
        let mut chain = Vec::new();
        let mut current = self.nodes[node].function;
        while let Some(f) = current {
            chain.push(f);
            current = self.functions.get(f).and_then(|f| f.parent);
        }
        chain
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
