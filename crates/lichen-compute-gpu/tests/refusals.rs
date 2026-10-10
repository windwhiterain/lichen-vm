//! What this backend refuses, and that it says which cause.
//!
//! These need no device: they are properties of the emitter, and a refusal that
//! only appeared once a GPU was present would be a refusal nobody could test on
//! a machine without one.

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    Br, FlatOp, IntWidth, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    ScalarClass, Terminator,
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

#[test]
fn a_cross_kernel_call_is_refused_by_name() {
    let fragment = body_with(
        1,
        vec![
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            FlatOp::Read(1),
            FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)),
            FlatOp::Instr(KernelInstr::CallKernel(7)),
            FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        ],
    );
    let refusal = spirv::compile(&fragment, ONE_IN_ONE_OUT).expect_err("refused");
    // `at` is the instruction's position in the entry block.
    assert_eq!(refusal, SpirvRefusal::CrossKernelCall { kernel: 7, at: 3 });
    // The message has to name the kernel and say what is missing, or the reader
    // has nothing to act on.
    let message = refusal.to_string();
    assert!(message.contains('7'), "names the callee: {message}");
    assert!(message.contains("call graph"), "says why: {message}");
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
    let refusal = spirv::compile(&fragment, ONE_IN_ONE_OUT).expect_err("refused");
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
    spirv::compile(&fragment, binding).expect("two inputs, one output");

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
    let refusal = spirv::compile(&beyond, binding).expect_err("refused");
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
    let refusal = spirv::compile(&fragment, ONE_IN_ONE_OUT).expect_err("refused");
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
    let refusal = spirv::compile(&fragment, ONE_IN_ONE_OUT).expect_err("refused");
    // `at` is the write's position in the loop body block.
    assert_eq!(refusal, SpirvRefusal::WriteInsideLoop { at: 2 });
    let message = refusal.to_string();
    assert!(message.contains('2'), "names the write: {message}");
    assert!(
        message.contains("without initialising"),
        "says why the buffer matters: {message}"
    );
}
