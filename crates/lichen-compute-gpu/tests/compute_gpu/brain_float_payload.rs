//! What a `BFloat16` buffer's payload is, as the device reads it.

use super::common;

use lichen_compute_gpu::spirv::{self, Binding};
use lichen_kernel_ir::{
    BufferSlot, FlatOp, IntWidth, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaneHeight, LaneWidth, LaunchSet, ScalarClass, TargetFacts,
};

/// A copy of one buffer into another, both classed `BFloat16`.
///
/// # Invariant
/// The body is [`spirv_validation`]'s own shape with the multiply taken out: the read's value goes
/// straight to the write, so every number this answers came out of the payload rather than out of the
/// arithmetic.
fn a_copy_of_a_brain_float() -> KernelFragment {
    let access = |body: &mut Vec<FlatOp>| {
        body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
        body.push(FlatOp::Read(1));
    };
    let mut body = Vec::new();
    access(&mut body); // the output's position and index
    access(&mut body); // the input's
    body.push(FlatOp::Instr(KernelInstr::BufferReadCall(
        ScalarClass::BFloat16,
    )));
    body.push(FlatOp::Instr(KernelInstr::BufferWriteCall(
        ScalarClass::BFloat16,
    )));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(2, &body),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::BFloat16],
        output_classes: vec![ScalarClass::BFloat16],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// The half of each `f32`, little-endian, which is one `bf16` element.
fn halves(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
        .collect()
}

/// Four `bf16` elements go to the device and come back as the four numbers they hold.
///
/// # Invariant
/// The payload is two bytes an element and the count is the element count, so an element read at four
/// bytes would answer two numbers built from two halves each rather than these four.
#[test]
fn a_brain_float_payload_round_trips() {
    let Some(context) = common::context("a_brain_float_payload_round_trips") else {
        return;
    };
    let fragment = a_copy_of_a_brain_float();
    let words = spirv::compile(
        &LaunchSet::single(&fragment),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .unwrap_or_else(|refusal| panic!("a brain-float copy is emitted: {refusal}"));
    assert!(!words.is_empty(), "an emitted module holds instructions");

    let values = [1.0_f32, 2.0, 3.0, 4.0];
    let packed = halves(&values);
    let resident = context
        .run(
            &LaunchSet::single(&fragment),
            &[BufferSlot::Host(&packed)],
            4,
        )
        .unwrap_or_else(|refusal| panic!("a brain-float copy is dispatched: {refusal}"));
    let answered = match context
        .fetch(resident[0], 4)
        .expect("the answer comes back")
    {
        lichen_kernel_ir::ScalarData::Float(answered) => answered,
        other => panic!("a brain-float buffer answers as numbers, and this answered {other:?}"),
    };
    context.release(resident[0]);
    assert_eq!(
        answered,
        values.to_vec(),
        "the four elements come back as they went"
    );
}
