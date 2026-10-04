//! Structured, source-blind diagnostics for the highlevel checker.
//!
//! The highlevel never sees source positions — the source↔IR mapping is the
//! language frontend's own record.  So a diagnostic is expressed purely in
//! terms of the checker's own facts: the [`Loc`] (an IR expression plus its
//! position within the `[value, type]` pair), the two conflicting nodes, and
//! (for the non-unify kinds) the runtime fact that failed.
//!
//! The lowlevel records failures as facts ([`Module::unify_errors`],
//! [`Module::eval_errors`], [`Module::assert_errors`]).  This module turns
//! them into structured `Diag`s, attributing each to a [`Loc`] through the
//! records the checker kept while building: the [`Build::diary`], the
//! [`Build::apply_edges`], and the [`Build::node_edges`] — all keyed by node,
//! never on a node, so the lowlevel graph stays freely shareable.
//!
//! No `message` is stored: the language layer re-renders the wording from
//! the structured facts in its own type syntax.

use std::collections::HashSet;
use std::ops::Range;

use lichen_lowlevel::{AnyNodeId, BudgetExhausted, EvalError, LowValue, NodeId, Program};

use crate::{
    checker::Build,
    ir::{ExprKind, Loc, LocStep},
    program::{HighProgram, ValueType},
};

/// What kind of check a unification failure implements — drives the
/// expected/found direction of a [`Diag::Mismatch`]'s `a`/`b`.  All are
/// *type-mismatch* constructs; each variant names its own expected/found
/// category, so there is no separate coarse value/type discrimination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagKind {
    /// `inner : T` — expected = the annotation's type value.
    Annotation,
    /// An operand's type must have a particular **shape** and does not —
    /// expected = that shape.  The shape guards: a callee that must be a
    /// function (`check_lam`/`check_app`), a container that must be an array or
    /// a table (the index and table-lookup pins), and a membership test's right
    /// operand, which must be a set.
    Guard,
    /// Indexing a concretely non-indexable type (a function, an atomic
    /// type) — expected = a tuple, array, or struct type.
    IndexTarget,
    /// A named field read `a.name` on a struct that has no such field —
    /// expected = a field of that name, found = a struct without it.
    NamedField,
    /// A `.name` argument in a struct instantiation against a field the
    /// struct has no named field for — expected = a field of that name.
    StructUnknownField,
    /// A struct instantiation supplying the same named field twice.
    StructDuplicateField,
    /// A struct instantiation omitting a field (or supplying fewer arguments
    /// than the definition has fields).
    StructMissingField,
    /// A struct instantiation supplying more arguments than the definition has
    /// fields.
    StructExcessField,
    /// A `.name` argument in a struct instantiation against a struct type with
    /// no named fields.
    StructAnonymousField,
    /// A struct **definition** with a field that has
    /// no name — `struct<Int, Type>`.  Every struct field carries a name
    /// (`struct<.name T>`), because a struct
    /// instance reads by name only: the positional form `a(k)` is the
    /// *tuple* read (`docs/language-spec.md` §Structs).  A *block*'s fields are
    /// its bindings and need no such check: an expression statement there is
    /// simply not a field.
    StructFieldName,
    /// A struct instantiation whose callee's type is concretely not a struct
    /// type — structs are nominal, so only a struct type instantiates.
    /// Expected = a struct type, found = the callee's type.
    InstantiateCallee,
    /// A homogeneous literal's elements must share one type — an array's
    /// elements or a set's members.  Expected = the shared element type,
    /// found = this element's type.
    ArrayElement,
    /// A table literal's keys must share one type.
    TableKey,
    /// A table literal's values must share one type.
    TableValue,
    /// A binary operator's operand must be an `Int`.
    BinOp,
    /// A class conversion's operand must be the direction's *source* class —
    /// `Int` for `int2float`, `Float` for `float2int`.  The one operator kind
    /// whose own type is the other class.
    Conv,
    /// An attribute (perspective) check: an expression's attribute slot must
    /// equal the expected one.
    Attribute,
    /// A runtime apply-time failure (the parameter type check, executed by
    /// the VM) — no diary entry.  `a` = the parameter's expected type,
    /// `b` = the argument's found type.
    Runtime,
    /// A failed assert (see [`Diag::Assert`]) — not a mismatch.
    Assert,
    /// An out-of-bounds index (see [`Diag::Index`]).
    IndexOutOfBounds,
    /// A table read that missed (see [`Diag::TableMiss`]).
    TableMiss,
    /// A table build dropped a non-concrete key (see [`Diag::TableKeyUnbound`]).
    TableKeyUnbound,
    /// A read whose **runtime** target turned out not to be a container — the
    /// lowlevel's [`EvalError::IndexTarget`](lichen_lowlevel::EvalError::IndexTarget)
    /// reached the diagnostics as a fact about a *value*, with no type to
    /// print, so the wording is self-contained.  Distinct from
    /// [`Self::IndexTarget`], which reports a *type* the checker refused to
    /// index and therefore names that type.
    RuntimeIndexTarget,
    /// A raw read (`X<e>`, `X::a`) whose **element** turned out not to be a
    /// pair at runtime, so the element's own type slot — the read's type — does
    /// not exist.  The same lowlevel
    /// [`EvalError::IndexTarget`](lichen_lowlevel::EvalError::IndexTarget) as
    /// [`Self::RuntimeIndexTarget`], but the non-container is the element the
    /// read just produced, not the container the user wrote: the container
    /// (`[1, 2]`) was fine and the element (`1`) is a scalar.  Reached from
    /// source, never statically — the raw read validates nothing, so only the
    /// read itself can see it.  Distinct wording because the generic
    /// "this value is not a container" blames the wrong side of the read.
    RuntimeRawElement,
    /// An imported package whose export is not the `[value, type]` pair the
    /// importer reads — an export that never evaluated to a pair at all.
    /// Expected = a pair.
    ImportExport,
    /// A read whose **runtime subscript** turned out not to be an index — the
    /// lowlevel's [`EvalError::IndexSubscript`](lichen_lowlevel::EvalError::IndexSubscript)
    /// reached the diagnostics as a fact about a *value*, with no type to
    /// print, so the wording is self-contained.
    RuntimeIndexSubscript,
    /// An apply whose **runtime** target turned out not to be a function — the
    /// lowlevel's [`EvalError::ApplyTarget`](lichen_lowlevel::EvalError::ApplyTarget)
    /// reached the diagnostics as a fact about a *value*, with no type to
    /// print, so the wording is self-contained.  Distinct from
    /// [`Self::Guard`], which reports a *type* the checker refused to apply
    /// and therefore names that type.
    RuntimeApplyTarget,
    /// A `$name(args…)` call whose `name` no plugin registered with this
    /// module — the checker resolves `$name` against the module's own private
    /// [`NativeOps`](crate::NativeOps) registry, and this is the miss.  `a`/`b`
    /// are unused; the operator name rides in [`DiaryEntry::field`].
    NativeOpUnresolved,
    /// A `$name(args…)` call whose builder returned a term the checker cannot
    /// adopt: the three
    /// [`NativeApply`](crate::NativeApply) records must name one `[value,
    /// type]` pair — exactly two slots, element 1 the returned `ty`, element 0
    /// the returned `val` when the value is a decided node — built in the block
    /// the call is compiled into.  Every downstream read of the term reads
    /// those two slots, so anything else would install a term the checker never
    /// checked.  `a`/`b` are unused; the operator name rides in
    /// [`DiaryEntry::field`].
    NativeOpContract,
    /// A schema that carries an attribute, checked by a build that has no
    /// attribute extension to lower it — the public
    /// [`Checker::build`](crate::checker::Checker::build) and
    /// [`Checker::build_in`](crate::checker::Checker::build_in) install none.
    /// `a`/`b` are unused; the checker records this at the site that read the
    /// attribute rather than panicking, and the slot falls back to the
    /// attribute's well-formed hole.
    NoAttributeExtension,
    /// A top-level binding whose value computation never terminates — the VM's
    /// apply/depth guard fired while the build evaluated the user-written
    /// statement.  The checker reports this as an error instead of panicking.
    /// `a`/`b` are unused.
    NonTerminating,
    /// A `@loop`-marked recursion whose trip count was **not** decided before
    /// the body is lowered, and whose shape does **not** convert to a loop —
    /// the refusal half of Stage 0 in `docs/notes/loop-conversion.md` §8.  It
    /// replaces an emitter-side complaint that could only name a `NodeId`:
    /// this names the mark, the missing decision, and the missing loop.
    ///
    /// `a`/`b` carry the apply node the refusal is about, which is otherwise
    /// only a node the emitter meets.  [`DiaryEntry::field`] carries **which
    /// shape rule refused** ([`LoopRefusal::name`](lichen_lowlevel::LoopRefusal::name)),
    /// so the wording and the rule cannot drift; it is `None` when no
    /// conversion was run for the component.
    LoopNotRecorded,
    /// A `@loop`-marked recursion whose trip count was not decided, and whose
    /// shape **does** convert ([`Module::loop_conversion`](lichen_lowlevel::Module::loop_conversion))
    /// — the conversion exists, no backend consumes it yet.  Distinct from
    /// [`LoopNotRecorded`](Self::LoopNotRecorded) because the two ask the user
    /// for different things: one says the program's shape is not a loop, the
    /// other says the loop is ready and the backend is missing.
    ///
    /// `a`/`b` carry the apply node the refusal is about.
    LoopNotEmitted,
    /// A failed build that [`Build::diagnostics`] could attribute *nothing* to:
    /// the checker recorded a failure, but every recorded failure was skipped
    /// for want of an expression to blame.  The one live producer is an assert
    /// cloned out of an imported module — [`AssertError::template`] is then a
    /// node of that module, and this build's attribution tables hold no entry
    /// for it.  The assembly layer substitutes exactly one of these so a failed
    /// report never carries an empty diagnostic list; `loc` is `None`, since
    /// there is no expression in this source to point at.
    UnattributedFailure,
}

/// One checker check, attributed with where it came from.
///
/// The kind of check is [`Self::errors`], not an incidental reading of it: a
/// guard rejected the expression before any unify happened, so it owns no
/// error range and the entry is itself the whole diagnostic, while a failed
/// unification owns the range it produced.  What every entry has is a
/// recording position ([`Self::seq`]), which is the output order.
#[derive(Clone, Debug)]
pub struct DiaryEntry {
    /// What the check produced: `None` for a **guard** failure, which never
    /// unified, and `Some(range)` for the [`Module::unify_errors`] entries a
    /// failed unification produced.  One unify may own several of them (e.g.
    /// elementwise), so this is the range rather than a single index.
    ///
    /// The range is never empty: a unification that produced no error is not
    /// recorded at all (see
    /// [`Checker::check_unify`](crate::checker::Checker::check_unify)).  That
    /// is what makes this field the discriminant — reading an empty range as
    /// "a guard" would have classified a future informational entry with no
    /// owned error as a guard failure, which skips the whole definition pass.
    pub errors: Option<Range<usize>>,
    /// The position this entry was recorded at — the checker's monotonic
    /// recording counter.  It is the diagnostic's place in the output order
    /// (see [`Build::diagnostics`]), independent of whether it owns errors.
    pub seq: usize,
    pub a: NodeId,
    pub b: NodeId,
    /// The source-blind location of the mismatch — the IR expression and its
    /// position within the `[value, type]` spine.
    pub loc: Loc,
    pub kind: DiagKind,
    /// The offending (or missing) field name: a `a.name` named-field read
    /// where the struct has no such field, or a `struct<...>` instantiation
    /// mismatch.
    pub field: Option<String>,
}

/// A structured diagnostic.  The highlevel emits *facts*, never a rendered
/// message: the language layer derives the wording and the caret from these
/// fields, mapping a [`Diag::loc`] (when present) back to a source span
/// through its own source↔IR record.
/// What a failed assert of a condition should *say*, beyond the channel's
/// generic "expected 1, found …".
///
/// The channel reports one shape because an explicit `@assert e` means exactly
/// that: this condition is not `1`.  A **refinement** failure means something
/// more specific to a reader — the value is outside the set of classes the
/// contract admits — so when whoever registers it knows that set, it says so
/// (`docs/notes/operator-polymorphism.md` §8.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssertSpelling {
    /// An explicit `@assert e`, or a generated guard: the condition's own text.
    Condition,
    /// A refinement whose contract names the class **domain** in `domain`, to be
    /// rendered beside the failure.  An operator's checker built that domain, so
    /// it has it; a refinement a *user* wrote names no set, and reads as the
    /// channel's generic failure.
    Refinement { domain: NodeId },
    /// A **named read's container-kind requirement**: `a.name`'s container must
    /// be a struct type ([`crate::program::TypeOperator::IsStructType`]).
    ///
    /// `container` is the condition's checked subject — the container's type —
    /// and is the diagnostic's *found* side.  The checker registers this for an
    /// **undecided** container only: the read cannot be judged where it stands
    /// (a parameter's type is still a cell), so the requirement rides the assert
    /// channel and the apply clone re-checks it per call
    /// (`docs/notes/eval-before-unify.md` §6.2 option 1).  The field is filled
    /// from the **failing condition**'s operand, not from the registered node:
    /// a per-call clone's subject is the actual argument's type, while the
    /// template's own cell is still open and would render as a bare `?a`.
    StructKind { container: NodeId },
}

#[derive(Clone, Debug)]
pub struct Diag<P: Program> {
    /// The source-blind location of the diagnostic — the IR expression and its
    /// position within the `[value, type]` spine.  `None` only for a
    /// source-less failure (a static dependency's apply, an internal bind).
    pub loc: Option<Loc>,
    /// What kind of check failed — the mismatch sub-kind, or the specific
    /// non-mismatch kind ([`DiagKind::Assert`], [`DiagKind::IndexOutOfBounds`],
    /// [`DiagKind::TableMiss`], [`DiagKind::TableKeyUnbound`]).
    pub kind: DiagKind,
    /// The source-meaningful conflicting sides — the checker's operands (the
    /// parameter's type and the argument's type for a runtime failure).
    pub a: NodeId,
    pub b: NodeId,
    /// The conflicting classes' values at error time (snapshots).
    pub value_a: Option<P::Value>,
    pub value_b: Option<P::Value>,
    /// The resolved value of a failed assert (meaningful when
    /// `kind == DiagKind::Assert`).
    pub assert_value: Option<P::Value>,
    /// The **spelling** a failed assert was registered with — the extra facts
    /// the registering site knew beyond the channel's generic "expected 1,
    /// found …" (see [`AssertSpelling`]).  The renderer spells it; the
    /// registration is keyed by the assert's *template*, so this survives the
    /// per-call clone.  `None` for every other failure, and for an assert whose
    /// spelling is the channel's own
    /// [`Condition`](AssertSpelling::Condition).
    pub assert_spelling: Option<AssertSpelling>,
    /// The offending index of an out-of-bounds read (meaningful when
    /// `kind == DiagKind::IndexOutOfBounds`).
    pub index: Option<usize>,
    /// The container's length for an out-of-bounds read.
    pub length: Option<usize>,
    /// The offending (or missing) field name: a `a.name` named-field read
    /// where the struct has no such field, or a `struct<...>` instantiation
    /// mismatch.
    pub field: Option<String>,
    /// The budget the VM's guard refused on — the reason a non-termination
    /// diagnostic exists at all ([`DiagKind::NonTerminating`]).  `None` for
    /// every other kind; [`None`] for this kind means the record predates the
    /// budget being recorded, which the panel builders no longer produce.
    pub budget: Option<BudgetExhausted>,
    /// Which `Module::unify_errors` entry a mismatch came from — the key back
    /// to its diary entry, for callers that re-render.
    pub error_index: Option<usize>,
    /// The **template** of a failed assert whose body lives in another module:
    /// the static node the condition was cloned from, present exactly when
    /// `loc` is `None` for that reason.  This build's tables hold no entry for a
    /// static template, so the position is the *other* module's; a host that
    /// kept that module's source turns this into one
    /// (`docs/notes/core-prelude.md`), and a host that kept none leaves the
    /// failure unattributed.
    pub static_template: Option<lichen_lowlevel::StaticNodeId>,
}

impl<P: Program> Diag<P> {
    /// The [`Loc`] this diagnostic is attributed to, if any.
    pub fn loc(&self) -> Option<&Loc> {
        self.loc.as_ref()
    }

    /// A diagnostic that is only its kind and its location: every value,
    /// index, field and budget slot is empty.  Most failure kinds carry no
    /// more than that, so this is the common shape, and a new [`Diag`] field
    /// is filled in once here rather than at every construction site.
    pub fn factual(kind: DiagKind, loc: Option<Loc>) -> Self {
        Diag {
            loc,
            kind,
            a: NodeId::default(),
            b: NodeId::default(),
            value_a: None,
            value_b: None,
            assert_value: None,
            assert_spelling: None,
            index: None,
            length: None,
            field: None,
            budget: None,
            error_index: None,
            static_template: None,
        }
    }

    /// The placeholder for a failed build [`Build::diagnostics`] rendered
    /// nothing for — see [`DiagKind::UnattributedFailure`].  The report's
    /// assembly layer emits exactly one of these when a failed build would
    /// otherwise carry an empty diagnostic list, upholding the invariant that
    /// every consumer relies on.
    pub fn unattributed_failure() -> Self {
        Diag::factual(DiagKind::UnattributedFailure, None)
    }
}

impl<P: HighProgram> Build<P>
where
    P::Value: ValueType,
{
    /// Render the lowlevel's failure facts as structured diagnostics.
    ///
    /// The checker-attributed failures come out **in recording order**: every
    /// diary entry carries the [`Checker`](crate::checker::Checker) counter
    /// value it was recorded at (`DiaryEntry::seq`), and a unification error
    /// is emitted at the `seq` of the entry whose owned range contains it —
    /// so a guard failure (which unified nothing) interleaves with real
    /// unification failures exactly where it was recorded.  A unification
    /// error with *no* owner (a deep apply-time failure the checker never
    /// issued) is emitted after all of them, then come the runtime evaluation
    /// failures (deduplicated) and the user-facing asserts.
    pub fn diagnostics(&self) -> Vec<Diag<P>> {
        // One index for the whole report: the diary scan below is per error
        // without it, and an editor calls this on every keystroke.
        let index = self.unify_error_index();
        let mut out = Vec::new();
        for entry in &self.nonterminating {
            out.push(Diag {
                budget: entry.budget,
                ..Diag::factual(DiagKind::NonTerminating, Some(entry.loc.clone()))
            });
        }
        // Checker-attributed failures, in recording order.  A guard entry owns
        // no errors and contributes exactly one diagnostic at its own `seq`;
        // a unify entry contributes one diagnostic per error it owns, all at
        // its own `seq` (they are the same failed unify).  Sorting by `seq` is
        // stable, so several failures of one unify stay in their own order.
        let mut attributed: Vec<(usize, Diag<P>)> = Vec::new();
        for entry in &self.diary {
            let Some(errors) = entry.errors.clone() else {
                // A guard failure: the check refused before unifying, so the
                // entry itself is the diagnostic — no error, no expected/found
                // sides beyond what the guard recorded.
                attributed.push((
                    entry.seq,
                    Diag {
                        a: entry.a,
                        b: entry.b,
                        field: entry.field.clone(),
                        ..Diag::factual(entry.kind, Some(entry.loc.clone()))
                    },
                ));
                continue;
            };
            for i in errors {
                attributed.push((entry.seq, self.mismatch(i, &index)));
            }
        }
        attributed.sort_by_key(|&(seq, _)| seq);
        out.extend(attributed.into_iter().map(|(_, diag)| diag));
        // A unification error no diary entry owns — a deep apply-time failure,
        // recorded by the lowlevel rather than by a checker-issued check.  It
        // has no recording position, so it lands after every attributed one.
        out.extend(index.orphan_indexes().map(|i| self.mismatch(i, &index)));
        // Runtime evaluation failures (an out-of-bounds index, a table read).
        // The value and type evaluation of the same expression each record
        // one, so identical facts collapse to a single diagnostic — the key
        // is the failure's *kind and blamed node*, so two genuinely
        // different failures (two out-of-bounds reads, a dropped build key
        // and a later read miss) never collapse into each other.
        let mut seen = HashSet::new();
        for err in &self.module.eval_errors {
            let key = match err {
                EvalError::Index {
                    index,
                    index_value,
                    length,
                } => (0, Some(*index), Some(*index_value), Some(*length)),
                EvalError::TableMiss { key, .. } => (1, Some(*key), None, None),
                EvalError::TableKeyUnbound { key } => (2, Some(*key), None, None),
                EvalError::IndexTarget { target } => (3, Some(*target), None, None),
                EvalError::IndexSubscript { subscript } => (4, Some(*subscript), None, None),
                EvalError::ApplyTarget { function } => (5, Some(*function), None, None),
            };
            if !seen.insert(key) {
                continue;
            }
            match err {
                EvalError::Index {
                    index,
                    index_value,
                    length,
                } => out.push(Diag {
                    budget: None,
                    loc: self.node_loc(*index),
                    kind: DiagKind::IndexOutOfBounds,
                    a: NodeId::default(),
                    b: NodeId::default(),
                    value_a: Some(P::Value::from(LowValue::USize(*index_value))),
                    value_b: Some(P::Value::from(LowValue::USize(*length))),
                    assert_value: None,
                    assert_spelling: None,
                    index: Some(*index_value),
                    length: Some(*length),
                    field: None,
                    error_index: None,
                    static_template: None,
                }),
                EvalError::TableMiss { key, .. } => {
                    out.push(Diag::factual(DiagKind::TableMiss, self.node_loc(*key)))
                }
                EvalError::TableKeyUnbound { key } => out.push(Diag::factual(
                    DiagKind::TableKeyUnbound,
                    self.node_loc(*key),
                )),
                // A read applied to a non-container: the value itself is the
                // fact here, so this kind carries no type to print.  A raw
                // read's own slot reads target the element it produced rather
                // than the container the user wrote, and read as their own kind
                // for that reason.
                EvalError::IndexTarget { target } => {
                    let loc = self.node_loc(*target);
                    let kind = match loc.as_ref() {
                        Some(loc) if self.is_raw_read_element(loc) => DiagKind::RuntimeRawElement,
                        _ => DiagKind::RuntimeIndexTarget,
                    };
                    out.push(Diag::factual(kind, loc))
                }
                // A read whose subscript is not an index: like the
                // non-container target beside it, the value itself is the
                // fact, so this kind carries no type to print.
                EvalError::IndexSubscript { subscript } => out.push(Diag::factual(
                    DiagKind::RuntimeIndexSubscript,
                    self.node_loc(*subscript),
                )),
                // An apply of a non-function: the value itself is the fact, so
                // this kind carries no type to print.
                EvalError::ApplyTarget { function } => out.push(Diag::factual(
                    DiagKind::RuntimeApplyTarget,
                    self.node_loc(*function),
                )),
            }
        }
        // Failed asserts — only the explicit `assert` expressions (a
        // generated array-bounds guard duplicates the index eval error, so it
        // is not rendered separately).  Both the user-facing flag and the
        // location are keyed by the *template* condition, which is what an
        // apply's clone records: a per-call failure is attributed to the
        // `assert` expression the user wrote, not to a clone.
        for err in &self.module.assert_errors {
            let template = match err.template {
                AnyNodeId::Dynamic(template) => template,
                // Cloned out of a **static** module — a built-in package's
                // contract, or an imported one's.  This build's tables have no
                // entry for the template, so the diagnostic carries the static
                // ref instead: a host that keeps that module's source resolves it
                // to a position in *that* file, and a host that keeps none drops
                // the diagnostic (the package's own build reported it when it
                // compiled).  The `user_asserts` filter cannot apply — the flag
                // lives in the other build — so the check is left to the host
                // that knows which modules it kept sources for.
                AnyNodeId::Static(sref) => {
                    out.push(Diag {
                        assert_value: Some(err.value),
                        static_template: Some(sref),
                        ..Diag::factual(DiagKind::Assert, None)
                    });
                    continue;
                }
            };
            if self.user_asserts.contains(&template) {
                // The spelling the site registered, with a **read-kind**
                // requirement's subject resolved against the *failing*
                // condition: a per-call clone's subject is the argument's type,
                // while the registered template's own cell is still open.  Keyed
                // by the template, exactly as the registration was (a per-call
                // failure records the template).
                let assert_spelling = match self.assert_spellings.get(&template) {
                    Some(AssertSpelling::StructKind { container }) => {
                        Some(AssertSpelling::StructKind {
                            container: self
                                .read_assert_subject(err.condition)
                                .unwrap_or(*container),
                        })
                    }
                    spelling => spelling.copied(),
                };
                out.push(Diag {
                    assert_value: Some(err.value),
                    assert_spelling,
                    ..Diag::factual(DiagKind::Assert, self.node_edges.get(&template).cloned())
                });
            }
        }
        out
    }

    /// The per-`unify_errors` attribution index [`Self::diagnostics`] reads:
    /// for each error index, the diary entry that owns it and the first
    /// [`ApplyError`](lichen_lowlevel::ApplyError) that names it.
    ///
    /// Both lists are built in **one pass over their source**, which is what
    /// removes the quadratic term: the diary's owned ranges are disjoint
    /// slices of the append-only error list (recorded before the next check
    /// ran), so filling `owner` visits each error index at most once.
    fn unify_error_index(&self) -> UnifyErrorIndex {
        let count = self.module.unify_errors.len();
        let mut index = UnifyErrorIndex {
            owner: vec![None; count],
            apply: vec![None; count],
        };
        for (entry_index, entry) in self.diary.iter().enumerate() {
            let Some(range) = entry.errors.as_ref() else {
                continue;
            };
            for error_index in range.clone() {
                // First owner wins, matching the `find` this replaces.
                if let Some(slot) = index.owner.get_mut(error_index)
                    && slot.is_none()
                {
                    *slot = Some(entry_index);
                }
            }
        }
        for (apply_index, apply) in self.module.apply_errors.iter().enumerate() {
            // First apply error wins, matching the `find` this replaces; an
            // error index past the list is one no unify error can name.
            if let Some(slot) = index.apply.get_mut(apply.error_index)
                && slot.is_none()
            {
                *slot = Some(apply_index);
            }
        }
        index
    }

    /// The **subject** a read-kind assert checked — operand 0 of the failing
    /// condition's operand array, the container's type.
    ///
    /// The operand layout is
    /// [`TypeOperator::IsStructType`](crate::program::TypeOperator::IsStructType)'s:
    /// `[type value, universe]`.  The *failing* condition is read rather than the
    /// registered template because a per-call clone's operand 0 is the actual
    /// argument's type cell — the fact a reader needs — while the template's own
    /// cell is still unbound and would render as a bare `?a`.  `None` when the
    /// condition is not such an operation (or its operand is a static ref, which
    /// has no importer expression to point at), in which case the registered
    /// node answers.
    fn read_assert_subject(&self, condition: NodeId) -> Option<NodeId> {
        let operand = self.module.node_operation(condition)?.operand?;
        // SAFETY: `operand` is a live node of `module` — the failing condition's
        // own operand edge, which nothing here releases — and nothing in this
        // crate calls `Module::drop_block`.
        let items =
            unsafe { crate::shape::array_items(&self.module, AnyNodeId::Dynamic(operand)) }?;
        match items.first()?.node {
            AnyNodeId::Dynamic(node) => Some(node),
            AnyNodeId::Static(_) => None,
        }
    }

    /// The structured location for a node, or `None` for a static ref (which
    /// has no importer expression).
    ///
    /// A runtime failure names the node it failed on, and for a failure inside
    /// an applied function that node is a **per-apply clone** — the checker
    /// never saw it, so the build's `node_edges` has no entry and the failure
    /// would carry no source position.  A clone does record the node it is
    /// attributed through ([`Module::node_origin`]): for a clone of a dynamic
    /// template that is a node the checker attributed directly, so its edge is
    /// the fallback; for a clone of a **frozen** template (an imported
    /// function) no node of this module stands for the template, and the
    /// recorded node is the apply that materialized it — which
    /// [`Build::apply_edges`], keyed by the apply, attributes to the argument
    /// the caller passed there.  The edge for the node itself keeps priority:
    /// a node the checker attributed directly is never re-attributed through a
    /// clone of it.
    fn node_loc(&self, node: AnyNodeId) -> Option<Loc> {
        let AnyNodeId::Dynamic(node) = node else {
            return None;
        };
        if let Some(loc) = self.node_edges.get(&node) {
            return Some(loc.clone());
        }
        // One step suffices: the origin reaches a node the checker attributed
        // (see `Module::node_origin`).  It is not kept alive by the clone, so
        // a released one is absent rather than a panic.
        let origin = self.module.node_origin(node)?;
        if !self.module.nodes.contains_key(origin) {
            return None;
        }
        if let Some(loc) = self.node_edges.get(&origin) {
            return Some(loc.clone());
        }
        // The origin is the apply that materialized a clone of a frozen
        // template: the failure belongs to that call, and the caller's
        // argument is the location the checker recorded on the apply's edge.
        // A pathless `Loc` is the argument expression itself — the caret
        // target the edge was recorded for.
        self.apply_edges.get(&origin).map(|edge| Loc {
            expr: edge.argument_expr,
            path: Vec::new(),
        })
    }

    /// Whether `loc` was registered by one of the raw reads for its
    /// **element** — the node both of the read's slot reads target, so a
    /// non-container there is a non-pair *element*, not a non-container
    /// container.  Each raw read registers the element at the read's own type
    /// slot (`loc(e, 1)`), which is what tells it apart from the edges every
    /// other read registers; the IR kind is what keeps a typed read's own edges
    /// out of it.
    fn is_raw_read_element(&self, loc: &Loc) -> bool {
        loc.path == [LocStep::Type]
            && matches!(
                self.ir[loc.expr].kind,
                ExprKind::RawIndex { .. } | ExprKind::RawNamedField { .. }
            )
    }

    /// One unification-failure diagnostic — the `unify_errors` entry at `i`,
    /// attributed through whichever diary entry owns that index.
    fn mismatch(&self, i: usize, index: &UnifyErrorIndex) -> Diag<P> {
        let err = &self.module.unify_errors[i];
        // An apply-time parameter-check failure: attribute to the argument.
        if let Some(apply) = index.apply[i].map(|a| &self.module.apply_errors[a]) {
            // The highlevel parses the argument's structure (the "who encodes,
            // parses" rule): the descent tags each level as a `[value, type]`
            // pair slot or a tuple/array shape, so the language can build the
            // diagnostic without re-deriving the type grammar.
            let path =
                crate::shape::tag_descent(&self.module, Vec::new(), apply.argument, &err.steps);
            let loc = self.apply_edges.get(&apply.apply_node).map(|edge| Loc {
                expr: edge.argument_expr,
                path,
            });
            return Diag {
                budget: None,
                loc,
                kind: DiagKind::Runtime,
                a: apply.parameter_type,
                b: apply.argument_type,
                value_a: err.value_a,
                value_b: err.value_b,
                assert_value: None,
                assert_spelling: None,
                index: None,
                length: None,
                field: None,
                error_index: Some(i),
                static_template: None,
            };
        }
        // The owning diary entry: the one whose owned range contains this
        // error (one unify may own several, e.g. elementwise).  Ranges are
        // disjoint by construction — each is a slice of the append-only error
        // vec, recorded before the next unify ran — so exactly one matches.
        let entry = index.owner[i].map(|e| &self.diary[e]);
        let (a, b) = match entry {
            Some(entry) => (entry.a, entry.b),
            None => (err.a, err.b),
        };
        let value_a = entry.map(|e| (e.a, e.b)).and(err.value_a);
        let value_b = entry.map(|e| (e.a, e.b)).and(err.value_b);
        let loc = entry.map(|e| e.loc.clone());
        let kind = entry.map(|e| e.kind).unwrap_or(DiagKind::Runtime);
        let field = entry.and_then(|e| e.field.clone());
        Diag {
            budget: None,
            loc,
            kind,
            a,
            b,
            value_a,
            value_b,
            assert_value: None,
            assert_spelling: None,
            index: None,
            length: None,
            field,
            error_index: Some(i),
            static_template: None,
        }
    }
}

/// [`Build::unify_error_index`]'s answer: for every `unify_errors` index, the
/// diary entry that owns it (`owner`) and the first apply error that names it
/// (`apply`).  Index-aligned with `Module::unify_errors`.
///
/// It exists so that collecting one report's diagnostics is linear in their
/// number: without it, attributing each error rescans the whole diary (and
/// each orphan rescans it again), which is quadratic in the count an editor
/// produces on every keystroke.
#[derive(Default)]
struct UnifyErrorIndex {
    /// `owner[i]` is the index into [`Build::diary`] of the entry whose owned
    /// range contains `i`; `None` when no entry owns it (an orphan).
    owner: Vec<Option<usize>>,
    /// `apply[i]` is the index into `Module::apply_errors` of the first entry
    /// naming `i`; `None` when no apply error does.
    apply: Vec<Option<usize>>,
}

impl UnifyErrorIndex {
    /// The `unify_errors` indices no diary entry owns — a deep apply-time
    /// failure the lowlevel recorded rather than a checker-issued check, so
    /// there is no recording position behind it.  Ascending, so they keep the
    /// order the lowlevel recorded them in.
    fn orphan_indexes(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.owner.len()).filter(|&i| self.owner[i].is_none())
    }
}
