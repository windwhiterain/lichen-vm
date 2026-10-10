//! What this backend refuses, and that it says which cause.
//!
//! These need no device: they are properties of the emitter, and a refusal that
//! only appeared once a GPU was present would be a refusal nobody could test on
//! a machine without one.

use std::collections::HashMap;

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    Br, FlatOp, IntWidth, KernelBody, KernelFragment, KernelId, KernelInstr, KernelRoles,
    KernelShape, LaunchSet, ScalarClass, Terminator,
};

/// A one-output fragment over `(input, index)`.
///
/// `inputs` is taken rather than assumed because the tail decides it — a tail
/// that reads position 0 needs one buffer and a tail that reads nothing needs
/// none — and these tests exist precisely to be about the tail. The classes are
/// the only class this ABI has: an `i64` buffer element
/// (`docs/notes/floating-point.md` §3.8).
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

/// A callee whose domain is **one `Int` leaf**, and whose body is `tail` over it:
/// `from_flat` seeds its stack empty, so a body reads the parameter in by name.
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

/// A callee whose domain is **empty**: the shape a call has to disagree with for
/// the arity refusal to be reachable at all.
fn nullary_callee() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(Vec::new()),
        body: KernelBody::from_flat(
            0,
            &[FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1))],
        ),
        ..unary_callee(Vec::new())
    }
}

/// The caller every cross-kernel test here uses: it reads input 0 at the index,
/// calls kernel `7` with that element, and writes the answer to output 0.
///
/// **Hand-built, not `from_flat`.** A flat body gives a `CallKernel` no arguments
/// — the arity is the callee's own and a flat builder has no callee to read it
/// from — so the argument list these tests are about has to be written out.
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

/// **The refusal this test used to pin is gone, and it is gone rather than
/// unused.** A cross-kernel call is a call into a function the module declares, so
/// the launch set is what says which one, and two different callees are two
/// different modules — which is what says the callee reached the emitter instead
/// of being ignored.
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

/// A call whose callee the set does not hold is **refused by name**, and the
/// message says which kernel is missing: the set is the caller's to assemble, so
/// that is where the fix is.
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

/// The arity of a call is the **callee's own domain**, which is the read that
/// makes the callee's fragment necessary: `KernelInstr::arity` answers `None` for
/// a call because the IR does not carry it.
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
    // Parameter 0 is an input slot, not the index. On this target a buffer is
    // bound as a storage buffer rather than passed as a value, so there is no
    // value for it to hold.
    let fragment = body_with(
        0,
        vec![
            FlatOp::Read(0),
            FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        ],
    );
    let refusal = spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT).expect_err("refused");
    // `at` is the instruction's position in the entry block — not the operand's
    // position in its argument list, which is what the refusal used to name.
    assert_eq!(refusal, SpirvRefusal::NonIndexParameter { local: 0, at: 1 });
    assert!(refusal.to_string().contains("storage buffer"));
}

/// The regression that matters most, because it is silent: reads and writes
/// address **separate** position spaces, so a write's position `0` is the first
/// output. Collapsing them makes every run write into its input and read back
/// zeroes, with no error anywhere.
///
/// The fragment is the shape a real two-input parallel kernel lowers to:
/// `param_shape` is `(config, index)` — two leaves — however many inputs there
/// are, because the extra inputs are reached through a read's position rather
/// than through a further parameter. **`inputs` is what says how many there
/// are**, and this fragment is why: it reads input 1 and so declares 2, while
/// its shape still flattens to 2 and a shape-derived count would have read that
/// coincidence as agreement.
#[test]
fn a_write_position_counts_outputs_not_the_combined_buffer_list() {
    // Read input 1, write output 0. With the position spaces collapsed, that
    // write would land on input 0 and the output would stay zero.
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
            // **Two, matching the two reads below.** A domain of one would make
            // `Read(1)` name a parameter that does not exist, and validate()
            // refuses that as a structural break before the emitter reads a
            // position — so the test would be about the wrong refusal.
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
            // The position's own `ValueId`: under SSA the emitter reads the
            // constant by name rather than counting operand stack slots.
            at: 2,
        }
    );
    assert!(refusal.to_string().contains("output"));
}

#[test]
fn an_operator_with_too_few_operands_is_refused_by_arity() {
    // `Bin(Add)` names two operands and is given none. Under SSA there is no
    // operand stack: `KernelBody::validate` — the gate every backend calls
    // first — refuses this by arity, naming the operator and both counts.
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
    let refusal = spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT).expect_err("refused");
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
/// `dispatch` allocates output buffers without initialising them, so this is the
/// one refusal the no-zero-fill claim rests on and it is enforced here too, not
/// only in the lowering. See `docs/notes/loop-conversion.md` §6.
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
    let refusal = spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT).expect_err("refused");
    // `at` is the write's position in the loop body block.
    assert_eq!(refusal, SpirvRefusal::WriteInsideLoop { at: 2 });
    let message = refusal.to_string();
    assert!(message.contains('2'), "names the write: {message}");
    assert!(
        message.contains("without initialising"),
        "says why the buffer matters: {message}"
    );
}
