//! The `@loop` seam: which calls are a **marked recursion**, and the refusal
//! that stands in for a loop until one can be recorded.
//!
//! A marker on a binding says its recursion *may* become a loop; the absence of
//! one still means the unroll, and that default does not move
//! (`docs/notes/loop-conversion.md` §1.1).  So nothing here is reachable
//! unless the source wrote `@loop`, and the only behaviour the marker changes
//! is the **refusal**: a marked recursion whose state the evaluator cannot
//! decide is named ([`DiagKind::LoopNotRecorded`]) instead of reaching a
//! backend as a node nobody can name.
//!
//! # What counts as marked
//!
//! Three facts, in this order, and all three are needed:
//!
//! 1. the callee is an [`ExprKind::Function`] carrying `looping`,
//! 2. that function is **on a cycle** — reachable from its own body through
//!    further applies to marked functions ([`marked_recursions`]), and
//! 3. the call **enters** the cycle rather than being the cycle's own step.
//!
//! (2) is what keeps a `@loop` binding that is not recursive — `@loop inc =
//! x => x + 1` — an ordinary function: a call to it is not part of a
//! recursion, so it must not change behaviour.  (3) is what keeps the
//! recursion's *own* recursive call out of the answer: its argument is the
//! next state, which is undecided by construction for **every** trip count, so
//! testing it would refuse every marked recursion including the ones that
//! expand.  An entering call's argument is the state the trip count is read
//! from, which is a fact about the program rather than about the step.
//!
//! # The cycle is a component, not a function
//!
//! A recursion is a **group** of `@loop` bindings that reach each other, and the
//! group is the unit everything downstream speaks in: §3 step 3's nest has one
//! level per member, and one refusal covers the whole group. So the marked
//! functions are keyed by the **entry of their strongly connected component**
//! ([`component_entries`]), not by themselves — a mutual recursion is one fact,
//! and keying by binding said it twice.

use std::collections::{HashMap, HashSet};

use lichen_lowlevel::{AnyNodeId, LowValue, NodeId};
use lichen_utils::extend::AsEnum;

use crate::attr::AttrSpec;
use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind, IR, Loc};
use crate::program::{HighProgram, TypeOperator, ValueType};

use super::Checker;

/// What the `@loop` markers in an IR say about its recursions.
pub(crate) struct MarkedRecursions {
    /// Each cyclic marked function, mapped to the **entry** of the strongly
    /// connected component it belongs to — §3 step 1's components over the
    /// marked call graph.
    ///
    /// **The entry, not the member.** A component is one fact about the program
    /// — this group of `@loop` bindings reaches each other — and the entry is a
    /// stable, unique key for it, so a refusal or a nest is stated once for the
    /// whole group rather than once per member. Two members of one two-node cycle
    /// share a value here; before this was a component, each was its own key and
    /// a mutual recursion said the same thing twice.
    ///
    /// A marked function on no cycle is **absent**, because `@loop inc = x =>
    /// x + 1` is an ordinary function.
    ///
    /// **The component's member list is not here.** It is what §3 step 2
    /// defunctionalises — one base test, transition and environment per member —
    /// and it belongs to the same Tarjan run rather than to a second walk over
    /// the graph. Adding it is one more field, not another analysis.
    pub cycles: HashMap<ExprId, ExprId>,
    /// Every expression of a cyclic marked function's **own body** — the
    /// cycle calling itself.  A site outside this set enters the cycle.
    pub inside_cycle: HashSet<ExprId>,
}

/// The marked recursions of `ir`: which `@loop`-marked functions are on a
/// cycle, and which expressions that cycle's own body covers.
///
/// Two passes over the marked functions only, so a program without `@loop`
/// costs one scan of the expression table and nothing else.
pub(crate) fn marked_recursions<A: AttrSpec, L>(ir: &IR<A, L>) -> MarkedRecursions {
    let marked: Vec<ExprId> = (0..ir.expr.len() as u32)
        .map(ExprId)
        .filter(|&e| matches!(&ir[e].kind, ExprKind::Function { looping: true, .. }))
        .collect();
    let marked_set: HashSet<ExprId> = marked.iter().copied().collect();
    let mut out = MarkedRecursions {
        cycles: HashMap::new(),
        inside_cycle: HashSet::new(),
    };
    if marked.is_empty() {
        return out;
    }
    // `from` -> the marked functions `from`'s own body applies.  An apply's
    // callee *is* the callee binding's own node (the IR's graph-sharing
    // invariant), so the operand needs no resolution.
    let mut edges: HashMap<ExprId, Vec<ExprId>> = HashMap::new();
    for &from in &marked {
        let ExprKind::Function {
            parameter_type,
            r#return,
            ..
        } = &ir[from].kind
        else {
            continue;
        };
        let mut callees = Vec::new();
        for root in std::iter::once(*r#return).chain(parameter_type.iter().copied()) {
            collect_applies(ir, root, &mut callees);
        }
        edges.insert(
            from,
            callees
                .into_iter()
                .filter(|&to| marked_set.contains(&to))
                .collect(),
        );
    }
    // §3 step 1: the strongly connected components of the marked call graph.
    //
    // **Tarjan, not a per-function reachability search.** The previous test asked
    // "does `f` reach itself" once per marked function, which answers *whether*
    // and not *with whom*: two members of one two-node cycle came back as two
    // answers, and the defunctionalisation needs one group to cut levels from.
    // Tarjan gives the groups in one pass and, because a component pops off the
    // stack at its root, hands each one back in **reverse topological order** —
    // so a component's members arrive sink-first, which is the order §3 step 3
    // needs reversed into first-entry order below.
    let entries = component_entries(&marked, &edges);
    for (function, entry) in entries {
        out.cycles.insert(function, entry);
    }
    // The bodies a cycle covers: a site's own recursion is inside one, and an
    // entering call is not.
    for f in out.cycles.keys().copied().collect::<Vec<_>>() {
        let ExprKind::Function {
            parameter_type,
            r#return,
            ..
        } = &ir[f].kind
        else {
            continue;
        };
        for root in std::iter::once(*r#return).chain(parameter_type.iter().copied()) {
            let mut stack = vec![root];
            while let Some(e) = stack.pop() {
                if !out.inside_cycle.insert(e) {
                    continue;
                }
                stack.extend(ir.children(e));
            }
        }
    }
    out
}

/// Tarjan's strongly connected components over `edges`, restricted to `nodes`.
///
/// Each component comes back with its members **in first-entry order** — the
/// order of `nodes` as it was given, which is the IR's own expression order, so
/// "first entry" is the member whose body the user wrote earliest. That is §3
/// step 3's ordering of the nest's levels.
///
/// **The graph is as large as the program's marked functions**, which is small:
/// a program without `@loop` never reaches here, and one with a handful of
/// marked bindings is a handful of nodes. Recursion is avoided by the index
/// stack rather than by the call depth, so a pathological `@loop` group cannot
/// overflow while being analysed.
/// The **entry** of each strongly connected component of the marked call graph,
/// paired with the functions in it.
///
/// `nodes` order is the IR's own expression order, so "first entry" is the
/// member whose body the user wrote first — which is how §3 step 3 orders the
/// nest's levels.
///
/// **Tarjan, not a per-function reachability search.** The previous test asked
/// "does `f` reach itself" once per marked function, which answers *whether* and
/// not *with whom*: two members of one two-node cycle came back as two answers,
/// and the whole point is that they are one.
///
/// **The graph is as large as the program's marked functions**, which is small: a
/// program without `@loop` never reaches here, and one with a handful of marked
/// bindings is a handful of nodes. Recursion is avoided by the index stack rather
/// than by call depth, so a pathological `@loop` group cannot overflow while
/// being analysed.
fn component_entries(
    nodes: &[ExprId],
    edges: &HashMap<ExprId, Vec<ExprId>>,
) -> Vec<(ExprId, ExprId)> {
    // First-entry order, which is the IR's own expression order: "the member
    // whose body the user wrote first".
    let rank: HashMap<ExprId, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, &node)| (node, position))
        .collect();
    let mut index: HashMap<ExprId, usize> = HashMap::new();
    let mut lowlink: HashMap<ExprId, usize> = HashMap::new();
    let mut on_stack: HashSet<ExprId> = HashSet::new();
    let mut stack: Vec<ExprId> = Vec::new();
    // The traversal's own frames, `(node, how many successors are settled)`.
    let mut frames: Vec<(ExprId, usize)> = Vec::new();
    let mut found: Vec<(ExprId, ExprId)> = Vec::new();
    let mut counter = 0usize;

    for &root in nodes {
        if index.contains_key(&root) {
            continue;
        }
        frames.push((root, 0));
        while let Some(&(node, settled)) = frames.last() {
            // A frame is pushed only for an unvisited node, so this is its one
            // and only entry into `index`.
            if settled == 0 {
                index.insert(node, counter);
                lowlink.insert(node, counter);
                counter += 1;
                stack.push(node);
                on_stack.insert(node);
            }
            let successors = edges.get(&node).map(Vec::as_slice).unwrap_or_default();
            // Descend into one successor at a time. **A successor already on the
            // stack is not descended into again**, and that is the whole of what
            // makes a *mutual* cycle join the component instead of recursing.
            if let Some(&successor) = successors.get(settled) {
                frames.last_mut().expect("non-empty").1 += 1;
                if !index.contains_key(&successor) {
                    frames.push((successor, 0));
                    continue;
                }
                if on_stack.contains(&successor) {
                    let here = lowlink[&node].min(lowlink[&successor]);
                    lowlink.insert(node, here);
                }
                continue;
            }
            // Every successor is settled. The node leaves its frame, its lowlink
            // rises into its parent's, and a root of a component pops the whole
            // component off the stack.
            frames.pop();
            if let Some(&(parent, _)) = frames.last() {
                let here = lowlink[&node];
                let above = lowlink[&parent].min(here);
                lowlink.insert(parent, above);
            }
            if lowlink[&node] != index[&node] {
                continue;
            }
            let mut members = Vec::new();
            while let Some(popped) = stack.pop() {
                on_stack.remove(&popped);
                members.push(popped);
                if popped == node {
                    break;
                }
            }
            // A one-member component is a recursion only when the edge is there:
            // `@loop f = x => g x` where `g` is marked and does not reach back is
            // two components of one, not a cycle.
            if members.len() > 1 || successors.contains(&node) {
                // Tarjan pops sink-first; `rank` is the IR's own order, so one
                // sort states first entry rather than inheriting the traversal's.
                members.sort_by_key(|member| rank[member]);
                let entry = members[0];
                found.extend(members.into_iter().map(|member| (member, entry)));
            }
        }
    }
    found
}

/// Every apply callee in `root`'s subtree.  The IR is a DAG, so `seen` is what
/// keeps a shared body from being walked once per use.
fn collect_applies<A: AttrSpec, L>(ir: &IR<A, L>, root: ExprId, out: &mut Vec<ExprId>) {
    let mut seen = HashSet::new();
    let mut pending = vec![root];
    while let Some(e) = pending.pop() {
        if !seen.insert(e) {
            continue;
        }
        if let ExprKind::Apply { function, .. } = &ir[e].kind {
            out.push(*function);
        }
        pending.extend(ir.children(e));
    }
}

/// The binding an apply ultimately names, following a **curried** chain: the
/// operand of `loop inc` is itself an apply, and so is the operand of
/// `(loop inc) n` — the binding a program names is only the root of that
/// chain.  Every link is the same call, so a call is into a marked binding when
/// its chain roots at one.
///
/// `None` for a chain that reaches a node this does not follow: a
/// `Parameter` (a callee passed in, not a binding), a `Static` import ref, or a
/// kind whose operand is not a call at all.
fn callee_root<A: AttrSpec, L>(ir: &IR<A, L>, mut callee: ExprId) -> Option<ExprId> {
    loop {
        match &ir[callee].kind {
            ExprKind::Apply { function, .. } => callee = *function,
            ExprKind::Function { .. } => return Some(callee),
            _ => return None,
        }
    }
}

/// One compiled entering call into a marked cycle: the apply's IR expression,
/// the lowlevel `Apply` op node it compiled to (the node a backend meets), the
/// **value node of its argument** — the state the trip count is read from —
/// and where the user wrote the call ([`Loc`] names the expression).
#[derive(Clone, Debug)]
pub(crate) struct LoopSite {
    /// The **entry** of the component this call enters, as
    /// [`MarkedRecursions::cycles`] keys it.
    ///
    /// **The component's entry, not the binding the call names**: two members of
    /// one cycle are one fact about the program, and a refusal is stated once
    /// for both.
    pub cycle: ExprId,
    pub node: NodeId,
    pub argument_value: NodeId,
    pub loc: Loc,
}

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<lichen_lowlevel::LowOperator> + From<TypeOperator>,
{
    /// The marked component's entry that an apply **enters**, through a curried
    /// callee chain, or [`None`] when it enters none.
    ///
    /// A `@loop` binding is commonly curried (`loop = f => n => x => …`), and
    /// a call is a *chain* of applies rather than one — `loop inc n i` is
    /// three, and only the innermost operand is the binding.  Reading the
    /// chain is what makes the entry that actually carries the trip count the
    /// one classified, instead of the partial application before it.
    ///
    /// **Which member of the component the chain names does not matter** — that
    /// is what a component is for — so this resolves the chain to the *entry*,
    /// and every member of one component enters the same nest.
    pub(super) fn loop_cycle_entered(&self, callee: ExprId) -> Option<ExprId> {
        let binding = callee_root(&self.ir, callee)?;
        self.loop_cycles.cycles.get(&binding).copied()
    }

    /// The marked components the build could not expand, each refused **once**,
    /// at the first entering call whose state is undecided.
    ///
    /// A component with two entering calls is one fact about the program — it
    /// did not expand — so one refusal naming the first is both enough and less
    /// noise than a message per call site.  The sites are in source order
    /// ([`Checker::check_app`] met them in that order), so which one is named
    /// is decided, not arbitrary.
    pub(super) fn report_open_loop_sites(&mut self) {
        let open: Vec<LoopSite> = {
            let mut refused: HashSet<ExprId> = HashSet::new();
            let mut open = Vec::new();
            for site in &self.loop_sites {
                if refused.contains(&site.cycle) {
                    continue;
                }
                // A decided entry is not a refusal, and it does not speak for
                // the next one: a component may be entered once with a literal
                // and once from a run-time value, and only the second is open.
                if self.value_decided(site.argument_value) {
                    continue;
                }
                refused.insert(site.cycle);
                open.push(site.clone());
            }
            open
        };
        for site in open {
            // **The conversion is what decides the wording.** The refusal for
            // an undecided entry is not one fact but two: a shape the
            // conversion rejects (and which rule rejected it), or a shape that
            // converts with no backend to consume it. The lowlevel answers
            // which from the templates, and the rule rides
            // `DiaryEntry::field` so the message names the cause.
            let verdict = self
                .function_of
                .get(&site.cycle)
                .map(|&function| self.module.loop_conversion(function));
            let (kind, rule) = match verdict {
                Some(Ok(_)) => (DiagKind::LoopNotEmitted, None),
                Some(Err(refusal)) => (DiagKind::LoopNotRecorded, Some(refusal.name())),
                // No compiled function for the component: the checker recorded
                // an entering call but never built the binding (a callee it
                // could not resolve to a function). Nothing converted, so the
                // refusal stands unnamed rather than claiming a rule.
                None => (DiagKind::LoopNotRecorded, None),
            };
            self.record_guard(site.node, site.node, site.loc, kind, rule);
        }
    }

    /// Whether the lowlevel has **decided** `node`'s value by the time the
    /// definition pass ends — the test for a trip count being decidable before
    /// the body is lowered.
    ///
    /// A node is decided when the value its class holds is neither the
    /// undecided marker nor a structure holding one: an array is decided only
    /// when every one of its elements is, so a state vector whose count is a
    /// literal and whose carried value is a run-time read is **not** decided
    /// here.  That is the conservative edge of this test, and it is where
    /// Stage 2's defunctionalisation makes it exact: separating the count from
    /// the rest of the state is the conversion's own first step
    /// (`docs/notes/loop-conversion.md` §3), and until it exists the checker
    /// can only read the whole state.
    ///
    /// The same fact the unroll depends on: the deep pass expands a recursion
    /// by cloning the callee's template and unifying the substituted parameter
    /// with the argument ([`Module::function_apply`]), which terminates exactly
    /// when the state it is given is decided.  A decided state is expanded
    /// before this runs, so a marked program is never refused a recursion the
    /// unroll would have handled.
    fn value_decided(&self, node: NodeId) -> bool {
        let Some(value) = self.module.class_value(node) else {
            // An undecided class is not a decided value: nothing was ever
            // computed here.
            return false;
        };
        match value.as_enum() {
            Some(LowValue::Error) => false,
            Some(LowValue::Array(array)) => {
                // SAFETY: the payload is the value this module holds for
                // `node`, whose home block is alive — the walk below releases
                // nothing.
                unsafe { array.items() }.iter().all(|item| match item.node {
                    AnyNodeId::Dynamic(item) => self.value_decided(item),
                    // A static position is a value a solved module already
                    // decided, so it can never be the undecided one.
                    AnyNodeId::Static(_) => true,
                })
            }
            _ => true,
        }
    }
}
