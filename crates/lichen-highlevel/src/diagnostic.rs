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

use lichen_lowlevel::{AnyNodeId, EvalError, LowValue, NodeId, Program, UnifyError};

use crate::{checker::Build, ir::Loc, program::{HighProgram, ValueType}};

/// What kind of check a unification failure implements — drives the
/// expected/found direction of a [`Diag::Mismatch`]'s `a`/`b`.  All are
/// *type-mismatch* constructs; the coarse value/type/kind discrimination is
/// [`Loc::kind`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiagKind {
    /// `inner : T` — expected = the annotation's type value.
    Annotation,
    /// Applying a concretely non-function type — expected = a function.
    Guard,
    /// Indexing a concretely non-indexable type (a function, an atomic
    /// type) — expected = a tuple, array, or struct type.
    IndexTarget,
    /// An array literal's elements must share one type — expected = the
    /// shared element type, found = this element's type.
    ArrayElement,
    /// A table literal's keys must share one type.
    TableKey,
    /// A table literal's values must share one type.
    TableValue,
    /// A binary operator's operand must be an `Int`.
    BinOp,
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
}

/// One checker-issued unification, attributed with where it came from.
#[derive(Clone, Debug)]
pub struct DiaryEntry {
    /// Index into [`Module::unify_errors`] of the first error this unify
    /// produced (one unify may record several, e.g. elementwise).
    pub error_index: usize,
    pub a: NodeId,
    pub b: NodeId,
    /// The source-blind location of the mismatch — the IR expression and its
    /// position within the `[value, type]` spine.
    pub loc: Loc,
    pub kind: DiagKind,
}

/// A structured diagnostic.  The highlevel emits *facts*, never a rendered
/// message: the language layer derives the wording and the caret from these
/// fields, mapping a [`Diag::loc`] (when present) back to a source span
/// through its own source↔IR record.
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
    /// The offending index of an out-of-bounds read (meaningful when
    /// `kind == DiagKind::IndexOutOfBounds`).
    pub index: Option<usize>,
    /// The container's length for an out-of-bounds read.
    pub length: Option<usize>,
    /// Which `Module::unify_errors` entry a mismatch came from — the key back
    /// to its diary entry, for callers that re-render.
    pub error_index: Option<usize>,
}

impl<P: Program> Diag<P> {
    /// The [`Loc`] this diagnostic is attributed to, if any.
    pub fn loc(&self) -> Option<&Loc> {
        self.loc.as_ref()
    }
}

impl<P: HighProgram> Build<P>
where
    P::Value: ValueType,
{
    /// Render the lowlevel's failure facts as structured diagnostics — one per
    /// entry in [`Module::unify_errors`], in order, then the runtime
    /// evaluation failures (deduplicated), then the user-facing asserts.
    pub fn diagnostics(&self) -> Vec<Diag<P>> {
        let mut out = Vec::new();
        for (i, err) in self.module.unify_errors.iter().enumerate() {
            out.push(self.mismatch(i, err));
        }
        // Runtime evaluation failures (an out-of-bounds index, a table read).
        // The value and type evaluation of the same expression each record
        // one, so identical facts collapse to a single diagnostic.
        let mut seen = HashSet::new();
        for err in &self.module.eval_errors {
            let key = match err {
                EvalError::Index {
                    index,
                    index_value,
                    length,
                } => (Some(*index), Some(*index_value), Some(*length)),
                _ => (None, None, None),
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
                    loc: self.node_loc(*index),
                    kind: DiagKind::IndexOutOfBounds,
                    a: NodeId::default(),
                    b: NodeId::default(),
                    value_a: Some(P::Value::from(LowValue::USize(*index_value))),
                    value_b: Some(P::Value::from(LowValue::USize(*length))),
                    assert_value: None,
                    index: Some(*index_value),
                    length: Some(*length),
                    error_index: None,
                }),
                EvalError::TableMiss { key, .. } => out.push(Diag {
                    loc: self.node_loc(*key),
                    kind: DiagKind::TableMiss,
                    a: NodeId::default(),
                    b: NodeId::default(),
                    value_a: None,
                    value_b: None,
                    assert_value: None,
                    index: None,
                    length: None,
                    error_index: None,
                }),
                EvalError::TableKeyUnbound { key } => out.push(Diag {
                    loc: self.node_loc(*key),
                    kind: DiagKind::TableKeyUnbound,
                    a: NodeId::default(),
                    b: NodeId::default(),
                    value_a: None,
                    value_b: None,
                    assert_value: None,
                    index: None,
                    length: None,
                    error_index: None,
                }),
            }
        }
        // Failed asserts — only the explicit `assert` expressions (a
        // generated array-bounds guard duplicates the index eval error, so it
        // is not rendered separately).
        for err in &self.module.assert_errors {
            if self.module.user_asserts.contains(&err.condition) {
                out.push(Diag {
                    loc: self.node_edges.get(&err.condition).cloned(),
                    kind: DiagKind::Assert,
                    a: NodeId::default(),
                    b: NodeId::default(),
                    value_a: None,
                    value_b: None,
                    assert_value: Some(err.value),
                    index: None,
                    length: None,
                    error_index: None,
                });
            }
        }
        out
    }

    /// The structured location for a node, or `None` for a static ref (which
    /// has no importer expression).
    fn node_loc(&self, node: AnyNodeId) -> Option<Loc> {
        let AnyNodeId::Dynamic(node) = node else {
            return None;
        };
        self.node_edges.get(&node).cloned()
    }

    /// One unification-failure diagnostic.
    fn mismatch(&self, i: usize, err: &UnifyError<P>) -> Diag<P> {
        // An apply-time parameter-check failure: attribute to the argument.
        if let Some(apply) = self.module.apply_errors.iter().find(|a| a.error_index == i) {
            // The highlevel parses the argument's structure (the "who encodes,
            // parses" rule): the descent tags each level as a `[value, type]`
            // pair slot or a tuple/array shape, so the language can build the
            // diagnostic without re-deriving the type grammar.
            let path = crate::checker::tag_descent(
                &self.module,
                Vec::new(),
                apply.argument,
                &err.steps,
            );
            let loc = self.apply_edges.get(&apply.apply_node).map(|edge| Loc {
                expr: edge.argument_expr,
                path,
            });
            return Diag {
                loc,
                kind: DiagKind::Runtime,
                a: apply.parameter_type,
                b: apply.argument_type,
                value_a: err.value_a,
                value_b: err.value_b,
                assert_value: None,
                index: None,
                length: None,
                error_index: Some(i),
            };
        }
        // The owning diary entry: the last one whose error_index <= i (one
        // unify may own a whole run of errors, e.g. elementwise).
        let entry = self.diary.iter().rev().find(|e| e.error_index <= i);
        let (a, b) = match entry {
            Some(entry) => (entry.a, entry.b),
            None => (err.a, err.b),
        };
        let loc = entry.map(|e| e.loc.clone());
        let kind = entry.map(|e| e.kind).unwrap_or(DiagKind::Runtime);
        Diag {
            loc,
            kind,
            a,
            b,
            value_a: err.value_a,
            value_b: err.value_b,
            assert_value: None,
            index: None,
            length: None,
            error_index: Some(i),
        }
    }
}
