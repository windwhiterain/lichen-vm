//! Asserts: explicit constraints — an assert registers a condition node
//! that the checker deep-evaluates and requires to be
//! `USize(1)`.  Unlike a unification, the constraint does not *bind* its
//! node: an unbound condition stays untriggered rather than being forced to
//! `1`, and the apply clone re-checks the instantiated condition per call.
//!
//! The walk is the ordinary deep pass, and a shallow-marked array position is
//! **not** descended into.  A condition that sits behind such a mark does not
//! resolve here: the operator's own operand gate
//! ([`OperatorExt::run_deferred`](crate::OperatorExt::run_deferred)) reads the
//! operand array's verdict, which the shallow flag alone makes parameterized, so
//! it answers undecided and the entry stays pending.  The walk that descended the
//! masked positions could not change that — it was deleted — and the lowlevel
//! test that claimed otherwise only ever observed "no error recorded", which is
//! also true of a condition that never resolves.

use crate::{AnyNodeId, LowValue, Module, NodeId, Program};
use lichen_utils::extend::AsEnum;

/// A failed assert: the checked condition resolved to a concrete value
/// other than `USize(1)`.  Structured facts (the condition node, the actual
/// value) so the highlevel layer can attribute a span and render a message
/// without walking the module graph.
#[derive(Debug, Clone, Copy)]
pub struct AssertError<P: Program> {
    /// The asserted condition node — the one that was evaluated.  For a
    /// per-call clone that is the clone, not the body's condition.
    pub condition: NodeId,
    /// The body condition this one came from: a clone's template, or the
    /// condition itself when it is the body's own.  This is the node a host
    /// keyed its own metadata by (a source position, a user-facing flag), so
    /// the lowlevel carries provenance instead of any knowledge of what that
    /// metadata means.  A template cloned out of a static module keeps its
    /// static identity — a host table keyed by the importing module's nodes
    /// simply has no entry for it, which is the same answer it gave when the
    /// lowlevel tracked user-facing conditions itself.
    pub template: AnyNodeId,
    /// The value the condition resolved to.
    pub value: P::Value,
}

/// One entry of the assert worklist: the condition to evaluate beside the
/// body condition it descends from (itself, unless an apply clone
/// instantiated it — see [`AssertError::template`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingAssert {
    pub condition: NodeId,
    pub template: AnyNodeId,
}

impl<P: Program> Module<P> {
    /// The constraint worklist: pop every registered assert and
    /// deep-evaluate its condition, requiring `USize(1)`.  The walk is
    /// [`Self::evaluate_node_deep`], so a shallow-marked position is not
    /// descended into and a condition behind one stays undecided (see the module
    /// docs).
    ///
    /// Each entry is *consumed* once decided: a condition resolving to
    /// anything other than `USize(1)` records an [`AssertError`] in
    /// [`Self::assert_errors`] and the entry is dropped either way.  A
    /// condition that stays lazy (an unbound parameter, or any computation
    /// whose operands cannot resolve) is *not triggered*: no error is
    /// recorded, and the entry is kept as pending — it is the template an
    /// apply clone instantiates against each call's argument.
    ///
    /// Asserts spawned while the worklist drains (a decided condition may
    /// itself apply a function, cloning more asserts) join the same run via
    /// the live-length walk.  One run is a fixpoint: nothing the pass
    /// evaluates can activate an earlier pending entry (the unifications the
    /// pass triggers bind only private per-apply clones), so a single drain
    /// decides everything decidable.  On return, [`Self::asserts`] holds
    /// exactly the pending entries — a later call re-checks only those plus
    /// whatever has been registered since.
    pub fn check_asserts(&mut self) {
        // In-place two-region drain: `[0..pending)` entries judged
        // untriggered, `[pending..i)` already consumed, `[i..len)` the queue
        // including fresh registrations landing behind it.  A GC-dropped
        // entry is consumed like a decided one — there is nothing left to
        // check.
        let mut pending = 0;
        let mut i = 0;
        while i < self.asserts.len() {
            let entry = self.asserts[i];
            let condition = entry.condition;
            i += 1;
            let Some(node) = self.nodes.get(condition) else {
                continue; // the condition's block was garbage-collected
            };
            let block = node.block;
            let Some(value) = self.evaluate_node_deep(condition, Some(block)) else {
                // Not triggered — deferred to the apply clone.  An undecided
                // condition (an unbound parameter, or any computation whose
                // operands cannot resolve) records no error.
                self.asserts.swap(pending, i - 1);
                pending += 1;
                continue;
            };
            if !matches!(value.as_enum(), Some(LowValue::USize(1))) {
                self.assert_errors.push(AssertError {
                    condition,
                    template: entry.template,
                    value,
                });
            }
        }
        self.asserts.truncate(pending);
    }
}
