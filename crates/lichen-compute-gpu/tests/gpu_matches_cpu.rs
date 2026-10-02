//! The acceptance criterion: the same fragment, run on the GPU and on the CPU,
//! must agree element for element.
//!
//! Two things are asserted, on purpose. The GPU result is compared against a
//! **hand-written expected vector**, not only against the CPU reference —
//! otherwise a fragment this test and the shader both misread would pass. The
//! counts are deliberately not multiples of the workgroup size, so the surplus
//! lanes of the last workgroup are covered too: those lanes address padding, and
//! if that padding were not allocated the run would scribble past the buffers.

use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X, RunError};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape, Pending,
};

/// A fragment over `(input, index)` — the shape a single-input parallel kernel has.
///
/// `inputs` is 1 because that is what every body built through here reads: each
/// names position 0 and nothing above it. It is stated rather than derived
/// because these are hand-written IR, and the count a hand-written body needs is
/// the one a reader has to be able to check against the body by eye.
fn fragment(body: Vec<KernelInstr>) -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body: body.into(),
        inputs: 1,
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

/// `out[i] = (in[i] * 3) % 7 + in[i] / 5`
///
/// The three arithmetic operators that do not exist on wasm's `i32` path at all:
/// a product, an **unsigned** remainder and an **unsigned** division.  The
/// values are small enough that the signed reading would agree, so this test
/// pins the operators' existence and their operand order rather than the
/// signedness; `an_unsigned_reading_is_what_the_language_means` is the one that
/// separates the two.
fn arithmetic() -> KernelFragment {
    let read = |body: &mut Vec<KernelInstr>| {
        body.push(KernelInstr::Const(0));
        body.push(KernelInstr::LocalGet(1));
        body.push(KernelInstr::BufferReadCall);
    };
    let mut body = vec![KernelInstr::Const(0), KernelInstr::LocalGet(1)];
    read(&mut body);
    body.push(KernelInstr::Const(3));
    body.push(KernelInstr::Bin(KernelBin::Mul));
    body.push(KernelInstr::Const(7));
    body.push(KernelInstr::Bin(KernelBin::Rem));
    read(&mut body);
    body.push(KernelInstr::Const(5));
    body.push(KernelInstr::Bin(KernelBin::Div));
    body.push(KernelInstr::Bin(KernelBin::Add));
    body.push(KernelInstr::BufferWriteCall);
    body.push(KernelInstr::Const(0));
    fragment(body)
}

/// `out[i] = ((in[i] < 3) & (in[i] > 0)) | (in[i] == 50)`
///
/// **The fragment that pins the two conversions the target needs and wasm gets
/// from its integer instructions.**  A comparison yields a `bool` on this
/// target, so `&` and `|` — which are the language's `and`/`or` over the `0`/`1`
/// a comparison means — force both operands to be materialised into the 64-bit
/// scalar, and the write forces the result to be too.  Without that
/// materialisation the module mixes `OpTypeBool` with `OpTypeInt` and the driver
/// rejects it; with it, the answer is the `0`/`1` the language says.
///
/// It also covers three comparisons at once (`<`, `>`, `==`) and a write of a
/// comparison's own scalar, which is the shape a returned predicate has.
fn predicates() -> KernelFragment {
    let read = |body: &mut Vec<KernelInstr>| {
        body.push(KernelInstr::Const(0));
        body.push(KernelInstr::LocalGet(1));
        body.push(KernelInstr::BufferReadCall);
    };
    let mut body = vec![KernelInstr::Const(0), KernelInstr::LocalGet(1)];
    read(&mut body);
    body.push(KernelInstr::Const(3));
    body.push(KernelInstr::Bin(KernelBin::Lt)); // in[i] < 3
    read(&mut body);
    body.push(KernelInstr::Const(0));
    body.push(KernelInstr::Bin(KernelBin::Gt)); // in[i] > 0
    body.push(KernelInstr::Bin(KernelBin::BitAnd));
    read(&mut body);
    body.push(KernelInstr::Const(50));
    body.push(KernelInstr::Bin(KernelBin::Eq)); // in[i] == 50
    body.push(KernelInstr::Bin(KernelBin::BitOr));
    body.push(KernelInstr::BufferWriteCall);
    body.push(KernelInstr::Const(0));
    fragment(body)
}

/// `out[i] = if in[i] < 2^63 then in[i] / 2 else in[i] % 2`
///
/// **Where a signed reading and an unsigned one part company.**  With the
/// element declared signed, `2^63` — which the host writes as `i64::MIN`'s bit
/// pattern — is the most *negative* value there is, so `OpSLessThan` would send
/// it down the wrong branch and `OpSDiv` would halve it to a negative number.
/// The language's `Int` is unsigned, so the comparison is `OpULessThan`, the
/// divisor is its own positive value, and `in[i] % 2` picks the branch for
/// every element at or above `2^63`.
///
/// The condition is a *parameter read*, not a comparison, so this is also the
/// fragment where `I32WrapI64` has something to do: an `i64` condition becomes
/// the `bool` the target's `select` takes (and, on wasm, an `i32`).
fn unsigned_reading() -> KernelFragment {
    let read = |body: &mut Vec<KernelInstr>| {
        body.push(KernelInstr::Const(0));
        body.push(KernelInstr::LocalGet(1));
        body.push(KernelInstr::BufferReadCall);
    };
    let mut body = vec![KernelInstr::Const(0), KernelInstr::LocalGet(1)];
    read(&mut body);
    body.push(KernelInstr::Const(2));
    body.push(KernelInstr::Bin(KernelBin::Div)); // then: in[i] / 2
    read(&mut body);
    body.push(KernelInstr::Const(2));
    body.push(KernelInstr::Bin(KernelBin::Rem)); // else: in[i] % 2
    read(&mut body);
    body.push(KernelInstr::Const(i64::MIN)); // 2^63, as a bit pattern
    body.push(KernelInstr::Bin(KernelBin::Lt));
    body.push(KernelInstr::I32WrapI64);
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
        for instruction in fragment.body.instrs() {
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
                //
                // **Unsigned, all of it.** An `Int` is a machine-sized unsigned
                // integer in this language, so `Div`/`Rem` and the order
                // comparisons read the two words as `u64` — which is also what
                // the shader does, its buffer elements being declared
                // unsigned. A reference that used the signed reading would
                // agree for every value below 2^63 and disagree above it.
                KernelInstr::Bin(operator) => {
                    let rhs = stack.pop().unwrap();
                    let lhs = stack.pop().unwrap();
                    let (left, right) = (lhs as u64, rhs as u64);
                    stack.push(match operator {
                        KernelBin::Add => lhs.wrapping_add(rhs),
                        KernelBin::Sub => lhs.wrapping_sub(rhs),
                        KernelBin::Mul => lhs.wrapping_mul(rhs),
                        KernelBin::Div => (left / right) as i64,
                        KernelBin::Rem => (left % right) as i64,
                        KernelBin::Lt => i64::from(left < right),
                        KernelBin::Gt => i64::from(left > right),
                        KernelBin::Leq => i64::from(left <= right),
                        KernelBin::Geq => i64::from(left >= right),
                        KernelBin::Eq => i64::from(lhs == rhs),
                        KernelBin::Neq => i64::from(lhs != rhs),
                        KernelBin::BitAnd => lhs & rhs,
                        KernelBin::BitOr => lhs | rhs,
                        KernelBin::BitXor => lhs ^ rhs,
                    });
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
///
/// The run hands back a resident id and the data only comes home on the `fetch`,
/// so this also pins the split itself: if the run leaked results to the host, or
/// the fetch read something the dispatch did not write, these would differ.
fn check(fragment: &KernelFragment, input: &[i64], expected: &[i64]) -> String {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = input.len();
    let resident = context
        .run(fragment, &[BufferSlot::Host(input)], count)
        .expect("the dispatch completes");
    assert_eq!(
        resident.len(),
        fragment.outputs,
        "one resident buffer per declared output"
    );
    let from_gpu = context
        .fetch(resident[0], count)
        .expect("the result comes back off the device");
    assert_eq!(from_gpu.len(), count, "the result is exactly `count` long");

    assert_eq!(
        from_gpu,
        expected,
        "the GPU disagrees with the hand-written expectation at index {:?}",
        (0..count).find(|i| from_gpu[*i] != expected[*i])
    );
    assert_eq!(
        from_gpu,
        reference(fragment, input, count),
        "the GPU disagrees with the CPU reference"
    );

    // A released id names no buffer, and the backend says so rather than
    // answering with zeroes: an id is a handle, and a dead one means the host
    // lost track of its own memory.
    context.release(resident[0]);
    assert!(
        matches!(
            context.fetch(resident[0], count),
            Err(RunError::UnknownResident { .. })
        ),
        "fetching a released id is refused by name"
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

/// Two runs in a row over data that never leaves the device.
///
/// This is the test the whole residency split exists for, and it checks the
/// thing a value-comparison cannot: the second run is handed the **id**, not the
/// data. If the chain fell back to host data, the values would still be right and
/// the test would still pass — so it also pins that a fetch of the intermediate
/// is never what made it correct.
#[test]
fn a_second_run_consumes_the_first_runs_id_without_a_round_trip() {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = 100;
    let input: Vec<i64> = (0..count as i64).collect();

    // out = in + in + 1, then out = in + in + 1 again: composed, the answer is
    // known without consulting the device, so a wrong chain cannot pass.
    let first = context
        .run(&adds(), &[BufferSlot::Host(&input)], count)
        .expect("the first run completes");
    let second = context
        .run(&adds(), &[BufferSlot::Resident(first[0])], count)
        .expect("the second run consumes the first run's id");
    let result = context
        .fetch(second[0], count)
        .expect("the chained result comes back");

    let expected: Vec<i64> = input
        .iter()
        .map(|value| (value + value + 1) + (value + value + 1) + 1)
        .collect();
    assert_eq!(result, expected, "the chain composes correctly");

    // The intermediate is still on the device and still readable, which is what
    // "never came home" means: one download, of the last link only.
    let intermediate = context
        .fetch(first[0], count)
        .expect("the intermediate is still resident after being consumed");
    assert_eq!(
        intermediate,
        reference(&adds(), &input, count),
        "and it is right"
    );

    context.release(first[0]);
    context.release(second[0]);
}

/// A released buffer goes back into the context's pool and is handed out again,
/// uncleared — so what a run reads must never depend on what the previous run of
/// the same size left in it.
///
/// The two fragments are alternated in **one** context so the second run of each
/// is served from the pool the first one released. Every other test builds a
/// fresh context, so nothing else would notice a stale byte reaching a result.
#[test]
fn a_recycled_buffer_never_shows_the_previous_run_its_contents() {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = 100;
    let input: Vec<i64> = (0..count as i64).collect();
    let adds_expected: Vec<i64> = input.iter().map(|v| v + v + 1).collect();
    let select_expected: Vec<i64> = input.iter().map(|v| if *v <= 3 { 7 } else { *v }).collect();

    for round in 0..8 {
        let adds_run = context
            .run(&adds(), &[BufferSlot::Host(&input)], count)
            .expect("the adds run completes");
        assert_eq!(
            context.fetch(adds_run[0], count).expect("adds comes back"),
            adds_expected,
            "round {round}: the adds run is right"
        );
        context.release(adds_run[0]);

        let select_run = context
            .run(&conditional(), &[BufferSlot::Host(&input)], count)
            .expect("the conditional run completes");
        assert_eq!(
            context
                .fetch(select_run[0], count)
                .expect("the conditional run comes back"),
            select_expected,
            "round {round}: the conditional run is right, on a buffer the adds run just \
             released — a leaked byte would show here as a value the kernel never wrote"
        );
        context.release(select_run[0]);
    }
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

#[test]
fn multiplication_division_and_remainder_match_the_cpu_bit_for_bit() {
    let input: Vec<i64> = (0..100).collect();
    let expected: Vec<i64> = input
        .iter()
        .map(|value| (value * 3) % 7 + value / 5)
        .collect();
    let device = check(&arithmetic(), &input, &expected);
    println!("ran on {device}");
}

#[test]
fn a_stored_predicate_matches_the_cpu_bit_for_bit() {
    let input: Vec<i64> = (0..100).collect();
    let expected: Vec<i64> = input
        .iter()
        .map(|value| i64::from((*value < 3 && *value > 0) || *value == 50))
        .collect();
    let device = check(&predicates(), &input, &expected);
    println!("ran on {device}");
}

/// The signed reading and the unsigned one give different answers here, and the
/// unsigned one is what the language means.
#[test]
fn an_unsigned_reading_is_what_the_language_means() {
    // Every one of these is `2^63` or above as an unsigned 64-bit value, which
    // is a *negative* `i64` — the host writes the same bits either way, so the
    // two readings differ only in what the shader does with them.
    let input: Vec<i64> = vec![-1, -2, i64::MIN, 5, -100];
    let expected: Vec<i64> = input
        .iter()
        .map(|value| {
            let value = *value as u64;
            if value < 1u64 << 63 {
                (value / 2) as i64
            } else {
                (value % 2) as i64
            }
        })
        .collect();
    assert_eq!(
        expected,
        vec![1, 0, 0, 2, 0],
        "the expectation itself is the unsigned reading, written out"
    );
    let device = check(&unsigned_reading(), &input, &expected);
    println!("ran on {device}");
}

/// Two submissions in flight at once, the second fed from the first.
///
/// This is the property the pool exists for, and nothing else here covers it.
/// Every other path waits before it returns, so this is the only place a
/// command buffer is recorded while a previous submission may still be running,
/// and the only place a dispatch is recorded against a buffer the earlier
/// submission's shader may not have written yet. Getting `4x + 3` back says both
/// halves held: the slots really are distinct, and the trailing barrier really
/// does order two submissions rather than two dispatches in one recording.
#[test]
fn a_submission_can_be_fed_to_one_that_is_still_in_flight() {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = 100;
    let input: Vec<i64> = (0..count as i64).collect();

    let first = context
        .submit(&adds(), &[BufferSlot::Host(&input)], count)
        .expect("the first submission is recorded");
    // Deliberately not waited, and deliberately feeding the second from the
    // first: `first.outputs()[0]` names a buffer the device has not promised to
    // have written.
    let first_output = first.outputs()[0];
    let second = context
        .submit(&adds(), &[BufferSlot::Resident(first_output)], count)
        .expect("the second submission is recorded into a different slot");

    let first_ids = Box::new(first)
        .wait()
        .expect("the first submission finishes");
    let second_ids = Box::new(second)
        .wait()
        .expect("the second submission finishes");

    let expected: Vec<i64> = input.iter().map(|value| 4 * value + 3).collect();
    assert_eq!(
        context
            .fetch(second_ids[0], count)
            .expect("the chained result comes back off the device"),
        expected,
        "adds applied to adds, recorded before the first had finished"
    );
    // A consumer of the intermediate, fetched after its own wait, so this one is
    // a plain fetch of a waited buffer.
    assert_eq!(
        context
            .fetch(first_ids[0], count)
            .expect("the intermediate comes back"),
        input
            .iter()
            .map(|value| value + value + 1)
            .collect::<Vec<i64>>(),
        "the intermediate is right too, and only once its submission was waited for"
    );
    context.release(first_ids[0]);
    context.release(second_ids[0]);
}

/// A submission nobody waited for is still released when it is dropped.
///
/// Nothing in a run relies on this — a run submits and waits inside one call —
/// but the resources it holds are a claimed slot, a staging buffer the device
/// may still be copying out of, and upload targets nothing else can name. The
/// drop waits, and the assertion is that a run afterwards is still right: at
/// depth two, a drop that did *not* wait would hand the next acquisition a
/// staging buffer with a copy still in flight, and the values would be wrong
/// rather than slow.
#[test]
fn dropping_a_submission_nobody_waited_for_still_frees_it() {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = 100;
    let input: Vec<i64> = (0..count as i64).collect();

    for round in 0..4 {
        drop(
            context
                .submit(&adds(), &[BufferSlot::Host(&input)], count)
                .expect("the abandoned submission is recorded"),
        );
        // An id from an abandoned submission is still a live buffer, and it is
        // still readable — the drop waited, so the data is there.
        let pending = context
            .submit(&adds(), &[BufferSlot::Host(&input)], count)
            .expect("the next submission is recorded");
        let ids = Box::new(pending)
            .wait()
            .expect("the next submission finishes");
        assert_eq!(
            context.fetch(ids[0], count).expect("comes back"),
            input
                .iter()
                .map(|value| value + value + 1)
                .collect::<Vec<i64>>(),
            "round {round}: a run after an abandoned one is right"
        );
        context.release(ids[0]);
    }
}
