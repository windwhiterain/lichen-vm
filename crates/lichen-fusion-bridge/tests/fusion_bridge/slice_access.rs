//! What the bridge refuses, and that it names the cause.
//!
//! # Invariant
//! A projection is one symbolic expression, so it carries no staging: a marked value that
//! reaches a slice is refused rather than read as a term.

use lichen_fusion_bridge::{BridgeRefusal, ReadRoles, projection};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    LaneHeight, LaneWidth, ScalarClass, Slice, Space, TargetFacts, ValueDef, ValueId,
};

/// A body that writes one slice, leaving behind the version a read would name.
///
/// # Invariant
/// The write's operands are `[slice, index, value]` and its result is the version, so the body
/// stages a value between lanes; the trailing constant is the one value the ABI's result wants.
fn stages_a_slice() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // the slice ordinal
                FlatOp::Read(1),                                        // the index into it
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 7)), // the value written
                FlatOp::Instr(KernelInstr::SliceWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: 64,
            space: Space::Shared,
        }],
        marks: Vec::new(),
    }
}

/// The version a slice write leaves, which is the value a mark would ride.
fn the_version(fragment: &KernelFragment) -> ValueId {
    fragment
        .body
        .values
        .iter()
        .position(|definition| {
            matches!(
                definition,
                ValueDef::Instr {
                    op: KernelInstr::SliceWriteCall(_),
                    ..
                }
            )
        })
        .map(|at| ValueId(at as u32))
        .expect("the body stages a slice")
}

/// A projection that reads a staged value is refused by name, never lowered to a term.
#[test]
fn a_slice_access_is_refused_by_name() {
    let fragment = stages_a_slice();
    let marked = the_version(&fragment);
    let refusal = projection(&fragment, marked, ReadRoles::DATUM_ONLY)
        .expect_err("a symbolic projection cannot stage a value between lanes");
    assert_eq!(refusal, BridgeRefusal::SliceAccess { slice: 0 });
    let message = refusal.to_string();
    assert!(message.contains("slice 0"), "names the slice: {message}");
    assert!(
        message.contains("no staging"),
        "says what a symbolic expression cannot carry: {message}"
    );
}
