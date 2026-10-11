//! The checker's diagnostics glue: the unification wrappers, the guard
//! channel, and the source-blind location builder.

use std::ops::Range;

use lichen_lowlevel::{AnyNodeId, LowOperator, NodeId};

use crate::diagnostic::{DiagKind, DiaryEntry};
use crate::ir::{ExprId, Loc, LocStep};
use crate::program::{Ctx, HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A checker-issued unification, diary-attributed: records the
    /// [`DiaryEntry`] with the [`Loc`] and check kind.
    ///
    /// # Invariant
    /// The `loc`'s `slot` names which element of the expression's
    /// `[value, type, attrs…]` pair the check is about; its `path` is extended
    /// from the unify's own descent. The return value is the *precondition* of
    /// the expression the caller will build: a guard that refused a construct
    /// has judged it undefined, so its graph is not built behind the refusal
    /// (`refused_pair`).
    pub fn check_unify(&mut self, a: NodeId, b: NodeId, loc: Loc, kind: DiagKind) -> bool {
        let (_, errors) = self.module.try_unify(a, b);
        if errors.is_empty() {
            return true;
        }
        self.record_unify(a, b, loc, kind, None, errors);
        false
    }

    /// The `[value, type]` pair a **refused** expression carries on with: one
    /// fresh undecided cell in each slot.
    ///
    /// # Invariant
    /// A reported definition is a rejected build either way, so the consumer of
    /// a refused expression may read a hole; what it must never read is the graph
    /// the refusal was about. Building that is how an already-reported refusal
    /// goes on to panic ([`Self::fresh_cell`]: an empty cell is undecided's only
    /// in-VM representation).
    pub(super) fn refused_pair(&mut self, e: ExprId) -> NodeId {
        let cell = self.fresh_cell();
        let pair = self.pair_of(cell, cell);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(cell);
        self.state[e].ty = Some(cell);
        pair
    }

    /// Compute both operands of a check before it compares them.
    ///
    /// # Invariant
    /// A gate compares a requirement with the node it judges only when both are
    /// decided: undecided-against-decided is the one unify arm that *writes*,
    /// so an uncomputed operand silently takes the requirement. Two measured
    /// bugs motivated this — see `docs/notes/attributes.md` and
    /// `docs/notes/function-type-merge.md`.
    pub(super) fn compute_operands(&mut self, a: NodeId, b: NodeId) {
        self.module.evaluate_node(AnyNodeId::Dynamic(a), None);
        self.module.evaluate_node(AnyNodeId::Dynamic(b), None);
    }

    /// A checker-issued unification an attribute may relax: a failed unify's
    /// errors are dropped when `is_subtype` holds.
    ///
    /// # Invariant
    /// Both operands are computed first: a compound provider is a `Gcd`-meet
    /// node, and undecided-against-decided is the one arm that writes.
    /// `is_subtype` sees the found side first, the expected side second
    /// ([`AttrExt::unify_slots`](crate::attr::AttrExt::unify_slots)).
    ///
    /// Truncation needs no "nothing was merged" argument: computed operands
    /// are scalar, so the range names its own errors.
    pub fn check_unify_relaxed(
        &mut self,
        a: NodeId,
        b: NodeId,
        loc: Loc,
        kind: DiagKind,
        is_subtype: &dyn Fn(&dyn Ctx<P>, NodeId, NodeId) -> bool,
    ) {
        self.compute_operands(a, b);
        let (_, errors) = self.module.try_unify(a, b);
        if errors.is_empty() {
            return;
        }
        if is_subtype(self, a, b) {
            self.module.unify_errors.truncate(errors.start);
            return;
        }
        self.record_unify(a, b, loc, kind, None, errors);
    }

    /// Record a **guard** failure: a check-time refusal, never a fabricated
    /// `UnifyError`.
    ///
    /// # Invariant
    /// The diary entry is itself the diagnostic: [`DiaryEntry::errors`] is
    /// `None`, and the diagnostics layer emits it at its recording position like
    /// any other failure. `a`/`b` name the offending type node and where it was
    /// rejected; where both sides are one node the caller passes it twice.
    pub(super) fn record_guard(
        &mut self,
        a: NodeId,
        b: NodeId,
        loc: Loc,
        kind: DiagKind,
        field: Option<&'static str>,
    ) {
        self.diary.push(DiaryEntry {
            errors: None,
            seq: self.check_seq,
            a,
            b,
            loc,
            kind,
            field: field.map(|f| f.to_string()),
        });
        self.check_seq += 1;
    }

    /// Record a failed checker-issued unify.
    ///
    /// # Invariant
    /// The entry owns the exact range [`Module::try_unify`] reported, so every
    /// error is attributed to `loc` and `kind` without guessing at ownership.
    /// Only a failed unify is recorded — a successful one produced no range.
    fn record_unify(
        &mut self,
        a: NodeId,
        b: NodeId,
        loc: Loc,
        kind: DiagKind,
        field: Option<&'static str>,
        errors: Range<usize>,
    ) {
        let first = self.module.unify_errors[errors.start].clone();
        let path = shape::tag_descent(&self.module, loc.path.clone(), b, &first.steps);
        self.diary.push(DiaryEntry {
            errors: Some(errors),
            seq: self.check_seq,
            a,
            b,
            loc: Loc {
                expr: loc.expr,
                path,
            },
            kind,
            field: field.map(|f| f.to_string()),
        });
        self.check_seq += 1;
    }

    /// A source-blind location naming `slot` of expression `e` (0 = value,
    /// 1 = type, 2+ = the attribute tail).
    ///
    /// # Invariant
    /// For a check issued through [`Self::check_unify`], the descent beyond the
    /// leading slot is filled from the lowlevel's unify `steps`.
    pub(super) fn loc(&self, e: ExprId, slot: usize) -> Loc {
        let step = match slot {
            shape::PAIR_VALUE_SLOT => LocStep::Value,
            shape::PAIR_TYPE_SLOT => LocStep::Type,
            n => LocStep::Attr(shape::attr_index(n)),
        };
        Loc {
            expr: e,
            path: vec![step],
        }
    }
}
