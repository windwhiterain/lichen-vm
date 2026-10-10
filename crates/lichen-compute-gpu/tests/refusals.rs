//! What this backend refuses, and that it says which cause.
//!
//! # Invariant
//! These need no device: they are properties of the emitter, and a refusal that only appeared once a
//! GPU was present would be a refusal nobody could test on a machine without one.

use std::collections::HashMap;

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    Br, FlatOp, IntWidth, KernelBody, KernelFragment, KernelId, KernelInstr, KernelRoles,
    KernelShape, LaunchSet, ScalarClass, Terminator,
};

/// A one-output fragment over `(input, index)`.
///
/// # Invariant
/// `inputs` is taken rather than assumed because the tail decides it: a tail reading position 0 needs
/// one buffer and a tail reading nothing needs none. The classes are the only class this ABI has.
fn body_with(inputs: usize, tail: Vec<FlatOp>) -> KernelFragment {
    let mut body: Vec<FlatOp> = vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos, in the *output* space
        FlatOp::Read(1),                                        // idx
    ];
    body.extend(tail);
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(2, &body),
        inputs,
        outputs: 1,
        input_classes: vec![ScalarClass::Int; inputs],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

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
        int_width: IntWidth::I64,
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
        int_width: IntWidth::I64,
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

#[test]
fn reading_a_non_index_parameter_is_refused_by_name() {
    // Parameter 0 is an input slot, not the index: a buffer is bound, not passed.
    let fragment = body_with(
        0,
        vec![
            FlatOp::Read(0),
            FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        ],
    );
    let refusal =
        spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT).expect_err("refused");
    // `at` is the instruction's position in the entry block.
    assert_eq!(refusal, SpirvRefusal::NonIndexParameter { local: 0, at: 1 });
    assert!(refusal.to_string().contains("storage buffer"));
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
        int_width: IntWidth::I64,
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
        int_width: IntWidth::I64,
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

/// A loop body that writes: the write a zero trip count can skip.
///
/// # Invariant
/// `dispatch` allocates output buffers uninitialised, so this is the refusal the no-zero-fill claim
/// rests on.
#[test]
fn a_write_inside_a_loop_is_refused_by_name() {
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
        int_width: IntWidth::I64,
    };
    fragment
        .body
        .validate()
        .expect("the body is well formed; it is the *write* that is refused");
    let refusal =
        spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT).expect_err("refused");
    // `at` is the write's position in the loop body block.
    assert_eq!(refusal, SpirvRefusal::WriteInsideLoop { at: 2 });
    let message = refusal.to_string();
    assert!(message.contains('2'), "names the write: {message}");
    assert!(
        message.contains("without initialising"),
        "says why the buffer matters: {message}"
    );
}
