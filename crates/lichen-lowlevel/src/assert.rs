//! Asserts: explicit constraints — an assert point names a condition node
//! that the checker force-evaluates (ignoring laziness) and requires to be
//! `USize(1)`.  Unlike a unification, the constraint does not *bind* its
//! node: an unbound condition stays untriggered rather than being forced to
//! `1`, and the apply clone re-checks the instantiated condition per call.

use crate::{is_unbound, LowValue, Module, NodeId, Program};
use lichen_utils::extend::AsEnum;

/// A failed assert: the checked condition resolved to a concrete value
/// other than `USize(1)`.  Structured facts (the assert point node, the
/// condition node, the actual value) so the highlevel layer can attribute a
/// span and render a message without walking the module graph.
#[derive(Debug, Clone, Copy)]
pub struct AssertError<P: Program> {
    /// The assert point node — its source span attributes the diagnostic.
    pub assert: NodeId,
    /// The condition node — the checked node.
    pub condition: NodeId,
    /// The value the condition resolved to.
    pub value: P::Value,
}

impl<P: Program> Module<P> {
    /// Force-evaluate every registered assert's condition — ignoring
    /// laziness, so a shallow-marked subtree is fully evaluated — and
    /// require `USize(1)`.  A condition that stays lazy (an unbound
    /// parameter, or any computation whose operands cannot resolve) is
    /// *not triggered*: no error is recorded, and the assert is left
    /// pending — the apply clone re-registers it against the instantiated
    /// body, where the parameter is bound, so it is re-checked per call.  A
    /// condition that resolves to anything other than `USize(1)` records an
    /// [`AssertError`] in [`Self::assert_errors`].
    ///
    /// Runs after the definition pass, so conditions that resolve through
    /// the program's own applies are bound.  The registry grows while the
    /// pass runs (a forced condition may itself apply a function and clone
    /// more asserts), so the walk reads the length live instead of
    /// snapshotting it; an entry whose block was garbage-collected is
    /// skipped.
    pub fn check_asserts(&mut self) {
        let mut i = 0;
        while i < self.asserts.len() {
            let point = self.asserts[i];
            i += 1;
            let Some(condition) = self.nodes.get(point).and_then(|node| node.assert) else {
                continue; // the point's block was garbage-collected
            };
            let block = self.nodes[point].block;
            let value = self.evaluate_node_forced(condition, Some(block));
            if is_unbound(Some(value)) {
                continue; // not triggered — deferred to the apply clone
            }
            if !matches!(value.as_enum(), Some(LowValue::USize(1))) {
                self.assert_errors.push(AssertError {
                    assert: point,
                    condition,
                    value,
                });
            }
        }
    }
}
