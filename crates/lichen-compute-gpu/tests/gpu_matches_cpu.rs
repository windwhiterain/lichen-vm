//! The acceptance criterion: the same fragment, run on the GPU and on the CPU,
//! must agree element for element.
//!
//! Two things are asserted, on purpose. The GPU result is compared against a
//! **hand-written expected vector**, not only against the CPU reference —
//! otherwise a fragment this test and the shader both misread would pass. The
//! counts are deliberately not multiples of the workgroup size, so the surplus
//! lanes of the last workgroup are covered too: those lanes address padding, and
//! if that padding were not allocated the run would scribble past the buffers.

use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X};
use lichen_kernel_ir::{IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape};

/// A fragment over `(input, index)` — the shape a single-input parallel kernel has.
fn fragment(body: Vec<KernelInstr>) -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body,
        outputs: 1,
        results: 1,
        int_width: IntWidth::I64,
    }
}

/// `out[i] = in[i] + in[i] + 1`
///
/// The push order is the IR's, and it is not the order the expression reads in:
/// a `BufferWriteCall` takes `[out_pos, idx, val]` with the **value on top**, so
/// the two cheap operands go on the stack first and the value is computed last.
fn adds() -> KernelFragment {
    fragment(vec![
        KernelInstr::Const(0),    // out_pos
        KernelInstr::LocalGet(1), // idx
        KernelInstr::Const(0),
        KernelInstr::LocalGet(1),
        KernelInstr::BufferReadCall, // in[i]
        KernelInstr::Const(0),
        KernelInstr::LocalGet(1),
        KernelInstr::BufferReadCall, // in[i]
        KernelInstr::Bin(KernelBin::Add),
        KernelInstr::Const(1),
        KernelInstr::Bin(KernelBin::Add),
        KernelInstr::BufferWriteCall,
        KernelInstr::Const(0),
    ])
}

/// `out[i] = if in[i] <= 3 then 7 else in[i]`
///
/// This is the fragment that exercises the one instruction the two backends
/// disagree about: the comparison produces a `bool` on this target, so
/// `I32WrapI64` has nothing to do, while the wasm backend needs it to narrow an
/// `i64` condition to the `i32` its `select` takes.
///
/// `Select` takes `[then, else, condition]` with the condition on top, so the
/// branch values are pushed before the condition is even computed.
fn conditional() -> KernelFragment {
    let read = |body: &mut Vec<KernelInstr>| {
        body.push(KernelInstr::Const(0));
        body.push(KernelInstr::LocalGet(1));
        body.push(KernelInstr::BufferReadCall);
    };
    let mut body = vec![KernelInstr::Const(0), KernelInstr::LocalGet(1)];
    body.push(KernelInstr::Const(7)); // then
    read(&mut body); // else
    read(&mut body); // the condition's operand
    body.push(KernelInstr::Const(3));
    body.push(KernelInstr::Bin(KernelBin::Leq));
    body.push(KernelInstr::I32WrapI64); // a no-op on this target
    body.push(KernelInstr::Select);
    body.push(KernelInstr::BufferWriteCall);
    body.push(KernelInstr::Const(0));
    fragment(body)
}

/// An independent reading of the IR, written from the `KernelInstr` docs rather
/// than from the shader, used as the second opinion on the GPU's answer.
fn reference(fragment: &KernelFragment, input: &[i64], count: usize) -> Vec<i64> {
    let index = fragment.param_shape.flat_arity() - 1;
    let mut output = vec![0i64; count];
    for element in 0..count {
        let mut stack: Vec<i64> = Vec::new();
        for instruction in &fragment.body {
            match instruction {
                KernelInstr::Const(value) => stack.push(*value),
                KernelInstr::LocalGet(local) => {
                    let value = if *local as usize == index {
                        element as i64
                    } else {
                        input[*local as usize]
                    };
                    stack.push(value);
                }
                // A comparison yields 1 or 0 here; a `select` only tests it.
                KernelInstr::Bin(KernelBin::Add) => {
                    let rhs = stack.pop().unwrap();
                    let lhs = stack.pop().unwrap();
                    stack.push(lhs + rhs);
                }
                KernelInstr::Bin(KernelBin::Sub) => {
                    let rhs = stack.pop().unwrap();
                    let lhs = stack.pop().unwrap();
                    stack.push(lhs - rhs);
                }
                KernelInstr::Bin(KernelBin::Leq) => {
                    let rhs = stack.pop().unwrap();
                    let lhs = stack.pop().unwrap();
                    stack.push(i64::from(lhs <= rhs));
                }
                KernelInstr::Bin(KernelBin::Eq) => {
                    let rhs = stack.pop().unwrap();
                    let lhs = stack.pop().unwrap();
                    stack.push(i64::from(lhs == rhs));
                }
                KernelInstr::I32WrapI64 => {}
                KernelInstr::Select => {
                    let condition = stack.pop().unwrap();
                    let otherwise = stack.pop().unwrap();
                    let then = stack.pop().unwrap();
                    stack.push(if condition != 0 { then } else { otherwise });
                }
                KernelInstr::BufferReadCall => {
                    let element_index = stack.pop().unwrap() as usize;
                    let position = stack.pop().unwrap() as usize;
                    let value = if position == 0 {
                        input[element_index]
                    } else {
                        output[element_index]
                    };
                    stack.push(value);
                }
                KernelInstr::BufferWriteCall => {
                    let value = stack.pop().unwrap();
                    let element_index = stack.pop().unwrap() as usize;
                    let position = stack.pop().unwrap() as usize;
                    if position == 0 {
                        output[element_index] = value;
                    }
                }
                KernelInstr::CallKernel(_) => panic!("the reference emits no cross-kernel calls"),
            }
        }
    }
    output
}

/// One run, checked three ways.  Returns the device name so a passing test says
/// which GPU it actually ran on.
fn check(fragment: &KernelFragment, input: &[i64], expected: &[i64]) -> String {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = input.len();
    let from_gpu = context
        .run(fragment, &[input.to_vec()], count)
        .expect("the dispatch completes");
    assert_eq!(
        from_gpu.len(),
        fragment.outputs,
        "one buffer per declared output"
    );
    let from_gpu = &from_gpu[0];
    assert_eq!(from_gpu.len(), count, "the result is exactly `count` long");

    assert_eq!(
        from_gpu,
        expected,
        "the GPU disagrees with the hand-written expectation at index {:?}",
        (0..count).find(|i| from_gpu[*i] != expected[*i])
    );
    assert_eq!(
        from_gpu,
        &reference(fragment, input, count),
        "the GPU disagrees with the CPU reference"
    );
    context.device_name().to_string()
}

#[test]
fn an_arithmetic_map_matches_the_cpu_bit_for_bit() {
    // 100 is not a multiple of 64, so the last workgroup overruns `count` and
    // the padding is what the surplus lanes touch.
    let input: Vec<i64> = (0..100).map(|value| value - 40).collect();
    let expected: Vec<i64> = input.iter().map(|value| value + value + 1).collect();
    let device = check(&adds(), &input, &expected);
    println!("ran on {device}");
}

#[test]
fn a_conditional_write_matches_the_cpu_bit_for_bit() {
    let input: Vec<i64> = (0..100).collect();
    let expected: Vec<i64> = input
        .iter()
        .map(|value| if *value <= 3 { 7 } else { *value })
        .collect();
    let device = check(&conditional(), &input, &expected);
    println!("ran on {device}");
}

/// A count that is an exact multiple of the workgroup, so the padding path is
/// *not* exercised — the complement of the tests above, which pin both ends.
#[test]
fn an_exact_workgroup_multiple_matches_too() {
    let input: Vec<i64> = (0..(LOCAL_SIZE_X as usize * 2))
        .map(|value| value as i64)
        .collect();
    let expected: Vec<i64> = input.iter().map(|value| value + value + 1).collect();
    let device = check(&adds(), &input, &expected);
    println!("ran on {device}");
}
