//! A **value** is in the class its definition states; a crossing is where
//! two readings part company.

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    LaneHeight, LaneWidth, LaunchSet, ScalarClass, TargetFacts,
};

/// One `Float` input and one `Float` output, over `(n, a, index)`.
///
/// # Invariant
/// **The second leaf is a config leaf and is read as the config the host fixes**, so a
/// crossing over it is `int2float` of a parameter rather than of a literal: this is the
/// shape `examples/attention.lichen` writes, where `k.dim` is a third leaf beside the
/// launch extent and the second extent.
fn fragment(body: KernelBody) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Float; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// A `Float` constant pushed on the flat stack.
fn number(value: f32) -> FlatOp {
    FlatOp::Instr(KernelInstr::Const(
        ScalarClass::Float,
        i64::from(value.to_bits()),
    ))
}

/// `out[i] = in[i] · sqrt (int2float a)`, with the `sqrt` still the instruction the
/// expansion rewrites.
///
/// # Invariant
/// **The definition is expanded here, by the pass the lowering itself runs**, so the
/// `Select` the emitter refuses is the one `unary::sqrt_body` writes: a body that kept
/// `KernelInstr::Sqrt` would be refused for leaving a transcendental, which pins nothing.
/// The `+ 0.0` spelling already ran, so it is the control beside the shape that did not.
/// `docs/notes/floating-point.md` §7.
fn scores(through_the_crossing_directly: bool) -> KernelFragment {
    let read = |body: &mut Vec<FlatOp>| {
        body.push(number(0.0)); // the element's position, an `Int` in the input space
        body.push(FlatOp::Read(2)); // the index
        body.push(FlatOp::Instr(KernelInstr::BufferReadCall(
            ScalarClass::Float,
        )));
    };
    let mut body = vec![FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0))];
    body.push(FlatOp::Read(2)); // the write's index
    read(&mut body); // in[i]
    body.push(FlatOp::Read(1)); // the config leaf `a`
    body.push(FlatOp::Instr(KernelInstr::Conv {
        from: ScalarClass::Int,
        to: ScalarClass::Float,
    })); // int2float a
    if !through_the_crossing_directly {
        body.push(number(0.0));
        body.push(FlatOp::Instr(KernelInstr::Bin(
            ScalarClass::Float,
            KernelBin::Add,
        )));
    }
    body.push(FlatOp::Instr(KernelInstr::Sqrt));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Float,
        KernelBin::Mul,
    )));
    body.push(FlatOp::Instr(KernelInstr::BufferWriteCall(
        ScalarClass::Float,
    )));
    body.push(number(0.0)); // the ABI's one result
    let mut written = fragment(KernelBody::from_flat(3, &body));
    written
        .body
        .expand_transcendentals()
        .expect("the square root's definition inlines");
    written
}

/// A `Select` whose two arms are constants of different classes — the refusal the fix must
/// not have widened.
///
/// # Invariant
/// **Two constants, so nothing else can be what refuses.** The arms are the one place a
/// class cannot be inherited from a declaration, so this is the only fragment whose answer
/// depends on the crossing's class and nothing else; the index is a parameter rather than a
/// value here, since a parameter read as a value has no class of its own to disagree.
fn arms_disagree() -> KernelFragment {
    let body = KernelBody::from_flat(
        3,
        &[
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // the write's position
            FlatOp::Read(2),                                        // the write's index
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 7)), // an `Int` arm
            number(1.0),                                            // a `Float` arm
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)), // the condition
            FlatOp::Instr(KernelInstr::Select),
            FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Float)),
            number(0.0),
        ],
    );
    fragment(body)
}

/// `sqrt (int2float a)` is emitted, and the refusal beside it is still the refusal.
///
/// # Invariant
/// **The fix is the crossing's class, not the check.** A `Select` whose arms really are
/// different classes must still be refused by name, so both are pinned in one place: the
/// first is the shape that used to be refused, the second is what the answer may not have
/// become. Both need no device — they are properties of the emitter.
#[test]
fn a_crossing_feeding_a_conditional_is_the_float_it_converts_to() {
    let binding = Binding {
        inputs: 1,
        outputs: 1,
    };
    spirv::compile(&LaunchSet::single(&scores(true)), binding)
        .expect("a crossing feeding a spliced definition's `Select` directly is emitted");
    spirv::compile(&LaunchSet::single(&scores(false)), binding)
        .expect("the same kernel through `+ 0.0` is emitted, as it always was");

    let refusal = spirv::compile(&LaunchSet::single(&arms_disagree()), binding)
        .expect_err("two arms of genuinely different classes are refused");
    assert_eq!(
        refusal,
        SpirvRefusal::SelectArmClassesDisagree {
            classes: vec![ScalarClass::Int, ScalarClass::Float],
        },
        "the refusal names the two classes, as it always did"
    );
}
