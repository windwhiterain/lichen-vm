//! The `assert` rule and the registration it shares with the generated guards.

use lichen_lowlevel::{LowOperator, NodeId};

use crate::diagnostic::AssertSpelling;
use crate::ir::{ExprId, Loc};
use crate::program::{HighProgram, TypeOperator, ValueType};

use super::Checker;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// `assert(condition)` — an explicit constraint, not a unify: the
    /// condition's *value* node is registered as an assert.  The
    /// lowlevel's [`Module::check_asserts`] then deep-evaluates every
    /// assert after the definition pass and requires
    /// `USize(1)` — an undecided condition is not bound to `1`, it stays
    /// untriggered, and the apply clone re-checks the instantiated
    /// condition per call.  The expression compiles to the condition
    /// itself: an assert checks its subject, it does not replace it.
    pub(super) fn check_assert(&mut self, e: ExprId, condition: ExprId) -> NodeId {
        self.check_expr(condition);
        // The checked thing is a `USize`, so the assert names the value
        // node — element 0 of the pair — not the pair itself.
        let value = self.value_of(condition);
        self.register_assert(value, self.loc(e, 0), true, AssertSpelling::Condition);
        let pair = self.state[condition].term.unwrap();
        self.state[e].term = Some(pair);
        self.state[e].val = self.state[condition].val;
        self.state[e].ty = self.state[condition].ty;
        pair
    }

    /// Registers an assert condition: the module worklist entry plus, when
    /// a function body is being compiled, the current function's own list —
    /// the function owns it, so an apply clones it and re-checks the
    /// instantiated condition against each call's argument.  `loc` is
    /// recorded as the runtime attribution edge for the condition node;
    /// `user_facing` marks an explicit `assert` (rendered as a diagnostic) as
    /// opposed to a generated guard (the array-bounds check, which duplicates
    /// the index eval error and is not rendered); `spelling` says how a failure
    /// reads (see [`AssertSpelling`]).  The location is source-blind
    /// (an [`ExprId`]-based [`Loc`]), so no span reaches the lowlevel module.
    pub(super) fn register_assert(
        &mut self,
        condition: NodeId,
        loc: Loc,
        user_facing: bool,
        spelling: AssertSpelling,
    ) {
        self.node_edges.insert(condition, loc);
        if user_facing {
            self.user_asserts.insert(condition);
        }
        // Keyed by the **template** condition, which is what a per-call failure
        // records (`AssertError::template`): the clone's own node is a runtime
        // artifact the diagnostics layer never sees.
        self.assert_spellings.insert(condition, spelling);
        self.module.add_assert(condition);
        if let Some(function) = self.current_function() {
            self.module.functions[function].asserts.push(condition);
        }
    }
}
