//! The `ln` reader answers the operator, as `exp`'s does.
//! See docs/notes/transcendentals-as-operators.md §5.1.

use lichen_fusion::{BinaryOp, Expr, UnaryOp, Variable};
use lichen_fusion_bridge::{BridgeRefusal, ReadRoles, projection};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelShape, LaneWidth,
    ScalarClass, TargetFacts,
};

/// A scalar fragment over `body`, whose entry leaf `0` is its datum.
fn fragment(body: KernelBody) -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Float),
        ]),
        body,
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        result_classes: vec![ScalarClass::Float],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
        roles: Default::default(),
    }
}

/// The expression the fragment's last computed value denotes.
fn read(fragment: &KernelFragment) -> Result<Expr, BridgeRefusal> {
    let marked = fragment.body.blocks[fragment.body.entry]
        .instrs
        .last()
        .copied()
        .expect("the body computes something");
    projection(fragment, marked, ReadRoles::parameters(0, 1))
}

/// The body `ln (2 · datum)` takes once the lowering has written `ln` out.
fn inlined() -> KernelBody {
    let ops = [
        FlatOp::Read(0),
        FlatOp::Instr(KernelInstr::Const(
            ScalarClass::Float,
            f32::to_bits(2.0) as i64,
        )),
        FlatOp::Instr(KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul)),
        FlatOp::Instr(KernelInstr::Ln),
    ];
    let mut body = KernelBody::from_flat(2, &ops);
    body.expand_transcendentals()
        .expect("the definition inlines");
    body
}

/// A registered `ln` reads as the operator, named over the same operand an `exp` reads.
///
/// # Invariant
/// **The fragment holds the expansion, not [`KernelInstr::Ln`]**, so the reader and not the
/// instruction's own arm is what answers — the one asymmetry §5.1 recorded. The expression
/// carries the operand, so a body reading as some other operator would answer another one.
#[test]
fn a_ln_is_read_as_a_ln_where_the_definition_is() {
    assert_eq!(
        read(&fragment(inlined())).expect("the expansion is a ln"),
        UnaryOp::Ln.apply(Expr::Binary(
            BinaryOp::Mul,
            Box::new(Expr::Variable(Variable::DATUM)),
            Box::new(Expr::Constant(2.0)),
        )),
        "the answer names the operand, not only itself"
    );
}
