use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, BlockId, Function, FunctionId,
    LowValue, Module, NodeId, Operation, PendingAssert, Program, TableItem, is_unbound,
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
    /// The fresh closure this walk has already minted for a template closure:
    /// a source [`FunctionId`] → the fresh [`FunctionId`] minted for it.  The
    /// `Function(fresh)` value is rebuilt on read, so this table needs no value
    /// type of its own.
    ///
    /// A per-call closure is minted **once**.  The walk reaches a template
    /// closure from more than one node (its own value node, and any answer
    /// value that names it — a recursive call's result pair, a captured
    /// binding), and each of those is a per-call fact about the *same*
    /// closure.  Minting per reach would give this call two closures where its
    /// body means one, and the two meet in a unification as two different
    /// functions.  The node-level [`ApplyCtx::remap`] dedups *nodes*; this
    /// dedups the closure those nodes carry.
    minted: &'a mut HashMap<FunctionId, FunctionId>,
}

/// One **instantiation** of a function's template for one argument: the clone
/// of its return pair, and the map from each template node to the node of this
/// instantiation that stands for it.
///
/// This is the half of an apply that its two consumers share.  The unroll
/// evaluates the instantiated return and unwinds; a converted loop instantiates
/// **once per iteration** and reads its test, its next state and its exit out of
/// the same map, so a loop's iterations are ordinary applies with the nesting
/// removed (see `loop_run.rs`).
pub(super) struct Instantiation {
    /// The clone of the function's return pair — what an apply evaluates.
    pub applied: NodeId,
    /// Template node → the node of this instantiation standing for it.
    ///
    /// A template node the walk referenced in place — a concrete or per-call
    /// invariant value — is absent, and *is* its own clone; read through
    /// [`Instantiation::node_of`] rather than indexing the map.
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
        // **The nesting guard, before any work.** `node` is the apply node this
        // instantiation is for, and the node's own depth is how many apply
        // levels it already sits under — a fact of the graph, so an expansion
        // meets this bound at its trip count whether it is forced as it is built
        // or walked later by the deep pass. A converted loop instantiates the
        // same entering apply node every iteration, so it stays at that node's
        // depth however long it runs: it spends work, never nesting.
        if self.depth_exhausted(node) {
            return P::Value::from(LowValue::Parameterized);
        }
        // **A marked recursion runs as a loop.** `@loop` is permission to
        // convert, and a shape that converts ([`Module::loop_conversion`]) is
        // driven by [`Module::apply_loop`] instead of expanded level by level:
        // same values, one instantiation per iteration rather than one per
        // nested level, so the trip count stops costing nesting.  A marked
        // function whose shape does *not* convert, and every unmarked one, is
        // untouched and takes the unroll below.
        if self.function_is_looping(function)
            && let Ok(conversion) = self.loop_conversion(function)
        {
            return self.apply_loop(&conversion, function, argument, block, node, cell);
        }
        self.with_apply_frame(|module| {
            let Some(instantiation) = module.instantiate(function, argument, block, node) else {
                // A failed parameter check leaves the apply's result unknown:
                // the body must not run under a mismatched argument.
                return P::Value::from(LowValue::Parameterized);
            };
            let result = module.evaluate_node(Dyn(instantiation.applied), Some(block));
            module.wire_apply_result(node, cell, result, instantiation.applied, block)
        })
    }

    /// Clone a function template's **signature** — its parameter and return
    /// *type cells* — into the template's own block, preserving the template's
    /// internal class topology among the fresh clones, *without* evaluating
    /// the body or re-registering asserts.
    ///
    /// This is the type-level clone-on-unify primitive, used by
    /// [`Program::unify_function_type`](crate::Program::unify_function_type)
    /// when a function-type node `[Function(fid), ↺]` (a function's own type,
    /// `f : f`) is unified against another type: the fresh clone's signature
    /// cells are bound against the counterpart, so the *template's* shared
    /// cells are never bound and the function stays let-polymorphic. It
    /// reuses the apply clone walk ([`Self::function_apply`]'s `node_apply`
    /// setup) verbatim — the same `Function` re-homing and the same
    /// `unify_clone_groups` topology re-establishment — scoped to the two
    /// type cells and shedding the apply-specific frame (no argument, no body
    /// evaluation, no assert re-registration).
    ///
    /// The type cells are cloned, not the `parameter`/`r#return` pairs:
    /// `r#return` may be an unevaluated operation node (a native-call return)
    /// whose own slots do not name the type, so the return's type is read from
    /// [`Function::return_type`]. The parameter's type is the parameter pair's
    /// slot 1 (always a `[value, type, attrs…]` pair).
    ///
    /// Returns the cloned parameter and return *type* cells (domain, codomain),
    /// or `None` when `function` is not a dynamic function of this module.
    pub fn clone_signature(&mut self, function: FunctionId) -> Option<(NodeId, NodeId)> {
        let (block, parameter, return_type) = {
            let function = &self.functions[function];
            (function.block, function.parameter, function.return_type)
        };
        // A hand-built function (a lowlevel test) may leave `return_type`
        // unset: its signature is not readable, so decline to clone.
        if !self.nodes.contains_key(return_type) {
            return None;
        }
        // The parameter's type cell = the parameter pair's slot 1
        // (`[value, type, attrs…]`). The parameter is always a pair.
        // SAFETY: `parameter` is a live node of this module; nothing here
        // drops a block.
        let param_type = match unsafe { self.array_items(parameter) }
            .and_then(|items| items.get(1))
            .map(|item| item.node)
        {
            Some(Dyn(n)) => n,
            _ => return None,
        };
        let mut remap = HashMap::new();
        let mut minted = HashMap::new();
        let mut ctx = ApplyCtx {
            target: block,
            // The signature's own template is the membership anchor: its
            // type cells and the nodes they reference are members, cloned
            // fresh; captured cells (from an enclosing scope) are outside the
            // chain and referenced as-is, exactly as an apply treats a
            // capture.
            anchor: function,
            branch_top: function,
            closure_scope: None,
            // `applied` is the function being cloned, so its own
            // self-reference (a recursive type) is referenced in place rather
            // than re-homed into a fresh closure — the same rule an apply
            // applies.
            applied: function,
            parameter,
            // The signature clones carry no template role: a later apply of
            // the enclosing scope reaches them (if at all) through a unified
            // class, not through `Function::nodes`, so no owner tag is
            // stamped.
            tag: None,
            remap: &mut remap,
            minted: &mut minted,
        };
        let dom = self.node_apply(param_type, &mut ctx);
        let cod = self.node_apply(return_type, &mut ctx);
        // Re-establish the template's internal class topology among the fresh
        // clones, so a signature whose template unified two cells at
        // definition time (e.g. identity's shared param/return type cell)
        // keeps them unified after cloning — the clone carries the same
        // internal constraints as the template. A single clone has no
        // topology.
        if ctx.remap.len() > 1 {
            let groups =
                crate::apply::regroup_clones(ctx.remap.iter().map(|(&t, &c)| (t, c)), |t| {
                    disjoint::find(&mut self.nodes, t)
                });
            crate::apply::unify_clone_groups(groups, |first, clone| {
                self.unify(first, clone);
            });
        }
        Some((dom, cod))
    }

    /// One **instantiation** of `function`'s template for `argument`: the clone
    /// of its return pair, the template→clone map, and the parameter check that
    /// makes the argument satisfy the declared parameter type.
    ///
    /// This is the half of an apply that its two consumers share.  The unroll
    /// evaluates the instantiated return and unwinds; a converted loop
    /// instantiates **once per iteration** and reads its condition, its next
    /// state and its exit out of the same map, so the loop's iterations are
    /// ordinary applies with the nesting removed.
    ///
    /// **Every node it creates is stamped with the instantiation's depth** —
    /// the apply node's own depth plus one ([`Module::stamp_depth`]) — which is
    /// what makes a node's [`depth`](Module::node_depth) a fact about the graph
    /// rather than about the walk that happened to build it. Nested
    /// instantiation (an argument whose evaluation applies something) restores
    /// the previous stamp on the way out.
    #[stacksafe]
    pub(super) fn instantiate(
        &mut self,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
        node: NodeId,
    ) -> Option<Instantiation> {
        let stamp = self.stamp_depth;
        self.stamp_depth = self.node_depth(node) + 1;
        let instantiation = self.instantiate_stamped(function, argument, block, node);
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
            // Membership is the chain test, not a scope snapshot: the clones
            // this pass creates are stamped with the apply node's owner, so
            // the enclosing template re-instantiates them per call.
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
        // The parameter is an entry point of the clone walk, not just a node
        // the return subtree happens to reach: the argument must satisfy the
        // parameter's type even when the body never references the parameter
        // (an ignored parameter), and a parameter read whose value a type
        // annotation pinned is referenced in place and so is invisible from
        // the return.  Walking it regardless guarantees the parameter unify
        // below fires.  Idempotent: if the return clone already remapped it,
        // this returns the same clone.
        self.node_apply(parameter, &mut ctx);
        // The body's asserts are the function's own registry entries (see
        // `Function::asserts`): the return clone cannot reach a condition
        // that no value references, so each one is instantiated through the
        // shared remap — a condition the deep pass proved concrete is
        // per-call invariant and is referenced in place (decided at
        // normalize), while an unbound one rewrites to this call's clones,
        // so the body's assert re-checks against the argument.  Only actual
        // clones register: a fresh entry is a constraint on this call.  The
        // entry keeps the body condition as its template, which is all the
        // host needs to attribute a per-call failure (a user-facing flag, a
        // source position) through its own table.
        // Walked by index rather than over a clone of the list: the loop
        // body needs `&mut self` to instantiate each condition, and the
        // registry lives on `self` itself, so no borrow of it can be
        // held across the call.  The list is only read here — the entries
        // this loop adds go to `self.asserts`, the per-call registry,
        // not to the function's own.
        for index in 0..assert_count {
            let condition = self.functions[function].asserts[index];
            let instantiated = self.node_apply(condition, &mut ctx);
            if instantiated != condition {
                self.asserts.push(PendingAssert {
                    condition: instantiated,
                    template: Dyn(condition),
                });
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
            if self.apply_parameter_check(
                cloned_param,
                argument,
                block,
                node,
                AnyFunctionId::Dynamic(function),
                parameter,
            ) {
                return None;
            }
        }
        Some(Instantiation { applied, remap })
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
        // SAFETY: `pattern` is a live node of this module (a static ref
        // resolves through the registered module) and `argument` is the value
        // just evaluated from a live node; neither home block is released by
        // the descent below.
        for (pattern_item, argument_item) in unsafe { pattern.items() }
            .iter()
            .zip(unsafe { argument.items() }.iter())
        {
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
        // The clone is a fresh class, but the same computation over the same
        // values, so the source's *class* low type seeds it.  Read through the
        // representative (see `class_low_type`), never from the source's own
        // slot.
        let low_shape = self.class_low_type(node).cloned();
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
        // The clone's template origin: the node it instantiates, so a reader
        // that holds a *clone* — a runtime failure's own operand — can reach the
        // source node the layer above attributes by ([`Module::node_origin`]).
        self.nodes[clone].origin = Some(node);
        // The owner tag.  A plain apply's clones are stamped with the **apply
        // node's owner** ([`ApplyCtx::tag`]) — the template the call sits in —
        // so that template re-instantiates them per call; inheriting the
        // applied function's own id instead would leave them looking like
        // another function's instance, and the enclosing template would
        // reference them in place for every call (measured: `id = x => x`,
        // `f = x => id x`, `f 1` read the first call's parameter cells).  The
        // static path stamps the same way ([`Module::static_node_apply`]).
        //
        // A closure walk is the exception: a node of the closure's own scope
        // joins the fresh id (its template reads as members of that id,
        // re-instantiated per call), while a capture — a member of the
        // *enclosing* template cloned through the closure's edges — keeps the
        // source's own owner.  It is then a member of the enclosing template
        // (the instance this closure references in place), not of the fresh
        // closure: re-cloning it under the fresh id would re-instantiate the
        // captured value on every nested apply, tearing it out of the
        // enclosing instance the walk already built.
        self.nodes[clone].function = match ctx.closure_scope {
            Some(scope) if scope.contains(&node) => ctx.tag,
            Some(_) => self.nodes[node].function,
            None => ctx.tag,
        };
        ctx.remap.insert(node, clone);
        // **Whether the operator runs is `runned`, and nothing else** — the
        // operand's rewrite does not decide it.  The clone carries the
        // template's answer, mapped recursively so every node the answer names
        // is this call's node, and it carries the answer's `runned` with it:
        // an answer the template's own operator produced is this call's answer
        // (the mapping has already substituted this call's nodes), so the
        // operator owes nothing more.  A value whose cells are still unbound is
        // exactly that case — the remap substituted the cells, and whatever
        // binds them (the parameter unify, a field check) binds *these* cells.
        // A struct type expression's answer is one such answer, and carrying it
        // is what stops a re-run from minting a second generation of holes that
        // the call-time constraints never reach.
        //
        // Which axis answers "the operator owes an answer still", and which
        // answers "is the value a *template* fact":
        //
        // * `runned` — the source's **own** operator produced the value
        //   (`false` with a value present means a unification wrote it, an
        //   assertion, not a computation).  A clone that would owe an answer
        //   carries **no value at all**, because a slot holding a value is a
        //   slot a static reader (a backend compiling from this graph) reads
        //   as decided — the two axes are not to be conflated here.
        // * `evaluated_deep` — the **deep pass** evaluated this node, so its
        //   answer is a fact about the *template* and every call shares it.
        //   Without it the slot holds whatever the last runtime application
        //   produced, which is this call's business, not the template's.
        //
        // A **function id** is never carried either: it is a per-call
        // allocation, not a value the operator computed from its operand, and
        // mapping it mints a *second* per-call closure beside the one the
        // element walk already cloned, for the two to meet in a unification.
        // A *constant* node (no operation) always carries its mapped value:
        // there is no operator to owe an answer.
        let operation = operation.map(|operation| Operation {
            operand: operation
                .operand
                .map(|operand| self.node_apply(operand, ctx)),
            ..operation
        });
        let template_answer = self.nodes[node].runned && self.nodes[node].evaluated_deep.is_some();
        let carried = match &operation {
            None => true,
            Some(_) => {
                template_answer
                    && !value
                        .is_some_and(|value| matches!(value.as_enum(), Some(LowValue::Function(_))))
                    // A structure *containing* a foreign function id is no more
                    // carriable than a bare one: the id names a closure minted
                    // by one symbolic application of this template (the
                    // checker's own, with marker captures), and the deep-pass
                    // proof cannot see through its body — the same reason the
                    // bake guard above refuses to reference it in place.  The
                    // clone must carry nothing and re-run, so the call mints
                    // its own closure with this call's captures.
                    && !self.value_contains_foreign_function(value, ctx.applied)
            }
        };
        // The mapping runs only for a value that is kept: an answer that is
        // about to be dropped must not clone a closure into the target block.
        let mapped = if carried {
            value.map(|value| self.value_apply(value, ctx))
        } else {
            None
        };
        self.write_node_value(clone, mapped);
        // An answer whose **own slots are still open** is the operator's result
        // structure — the pair a call answers with — and only that operator's
        // re-run and wiring settle it (the parameter unify, `wire_apply_result`
        // binding the call's cell and its type).  The value still carries (its
        // structure is the template's fact, and mapping it is what puts this
        // call's cells in it), but the clone does not claim the operator's
        // answer: `runned` stays false, so a read runs it and the computed
        // answer is reconciled with the carried one instead of the open slots
        // being read as final.
        let owes_answer = mapped.is_some_and(|value| self.answer_elements_are_unbound(value));
        self.nodes[clone].runned = carried && !owes_answer;
        self.nodes[clone].operation = operation;
        // The clone is still a singleton class here, so the slot write *is* the
        // class write; a later unify joins the two through `add_equality`.
        if let Some(low_shape) = low_shape {
            self.nodes[clone].low_shape = Some(low_shape);
        }
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
                // SAFETY: `array` is the payload of `value`, the value this
                // pass is applying; the caller holds it reachable and
                // `node_apply` never releases a block, so its arena stays
                // alive across the map.
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
                P::Value::from(LowValue::Array(self.alloc_array(&items, ctx.target)))
            }
            Some(LowValue::Table(table)) => {
                // The entry nodes clone like array items; the stored hash
                // travels verbatim — a cloned key holds the same forced
                // value, so its hash stays valid for the fresh instance.
                // SAFETY: `table` is the payload of `value`, the value this
                // pass is applying; the caller holds it reachable and
                // `node_apply` never releases a block, so its arena stays
                // alive across the map.
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
                P::Value::from(LowValue::Table(self.alloc_table(&items, ctx.target)))
            }
            // A static function value is a frozen template — its captures are
            // static, so nothing needs rebinding per call; it is referenced
            // in place (its apply materializes per call).
            Some(LowValue::Function(AnyFunctionId::Static(_))) => value,
            Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
                // **One closure per call.**  The walk reaches a template closure
                // from several nodes, and each reach is a fact about the same
                // closure; minting per reach would give this call two closures
                // where its body means one, and the two would meet in a
                // unification as two different functions.  So the first reach
                // mints and the rest read that one back.
                if let Some(minted) = ctx.minted.get(&function).copied() {
                    return P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(minted)));
                }
                // A cloned function's scope is mapped like an array: every
                // member and both entry points are cloned into the target,
                // and the result is a fresh function homed on the target
                // block, so it is dropped with it.
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
                    return_type: NodeId::default(),
                    // A clone of a materialized static closure is still that
                    // logical function, so the static origin travels with it.
                    static_origin,
                    asserts: Vec::new(),
                    parent: Some(ctx.branch_top),
                    block: ctx.target,
                    // **A clone does not carry its template's `@loop` mark**, and
                    // this is the mark's "permission, not a command" half rather
                    // than an oversight: an instance is what an apply *made*, so
                    // it is expanded like any other call, and a marked recursion
                    // whose state the deep pass can decide is expanded exactly as
                    // an unmarked one is. The mark lives on the template, which
                    // is the only place the recursion is still a cycle — every
                    // apply clones templates away, so the analysis that reads the
                    // mark runs before any of this.
                    looping: false,
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
                let minted = &mut *ctx.minted;
                let mut inner = ApplyCtx {
                    target,
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
                // The fresh closure's asserts instantiate with its scope: a
                // condition reading the closure's captures rewrites to this
                // call's clones and re-registers, while one proven concrete
                // at build is per-call invariant and stays referenced in
                // place.
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
                fresh_function.return_type = return_type;
                fresh_function.asserts = fresh_asserts;
                self.blocks[target].functions.push(fresh);
                ctx.minted.insert(function, fresh);
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

    /// Whether an answer's **own** slots are still unbound — an array whose
    /// elements are cells no operator has filled.  Such an answer is the
    /// operator's own result structure (the pair a call answers with), and the
    /// only thing that settles it is that operator's own re-run and wiring, so
    /// a clone that claimed the operator's answer would read open slots as
    /// final.  One level deep by design: a structure whose *elements* are
    /// decided is a fact a clone may answer with, however open its interior is
    /// (a struct type's field cells are bound by the enclosing call's checks).
    fn answer_elements_are_unbound(&self, value: P::Value) -> bool {
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return false;
        };
        // SAFETY: `array` is the payload of `value`, a value the caller holds
        // reachable; this method only reads, so its home block is not released.
        unsafe { array.items() }.iter().any(|item| match item.node {
            AnyNodeId::Dynamic(node) => is_unbound(self.node_value(Dyn(node))),
            AnyNodeId::Static(sref) => is_unbound(Some(self.static_read(sref))),
        })
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
            // SAFETY: `array`/`table` are payloads of `value`, which the
            // caller holds reachable; this method only reads, so neither home
            // block is released.  The note covers both arms.
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
            match node {
                AnyNodeId::Static(_) => {}
                Dyn(node) => match self.nodes[node].value.and_then(|value| value.as_enum()) {
                    Some(LowValue::Function(AnyFunctionId::Dynamic(function)))
                        if function != applied =>
                    {
                        return true;
                    }
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is the payload of `node`, a live node
                        // of this module; this method only reads.
                        stack.extend(unsafe { array.items() }.iter().map(|item| item.node))
                    }
                    Some(LowValue::Table(table)) => {
                        // SAFETY: `table` is the payload of `node`, a live node
                        // of this module; this method only reads.
                        for item in unsafe { table.items() } {
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
