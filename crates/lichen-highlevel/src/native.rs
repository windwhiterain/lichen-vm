//! The native-operator extension point.
//!
//! A downstream may inject operators that are **values** with a distinctive
//! type (e.g. a `Native(Jit)` value typed `TypeNativeJit`).  To call one from
//! source, the program makes `jit f` an ordinary `Apply`; the checker, when it
//! sees an `Apply` whose callee's *type* is a native-operator type, delegates
//! to a [`NativeExt`] instead of building a runtime function apply.  This is
//! the mechanism behind "Option B": the native operator is a value, its *type*
//! carries the identity, and the core apply dispatch stays generic with one
//! consult point ([`Checker::native_ext`]).
//!
//! The extension point mirrors [`AttrExt`](crate::attr::AttrExt): the checker
//! knows only the shape — "an `Apply` whose callee has one of your operator
//! types should call you" — and a concrete extension supplies the operator's
//! check-and-emit behaviour in the layer that defines it.  Highlevel ships the
//! no-op [`no_native_ext`].

use lichen_lowlevel::NodeId;

use crate::checker::Checker;
use crate::ir::{ExprId, Span};
use crate::program::{HighProgram, ValueType};

/// The result of a native operator's [`NativeExt::check_apply`]: the compiled
/// pair node, the value node (or `None` when only known at runtime, like a call
/// result), and the type node — the same three records the ordinary apply
/// wiring sets on an expression.
#[derive(Debug, Clone, Copy)]
pub struct NativeApply {
    pub node: NodeId,
    pub val: Option<NodeId>,
    pub ty: NodeId,
}

/// The compile-time lowering behaviour of one native operator.
///
/// `check_apply` is called by [`Checker::check_app`] for an `Apply` whose
/// callee's type is one of this extension's operator types.  The callee and the
/// argument have already been compiled, so the implementation receives their
/// value and type nodes, the checks the operator's types, emits the operator's
/// operation node (through the public `Checker` surface), and returns the
/// applied pair.
pub trait NativeExt<P: HighProgram>
where
    P::Value: ValueType,
{
    /// Check and emit this native operator's application.  `e` is the `Apply`
    /// expression being compiled; `callee_value`/`callee_ty` are the applied
    /// value and its type; `argument_value`/`argument_ty` are the argument's
    /// value and its type; `argument` is the argument expression id.
    fn check_apply(
        &self,
        checker: &mut Checker<P>,
        e: ExprId,
        callee_value: NodeId,
        callee_ty: NodeId,
        argument_value: NodeId,
        argument_ty: NodeId,
        argument: ExprId,
        span: Option<Span>,
    ) -> NativeApply;
}

/// The no-op native-operator registry of a program with no native operators:
/// no callee is recognised, so every `Apply` stays an ordinary function apply.
pub fn no_native_ext<P: HighProgram>() -> Box<dyn Fn(&P::Value) -> Option<&'static dyn NativeExt<P>>>
where
    P::Value: ValueType,
{
    Box::new(|_| None)
}
