//! A plugin's `$name(args…)` native-operator call, resolved in the compiling
//! module's own registry.
//!
//! # Invariant
//! The compiling module's own registry is the only one a `$name` resolves
//! against, so two plugins registering one `$name` never collide; the operator
//! returns a raw value node, and the checker mints the call's type.
//! See docs/notes/compiler-plugin.md.

use lichen_lowlevel::NodeId;

use crate::ir::{ExprId, Loc};
use crate::program::{Ctx, HighProgram, ValueType};

/// The value node a native operator's [`NativeOp::build`] returns, and whether
/// it is decided.
///
/// # Invariant
/// A native operator is an operator: raw operands in, one raw result node out,
/// and no type anywhere. The checker mints the expression's type
/// unconditionally as a fresh cell and pairs it with `value`; the lichen
/// wrapper around `$name` states the types it knows.
#[derive(Debug, Clone, Copy)]
pub struct NativeApply {
    /// The expression's value node — element 0 of the term pair the checker
    /// builds around it. See docs/language-spec.md.
    pub value: NodeId,
    /// Whether `value` is a decided value the checker caches as the
    /// expression's value, or a computation the runtime reads.
    pub decided: bool,
}

/// A native operator's compiled argument: its IR expression and the value node
/// the checker already built for it.
///
/// # Invariant
/// The operator is handed the value node alone, never the argument's type — an
/// operator sees raw values, as an operator node's operands are raw.
#[derive(Debug, Clone, Copy)]
pub struct NativeArg {
    pub expr: ExprId,
    pub value: NodeId,
}

/// The compile-time lowering behaviour of one native operator.
///
/// # Invariant
/// `build` receives already-compiled arguments and emits one operation node;
/// it states no type, because the checker mints the call's result type.
pub trait NativeOp<P: HighProgram>: Sync
where
    P::Value: ValueType,
{
    /// Emit this native operator's call for `e`, its compiled `args` and source
    /// `loc`, through the curated `ctx`.
    ///
    /// # Invariant
    /// The operator builds through `Ctx`, never raw lowlevel nodes.
    fn build(&self, ctx: &mut dyn Ctx<P>, e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply;
}

/// The registry attached to one module's checker: a private name→operator
/// mapping, empty for an ordinary file.
///
/// # Invariant
/// The registry is the compiling module's alone: a `$name` resolves only
/// against it, so two plugins registering one name never collide.
pub type NativeOps<P> = &'static [(&'static str, &'static dyn NativeOp<P>)];

/// The no-op registry of a program with no native operators: every `$name` call
/// misses it and is reported as unresolved.
pub fn no_native_ops<P: HighProgram>() -> NativeOps<P>
where
    P::Value: ValueType,
{
    &[]
}
