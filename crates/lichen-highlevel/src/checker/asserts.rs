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
    /// `assert(condition)` — registers the condition's value node; the assert
    /// compiles to the condition itself.
    ///
    /// # Invariant
    /// A condition that never unifies to `USize(1)` stays untriggered rather
    /// than failing: [`Module::check_asserts`] deep-evaluates every assert
    /// after the definition pass, and the apply clone re-checks the
    /// instantiated condition at each call.
    pub(super) fn check_assert(&mut self, e: ExprId, condition: ExprId) -> NodeId {
        self.check_expr(condition);
        // The assert names the value node — element 0 of the pair.
        let value = self.value_of(condition);
        self.register_assert(value, self.loc(e, 0), true, AssertSpelling::Condition);
        let pair = self.state[condition].term.unwrap();
        self.state[e].term = Some(pair);
        self.state[e].val = self.state[condition].val;
        self.state[e].ty = self.state[condition].ty;
        pair
    }

    /// Registers an assert condition: the module worklist entry, plus the
    /// current function's own list.
    ///
    /// # Invariant
    /// The function owns its conditions, so an apply clones them and re-checks
    /// the instantiated one against each call's argument. `user_facing` marks an
    /// explicit `assert` (rendered), not a generated guard, which duplicates the
    /// index eval error. The `Loc` is source-blind, so no span reaches the
    /// module. `spelling` says how a failure reads (see [`AssertSpelling`]).
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
        // Keyed by the template condition, which is what a failure records
        // (`AssertError::template`).
        self.assert_spellings.insert(condition, spelling);
        self.module.add_assert(condition);
        if let Some(function) = self.current_function() {
            self.module.functions[function].asserts.push(condition);
        }
    }
}
