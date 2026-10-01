//! What this backend refuses, and that it says which cause.
//!
//! These need no device: they are properties of the emitter, and a refusal that
//! only appeared once a GPU was present would be a refusal nobody could test on
//! a machine without one.

use lichen_compute_gpu::spirv::{self, Binding, SpirvRefusal};
use lichen_kernel_ir::{IntWidth, KernelFragment, KernelInstr, KernelShape};

/// A one-output fragment over `(input, index)`.
fn body_with(tail: Vec<KernelInstr>) -> KernelFragment {
    let mut body = vec![
        KernelInstr::Const(0),    // out_pos, in the *output* space
        KernelInstr::LocalGet(1), // idx
    ];
    body.extend(tail);
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body,
        outputs: 1,
        results: 1,
        int_width: IntWidth::I64,
    }
}

const ONE_IN_ONE_OUT: Binding = Binding {
    inputs: 1,
    outputs: 1,
};

#[test]
fn a_cross_kernel_call_is_refused_by_name() {
    let fragment = body_with(vec![
        KernelInstr::Const(0),
        KernelInstr::LocalGet(1),
        KernelInstr::BufferReadCall,
        KernelInstr::CallKernel(7),
        KernelInstr::BufferWriteCall,
        KernelInstr::Const(0),
    ]);
    let refusal = spirv::compile(&fragment, ONE_IN_ONE_OUT).expect_err("refused");
    assert_eq!(refusal, SpirvRefusal::CrossKernelCall { kernel: 7, at: 5 });
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
    let fragment = body_with(vec![
        KernelInstr::LocalGet(0),
        KernelInstr::BufferWriteCall,
        KernelInstr::Const(0),
    ]);
    let refusal = spirv::compile(&fragment, ONE_IN_ONE_OUT).expect_err("refused");
    assert_eq!(refusal, SpirvRefusal::NonIndexParameter { local: 0, at: 2 });
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
/// than through a further parameter.
#[test]
fn a_write_position_counts_outputs_not_the_combined_buffer_list() {
    // Read input 1, write output 0. With the position spaces collapsed, that
    // write would land on input 0 and the output would stay zero.
    let fragment = KernelFragment {
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body: vec![
            KernelInstr::Const(0),    // out_pos, in the *output* space
            KernelInstr::LocalGet(1), // idx
            KernelInstr::Const(1),    // cfg_pos, in the *input* space
            KernelInstr::LocalGet(1),
            KernelInstr::BufferReadCall,
            KernelInstr::BufferWriteCall,
            KernelInstr::Const(0),
        ],
        outputs: 1,
        results: 1,
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
        body: vec![
            KernelInstr::Const(1), // out_pos 1, but there is one output
            KernelInstr::LocalGet(1),
            KernelInstr::Const(5),
            KernelInstr::BufferWriteCall,
            KernelInstr::Const(0),
        ],
        ..fragment
    };
    let refusal = spirv::compile(&beyond, binding).expect_err("refused");
    assert_eq!(
        refusal,
        SpirvRefusal::BufferPositionOutOfRange {
            position: 1,
            space: "output",
            bound: 1,
            at: 3,
        }
    );
    assert!(refusal.to_string().contains("output"));
}

#[test]
fn an_unbalanced_body_is_refused() {
    // No operands pushed before the operator, so it pops from an empty stack.
    let fragment = KernelFragment {
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body: vec![
            KernelInstr::Bin(lichen_kernel_ir::KernelBin::Add),
            KernelInstr::Const(0),
        ],
        outputs: 1,
        results: 1,
        int_width: IntWidth::I64,
    };
    let refusal = spirv::compile(&fragment, ONE_IN_ONE_OUT).expect_err("refused");
    assert_eq!(refusal, SpirvRefusal::UnbalancedStack { at: 0 });
}
