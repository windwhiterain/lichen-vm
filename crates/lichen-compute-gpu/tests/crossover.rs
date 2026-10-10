//! What this backend refuses, and that it says which cause.
//!
//! These need no device: they are properties of the emitter, and a refusal that
//! only appeared once a GPU was present would be a refusal nobody could test on
//! a machine without one.

use std::collections::HashMap;

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBody, KernelFragment, KernelId, KernelInstr, KernelRoles, KernelShape,
    LaunchSet, ScalarClass, Terminator,
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

/// The caller the cross-kernel tests here use: it reads input 0 at the index,
/// calls kernel `7` with that element, and writes the answer to output 0.
///
/// **Hand-built, not `from_flat`.** A flat body gives a `CallKernel` no arguments
/// — the arity is the callee's own and a flat builder has no callee to read it
/// from — so the argument list this test is about has to be written out.
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
/// unused.** A cross-kernel call becomes a call into a function the module
/// declares, so the launch set is what resolves it — and the callee's own body is
/// what the module then holds, which is what the two emissions below differ in.
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

/// **`at` is the call's own index**, which is what it names now that a body is
/// SSA: the stack position it used to be has no meaning here, and a refusal
/// pointing at one would point at something that is not in the body.
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
    // `at` is the instruction's position in the entry block, as it is for every
    // other refusal, and not the operand's position in its argument list.
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
            // **Two leaves, matching `param_shape` below**, because the body reads
            // parameter 1. A `Read` past the domain pushes nothing, so a fragment
            // that declared fewer parameters than it reads comes up short and the
            // refusal names the *consumer's* arity rather than the read — which is
            // what this test was accidentally asserting.
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
            at: 2,
        }
    );
    assert!(refusal.to_string().contains("output"));
}

#[test]
fn an_unbalanced_body_is_refused() {
    // No operands pushed before the operator, so it pops from an empty stack.
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
    // **There is no such thing as an unbalanced body any more.** An instruction
    // names its operands by `ValueId`, so an operator cannot "pop from an empty
    // stack" — the shape that made `UnbalancedStack` mean something is gone, and
    // the variant went with it. What is left is the honest refusal: the operator
    // declares two operands and the body gives it none, and
    // `KernelBody::validate` says so before the emitter reads anything.
    assert!(
        refusal
            .to_string()
            .contains("reads 2 value(s) but is given 0"),
        "the refusal names the arity it could not satisfy: {refusal}"
    );
}
