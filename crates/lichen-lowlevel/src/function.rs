use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, BlockId, Function, FunctionId,
    LowValue, Module, NodeId, Operation, Program, TableItem,
};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

/// The context of a failed apply-time parameter check: the declaration the
/// argument had to satisfy.  `parameter_type` is the applied function's
/// declared (template) parameter-type node, `argument_type` the argument's
/// own type node — the two top-level sides of the failing `unify`, which the
/// raw [`UnifyError`] (deep conflict leaves) discards.  `argument` is the
/// argument's pair node (fallback span source), `apply_node` the apply
/// operation node — the identity of the *edge* whose highlevel structure the
/// checker recorded (`Build::apply_edges`), keyed by it, so a diagnosis can
/// reach the argument's source span even when the argument node is shared.
/// `error_index` is the index into [`Module::unify_errors`] of the first
/// error this parameter check produced — the key back to it for the
/// diagnostics, mirroring the highlevel diary.
#[derive(Debug, Clone, Copy)]
pub struct ApplyError {
    /// The applied function — a static function ref when the apply
    /// materialized a static dependency (no importer span behind it).
    pub function: AnyFunctionId,
    pub parameter_type: NodeId,
    pub argument_type: NodeId,
    pub argument: NodeId,
    pub apply_node: NodeId,
    pub error_index: usize,
}

/// The fixed context of one clone pass: where the clones land, the
/// membership anchor, the running node-id remap (template node to its
/// clone), and the owner tag assigned to the clones this pass creates.
struct ApplyCtx<'a> {
    target: BlockId,
    /// The template membership anchor: the applied function.  A node
    /// belongs to the template iff its [`Node::function`] chain (through
    /// [`Function::parent`]) reaches it — a nested closure's nodes are
    /// members of the enclosing function's template, while a sibling's are
    /// not (the mutual-recursion invariant).
    anchor: FunctionId,
    /// The branch stack top: the id a freshly cloned closure hangs under
    /// ([`Function::parent`]).  The applied function at the top of an
    /// apply; each closure clone re-anchors it at its own fresh id, so
    /// every fresh template's chain runs back through the enclosing
    /// instances to the anchor.  Distinct from [`Self::tag`], which is the
    /// apply node's owner at the top of a direct apply (so an apply's
    /// results read as members of the *enclosing* template).
    branch_top: FunctionId,
    /// The scope of the closure being cloned, when inside a closure
    /// branch.  The chain test alone does not cover it: a closure whose
    /// value flowed in through a unification (a parameter bound to a
    /// function value) is walked under the *enclosing* anchor, and its own
    /// nodes' chains — rooted at the original id, whose parent chain need
    /// not reach that anchor (a top-level closure) — would read as outside
    /// the template, leaving the fresh closure's scope shared across
    /// calls.  The closure's own scope is always in its own template, so
    /// membership is the chain test or a scope hit.
    closure_scope: Option<&'a [NodeId]>,
    /// The function being applied: its own value node is the recursion
    /// self-reference (referenced in place when proven concrete), while any
    /// *other* function value in the scope is a nested closure and must be
    /// cloned per call — its captures bind to this call's clones, which a
    /// concreteness proof of the value node cannot see.
    applied: FunctionId,
    /// The applied function's parameter pair, always cloned: the parameter
    /// check runs against the fresh clone, and every call must bind its own
    /// cells (recursion re-applies the template per level).
    parameter: NodeId,
    /// The owner tag stamped on the clones this pass creates: the apply
    /// node's owning function (so an apply's results read as members of the
    /// enclosing template and are re-instantiated per call), or the fresh
    /// id of a closure being cloned (so its own template reads as members
    /// of that id, not of the original).  Runtime-created nodes with no
    /// template role carry [`None`].
    tag: Option<FunctionId>,
    remap: &'a mut HashMap<NodeId, NodeId>,
}

impl<P: Program> Module<P> {
    /// `#[stacksafe]`: application recursion runs through here (and
    /// [`Module::evaluate_node`]) at one frame per level, so the depth guard
    /// must be able to grow the stack — otherwise a deep recursion overflows
    /// the native stack before the guard panics.
    #[stacksafe]
    pub(super) fn function_apply(
        &mut self,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
        cell: Option<NodeId>,
    ) -> P::Value {
        self.with_apply_frame(|module| {
            let (r#return, parameter, asserts) = {
                let function = &module.functions[function];
                (
                    function.r#return,
                    function.parameter,
                    function.asserts.clone(),
                )
            };
            debug_assert!(
                module.functions[function].nodes.contains(&r#return),
                "function {function:?} (block {:?}, parent {:?}) return {return:?} not in scope {:?}",
                module.functions[function].block,
                module.functions[function].parent,
                module.functions[function].nodes
            );
            debug_assert!(module.functions[function].nodes.contains(&parameter));
            let mut remap = HashMap::new();
            let mut ctx = ApplyCtx {
                target: block,
                // Membership is the chain test, not a scope snapshot: the clones
                // this pass creates are stamped with the apply node's owner, so
                // the enclosing template re-instantiates them per call.
                anchor: function,
                branch_top: function,
                closure_scope: None,
                applied: function,
                parameter,
                tag: module.nodes[node].function,
                remap: &mut remap,
            };
            let applied = module.node_apply(r#return, &mut ctx);
            // The parameter is an entry point of the clone walk, not just a node
            // the return subtree happens to reach: the argument must satisfy the
            // parameter's type even when the body never references the parameter
            // (an ignored parameter), and a parameter read whose value a type
            // annotation pinned is referenced in place and so is invisible from
            // the return.  Walking it regardless guarantees the parameter unify
            // below fires.  Idempotent: if the return clone already remapped it,
            // this returns the same clone.
            module.node_apply(parameter, &mut ctx);
            // The body's asserts are the function's own registry entries (see
            // `Function::asserts`): the return clone cannot reach a condition
            // that no value references, so each one is instantiated through the
            // shared remap — a condition the deep pass proved concrete is
            // per-call invariant and is referenced in place (decided at
            // normalize), while an unbound one rewrites to this call's clones,
            // so the body's assert re-checks against the argument.  Only actual
            // clones register: a fresh entry is a constraint on this call.  A
            // user-facing assert survives the clone — an explicit `assert` is
            // rendered on failure even when the failure came from a per-call
            // clone (the generated bounds guards stay unmarked, so they are
            // still suppressed after cloning).
            for &condition in &asserts {
                let user_facing = module.user_asserts.contains(&condition);
                let instantiated = module.node_apply(condition, &mut ctx);
                if instantiated != condition {
                    module.asserts.push(instantiated);
                    if user_facing {
                        module.user_asserts.insert(instantiated);
                    }
                    if let Some(&span) = module.assert_spans.get(&condition) {
                        module.assert_spans.insert(instantiated, span);
                    }
                }
            }
            // The parameter is cloned like any parameterized node, and the clone
            // is unified with the argument instead of being replaced by it: the
            // class binding propagates the argument's value to every reference
            // to the parameter in the body.
            if let Some(&cloned_param) = ctx.remap.get(&parameter) {
                // The clones are fresh singleton classes; re-establish the
                // template's internal class topology among them, so template
                // nodes unified at definition time (e.g. the elements of a
                // homogeneous array pattern) stay unified after cloning — the
                // elementwise unify below then forces the argument to satisfy
                // the pattern's internal constraints.  A single clone (just the
                // parameter) has no topology to re-establish.
                if ctx.remap.len() > 1 {
                    for clones in crate::apply::regroup_clones(
                        ctx.remap.iter().map(|(&template, &clone)| (template, clone)),
                        |template| disjoint::find(&mut module.nodes, template),
                    )
                    .values()
                    {
                        let first = clones[0];
                        for &clone in &clones[1..] {
                            module.unify(first, clone);
                        }
                    }
                }
                // A failed parameter check leaves the apply's result unknown:
                // the body must not run under a mismatched argument.
                if module.apply_parameter_check(
                    cloned_param,
                    argument,
                    block,
                    node,
                    AnyFunctionId::Dynamic(function),
                    parameter,
                ) {
                    return P::Value::from(LowValue::Parameterized);
                }
            }
            let result = module.evaluate_node(Dyn(applied), Some(block));
            module.wire_apply_result(node, cell, result, applied, block)
        })
    }

    /// Evaluate `argument` to the structural depth `pattern` (the cloned
    /// parameter) references, so the apply's unify sees the argument's
    /// element values instead of unbound slots.  Only array positions in
    /// the pattern recurse; sub-values the pattern treats as opaque stay
    /// unevaluated.  `seen` holds the `(pattern, argument)` pairs on the
    /// current recursion, so a structural cycle (the `Type : Type` universe,
    /// which a typed pattern's spine reaches twice) is walked once instead
    /// of looping.
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
        // The argument side is evaluated through `evaluate_node` so a static
        // ref resolves to its solved value (absolute refs — the recursion
        // below can walk them); the pattern side is always a dynamic clone,
        // read raw.
        self.evaluate_node(argument, Some(block));
        let pattern_value = match pattern {
            Dyn(pattern) => self.nodes[pattern].value.and_then(|value| value.as_enum()),
            AnyNodeId::Static(pattern) => self.static_read(pattern).as_enum(),
        };
        let (Some(LowValue::Array(pattern)), Some(LowValue::Array(argument))) = (
            pattern_value,
            self.evaluate_node(argument, Some(block)).as_enum(),
        ) else {
            return;
        };
        for (pattern_item, argument_item) in pattern.items().iter().zip(argument.items().iter()) {
            // A shallow position on either side is opaque — its subtree
            // stays lazy, so the apply's argument evaluation does not force
            // what the marker deliberately left unevaluated.
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

    #[stacksafe]
    fn node_apply(&mut self, node: NodeId, ctx: &mut ApplyCtx<'_>) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&node) {
            return clone;
        }
        // The chain membership test: a node belongs to the template iff its
        // owner's chain of lexical parents reaches the anchor — or it is one
        // of the closure's own scope nodes, when a closure is being cloned
        // (see [`ApplyCtx::closure_scope`]).  A node whose owner is outside
        // the applied function's nesting (a top-level value, a sibling's
        // body) is referenced as-is.
        let member = self.function_descends_from(self.nodes[node].function, ctx.anchor)
            || ctx.closure_scope.is_some_and(|scope| scope.contains(&node));
        if !member {
            return node; // outside the template scope — reference as-is
        }
        // The body always exists, so only the parts whose value could
        // differ per call need fresh nodes: the parameter and nodes the deep
        // pass could not prove concrete — flagged parameterized nodes, plus
        // nodes whose dependence was never resolved (the deep pass never ran
        // on them).  A node the deep pass proved concrete
        // (`evaluated_deep == Some(EvaluatedDeep { parameterized: false })`)
        // is baked — reference it in place.  The deep pass evaluates an operation node for real even
        // when it merely holds a value (a type annotation's pin is a
        // constraint, not a computation), so a concrete proof on an
        // operation node covers what the operation actually produces — no
        // operation node is special-cased here.  A *function value* is never
        // baked by that proof: its body's dependence on this call is
        // invisible to the deep pass, so any function value other than the
        // applied function's own self-reference (the recursion point) is
        // cloned per call — a nested closure's captures must rebind to this
        // call's clones.  The same goes for a proven-concrete structure
        // *containing* such a function value (a function's pair, a tuple of
        // closures): the proof cannot see through the function's body
        // either.
        let (value, operation, evaluated_deep) = {
            let source = &self.nodes[node];
            (source.value, source.operation, source.evaluated_deep)
        };
        // A node the deep pass proved concrete can be baked (referenced in
        // place); one it never ran on (`None`) or flagged parameterized is
        // cloned.
        let proven_concrete = evaluated_deep.is_some_and(|e| !e.parameterized);
        let depends_on_parameter = node == ctx.parameter
            || !proven_concrete
            || value.is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    Some(LowValue::Function(AnyFunctionId::Dynamic(function)))
                        if function != ctx.applied
                )
            })
            || (proven_concrete && self.value_contains_foreign_function(value, ctx.applied));
        if !depends_on_parameter {
            return node;
        }
        // Reserve the clone id before recursing so diamonds resolve to one
        // clone and value cycles to the clone's own (still evaluating) id.
        let clone = self.add_node(ctx.target, None, None);
        // The owner tag: a node of the closure's own scope joins the fresh
        // id (its template reads as members of that id, re-instantiated per
        // call), while a capture — a member of the *enclosing* template
        // cloned through the closure's edges — keeps the source's own
        // owner.  It is then a member of the enclosing template (the
        // instance this closure references in place), not of the fresh
        // closure: re-cloning it under the fresh id would re-instantiate
        // the captured value on every nested apply, tearing it out of the
        // enclosing instance the walk already built.
        self.nodes[clone].function = if ctx.closure_scope.is_some_and(|scope| scope.contains(&node))
        {
            ctx.tag
        } else {
            self.nodes[node].function
        };
        ctx.remap.insert(node, clone);
        // A cached value on an operation node was computed against the
        // body's parameter and is stale once the argument is mapped in, so
        // such clones are left unevaluated — the kept operand chain
        // recomputes against the argument.  Constant nodes (no operation)
        // carry their remapped value.
        let value = if operation.is_some() {
            None
        } else {
            value.map(|value| self.value_apply(value, ctx))
        };
        let operation = operation.map(|operation| Operation {
            operand: operation
                .operand
                .map(|operand| self.node_apply(operand, ctx)),
            ..operation
        });
        self.write_node_value(clone, value);
        self.nodes[clone].operation = operation;
        clone
    }

    #[stacksafe]
    fn value_apply(&mut self, value: P::Value, ctx: &mut ApplyCtx<'_>) -> P::Value {
        match value.as_enum() {
            Some(LowValue::Array(array)) => {
                // Each element rides with its shallow flag: the flag travels
                // with the remapped node, so each call's clone honors its
                // own markers.  A baked static reference is absolute and
                // per-call invariant — referenced in place.
                let items: Vec<ArrayItem> = array
                    .items()
                    .iter()
                    .map(|&item| ArrayItem {
                        node: match item.node {
                            AnyNodeId::Static(_) => item.node,
                            Dyn(node) => Dyn(self.node_apply(node, ctx)),
                        },
                        ..item
                    })
                    .collect();
                P::Value::from(LowValue::Array(self.alloc_array(&items, ctx.target)))
            }
            Some(LowValue::Table(table)) => {
                // The entry nodes clone like array items; the stored hash
                // travels verbatim — a cloned key holds the same forced
                // value, so its hash stays valid for the fresh instance.
                let items: Vec<TableItem> = table
                    .items()
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
                P::Value::from(LowValue::Table(self.alloc_table(&items, ctx.target)))
            }
            // A static function value is a frozen template — its captures are
            // static, so nothing needs rebinding per call; it is referenced
            // in place (its apply materializes per call).
            Some(LowValue::Function(AnyFunctionId::Static(_))) => value,
            Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
                // A cloned function's scope is mapped like an array: every
                // member and both entry points are cloned into the target,
                // and the result is a fresh function homed on the target
                // block, so it is dropped with it.
                let (scope, r#return, parameter, asserts) = {
                    let function = &self.functions[function];
                    (
                        function.nodes.clone(),
                        function.r#return,
                        function.parameter,
                        function.asserts.clone(),
                    )
                };
                // The fresh closure's id is reserved before the walk so the
                // clones it creates are stamped with it — its own template
                // must read as members of the fresh id (re-instantiated per
                // call), never of the original.  It hangs under the branch
                // stack top — the enclosing instance, or the applied
                // function itself for the outermost closure of an apply —
                // so its chain runs back to the membership anchor.
                let fresh = self.functions.insert(Function {
                    nodes: Vec::new(),
                    r#return,
                    parameter,
                    asserts: Vec::new(),
                    parent: Some(ctx.branch_top),
                    block: ctx.target,
                });
                // The nested function's own scope joins the clone's
                // template: its body may capture the applied function's
                // members (an outer parameter), and those references must
                // be rewritten to the fresh clones — the ones the apply's
                // parameter unify binds.  The remap is shared, so a member
                // reached from the outer body and from here is one clone.
                // `applied` switches to the function being instantiated, so
                // its own self-reference (if recursive) stays in place.
                let target = ctx.target;
                let mut inner = ApplyCtx {
                    target,
                    anchor: ctx.anchor,
                    branch_top: fresh,
                    closure_scope: Some(&scope),
                    applied: function,
                    parameter,
                    tag: Some(fresh),
                    remap: ctx.remap,
                };
                let nodes: Vec<NodeId> = scope
                    .iter()
                    .map(|&id| self.node_apply(id, &mut inner))
                    .collect();
                let r#return = self.node_apply(r#return, &mut inner);
                let parameter = self.node_apply(parameter, &mut inner);
                // The fresh closure's asserts instantiate with its scope: a
                // condition reading the closure's captures rewrites to this
                // call's clones and re-registers, while one proven concrete
                // at build is per-call invariant and stays referenced in
                // place.
                let mut fresh_asserts = Vec::with_capacity(asserts.len());
                for &condition in &asserts {
                    let user_facing = self.user_asserts.contains(&condition);
                    let instantiated = self.node_apply(condition, &mut inner);
                    if instantiated != condition {
                        self.asserts.push(instantiated);
                        if user_facing {
                            self.user_asserts.insert(instantiated);
                        }
                        if let Some(&span) = self.assert_spans.get(&condition) {
                            self.assert_spans.insert(instantiated, span);
                        }
                    }
                    fresh_asserts.push(instantiated);
                }
                // The shared remap may have handed the branch nodes the
                // enclosing walk already cloned — a self-reference: the
                // function's own value node sits in its return subtree, so
                // the outer walk reaches and clones it (with the outer tag)
                // before the closure branch runs, and every other scope node
                // follows through the shared remap.  The fresh template must
                // read as members of the fresh id — re-instantiated per call
                // — so re-stamp its nodes.  Runs after the entry-point walks
                // so they resolve through the original tags.  Only nodes
                // actually cloned into the target are re-stamped: a node the
                // walk referenced in place (the self-reference, proven
                // concrete) lives in its home block and keeps its original
                // owner.  Nodes reached only through the template's edges
                // (captures) keep the enclosing tag: they are already the
                // instance and read as members of the enclosing template,
                // referenced in place by this closure.
                for &id in &nodes {
                    if self.nodes[id].block == ctx.target {
                        self.nodes[id].function = Some(fresh);
                    }
                }
                let fresh_function = &mut self.functions[fresh];
                fresh_function.nodes = nodes;
                fresh_function.r#return = r#return;
                fresh_function.parameter = parameter;
                fresh_function.asserts = fresh_asserts;
                self.blocks[target].functions.push(fresh);
                P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(fresh)))
            }
            // A program-specific value may carry a handle into an arena —
            // relocate it into the target block like any other payload.
            None => Self::copy_ext(self, value, ctx.target),
            _ => value,
        }
    }

    /// Whether `function`'s chain of lexical parents ([`Function::parent`])
    /// reaches `anchor` — the template membership test.  A node whose owner
    /// is the applied function, or a closure nested inside it, belongs to
    /// the template; a node owned by an enclosing function (a capture that
    /// was already instantiated by the enclosing apply) does not.  Walks
    /// with [`SlotMap::get`] so a dangling parent — a function dropped with
    /// its home block — reads as non-membership instead of panicking.
    fn function_descends_from(&self, function: Option<FunctionId>, anchor: FunctionId) -> bool {
        let mut current = function;
        while let Some(f) = current {
            if f == anchor {
                return true;
            }
            current = self.functions.get(f).and_then(|f| f.parent);
        }
        false
    }

    /// Whether `value`'s array tree contains a function value other than
    /// `applied` — a nested closure whose captures must rebind to this
    /// call.  A concreteness proof ([`Node::evaluated_deep`]) cannot see
    /// a function's body, so a proven-concrete structure that contains one
    /// must still be cloned, never referenced in place.  A *static* function
    /// value is frozen — no dynamic captures — and is never foreign.
    fn value_contains_foreign_function(
        &self,
        value: Option<P::Value>,
        applied: FunctionId,
    ) -> bool {
        let Some(value) = value else {
            return false;
        };
        let mut stack: Vec<AnyNodeId> = match value.as_enum() {
            Some(LowValue::Array(array)) => array.items().iter().map(|item| item.node).collect(),
            Some(LowValue::Table(table)) => table
                .items()
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
            match node {
                AnyNodeId::Static(_) => {}
                Dyn(node) => match self.nodes[node].value.and_then(|value| value.as_enum()) {
                    Some(LowValue::Function(AnyFunctionId::Dynamic(function)))
                        if function != applied =>
                    {
                        return true;
                    }
                    Some(LowValue::Array(array)) => {
                        stack.extend(array.items().iter().map(|item| item.node))
                    }
                    Some(LowValue::Table(table)) => {
                        for item in table.items() {
                            stack.push(item.key);
                            stack.push(item.value);
                        }
                    }
                    _ => {}
                },
            }
        }
        false
    }
}
