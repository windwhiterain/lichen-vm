//! A native operator's [`NativeApply`] is validated before the checker adopts
//! it.
//!
//! The extension point is a public API a host composes: a plugin implements
//! [`NativeOp::build`] and returns three raw node ids.  Every downstream read
//! of the expression's term reads it as a `[value, type]` pair, so a builder
//! that returns anything else would install a term the checker never checked —
//! silently wrong, or a failure inside a later pass.  `Ctx` is the way a plugin
//! builds the shape; this pins that a violation of that shape is a reported
//! guard rather than an adoption.

use lichen_highlevel::NoAttr;
use lichen_highlevel::attr::AttrExt;
use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ChildRange, ExprId, ExprKind, IR, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp, NativeOps};
use lichen_highlevel::program::{
    Ctx, HighProgram, HighProgramLiteral, HighProgramOperator, HighProgramValue, IntLit,
    ProgramImpl, ValueType,
};
use lichen_lowlevel::{LowValue, Registry};
use std::sync::{Arc, RwLock};

/// The probe program: the built-in vocabularies and no attribute.
type ProbeProgram = ProgramImpl<HighProgramValue, HighProgramOperator, NoAttr, HighProgramLiteral>;

/// A well-formed builder: the term is the `[value, type]` pair `Ctx` built,
/// with its value slot memoized as the returned `val`.
struct WellFormed;

/// A builder that returns a bare value node as the expression's term — not a
/// pair at all.
struct NotAPair;

/// A builder whose returned `ty` is not the pair's element 1, so the checker's
/// two views of the expression's type disagree.
struct TypeSlotDisagrees;

static WELL_FORMED: WellFormed = WellFormed;
static NOT_A_PAIR: NotAPair = NotAPair;
static TYPE_SLOT_DISAGREES: TypeSlotDisagrees = TypeSlotDisagrees;

impl<P: HighProgram> NativeOp<P> for WellFormed
where
    P::Value: ValueType,
{
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let value = args[0].value;
        let ty = ctx.fresh();
        let pair = ctx.pair(value, ty);
        NativeApply {
            node: pair,
            val: Some(value),
            ty,
        }
    }
}

impl<P: HighProgram> NativeOp<P> for NotAPair
where
    P::Value: ValueType,
{
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        _args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let node = ctx.value_node(P::Value::from(LowValue::USize(7)));
        let ty = ctx.fresh();
        NativeApply {
            node,
            val: Some(node),
            ty,
        }
    }
}

impl<P: HighProgram> NativeOp<P> for TypeSlotDisagrees
where
    P::Value: ValueType,
{
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        _args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let value = ctx.value_node(P::Value::from(LowValue::USize(7)));
        let pair_ty = ctx.fresh();
        let pair = ctx.pair(value, pair_ty);
        let ty = ctx.fresh();
        NativeApply {
            node: pair,
            val: Some(value),
            ty,
        }
    }
}

/// `$probe(5)` — one native call whose single argument is an int literal.
fn probe_call() -> IR<NoAttr, HighProgramLiteral> {
    let mut ir: IR<NoAttr, HighProgramLiteral> = IR::new();
    let five = ir.alloc(ExprKind::Literal(HighProgramLiteral::from(IntLit(5))));
    let start = ir.children.len() as u32;
    ir.children.push(five);
    let args = ChildRange {
        start,
        end: ir.children.len() as u32,
    };
    let call = ir.alloc(ExprKind::NativeCall { op: "probe", args });
    ir.set_root(call);
    ir
}

fn ops(operator: &'static dyn NativeOp<ProbeProgram>) -> NativeOps<ProbeProgram> {
    Box::leak(vec![("probe", operator)].into_boxed_slice())
}

fn build(operator: &'static dyn NativeOp<ProbeProgram>) -> Build<ProbeProgram> {
    Checker::<ProbeProgram>::build_in_attr_native(
        probe_call(),
        Arc::new(RwLock::new(Registry::new())),
        Box::new(|_: &NoAttr| -> &'static dyn AttrExt<ProbeProgram> { unreachable!() }),
        ops(operator),
    )
}

#[test]
fn a_native_operator_that_returns_a_non_pair_is_refused() {
    let build = build(&NOT_A_PAIR);
    assert!(
        !build.ok,
        "a term that is not the [value, type] pair must fail the build"
    );
    let guard = build
        .diagnostics()
        .into_iter()
        .find(|d| d.kind == DiagKind::NativeOpContract)
        .expect("the broken native-operator contract is a diagnostic");
    assert_eq!(guard.field.as_deref(), Some("probe"));
}

#[test]
fn a_native_operator_whose_type_disagrees_with_its_pair_is_refused() {
    let build = build(&TYPE_SLOT_DISAGREES);
    assert!(
        !build.ok,
        "a returned ty that is not the pair's element 1 must fail the build"
    );
    assert!(
        build
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagKind::NativeOpContract),
        "the disagreement is the native-operator contract"
    );
}

#[test]
fn a_well_formed_native_operator_is_adopted() {
    let build = build(&WELL_FORMED);
    assert!(
        build.ok,
        "a builder that returns the pair it built must be adopted, got: {:?}",
        build.diagnostics()
    );
}
