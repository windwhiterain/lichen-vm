//! Structured, source-blind diagnostics for the highlevel checker; see
//! docs/notes/language-toolchain.md.
//!
//! # Invariant
//! A diagnostic is expressed only in the checker's own facts — a `Loc`, the two
//! conflicting nodes, and the runtime fact that failed — because the highlevel
//! never sees source positions, and no rendered `message` is stored: the
//! language layer derives the wording and the caret from those facts through
//! its own source↔IR record.

use std::collections::HashSet;
use std::ops::Range;

use lichen_lowlevel::{AnyNodeId, BudgetExhausted, EvalError, LowValue, NodeId, Program};

use crate::{
    checker::Build,
    ir::{ExprKind, Loc, LocStep},
    program::{HighProgram, ValueType},
};

/// The check a unification failure implements, fixing the expected/found
/// direction of a mismatch's `a`/`b`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagKind {
    /// `inner : T` — expected = the annotation's type value.
    Annotation,
    /// An operand's type must have a particular shape and does not;
    /// expected = that shape (a callee, a container, a set).
    Guard,
    /// Indexing a concretely non-indexable type (a function, an atomic
    /// type) — expected = a tuple, array, or struct type.
    IndexTarget,
    /// A named field read `a.name` on a struct with no such field: expected is
    /// that field.
    NamedField,
    /// A `.name` argument whose struct has no named field of that name.
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
    /// A struct **definition** with a field that has no name — `struct<Int, Type>`.
    ///
    /// # Invariant
    /// Every struct field carries a name (`struct<.name T>`), because a struct
    /// instance reads by name only; the positional form `a(k)` is the *tuple*
    /// read (`docs/language-spec.md` §Structs).  A *block*'s fields are its
    /// bindings and need no such check.
    StructFieldName,
    /// A struct instantiation whose callee's type is concretely not a struct type.
    InstantiateCallee,
    /// A homogeneous literal's elements must share one type: the shared type is
    /// expected.
    ArrayElement,
    /// A table literal's keys must share one type.
    TableKey,
    /// A table literal's values must share one type.
    TableValue,
    /// A binary operator's operand must be an `Int`.
    BinOp,
    /// A class conversion's operand must be the direction's source class.
    Conv,
    /// An attribute (perspective) check: an expression's attribute slot must
    /// equal the expected one.
    Attribute,
    /// A runtime apply-time failure (the VM's parameter type check), with no
    /// diary entry; `a` is the parameter's type.
    Runtime,
    /// A failed assert (see [`Diag::Assert`]) — not a mismatch.
    Assert,
    /// An out-of-bounds index (see [`Diag::Index`]).
    IndexOutOfBounds,
    /// A table read that missed (see [`Diag::TableMiss`]).
    TableMiss,
    /// A table build dropped a non-concrete key (see [`Diag::TableKeyUndecided`]).
    TableKeyUndecided,
    /// A read whose **runtime** target turned out not to be a container.
    ///
    /// # Invariant
    /// The lowlevel's `EvalError::IndexTarget` reaches the diagnostics as a fact
    /// about a *value*, with no type to print, so the wording is
    /// self-contained.  Distinct from [`Self::IndexTarget`], which reports a
    /// *type* the checker refused to index and therefore names that type.
    RuntimeIndexTarget,
    /// A raw read (`X<e>`, `X::a`) whose **element** is not a pair at runtime.
    ///
    /// # Invariant
    /// The element's own type slot — the read's type — does not exist then.  It
    /// is the same lowlevel `EvalError::IndexTarget` as
    /// [`Self::RuntimeIndexTarget`], but the non-container is the element the
    /// read produced, not the container the user wrote.  Only the read itself
    /// can see this; the wording is distinct because the generic "not a
    /// container" blames the wrong side.
    RuntimeRawElement,
    /// An imported package whose export is not the `[value, type]` pair the
    /// importer reads.
    ImportExport,
    /// A read whose **runtime subscript** is not an index: the lowlevel's
    /// `EvalError::IndexSubscript`, a fact about a value.
    RuntimeIndexSubscript,
    /// An apply whose **runtime** target is not a function.
    ///
    /// # Invariant
    /// The lowlevel's `EvalError::ApplyTarget` reaches the diagnostics as a fact
    /// about a *value*, with no type to print, so the wording is self-contained.
    /// Distinct from [`Self::Guard`], which reports a *type* the checker refused
    /// to apply and therefore names that type.
    RuntimeApplyTarget,
    /// A `$name(args…)` call whose `name` is not in this module's registry.
    NativeOpUnresolved,
    /// A `$name(args…)` call whose builder returned an inadoptable term; see
    /// [`DiaryEntry::field`].
    ///
    /// # Invariant
    /// The three `NativeApply` records must name one `[value, type]` pair —
    /// exactly two slots, element 1 the returned `ty`, element 0 the returned
    /// `val` when the value is a decided node — built in the block the call is
    /// compiled into.  Every downstream read of the term reads those two slots,
    /// so anything else would install a term the checker never checked.
    NativeOpContract,
    /// A schema carrying an attribute, checked by a build that installs no
    /// attribute extension.
    ///
    /// # Invariant
    /// The checker records this at the site that read the attribute rather than
    /// panicking, and the slot falls back to the attribute's well-formed hole.
    /// `a`/`b` are unused.  The public `Checker::build` and `Checker::build_in`
    /// install none.
    NoAttributeExtension,
    /// A top-level binding whose value computation never terminates — the VM's
    /// guard fired while the build evaluated it.
    NonTerminating,
    /// A `@loop`-marked recursion whose trip count was not decided and whose
    /// shape does not convert to a loop.
    ///
    /// # Invariant
    /// The refusal half of Stage 0 in `docs/notes/loop-conversion.md` §8: it
    /// names the mark, the missing decision, and the missing loop, replacing an
    /// emitter-side complaint that named only a `NodeId`.  The apply node is in
    /// `a`/`b`; [`DiaryEntry::field`] carries the shape rule that refused
    /// (`LoopRefusal::name`), so wording and rule cannot drift, and is `None`
    /// when no conversion ran.
    LoopNotRecorded,
    /// A marked recursion whose shape converts: the conversion exists, no
    /// backend consumes it yet.
    ///
    /// # Invariant
    /// Distinct from [`LoopNotRecorded`](Self::LoopNotRecorded) because the two
    /// ask the user for different things: one says the program's shape is not a
    /// loop, the other says the loop is ready and the backend is missing.
    /// `a`/`b` carry the apply node the refusal is about.
    LoopNotEmitted,
    /// A failed build that [`Build::diagnostics`] could attribute *nothing* to.
    ///
    /// # Invariant
    /// The checker recorded a failure, but every recorded failure was skipped for
    /// want of an expression to blame; the one live producer is an assert cloned
    /// out of an imported module, whose template this build's tables hold no
    /// entry for.  The assembly layer substitutes exactly one of these, so a
    /// failed report never carries an empty diagnostic list; `loc` is `None`.
    UnattributedFailure,
}

/// One checker check, attributed with where it came from.
///
/// # Invariant
/// The discriminant is [`Self::errors`]: a guard rejected the expression
/// before any unify happened, so it owns no range and the entry is itself the
/// whole diagnostic, while a failed unification owns the range it produced.
/// Every entry has a recording position ([`Self::seq`]), the output order.
#[derive(Clone, Debug)]
pub struct DiaryEntry {
    /// What the check produced: `None` for a guard failure, `Some(range)` for
    /// a failed unification's errors.
    ///
    /// # Invariant
    /// The range is never empty — a unification that produced no error is not
    /// recorded at all — which makes this field the discriminant; one unify may
    /// own several entries (e.g. elementwise), so it is a range, not an index.
    /// Reading an empty range as "a guard" would classify a future informational
    /// entry with no owned error as a guard failure, which skips the definition
    /// pass.
    pub errors: Option<Range<usize>>,
    /// The checker counter value this entry was recorded at: the output order.
    pub seq: usize,
    pub a: NodeId,
    pub b: NodeId,
    /// The source-blind location of the mismatch — the IR expression and its
    /// position within the `[value, type]` spine.
    pub loc: Loc,
    pub kind: DiagKind,
    /// The offending or missing field name of a named-field read or
    /// instantiation.
    pub field: Option<String>,
}

/// What a failed assert of a condition should *say*, beyond the channel's
/// generic "expected 1, found …".
///
/// # Invariant
/// The channel reports one shape because an explicit `@assert e` means exactly
/// that: this condition is not `1`.  A **refinement** failure means something
/// more specific to a reader — the value is outside the set of classes the
/// contract admits — so when whoever registers it knows that set, it says so
/// (`docs/notes/operator-polymorphism.md` §8.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AssertSpelling {
    /// An explicit `@assert e`, or a generated guard: the condition's own text.
    Condition,
    /// A refinement whose contract names the class **domain** in `domain`, to
    /// be rendered beside the failure.
    Refinement { domain: NodeId },
    /// A named read's container-kind requirement: `a.name`'s container must be
    /// a struct type.
    ///
    /// # Invariant
    /// `container` is the condition's checked subject — the container's type — and
    /// is the diagnostic's *found* side.  It is registered for an **undecided**
    /// container only, so the requirement rides the assert channel and the apply
    /// clone re-checks it per call (`docs/notes/eval-before-unify.md` §6.2 option
    /// 1).
    StructKind { container: NodeId },
}

/// A structured diagnostic: the highlevel emits *facts*, never a rendered
/// message, and the language layer spells them.
#[derive(Clone, Debug)]
pub struct Diag<P: Program> {
    /// The source-blind location: the IR expression and its position in the
    /// `[value, type]` spine; `None` if source-less.
    pub loc: Option<Loc>,
    /// What kind of check failed: the mismatch sub-kind, or the specific
    /// non-mismatch kind.
    pub kind: DiagKind,
    /// The conflicting sides: the checker's operands (for a runtime failure, the
    /// parameter and the argument).
    pub a: NodeId,
    pub b: NodeId,
    /// The conflicting classes' values at error time (snapshots).
    pub value_a: Option<P::Value>,
    pub value_b: Option<P::Value>,
    /// The resolved value of a failed assert (meaningful when
    /// `kind == DiagKind::Assert`).
    pub assert_value: Option<P::Value>,
    /// The **spelling** a failed assert was registered with (see
    /// [`AssertSpelling`]).
    ///
    /// # Invariant
    /// The registration is keyed by the assert's *template*, so the spelling
    /// survives the per-call clone.  `None` for every other failure, and for an
    /// assert whose spelling is the channel's own `Condition`.
    pub assert_spelling: Option<AssertSpelling>,
    /// The offending index of an out-of-bounds read (meaningful when
    /// `kind == DiagKind::IndexOutOfBounds`).
    pub index: Option<usize>,
    /// The container's length for an out-of-bounds read.
    pub length: Option<usize>,
    /// The offending or missing field name of a named-field read or
    /// instantiation.
    pub field: Option<String>,
    /// The budget the VM's guard refused on, for [`DiagKind::NonTerminating`];
    /// `None` for every other kind.
    pub budget: Option<BudgetExhausted>,
    /// Which `Module::unify_errors` entry a mismatch came from — the key back
    /// to its diary entry, for callers that re-render.
    pub error_index: Option<usize>,
    /// The **template** of a failed assert whose body is in another module:
    /// the static node the condition was cloned from.
    ///
    /// # Invariant
    /// Present exactly when `loc` is `None` for that reason.  This build's
    /// tables hold no entry for a static template, so the position is the
    /// *other* module's: a host that kept that module's source resolves it
    /// (`docs/notes/core-prelude.md`), and one that kept none leaves the
    /// failure unattributed.
    pub static_template: Option<lichen_lowlevel::StaticNodeId>,
}

impl<P: Program> Diag<P> {
    /// The [`Loc`] this diagnostic is attributed to, if any.
    pub fn loc(&self) -> Option<&Loc> {
        self.loc.as_ref()
    }

    /// A diagnostic that is only its kind and its location; every value, index,
    /// field and budget slot is empty.
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

    /// The placeholder for a failed build `Build::diagnostics` rendered nothing
    /// for; see [`DiagKind::UnattributedFailure`].
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
    /// # Invariant
    /// Checker-attributed failures come out in recording order: a unification
    /// error is emitted at the `seq` of the entry whose owned range contains it,
    /// so a guard failure interleaves with real unifications exactly where it was
    /// recorded.  An error with no owner comes after all of them, then the
    /// deduplicated runtime failures and the user-facing asserts.
    pub fn diagnostics(&self) -> Vec<Diag<P>> {
        // One index for the whole report: without it the diary scan is per error.
        let index = self.unify_error_index();
        let mut out = Vec::new();
        for entry in &self.nonterminating {
            out.push(Diag {
                budget: entry.budget,
                ..Diag::factual(DiagKind::NonTerminating, Some(entry.loc.clone()))
            });
        }
        // Checker-attributed failures, in recording order; sorting by `seq` is
        // stable.
        let mut attributed: Vec<(usize, Diag<P>)> = Vec::new();
        for entry in &self.diary {
            let Some(errors) = entry.errors.clone() else {
                // A guard failure: the check refused before unifying, so the
                // entry is the diagnostic.
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
        // An error no diary entry owns: a deep apply-time failure with no
        // recording position.
        out.extend(index.orphan_indexes().map(|i| self.mismatch(i, &index)));
        // Runtime evaluation failures, deduplicated by kind and blamed node.
        let mut seen = HashSet::new();
        for err in &self.module.eval_errors {
            let key = match err {
                EvalError::Index {
                    index,
                    index_value,
                    length,
                } => (0, Some(*index), Some(*index_value), Some(*length)),
                EvalError::TableMiss { key, .. } => (1, Some(*key), None, None),
                EvalError::TableKeyUndecided { key } => (2, Some(*key), None, None),
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
                EvalError::TableKeyUndecided { key } => out.push(Diag::factual(
                    DiagKind::TableKeyUndecided,
                    self.node_loc(*key),
                )),
                // A read applied to a non-container: the value itself is the
                // fact, so this kind carries no type to print.
                EvalError::IndexTarget { target } => {
                    let loc = self.node_loc(*target);
                    let kind = match loc.as_ref() {
                        Some(loc) if self.is_raw_read_element(loc) => DiagKind::RuntimeRawElement,
                        _ => DiagKind::RuntimeIndexTarget,
                    };
                    out.push(Diag::factual(kind, loc))
                }
                // A read whose subscript is not an index: the value itself is
                // the fact, no type.
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
        // Failed asserts, keyed by the template condition a clone records:
        // only explicit `assert` expressions are rendered.
        for err in &self.module.assert_errors {
            let template = match err.template {
                AnyNodeId::Dynamic(template) => template,
                // Cloned out of a static module: the diagnostic carries the
                // static ref.
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
                // The registered spelling, with a read-kind requirement's
                // subject resolved against the *failing* condition.
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

    /// The per-`unify_errors` attribution index [`Self::diagnostics`] reads.
    ///
    /// # Invariant
    /// For each error index it holds the owning diary entry and the first
    /// `ApplyError` that names it.  Both lists are built in one pass over their
    /// source: the diary's owned ranges are disjoint slices of the append-only
    /// error list, so filling `owner` visits each error index at most once and the
    /// collection stays linear.
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
            // First apply error wins; an index past the list names no unify error.
            if let Some(slot) = index.apply.get_mut(apply.error_index)
                && slot.is_none()
            {
                *slot = Some(apply_index);
            }
        }
        index
    }

    /// The **subject** a read-kind assert checked — operand 0 of the failing
    /// condition, the container's type.
    ///
    /// # Invariant
    /// The operand layout is `TypeOperator::IsStructType`'s: `[type value,
    /// universe]`.  The *failing* condition is read rather than the registered
    /// template, because a per-call clone's operand 0 is the actual argument's
    /// type cell while the template's own cell is still undecided; `None` when
    /// the condition is not such an operation, in which case the registered node
    /// answers.
    fn read_assert_subject(&self, condition: NodeId) -> Option<NodeId> {
        let operand = self.module.node_operation(condition)?.operand?;
        // SAFETY: `operand` is the failing condition's own operand edge, a live
        // node of `module`; nothing here releases it.

        // Nothing in this crate calls `Module::drop_block`.
        let items =
            unsafe { crate::shape::array_items(&self.module, AnyNodeId::Dynamic(operand)) }?;
        match items.first()?.node {
            AnyNodeId::Dynamic(node) => Some(node),
            AnyNodeId::Static(_) => None,
        }
    }

    /// The structured location for a node, or `None` for a static ref (which has
    /// no importer expression).
    ///
    /// # Invariant
    /// A failure inside an applied function names a **per-apply clone**: the
    /// checker never saw it, so its edge is the node the clone was attributed
    /// through ([`Module::node_origin`]) — a checker-attributed node for a
    /// dynamic template, or the apply that materialized a frozen one, which
    /// `Build::apply_edges` attributes to the caller's argument.  A node's own
    /// edge wins.
    fn node_loc(&self, node: AnyNodeId) -> Option<Loc> {
        let AnyNodeId::Dynamic(node) = node else {
            return None;
        };
        if let Some(loc) = self.node_edges.get(&node) {
            return Some(loc.clone());
        }
        // One step suffices: the origin reaches a node the checker attributed
        // (see `Module::node_origin`).
        let origin = self.module.node_origin(node)?;
        if !self.module.nodes.contains_key(origin) {
            return None;
        }
        if let Some(loc) = self.node_edges.get(&origin) {
            return Some(loc.clone());
        }
        // The origin is the apply that materialized a clone of a frozen
        // template: the caller's argument is the location.
        self.apply_edges.get(&origin).map(|edge| Loc {
            expr: edge.argument_expr,
            path: Vec::new(),
        })
    }

    /// Whether `loc` was registered by one of the raw reads for its **element**.
    ///
    /// # Invariant
    /// A non-container there is a non-pair element, not a non-container
    /// container.  Each raw read registers the element at the read's own type
    /// slot (`loc(e, 1)`), which tells it apart from the edges every other read
    /// registers; the IR kind keeps a typed read's own edges out of it.
    fn is_raw_read_element(&self, loc: &Loc) -> bool {
        loc.path == [LocStep::Type]
            && matches!(
                self.ir[loc.expr].kind,
                ExprKind::RawIndex { .. } | ExprKind::RawNamedField { .. }
            )
    }

    /// One unification-failure diagnostic: the `unify_errors` entry at `i`.
    fn mismatch(&self, i: usize, index: &UnifyErrorIndex) -> Diag<P> {
        let err = &self.module.unify_errors[i];
        // An apply-time parameter-check failure: attribute to the argument.
        if let Some(apply) = index.apply[i].map(|a| &self.module.apply_errors[a]) {
            // The highlevel parses the argument: the descent tags each level's
            // slot or shape.
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
        // The owning diary entry: ranges are disjoint, so exactly one contains
        // this error.
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

/// `Build::unify_error_index`'s answer, index-aligned with `Module::unify_errors`.
///
/// # Invariant
/// For every `unify_errors` index it holds the diary entry that owns it
/// (`owner`) and the first apply error that names it (`apply`).  It exists so
/// that collecting one report is linear: without it, attributing each error
/// rescans the whole diary (and each orphan rescans it again), which is
/// quadratic in the count an editor produces on every keystroke.
#[derive(Default)]
struct UnifyErrorIndex {
    /// `owner[i]` is the diary index owning `i`; `None` when no entry does.
    owner: Vec<Option<usize>>,
    /// `apply[i]` is the index into `Module::apply_errors` of the first entry
    /// naming `i`; `None` when no apply error does.
    apply: Vec<Option<usize>>,
}

impl UnifyErrorIndex {
    /// The `unify_errors` indices no diary entry owns, in the lowlevel's order.
    fn orphan_indexes(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.owner.len()).filter(|&i| self.owner[i].is_none())
    }
}
