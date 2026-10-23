//! What this backend refuses, and that it says which cause.
//!
//! # Invariant
//! These need no device: they are properties of the emitter, and a refusal that only appeared once a
//! GPU was present would be a refusal nobody could test on a machine without one.

use std::collections::HashMap;

use lichen_compute_gpu::SUBGROUP_WIDTH;
use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    Br, FlatOp, IntWidth, KernelBody, KernelFragment, KernelId, KernelInstr, KernelRoles,
    KernelShape, LaneHeight, LaneWidth, LaunchSet, ScalarClass, Slice, Space, TargetFacts,
    Terminator,
};

const ONE_IN_ONE_OUT: Binding = Binding {
    inputs: 1,
    outputs: 1,
};

/// A callee whose domain is one `Int` leaf, and whose body is `tail` over it.
fn unary_callee(tail: Vec<FlatOp>) -> KernelFragment {
    let mut ops = vec![FlatOp::Read(0)];
    ops.extend(tail);
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Scalar(ScalarClass::Int),
        body: KernelBody::from_flat(1, &ops),
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
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

/// A callee whose domain is empty: the shape an arity refusal needs.
fn nullary_callee() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(Vec::new()),
        body: KernelBody::from_flat(0, &[FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1))]),
        ..unary_callee(Vec::new())
    }
}

/// The caller every cross-kernel test here uses.
///
/// # Invariant
/// Hand-built, not `from_flat`: a flat body gives a `CallKernel` no arguments, because the arity is the
/// callee's own and a flat builder has no callee to read it from.
fn calls_kernel_7() -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _config = body.add_param(entry);
    let index = body.add_param(entry);
    let out_position = body.add_const(entry, ScalarClass::Int, 0);
    let read_position = body.add_const(entry, ScalarClass::Int, 0);
    let element = body.add_op(
        entry,
        KernelInstr::BufferReadCall(ScalarClass::Int),
        vec![read_position, index],
        vec![ScalarClass::Int],
    );
    let called = body.add_op(
        entry,
        KernelInstr::CallKernel(7),
        vec![element],
        vec![ScalarClass::Int],
    );
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![out_position, index, called],
        Vec::new(),
    );
    let returned = body.add_const(entry, ScalarClass::Int, 0);
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![returned],
        },
    );
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
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
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// The refusal this test used to pin is gone rather than unused: a cross-kernel
/// call is a call into a declared function.
#[test]
fn a_cross_kernel_call_is_emitted_into_the_launch_sets_module() {
    let caller = calls_kernel_7();
    let index: HashMap<KernelId, u32> = [(7, 1)].into();
    let identity = spirv::compile(
        &LaunchSet::new(&[caller.clone(), unary_callee(Vec::new())], &index),
        ONE_IN_ONE_OUT,
    )
    .expect("a call whose callee is in the set is emitted");
    let doubled = spirv::compile(
        &LaunchSet::new(
            &[
                caller.clone(),
                unary_callee(vec![
                    FlatOp::Read(0),
                    FlatOp::Instr(KernelInstr::Bin(
                        ScalarClass::Int,
                        lichen_kernel_ir::KernelBin::Add,
                    )),
                ]),
            ],
            &index,
        ),
        ONE_IN_ONE_OUT,
    )
    .expect("a call whose callee is in the set is emitted");
    assert_ne!(
        identity, doubled,
        "the callee's own body is what the caller's module calls"
    );
}

/// A call whose callee the set does not hold is refused by name.
#[test]
fn a_callee_outside_the_launch_set_is_refused_by_name() {
    let caller = calls_kernel_7();
    let refusal = spirv::compile(&LaunchSet::single(&caller), ONE_IN_ONE_OUT)
        .expect_err("a call to a kernel the set does not hold is refused");
    assert_eq!(
        refusal,
        SpirvRefusal::CalleeNotInLaunchSet { kernel: 7, at: 3 }
    );
    let message = refusal.to_string();
    assert!(message.contains('7'), "names the callee: {message}");
    assert!(
        message.contains("does not hold"),
        "says the set is what is missing: {message}"
    );
}

/// The arity of a call is the callee's own domain, the read that makes its
/// fragment necessary.
#[test]
fn a_call_the_callees_domain_does_not_fit_is_refused_by_name() {
    let caller = calls_kernel_7();
    let index: HashMap<KernelId, u32> = [(7, 1)].into();
    let refusal = spirv::compile(
        &LaunchSet::new(&[caller, nullary_callee()], &index),
        ONE_IN_ONE_OUT,
    )
    .expect_err("a call that does not fit its callee is refused");
    assert_eq!(
        refusal,
        SpirvRefusal::CrossKernelArity {
            kernel: 7,
            expected: 0,
            given: 1,
            at: 3,
        }
    );
    let message = refusal.to_string();
    assert!(message.contains('7'), "names the callee: {message}");
    assert!(
        message.contains("takes 0 argument(s)") && message.contains("with 1"),
        "names both counts: {message}"
    );
}

/// The regression that matters most, because it is silent: reads and writes address
/// separate position spaces.
///
/// # Invariant
/// A write's position `0` is the first output, and collapsing the two spaces makes every run write into
/// its input and read back zeroes with no error. The fragment reads input 1 so declares `inputs` 2
/// while its shape still flattens to 2 — a shape-derived count would read that coincidence as
/// agreement.
#[test]
fn a_write_position_counts_outputs_not_the_combined_buffer_list() {
    // Read input 1, write output 0: collapsed spaces would land the write on input 0.
    let fragment = KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos, in the *output* space
                FlatOp::Read(1),                                        // idx
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)), // cfg_pos, in the *input* space
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 2,
        outputs: 1,
        input_classes: vec![ScalarClass::Int, ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    };
    let binding = Binding {
        inputs: 2,
        outputs: 1,
    };
    spirv::compile(&LaunchSet::single(&fragment), binding).expect("two inputs, one output");

    // A write naming an output that does not exist is refused, and the message
    // says which space was addressed.
    let beyond = KernelFragment {
        roles: KernelRoles::default(),
        body: KernelBody::from_flat(
            // Two, matching the two reads below: a domain of one would make `Read(1)`
            // name a parameter that does not exist.
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)), // out_pos 1, but there is one output
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 5)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        ..fragment
    };
    let refusal = spirv::compile(&LaunchSet::single(&beyond), binding).expect_err("refused");
    assert_eq!(
        refusal,
        SpirvRefusal::BufferPositionOutOfRange {
            position: 1,
            space: "output",
            bound: 1,
            // The position's own `ValueId`: the emitter reads the constant by name.
            at: 2,
        }
    );
    assert!(refusal.to_string().contains("output"));
}

/// A `Float` module whose one `Select` has an `Int` arm and a `Float` one.
///
/// # Invariant
/// **The language cannot say this**: its checker refuses the conditional first — `expected Float,
/// found Int` at the arm — so a fragment whose arms disagree is something a builder or a merge
/// composed, and the emitter is the last place that can name it.
fn mixed_arm_conditional() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(1),                                        // idx
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)), // then, an integer
                FlatOp::Instr(KernelInstr::Const(
                    // otherwise, a float
                    ScalarClass::Float,
                    i64::from(2.0f32.to_bits()),
                )),
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 3)),
                FlatOp::Instr(KernelInstr::Bin(
                    ScalarClass::Int,
                    lichen_kernel_ir::KernelBin::Leq,
                )),
                FlatOp::Instr(KernelInstr::I32WrapI64),
                FlatOp::Instr(KernelInstr::Select),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Float)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
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

/// Two arms of different classes are refused by name, not typed by one of them.
///
/// # Invariant
/// An `OpSelect` has one result type and both arms feed it, so the refusal **carries the two
/// classes**: which pair met is what tells a fragment's author which arm to convert, and picking
/// either arm would answer a program the body does not state.
#[test]
fn a_select_whose_arms_disagree_is_refused_by_name() {
    let fragment = mixed_arm_conditional();
    fragment
        .body
        .validate()
        .expect("the body is well formed; it is the arms' classes that are refused");
    let refusal = spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT)
        .expect_err("arms of two classes have no one result type");
    assert_eq!(
        refusal,
        SpirvRefusal::SelectArmClassesDisagree {
            classes: vec![ScalarClass::Int, ScalarClass::Float],
        },
        "the refusal names the pair that met, in class order"
    );
    let message = refusal.to_string();
    assert!(
        message.contains("`Select`") && message.contains("different classes"),
        "names the instruction and the fact: {message}"
    );
    assert!(
        message.contains("Int") && message.contains("Float"),
        "and carries both classes: {message}"
    );
}

#[test]
fn an_operator_with_too_few_operands_is_refused_by_arity() {
    // `Bin(Add)` names two operands and is given none: `KernelBody::validate`
    // refuses this by arity.
    let fragment = KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Bin(
                    ScalarClass::Int,
                    lichen_kernel_ir::KernelBin::Add,
                )),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 0,
        outputs: 1,
        input_classes: Vec::new(),
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    };
    let refusal =
        spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT).expect_err("refused");
    match refusal {
        SpirvRefusal::ControlFlow { ref detail } => {
            assert!(
                detail.contains("Bin(Int, Add)")
                    && detail.contains("reads 2 value(s) but is given 0"),
                "the refusal names the operator and both counts: {detail}"
            );
        }
        other => panic!("an operator given too few operands is refused by arity, got {other:?}"),
    }
}

/// A loop body that writes: the write is emitted where the body writes it.
///
/// # Invariant
/// The ruling withdrew the refusal (`docs/notes/loop-conversion.md` §6): a lane whose trip
/// count is zero leaves its output element as the allocation held, and the module must still
/// be well formed — so the store is validated, not skipped.
#[test]
fn a_write_inside_a_loop_is_emitted() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _config = body.add_param(entry);
    let index = body.add_param(entry);
    let header = body.add_block();
    let latch = body.add_block();
    let exit = body.add_block();

    let enter = body.add_const(entry, ScalarClass::Int, 1);
    body.set_terminator(
        entry,
        Terminator::CondBr {
            cond: enter,
            if_true: Br {
                target: header,
                args: Vec::new(),
            },
            if_false: Br {
                target: exit,
                args: Vec::new(),
            },
        },
    );

    let test = body.add_const(header, ScalarClass::Int, 1);
    body.set_terminator(
        header,
        Terminator::CondBr {
            cond: test,
            if_true: Br {
                target: latch,
                args: Vec::new(),
            },
            if_false: Br {
                target: exit,
                args: Vec::new(),
            },
        },
    );

    let position = body.add_const(latch, ScalarClass::Int, 0);
    let written = body.add_const(latch, ScalarClass::Int, 3);
    body.add_op(
        latch,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![position, index, written],
        Vec::new(),
    );
    body.set_terminator(
        latch,
        Terminator::Br(Br {
            target: header,
            args: Vec::new(),
        }),
    );

    let returned = body.add_const(exit, ScalarClass::Int, 0);
    body.set_terminator(
        exit,
        Terminator::Return {
            values: vec![returned],
        },
    );

    let fragment = KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 0,
        outputs: 1,
        input_classes: Vec::new(),
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    };
    fragment
        .body
        .validate()
        .expect("the body is well formed: the write is now emitted, not refused");
    let words = spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT)
        .expect("a write inside a loop body is emitted");
    assert!(!words.is_empty(), "the module is not empty");
    super::spirv_validation::validate("a write inside a loop body", &words);
}

/// A workgroup width this target cannot schedule is refused by name, not rounded to one it can.
#[test]
fn a_lane_width_this_target_cannot_schedule_is_refused_by_name() {
    let fragment = KernelFragment {
        target: TargetFacts {
            lane_width: LaneWidth::new(32),
            lane_height: LaneHeight::new(1),
            ..unary_callee(Vec::new()).target
        },
        ..unary_callee(Vec::new())
    };
    let refusal = spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT)
        .expect_err("a workgroup width this target does not schedule is refused");
    assert_eq!(refusal, SpirvRefusal::UnsupportedLaneWidth { lanes: 32 });
    let message = refusal.to_string();
    assert!(message.contains("32"), "names the width: {message}");
    assert!(
        message.contains("does not schedule"),
        "says the target cannot honour it: {message}"
    );
}

/// A body that writes one element of a slice in the space it is given.
///
/// # Invariant
/// The operands are `[slice, index, value]` and the write leaves the version a read names, so the
/// trailing constant is the one value the ABI's result wants.
fn writes_a_slice(space: Space) -> KernelFragment {
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
        inputs: 0,
        outputs: 1,
        input_classes: Vec::new(),
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
            space,
        }],
        marks: Vec::new(),
    }
}

/// A space this target cannot honour is refused by name, not placed in the one it can.
#[test]
fn a_register_placed_slice_is_refused_by_name() {
    let fragment = writes_a_slice(Space::Register);
    fragment
        .body
        .validate()
        .expect("the body is well formed; it is the *space* that is refused");
    let refusal = spirv::compile(
        &LaunchSet::single(&fragment),
        Binding {
            inputs: 0,
            outputs: 1,
        },
    )
    .expect_err("this target emits `Shared` slices only");
    match refusal {
        SpirvRefusal::StagingNotOnThisBackend { space, .. } => {
            assert_eq!(
                space,
                Space::Register,
                "names the space the slice asked for"
            )
        }
        other => panic!("refused for the space, not something else: {other:?}"),
    }
    let message = refusal.to_string();
    assert!(message.contains("Register"), "names the space: {message}");
    assert!(
        message.contains("does not emit"),
        "says why this target cannot honour it: {message}"
    );
}

/// A `Warp` slice that reaches past one subgroup is refused, not quietly made shared.
///
/// # Invariant
/// `writes_a_slice` stages 64 elements, the **whole workgroup** and twice this target's subgroup:
/// a read at index `j` would name the lane `j` of the reader's own subgroup, and `j` reaches past
/// it. The refusal carries the count and the width because which two numbers met is what tells
/// the author what to change — `Shared`, or a narrower tile.
#[test]
fn a_warp_slice_reaching_past_one_subgroup_is_refused_by_name() {
    let fragment = writes_a_slice(Space::Warp);
    fragment
        .body
        .validate()
        .expect("the body is well formed; it is the *placement* that is refused");
    let refusal = spirv::compile(
        &LaunchSet::single(&fragment),
        Binding {
            inputs: 0,
            outputs: 1,
        },
    )
    .expect_err("a slice wider than a subgroup cannot be exchanged");
    match &refusal {
        SpirvRefusal::StagingNotOneElementPerLane {
            count,
            subgroup_width,
            ..
        } => {
            assert_eq!(*count, 64, "names the slice's count");
            assert_eq!(
                *subgroup_width, SUBGROUP_WIDTH,
                "names the width one subgroup runs"
            );
        }
        other => panic!("refused for the placement, not something else: {other:?}"),
    }
    let message = refusal.to_string();
    assert!(message.contains('6'), "names the count: {message}");
    assert!(
        message.contains(&SUBGROUP_WIDTH.to_string()),
        "names the width: {message}"
    );
    assert!(
        message.contains("cannot reach"),
        "says what the read would have named: {message}"
    );
}
