//! The native-operator extension point.
//!
//! A plugin registers native operators (e.g. `lichen-compute`'s `jit`/`launch`)
//! that its own embedded lichen source calls through the `$name(args…)` form,
//! compiled by the frontend to [`ExprKind::NativeCall`](crate::ir::ExprKind).
//! The checker is a bystander: it compiles the arguments, looks `name` up in
//! the *current module's* private [`NativeOps`] registry, and adopts the
//! `[value, type]` pair the operator's [`NativeOp::build`] returns.  It has no
//! knowledge of what the operator does or what its types are — the plugin's
//! registration owns that, as a private contract with its own source.
//!
//! The extension point mirrors [`AttrExt`](crate::attr::AttrExt): the checker
//! knows only the shape — "a `$name(args)` call should delegate to your
//! registry" — and a concrete plugin supplies the operator's check-and-emit
//! behaviour in the layer that defines it.  Highlevel ships the empty
//! [`no_native_ops`].  Because the registry is per-module and only a plugin's
//! own file is compiled against it, a `$name` resolves privately: two plugins
//! each registering `$jit` never collide.

use lichen_lowlevel::NodeId;

use crate::ir::{ExprId, Loc};
use crate::program::{Ctx, HighProgram, ValueType};

/// The result of a native operator's [`NativeOp::build`]: the compiled pair
/// node, the value node (or `None` when only known at runtime, like a call
/// result), and the type node — the same three records the ordinary apply
/// wiring sets on an expression.
#[derive(Debug, Clone, Copy)]
pub struct NativeApply {
    pub node: NodeId,
    pub val: Option<NodeId>,
    pub ty: NodeId,
}

/// A native operator's compiled argument: the expression id plus its value and
/// type nodes.  The checker compiles each argument before calling
/// [`NativeOp::build`] and hands the value/type nodes over, so the operator
/// builds without re-reading per-expression internals (the curated [`Ctx`]
/// does not expose them).
#[derive(Debug, Clone, Copy)]
pub struct NativeArg {
    pub expr: ExprId,
    pub value: NodeId,
    pub ty: NodeId,
}

/// The compile-time lowering behaviour of one native operator.
///
/// `build` is called by [`Checker::check_native_call`](crate::checker::Checker)
/// for an [`ExprKind::NativeCall`](crate::ir::ExprKind).  The arguments have
/// already been compiled, so the implementation receives their value/type
/// nodes, checks the operator's types, emits the operator's operation node
/// (through the curated [`Ctx`]), and returns the compiled pair.
pub trait NativeOp<P: HighProgram>: Sync
where
    P::Value: ValueType,
{
    /// Check and emit this native operator's call.  `e` is the `NativeCall`
    /// expression being compiled; `args` are its compiled arguments; `loc` is
    /// the call's source location.  `ctx` is the curated context — the
    /// operator builds through the highlevel's encoding ([`Ctx`]), never raw
    /// lowlevel nodes.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        e: ExprId,
        args: &[NativeArg],
        loc: Loc,
    ) -> NativeApply;
}

/// The registry attached to ONE module's checker: a private, name→operator
/// mapping.  Empty for a normal file.  Because a plugin's file is compiled on
/// its own, the slice is naturally private — resolving `$name` against it
/// never leaks into another plugin's names.
pub type NativeOps<P> = &'static [(&'static str, &'static dyn NativeOp<P>)];

/// The no-op registry of a program with no native operators: no `$name` is
/// recognised, so the frontend rejects any `$` form (a `NativeCall` that
/// reaches the checker with an empty registry is a frontend/checker bug).
pub fn no_native_ops<P: HighProgram>() -> NativeOps<P>
where
    P::Value: ValueType,
{
    &[]
}
