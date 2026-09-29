//! The checker's diagnostics glue: the diary-attributed unification wrappers
//! every check issues, the fabricated-error helpers that report a guard failure
//! as a type error instead of letting the runtime panic, and the source-blind
//! location builder they all point at.

use lichen_lowlevel::{AnyNodeId, LowOperator, NodeId, UnifyError};

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
    /// A checker-issued unification, diary-attributed: records which error
    /// (if any) it produced, with the source-blind [`Loc`] and check kind that
    /// drive the diagnostic's expected/found direction.  The `loc`'s `slot`
    /// names which element of the expression's `[value, type, attrs…]` pair
    /// the check is about; its `path` is filled from the unify's own descent
    /// (`steps`).
    pub fn check_unify(&mut self, a: NodeId, b: NodeId, loc: Loc, kind: DiagKind) {
        let before = self.module.unify_errors.len();
        self.module.unify(a, b);
        if self.module.unify_errors.len() > before {
            let err = &self.module.unify_errors[before];
            let path = shape::tag_descent(&self.module, loc.path.clone(), b, &err.steps);
            self.diary.push(DiaryEntry {
                error_index: before,
                a,
                b,
                loc: Loc {
                    expr: loc.expr,
                    path,
                },
                kind,
                field: None,
            });
        }
    }

    /// A checker-issued unification that may be relaxed by an attribute's
    /// optional subtype relation.  Attempts the ordinary unify; if it fails
    /// **and** `is_subtype` holds for the two operands, the error(s) this
    /// unify produced are discarded and the check counts as passed.
    ///
    /// `is_subtype` receives the unify's two operands `(a, b)` — the
    /// *found/value* side first and the *expected/declared* side second, the
    /// same convention as
    /// [`AttrExt::unify_slots`](crate::attr::AttrExt::unify_slots) — as a
    /// `&dyn Ctx<P>` (the
    /// curated context), and returns whether the relation is satisfied.  The
    /// attribute decides which operand is the subtype and which the supertype.
    ///
    /// Suppression is a safe truncate: an attribute unify's operands are
    /// scalar (a perspective is a `USize` or an unbound cell, never a
    /// compound array), so a failed unify merges nothing and records exactly
    /// the errors the truncate removes.  The checker's attribute check is a
    /// *validation gate* — the value itself flows in through the lowlevel
    /// apply's separate clone-unify — so suppressing leaves the graph correct.
    pub fn check_unify_relaxed(
        &mut self,
        a: NodeId,
        b: NodeId,
        loc: Loc,
        kind: DiagKind,
        is_subtype: &dyn Fn(&dyn Ctx<P>, NodeId, NodeId) -> bool,
    ) {
        let before = self.module.unify_errors.len();
        self.module.unify(a, b);
        if self.module.unify_errors.len() > before {
            if is_subtype(self, a, b) {
                self.module.unify_errors.truncate(before);
            } else {
                let err = &self.module.unify_errors[before];
                let path = shape::tag_descent(&self.module, loc.path.clone(), b, &err.steps);
                self.diary.push(DiaryEntry {
                    error_index: before,
                    a,
                    b,
                    loc: Loc {
                        expr: loc.expr,
                        path,
                    },
                    kind,
                    field: None,
                });
            }
        }
    }

    /// Record an instantiation-callee failure — the callee of an
    /// instantiation is concretely not a struct type (structs are nominal) —
    /// as a reported type error pointing at the callee's type slot, not a
    /// runtime panic.
    pub(super) fn record_instantiate_callee_error(&mut self, type_pair: NodeId, type_expr: ExprId) {
        let error_index = self.module.unify_errors.len();
        self.module.unify_errors.push(UnifyError {
            root_a: type_pair,
            root_b: type_pair,
            steps: Vec::new(),
            a: type_pair,
            b: type_pair,
            value_a: self.module.node_value(AnyNodeId::Dynamic(type_pair)),
            value_b: self.module.node_value(AnyNodeId::Dynamic(type_pair)),
        });
        self.diary.push(DiaryEntry {
            error_index,
            a: type_pair,
            b: type_pair,
            loc: self.loc(type_expr, 1),
            kind: DiagKind::InstantiateCallee,
            field: None,
        });
    }

    /// Record an "index target" guard failure — a concretely invalid named
    /// field read (a non-struct container, or a struct without that field) —
    /// as a reported type error, not a runtime panic.
    pub(super) fn record_index_target_error(&mut self, ty: NodeId, e: ExprId, slot: usize) {
        let error_index = self.module.unify_errors.len();
        self.module.unify_errors.push(UnifyError {
            root_a: ty,
            root_b: ty,
            steps: Vec::new(),
            a: ty,
            b: ty,
            value_a: self.module.node_value(AnyNodeId::Dynamic(ty)),
            value_b: self.module.node_value(AnyNodeId::Dynamic(ty)),
        });
        self.diary.push(DiaryEntry {
            error_index,
            a: ty,
            b: ty,
            loc: self.loc(e, slot),
            kind: DiagKind::IndexTarget,
            field: None,
        });
    }

    /// Record a "no such named field" failure — a `a.name` read on a struct
    /// that has no field named `name` — as a reported type error.  The offending
    /// name is carried in the diary entry (so the language layer can append a
    /// did-you-mean clause naming the struct's actual fields).
    pub(super) fn record_named_field_error(
        &mut self,
        ty: NodeId,
        e: ExprId,
        slot: usize,
        name: &'static str,
    ) {
        let error_index = self.module.unify_errors.len();
        self.module.unify_errors.push(UnifyError {
            root_a: ty,
            root_b: ty,
            steps: Vec::new(),
            a: ty,
            b: ty,
            value_a: self.module.node_value(AnyNodeId::Dynamic(ty)),
            value_b: self.module.node_value(AnyNodeId::Dynamic(ty)),
        });
        self.diary.push(DiaryEntry {
            error_index,
            a: ty,
            b: ty,
            loc: self.loc(e, slot),
            kind: DiagKind::NamedField,
            field: Some(name.to_string()),
        });
    }

    /// Record a struct-instantiation argument mismatch (an unknown, duplicate,
    /// missing, or excess field, or a `.name` argument against an anonymous
    /// struct) as a reported type error.  `loc_expr` is the IR expression the
    /// caret points at — the offending argument, or the instantiation itself
    /// for a missing field; `field` is the field's name when there is one.
    pub(super) fn record_struct_error(
        &mut self,
        loc_expr: ExprId,
        ty: NodeId,
        field: Option<&'static str>,
        kind: DiagKind,
    ) {
        let error_index = self.module.unify_errors.len();
        self.module.unify_errors.push(UnifyError {
            root_a: ty,
            root_b: ty,
            steps: Vec::new(),
            a: ty,
            b: ty,
            value_a: self.module.node_value(AnyNodeId::Dynamic(ty)),
            value_b: self.module.node_value(AnyNodeId::Dynamic(ty)),
        });
        self.diary.push(DiaryEntry {
            error_index,
            a: ty,
            b: ty,
            loc: self.loc(loc_expr, 0),
            kind,
            field: field.map(|f| f.to_string()),
        });
    }

    /// A source-blind location naming `slot` of expression `e` (0 = value,
    /// 1 = type, 2+ = the attribute tail).  The recursive descent beyond the
    /// leading slot is filled by [`Checker::check_unify`] from the lowlevel's
    /// unify `steps`.
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
