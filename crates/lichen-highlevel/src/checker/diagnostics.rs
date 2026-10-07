//! The checker's diagnostics glue: the diary-attributed unification wrappers
//! every check issues, the guard-reporting channel that records a check-time
//! refusal as a diagnostic instead of a fabricated unification error, and the
//! source-blind location builder they all point at.

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
    /// [`DiaryEntry`] for the failures it produced (if any), with the
    /// source-blind [`Loc`] and check kind that drive the diagnostic's
    /// expected/found direction.  The `loc`'s `slot` names which element of
    /// the expression's `[value, type, attrs…]` pair the check is about; its
    /// `path` is extended from the unify's own descent (`steps`).
    ///
    /// Nothing is recorded on success: what lands in the diary is the check
    /// that failed, together with the exact [`unify_errors`] range it owns.
    /// **It answers whether it passed**, for the callers whose check is also
    /// the *precondition* of the expression they are about to build: a guard
    /// that just refused a construct has judged it undefined, and the
    /// construct's own graph must not be built behind the refusal
    /// ([`Self::refused_pair`]).
    pub fn check_unify(&mut self, a: NodeId, b: NodeId, loc: Loc, kind: DiagKind) -> bool {
        let (_, errors) = self.module.try_unify(a, b);
        if errors.is_empty() {
            return true;
        }
        self.record_unify(a, b, loc, kind, None, errors);
        false
    }

    /// The `[value, type]` pair a **refused** expression carries on with: one
    /// fresh undecided cell in each slot.  A reported definition is a rejected
    /// build either way, so the consumer of a refused expression may read a
    /// hole; what it must never read is the graph the refusal was about —
    /// building that is how a refusal that has already been reported goes on
    /// to panic ([`Self::fresh_cell`]: an empty cell is undecided's only in-VM
    /// representation, and reading one yields nothing).
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
    /// A check that compares a **requirement** with the node it must judge is
    /// only a check if both sides are *decided*: an operand no operator has run
    /// holds no class value, and undecided-against-decided is the one
    /// unification arm that **writes** — the requirement lands in the operand's
    /// slot and the check reports nothing.  So a gate computes its operands
    /// first; measured twice, from both sides:
    ///
    /// - the attribute gate (`check_unify_relaxed`) with a compound provider: a
    ///   `Gcd`-meet node nothing had evaluated, which made every compound
    ///   perspective annotation accepted whatever its numbers were
    ///   (`docs/notes/attributes.md` §"the gate must compute its operands");
    /// - the type annotation gate ([`Checker::check_ann`]) with an *applied*
    ///   struct constructor: `x`'s type is the constructor application's result,
    ///   so the mismatch between `S1` and `S2` was written over rather than
    ///   compared (`pipeline::an_applied_struct_constructor_keeps_the_occurrence_identity`,
    ///   `docs/notes/applied-struct-nominal-id.md`).
    ///
    /// The single-node run is the right strength: the operand is *one* node
    /// whose own operator reads what it needs, and the deep pass would
    /// additionally descend its whole reachable subtree — for a type operand,
    /// the entire type value — and publish a concreteness verdict over it
    /// ([`Module::evaluate_node_deep`](lichen_lowlevel::Module::evaluate_node_deep)).
    pub(super) fn compute_operands(&mut self, a: NodeId, b: NodeId) {
        self.module.evaluate_node(AnyNodeId::Dynamic(a), None);
        self.module.evaluate_node(AnyNodeId::Dynamic(b), None);
    }

    /// A checker-issued unification that may be relaxed by an attribute's
    /// optional subtype relation.  Attempts the ordinary unify; if it fails
    /// **and** `is_subtype` holds for the two operands, the errors this unify
    /// produced are discarded and the check counts as passed.
    ///
    /// `is_subtype` receives the unify's two operands `(a, b)` — the
    /// *found/value* side first and the *expected/declared* side second, the
    /// same convention as
    /// [`AttrExt::unify_slots`](crate::attr::AttrExt::unify_slots) — as a
    /// `&dyn Ctx<P>` (the
    /// curated context), and returns whether the relation is satisfied.  The
    /// attribute decides which operand is the subtype and which the supertype.
    ///
    /// **Both operands are computed first**, and that is load-bearing rather
    /// than an optimization.  A compound provider is a `Gcd`-meet node
    /// ([`AttrExt::combine`](crate::attr::AttrExt::combine)), not a value, and
    /// nothing else has evaluated the expression by the time its annotation is
    /// checked — the checker's own statement pass runs afterwards
    /// ([`Checker::build`]).  An uncomputed provider against a decided
    /// requirement is *undecided against a value*, which is the one
    /// unification arm that **writes** rather than compares: the requirement
    /// would be written into the provider's slot and the check would report
    /// nothing, so every compound annotation would be accepted whatever its
    /// numbers were.  Forcing here is what makes the gate a gate
    /// (`docs/notes/attributes.md` §"the gate must compute its operands").
    ///
    /// Suppression truncates exactly the range this unify produced, which is
    /// what makes it safe without the "nothing was merged" argument: an
    /// attribute unify's operands are scalar once computed (a perspective is a
    /// `USize` or an undecided cell, never a compound array), so a failed unify
    /// merges nothing and the range names its own errors and no others.  The
    /// checker's attribute check is a *validation gate* — the value itself
    /// flows in through the lowlevel apply's separate clone-unify — so
    /// suppressing leaves the graph correct.
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

    /// Record a **guard** failure — a check-time refusal of an expression that
    /// never reached unification (a non-struct instantiation callee, a missing
    /// named field, an invalid index target).  No `UnifyError` is fabricated:
    /// the diagnostic is the diary entry itself, whose [`DiaryEntry::errors`]
    /// is `None`, and which the diagnostics layer emits at its recording
    /// position like any other failure.
    ///
    /// `a`/`b` name the offending type node and the location it was rejected
    /// at — for the kinds where both sides are the same node (the guard refused
    /// a property of one type, not a relation between two), the caller passes
    /// that node twice, which is what the language layer's wording per kind
    /// expects.
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

    /// Record a failed checker-issued unify: the diary entry owns `errors`,
    /// the exact range [`Module::try_unify`](lichen_lowlevel::Module::try_unify)
    /// reported, so the diagnostics layer attributes each of them to `loc`
    /// and `kind` without guessing at ownership.  Only a failed unify is
    /// recorded — a successful one produced no range to own.
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
    /// 1 = type, 2+ = the attribute tail).  For a check issued through
    /// [`Self::check_unify`], the recursive descent beyond the leading slot is
    /// filled from the lowlevel's unify `steps`.
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
