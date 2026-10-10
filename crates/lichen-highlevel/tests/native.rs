//! A native operator's [`NativeApply`] is adopted; see docs/notes/compiler-plugin.md.

use lichen_highlevel::NoAttr;
use lichen_highlevel::attr::AttrExt;
use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::ir::{ChildRange, ExprId, ExprKind, IR, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp, NativeOps};
use lichen_highlevel::program::{
    Ctx, HighProgram, HighProgramLiteral, HighProgramOperator, HighProgramValue, IntLit,
    ProgramImpl, ValueType,
};
use lichen_lowlevel::Registry;
use std::sync::{Arc, RwLock};

/// The probe program: the built-in vocabularies and no attribute.
type ProbeProgram = ProgramImpl<HighProgramValue, HighProgramOperator, NoAttr, HighProgramLiteral>;

/// A well-formed builder: it emits one node through `Ctx` and returns it as the
/// call's value.  The type is the checker's.
struct WellFormed;

static WELL_FORMED: WellFormed = WellFormed;

impl<P: HighProgram> NativeOp<P> for WellFormed
where
    P::Value: ValueType,
{
    fn build(
        &self,
        _ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        NativeApply {
            value: args[0].value,
            decided: true,
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
    // Single-threaded: filed values hold raw arena handles, so this `Arc` is
    // not `Send`; the builder takes it by value.
    #[allow(clippy::arc_with_non_send_sync)]
    let registry = Arc::new(RwLock::new(Registry::new()));
    Checker::<ProbeProgram>::build_in_attr_native(
        probe_call(),
        registry,
        Box::new(|_: &NoAttr| -> &'static dyn AttrExt<ProbeProgram> { unreachable!() }),
        ops(operator),
    )
}

#[test]
fn a_well_formed_native_operator_is_adopted() {
    let build = build(&WELL_FORMED);
    assert!(
        build.ok,
        "a builder that emits one node through Ctx must be adopted, got: {:?}",
        build.diagnostics()
    );
}
