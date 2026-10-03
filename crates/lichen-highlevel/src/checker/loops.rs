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
    /// The `@loop`-marked functions that lie on a cycle, each mapped to
    /// itself — the group a refusal is stated once for.  Stage 0 does not
    /// compute the strongly connected components of §3, so two members of one
    /// two-node cycle are two groups and can each be refused; that is the
    /// analysis Stage 2 owns, and until it exists the honest key is the
    /// binding the call names.
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
    // A function is cyclic when it reaches itself.  The least fixed point of
    // "reachable from here" is a worklist, not a transitive closure — this
    // graph is as large as the program's marked functions, which is small.
    let mut cyclic: HashSet<ExprId> = HashSet::new();
    for &start in &marked {
        let mut seen: HashSet<ExprId> = HashSet::new();
        let mut pending: Vec<ExprId> = edges.get(&start).cloned().unwrap_or_default();
        while let Some(next) = pending.pop() {
            if !seen.insert(next) {
                continue;
            }
            if next == start {
                cyclic.insert(start);
                break;
            }
            pending.extend(edges.get(&next).cloned().unwrap_or_default());
        }
    }
    // The cyclic set is the answer; no component key is computed here.  A
    // refusal is therefore stated once per **marked binding entered**, which is
    // one fact about the program and is what this stage can actually see: the
    // strongly connected components are Stage 2's own analysis
    // (`docs/notes/loop-conversion.md` §3), and a two-node cycle whose two
    // entries both fail says the same thing twice.
    out.cycles = cyclic.into_iter().map(|f| (f, f)).collect();
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
    /// The marked binding this call enters, as [`MarkedRecursions::cycles`]
    /// keys it.
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
    /// The marked cycle an apply **enters**, through a curried callee chain,
    /// or [`None`] when it enters none.
    ///
    /// A `@loop` binding is commonly curried (`loop = f => n => x => …`), and
    /// a call is a *chain* of applies rather than one — `loop inc n i` is
    /// three, and only the innermost operand is the binding.  Reading the
    /// chain is what makes the entry that actually carries the trip count the
    /// one classified, instead of the partial application before it.
    pub(super) fn loop_cycle_entered(&self, callee: ExprId) -> Option<ExprId> {
        let binding = callee_root(&self.ir, callee)?;
        self.loop_cycles.cycles.get(&binding).copied()
    }

    /// The marked cycles the build could not expand, each refused **once**, at
    /// the first entering call whose state is undecided.
    ///
    /// A marked binding with two entering calls is one fact about the program
    /// — it did not expand — so one refusal naming the first is both enough and
    /// less noise than a message per call site.  The sites are in source order
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
                // the next one: a binding may be entered once with a literal
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
            self.record_guard(
                site.node,
                site.node,
                site.loc,
                DiagKind::LoopNotRecorded,
                None,
            );
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
            // An unbound class is not a decided value: nothing was ever
            // computed here.
            return false;
        };
        match value.as_enum() {
            Some(LowValue::Parameterized | LowValue::Void) => false,
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
