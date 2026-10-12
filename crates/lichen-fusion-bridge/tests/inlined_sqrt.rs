//! The `sqrt` reader answers the operator, and only where the definition is.
//! See docs/notes/floating-point.md §7.

use lichen_fusion::{BinaryOp, Expr, UnaryOp, Variable};
use lichen_fusion_bridge::{BridgeRefusal, ReadRoles, projection};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelShape, LaneWidth,
    ScalarClass, TargetFacts, ValueId,
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

/// The body `sqrt (2 · datum)` takes once the lowering has written `sqrt` out.
///
/// # Invariant
/// The operand is **two instructions wide**, so a reader that answered some other value of the
/// definition — its zero, its track, its seed — would answer a different expression rather
/// than the same one refused.
fn inlined() -> KernelBody {
    let ops = [
        FlatOp::Read(0),
        FlatOp::Instr(KernelInstr::Const(
            ScalarClass::Float,
            f32::to_bits(2.0) as i64,
        )),
        FlatOp::Instr(KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul)),
        FlatOp::Instr(KernelInstr::Sqrt),
    ];
    let mut body = KernelBody::from_flat(2, &ops);
    body.expand_transcendentals()
        .expect("the definition inlines");
    body
}

/// A `Float` operation over two named values.
fn bin(
    body: &mut KernelBody,
    block: usize,
    op: KernelBin,
    left: ValueId,
    right: ValueId,
) -> ValueId {
    body.add_op(
        block,
        KernelInstr::Bin(ScalarClass::Float, op),
        vec![left, right],
        vec![ScalarClass::Float],
    )
}

/// A `Float` constant pushed in `block`.
fn number(body: &mut KernelBody, block: usize, value: f32) -> ValueId {
    body.add_const(block, ScalarClass::Float, f32::to_bits(value) as i64)
}

/// The `0`/`1` a comparison yields, at the width a `Select` reads.
fn condition(
    body: &mut KernelBody,
    block: usize,
    op: KernelBin,
    left: ValueId,
    right: ValueId,
) -> ValueId {
    let compared = bin(body, block, op, left, right);
    body.add_op(
        block,
        KernelInstr::I32WrapI64,
        vec![compared],
        vec![ScalarClass::Int],
    )
}

/// The square root's definition over `x`, written out by hand at `seed`.
///
/// # Invariant
/// **Not a call to `sqrt_body()`**: the reader rebuilds that body and matches it instruction
/// for instruction, so this is the second, independent statement of the same sequence — and a
/// reader matching a stale copy of the definition is the one failure here that would answer
/// with a `sqrt` the body does not compute.
fn definition(body: &mut KernelBody, block: usize, x: ValueId, seed: f32) -> ValueId {
    let zero = number(body, block, 0.0);
    let positive = condition(body, block, KernelBin::Gt, x, zero);
    let mut reduced = body.add_op(
        block,
        KernelInstr::Select,
        vec![x, zero, positive],
        vec![ScalarClass::Float],
    );
    let mut track = number(body, block, 1.0);

    for (steps, below, step, track_step) in [
        (5usize, true, 4_294_967_296.0f32, 1.0f32 / 65_536.0),
        (40, false, 1.0f32 / 16.0, 4.0f32),
    ] {
        for _ in 0..steps {
            let edge = number(body, block, if below { 1.0 } else { 16.0 });
            let test = condition(
                body,
                block,
                if below { KernelBin::Lt } else { KernelBin::Geq },
                reduced,
                edge,
            );
            let factor = number(body, block, step);
            let scaled = bin(body, block, KernelBin::Mul, reduced, factor);
            reduced = body.add_op(
                block,
                KernelInstr::Select,
                vec![scaled, reduced, test],
                vec![ScalarClass::Float],
            );
            let factor = number(body, block, track_step);
            let scaled = bin(body, block, KernelBin::Mul, track, factor);
            track = body.add_op(
                block,
                KernelInstr::Select,
                vec![scaled, track, test],
                vec![ScalarClass::Float],
            );
        }
    }

    let mut root = number(body, block, seed);
    for _ in 0..6 {
        let quotient = bin(body, block, KernelBin::Div, reduced, root);
        let sum = bin(body, block, KernelBin::Add, root, quotient);
        let half = number(body, block, 0.5);
        root = bin(body, block, KernelBin::Mul, sum, half);
    }
    let root = bin(body, block, KernelBin::Mul, root, track);
    body.add_op(
        block,
        KernelInstr::Select,
        vec![root, zero, positive],
        vec![ScalarClass::Float],
    )
}

/// The same computation with the definition written out by hand, at `seed`.
fn hand_written(seed: f32) -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let datum = body.add_param(entry);
    let two = number(&mut body, entry, 2.0);
    let operand = bin(&mut body, entry, KernelBin::Mul, datum, two);
    definition(&mut body, entry, operand, seed);
    body
}

/// The two spellings of one `sqrt` answer one expression, and a near miss answers none.
///
/// # Invariant
/// **The check a wrong recognition cannot pass.** A body that writes `KernelInstr::Sqrt` and
/// one carrying the definition by hand are one computation in two encodings, so the walk must
/// read both as `sqrt` of the *same* operand. A definition with one constant moved is a
/// different computation, refused by the `Select` it really holds.
#[test]
fn a_sqrt_is_read_as_a_sqrt_only_where_the_definition_is() {
    let from_instruction = read(&fragment(inlined())).expect("the expansion is a sqrt");
    let wanted = UnaryOp::Sqrt.apply(Expr::Binary(
        BinaryOp::Mul,
        Box::new(Expr::Variable(Variable::DATUM)),
        Box::new(Expr::Constant(2.0)),
    ));
    assert_eq!(
        from_instruction, wanted,
        "the answer names the operand, not only itself"
    );
    assert_eq!(
        from_instruction,
        read(&fragment(hand_written(2.0))).expect("the definition written out is a sqrt"),
        "the two spellings of one computation answer one expression"
    );

    assert_eq!(
        read(&fragment(hand_written(3.0))).expect_err("a moved constant is not the definition"),
        BridgeRefusal::Select,
        "a body that computes something else is refused by the instruction it holds"
    );
}
