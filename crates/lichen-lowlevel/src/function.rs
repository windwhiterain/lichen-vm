use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, BlockId, Function, FunctionId,
    LowValue, Module, NodeId, Operation, PendingAssert, Program, TableItem,
};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

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
        self.with_apply_frame(|module| {
            let Some(instantiation) = module.instantiate(function, argument, block, node, true)
            else {
                // A failed parameter check leaves the apply's result unknown:
                // the body must not run under a mismatched argument.
                return None;
            };
            let result = module.evaluate_node(Dyn(instantiation.applied), Some(block));
            module.wire_apply_result(node, result, instantiation.applied, block)
        })
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
    ) {
        let Some(instantiation) = self.instantiate(function, argument, block, node, false) else {
            return;
        };
        // The result is the clone's return type, resolved before the bind; the
        // deep pass replicates to no class member.
        let applied = instantiation.applied;
        // Only a clone whose pair is absent is evaluated; forcing one that answers costs an
        // apply the deep budget counts.
        if self.pair_type_half(applied).is_none() {
            self.evaluate_node(Dyn(applied), Some(block));
        }
        let Some(codomain) = self.pair_type_half(applied) else {
            return;
        };
        self.evaluate_node(Dyn(codomain), Some(block));
        if let Some(cell) = cell {
            self.unify(cell, codomain);
        }
    }

    /// Instantiate `function`'s template for `argument`: clone the return pair, then run
    /// the parameter check.
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
        check_parameter: bool,
    ) -> Option<Instantiation> {
        let stamp = self.stamp_depth;
        self.stamp_depth = self.node_depth(node) + 1;
        let instantiation =
            self.instantiate_stamped(function, argument, block, node, check_parameter);
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
        check_parameter: bool,
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
        let mut ctx = ApplyCtx {
            target: block,
            blocks: HashMap::new(),
            // Membership is the chain test, not a scope snapshot.
            anchor: function,
            branch_top: function,
            closure_scope: None,
            applied: function,
            parameter,
            tag: self.nodes[node].function,
            remap: &mut remap,
            minted: &mut minted,
        };
        let applied = self.node_apply(r#return, &mut ctx);
        // The parameter is a walk entry point, so the unify fires even when the body never
        // references it.  Idempotent.
        self.node_apply(parameter, &mut ctx);
        // The body's asserts come from the function's own registry, instantiated through the
        // remap; only a real clone registers.

        // The type side registers nothing: the value side reports each assert.
        for index in 0..assert_count * usize::from(check_parameter) {
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
            // The type side discards only the errors this check records.
            let recorded = self.unify_errors.len();
            let failed = self.apply_parameter_check(
                cloned_param,
                argument,
                block,
                node,
                AnyFunctionId::Dynamic(function),
                parameter,
            );
            if !check_parameter {
                self.unify_errors.truncate(recorded);
            }
            if failed {
                return None;
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
        // Membership is the anchor chain test or a closure-scope hit; otherwise the node is
        // referenced in place.
        let member = self.function_descends_from(self.nodes[node].function, ctx.anchor)
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
        // A value holding a foreign closure is cloned too: the proof cannot see through a body.
        let proven_concrete = evaluated_deep.is_some_and(|e| !e.undecided);
        // A scope's content is this call's: sharing it would collect the
        // template.
        let scoped = self.blocks[self.nodes[node].block].parent.is_some();
        // Judged against the call's anchor, so one call gives a node one answer.
        let depends_on_parameter = node == ctx.parameter
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
        // The run stamp is whether the operator runs; `evaluated_deep` is the other axis
        // (`apply-clone-ownership.md`).
        let operation = operation.map(|operation| Operation {
            operand: operation
                .operand
                .map(|operand| self.node_apply(operand, ctx)),
            ..operation
        });
        let template_answer =
            self.nodes[node].runned != 0 && self.nodes[node].evaluated_deep.is_some();
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
        // An answer with open slots is the operator's result structure: it carries, but the run
        // stamp stays zero.
        let carried_answer = mapped.is_some_and(|value| !self.answer_elements_are_undecided(value));
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
                P::Value::from(LowValue::Array(self.alloc_array(&items, target)))
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
                    // A clone of a materialized static closure is still that
                    // logical function, so the static origin travels with it.
                    static_origin,
                    asserts: Vec::new(),
                    parent: Some(ctx.branch_top),
                    block: target,
                    // A clone does not carry its template's `@loop` mark: an instance is expanded like
                    // any other call.
                    looping: false,
                });
                // The fresh id is recorded before the scope walk; a scope can name it again.
                ctx.minted.insert(function, fresh);
                // The nested function's scope joins the clone's template, so its captures
                // rewrite to the fresh clones.
                let minted = &mut *ctx.minted;
                let mut inner = ApplyCtx {
                    target,
                    blocks: HashMap::new(),
                    anchor: ctx.anchor,
                    branch_top: fresh,
                    closure_scope: Some(&scope),
                    applied: function,
                    parameter,
                    tag: Some(fresh),
                    remap: ctx.remap,
                    minted,
                };
                let nodes: Vec<NodeId> = scope
                    .iter()
                    .map(|&id| self.node_apply(id, &mut inner))
                    .collect();
                let r#return = self.node_apply(r#return, &mut inner);
                let parameter = self.node_apply(parameter, &mut inner);
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
