//! What this backend refuses, and that it says which cause.
//!
//! # Invariant
//! These need no device: they are properties of the emitter, and a refusal that only appeared once a
//! GPU was present would be a refusal nobody could test on a machine without one.

use std::collections::HashMap;

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBody, KernelFragment, KernelId, KernelInstr, KernelRoles, KernelShape,
    LaunchSet, ScalarClass, Terminator,
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
        int_width: IntWidth::I64,
    }
}

/// The caller the cross-kernel tests here use.
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

/// The refusal this test used to pin is gone rather than unused: a call becomes a
/// call into a declared function.
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

/// `at` is the call's own index, which is what it names now that a body is SSA.
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
            // Two leaves, matching `param_shape` below, because the body reads
            // parameter 1: a `Read` past the domain pushes nothing.
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
    let refusal =
        spirv::compile(&LaunchSet::single(&fragment), ONE_IN_ONE_OUT).expect_err("refused");
    // There is no unbalanced body any more: an instruction names its operands by
    // `ValueId`.
    assert!(
        refusal
            .to_string()
            .contains("reads 2 value(s) but is given 0"),
        "the refusal names the arity it could not satisfy: {refusal}"
    );
}
