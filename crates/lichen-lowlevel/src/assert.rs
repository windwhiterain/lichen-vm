//! Asserts: a registered condition node, deep-evaluated and required to be `USize(1)`.
//!
//! # Invariant
//! An undecided condition is pending, not failed — no error is recorded, and a
//! shallow-marked array position is not descended into, so it stays undecided.

use crate::{AnyNodeId, LowValue, Module, NodeId, Program};
use lichen_utils::extend::AsEnum;

/// A failed assert: the condition resolved to a concrete value other than `USize(1)`.
#[derive(Debug, Clone, Copy)]
pub struct AssertError<P: Program> {
    /// The condition node that was evaluated — a clone's own, not the body's.
    pub condition: NodeId,
    /// The body condition this came from: a clone's template, or itself.  Provenance only.
    pub template: AnyNodeId,
    /// The value the condition resolved to.
    pub value: P::Value,
}

/// One worklist entry: the condition to evaluate, beside the body condition it came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingAssert {
    pub condition: NodeId,
    pub template: AnyNodeId,
}

impl<P: Program> Module<P> {
    /// Deep-evaluate every registered assert's condition, requiring `USize(1)`;
    /// anything else records an [`AssertError`].
    ///
    /// # Invariant
    /// - An undecided condition records no error and stays pending: the template
    ///   an apply clone re-checks per call.
    /// - Asserts registered while the worklist drains join the same run; one
    ///   drain is a fixpoint, so nothing it evaluates can activate an earlier
    ///   pending entry.
    pub fn check_asserts(&mut self) {
        // Two-region drain: `[0..pending)` untriggered, `[pending..i)` consumed,
        // `[i..len)` queued.
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
                // Not triggered — the apply clone re-checks it per call.
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
