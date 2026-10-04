//! The native-operator extension point.
//!
//! A plugin registers native operators (e.g. `lichen-compute`'s `jit`/`launch`)
//! that its own embedded lichen source calls through the `$name(args…)` form,
//! compiled by the frontend to [`ExprKind::NativeCall`](crate::ir::ExprKind).
//! The checker is a bystander: it compiles the arguments, looks `name` up in
//! the *current module's* private [`NativeOps`] registry, and adopts the value
//! node the operator's [`NativeOp::build`] returns, typing the call with a
//! fresh cell of its own.  It has no knowledge of what the operator does or
//! what its types are — the plugin's registration owns that, as a private
//! contract with its own source.
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

/// The result of a native operator's [`NativeOp::build`]: the expression's
/// **value** node, plus whether that value is already decided.
///
/// A native operator is an operator: raw operands in, one raw result node out,
/// and no type anywhere.  The expression's **type** is the framework's, minted
/// unconditionally as a fresh cell for every native call ([`crate::checker`]'s
/// `check_native_call`) and paired with `value` through the crate's one
/// `[value, type]` construction site.  An operator therefore cannot state a
/// type, cannot assemble the pair, and cannot get the encoding wrong; the
/// lichen wrapper around `$name` states the types it knows, as ordinary
/// annotations.
#[derive(Debug, Clone, Copy)]
pub struct NativeApply {
    /// The expression's value node — element 0 of the term pair the checker
    /// builds around it.
    pub value: NodeId,
    /// Whether `value` is a decided value the checker may cache in
    /// [`ExprState::val`](crate::checker::ExprState), or a computation whose
    /// value the runtime reads — the latter being what the old `val: None`
    /// meant, where `value_of` builds the lazy `Index(pair, 0)` instead.
    pub decided: bool,
}

/// A native operator's compiled argument: the expression id plus its **value**
/// node.  The checker compiles each argument before calling
/// [`NativeOp::build`] and hands the value node over, so the operator builds
/// without re-reading per-expression internals (the curated [`Ctx`] does not
/// expose them).  The argument's type is not handed over: an operator sees raw
/// values only, exactly as an operator node's operands are raw.
#[derive(Debug, Clone, Copy)]
pub struct NativeArg {
    pub expr: ExprId,
    pub value: NodeId,
}

/// The compile-time lowering behaviour of one native operator.
///
/// `build` is called by [`Checker::check_native_call`](crate::checker::Checker)
/// for an [`ExprKind::NativeCall`](crate::ir::ExprKind).  The arguments have
/// already been compiled, so the implementation receives their value nodes,
/// emits the operator's operation node (through the curated [`Ctx`]), and
/// returns it.  No type: the checker mints the call's result type.
pub trait NativeOp<P: HighProgram>: Sync
where
    P::Value: ValueType,
{
    /// Emit this native operator's call.  `e` is the `NativeCall` expression
    /// being compiled; `args` are its compiled arguments; `loc` is the call's
    /// source location.  `ctx` is the curated context — the operator builds
    /// through the highlevel's encoding ([`Ctx`]), never raw lowlevel nodes.
    fn build(&self, ctx: &mut dyn Ctx<P>, e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply;
}

/// The registry attached to ONE module's checker: a private, name→operator
/// mapping.  Empty for a normal file.  Because a plugin's file is compiled on
/// its own, the slice is naturally private — resolving `$name` against it
/// never leaks into another plugin's names.
pub type NativeOps<P> = &'static [(&'static str, &'static dyn NativeOp<P>)];

/// The no-op registry of a program with no native operators: no `$name` is
/// recognised, so every `$name` call in such a module misses this slice and the
/// checker reports it as unresolved ([`DiagKind::NativeOpUnresolved`](crate::DiagKind)).
pub fn no_native_ops<P: HighProgram>() -> NativeOps<P>
where
    P::Value: ValueType,
{
    &[]
}
