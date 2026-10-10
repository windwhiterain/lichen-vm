//! The acceptance criterion: the same fragment on the GPU and on the CPU must agree
//! element for element.
//!
//! # Invariant
//! Two things are asserted on purpose: the GPU result is compared against a hand-written expected
//! vector as well as the CPU reference, so a fragment both this test and the shader misread cannot
//! pass; and the counts are not multiples of the workgroup size, so the surplus lanes of the last
//! workgroup are covered. The reference here is this file's own third reading of the IR.

mod common;

use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X, RunError};
use std::collections::HashMap;

use lichen_kernel_ir::{
    BufferSlot, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaunchSet, Pending, ScalarClass, ScalarData, ScalarLeaf,
};

/// A one-fragment launch set: every kernel here stands alone, and a set's
/// degenerate form is the shape that says so.
fn only(fragment: &KernelFragment) -> LaunchSet<'_> {
    LaunchSet::single(fragment)
}

/// The packed bytes of `words`, one `i64` each.
fn pack(words: &[i64]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// The `i64` words of a fetched payload, for a fragment whose class is `Int`.
fn words(data: ScalarData) -> Vec<i64> {
    match data {
        ScalarData::Int(elements) => elements,
        ScalarData::Float(elements) => {
            panic!(
                "an integer run's result came back as {} float element(s)",
                elements.len()
            )
        }
    }
}

/// A fragment over `(input, index)`: the shape a single-input parallel kernel has.
fn fragment(body: Vec<FlatOp>) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(2, &body),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// `out[i] = in[i] + in[i] + 1`
///
/// # Invariant
/// The push order is the IR's, not the expression's: a `BufferWriteCall` takes
/// `[out_pos, idx, val]` with the value on top.
fn adds() -> KernelFragment {
    fragment(vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
        FlatOp::Read(1),                                        // idx
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        FlatOp::Read(1),
        FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        FlatOp::Read(1),
        FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
        FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
        FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
        FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
    ])
}

/// `out[i] = if in[i] <= 3 then 7 else in[i]`
///
/// # Invariant
/// This exercises the one instruction the two backends disagree about: the comparison produces a
/// `bool` here, so `I32WrapI64` has nothing to do, while wasm needs it to narrow an `i64`
/// condition. `Select` takes `[then, else, condition]`.
fn conditional() -> KernelFragment {
    let read = |body: &mut Vec<FlatOp>| {
        body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
        body.push(FlatOp::Read(1));
        body.push(FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)));
    };
    let mut body = vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        FlatOp::Read(1),
    ];
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 7))); // then
    read(&mut body); // else
    read(&mut body); // the condition's operand
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 3)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Leq,
    )));
    body.push(FlatOp::Instr(KernelInstr::I32WrapI64)); // a no-op on this target
    body.push(FlatOp::Instr(KernelInstr::Select));
    body.push(FlatOp::Instr(KernelInstr::BufferWriteCall(
        ScalarClass::Int,
    )));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    fragment(body)
}

/// `out[i] = (in[i] * 3) % 7 + in[i] / 5`
///
/// # Invariant
/// The three operators wasm's `i32` path has no form of: a product, an unsigned remainder and an
/// unsigned division. The values are small enough that the signed reading agrees, so this pins
/// their existence and operand order.
fn arithmetic() -> KernelFragment {
    let read = |body: &mut Vec<FlatOp>| {
        body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
        body.push(FlatOp::Read(1));
        body.push(FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)));
    };
    let mut body = vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        FlatOp::Read(1),
    ];
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 3)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Mul,
    )));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 7)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Rem,
    )));
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 5)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Div,
    )));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Add,
    )));
    body.push(FlatOp::Instr(KernelInstr::BufferWriteCall(
        ScalarClass::Int,
    )));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    fragment(body)
}

/// `out[i] = ((in[i] < 3) & (in[i] > 0)) | (in[i] == 50)`
///
/// # Invariant
/// The fragment that pins the two conversions this target needs and wasm gets from its integer
/// instructions: a comparison yields a `bool`, so `&` and `|` force both operands to be
/// materialised into the 64-bit scalar and the write forces the result too. Without it the module
/// mixes `OpTypeBool` with `OpTypeInt` and the driver rejects it.
fn predicates() -> KernelFragment {
    let read = |body: &mut Vec<FlatOp>| {
        body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
        body.push(FlatOp::Read(1));
        body.push(FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)));
    };
    let mut body = vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        FlatOp::Read(1),
    ];
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 3)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Lt,
    ))); // in[i] < 3
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Gt,
    ))); // in[i] > 0
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::BitAnd,
    )));
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 50)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Eq,
    ))); // in[i] == 50
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::BitOr,
    )));
    body.push(FlatOp::Instr(KernelInstr::BufferWriteCall(
        ScalarClass::Int,
    )));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    fragment(body)
}

/// `out[i] = if in[i] < 2^63 then in[i] / 2 else in[i] % 2`
///
/// # Invariant
/// Where a signed reading and an unsigned one part company: with the element declared signed,
/// `2^63` is the most negative value there is, so `OpSLessThan` would take the wrong branch. The
/// condition is a parameter read rather than a comparison, so this is also where `I32WrapI64` has
/// something to do.
fn unsigned_reading() -> KernelFragment {
    let read = |body: &mut Vec<FlatOp>| {
        body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
        body.push(FlatOp::Read(1));
        body.push(FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)));
    };
    let mut body = vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        FlatOp::Read(1),
    ];
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 2)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Div,
    ))); // then: in[i] / 2
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 2)));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Rem,
    ))); // else: in[i] % 2
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(
        ScalarClass::Int,
        i64::MIN,
    ))); // 2^63, as a bit pattern
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Lt,
    )));
    body.push(FlatOp::Instr(KernelInstr::I32WrapI64));
    body.push(FlatOp::Instr(KernelInstr::Select));
    body.push(FlatOp::Instr(KernelInstr::BufferWriteCall(
        ScalarClass::Int,
    )));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    fragment(body)
}

/// One value on the reference's stack.
///
/// # Invariant
/// There is no separate boolean: a comparison yields its class's `1`/`0`.
#[derive(Clone, Copy)]
enum Scalar {
    /// A constant the body pushed.
    ///
    /// # Invariant
    /// The IR's `Const` is an `i64` and says nothing about its class, so it is read the way the
    /// position demands: the float whose bits the payload holds in a value position, the integer it
    /// spells in an index or a buffer position.
    Literal(i64),
    Int(i64),
    Float(f32),
}

impl Scalar {
    /// This value as `class`'s scalar, reading a literal.
    fn as_class(self, class: ScalarClass) -> Scalar {
        match (self, class) {
            (Scalar::Literal(value), ScalarClass::Int) => Scalar::Int(value),
            (Scalar::Literal(value), ScalarClass::Float) => {
                Scalar::Float(f32::from_bits(value as u32))
            }
            (value, _) => value,
        }
    }

    /// The element index a buffer operation takes: an integer, its payload as a number.
    fn as_index(self) -> usize {
        match self {
            Scalar::Literal(value) | Scalar::Int(value) => value as usize,
            Scalar::Float(_) => panic!("the reference takes an index from an integer, not a float"),
        }
    }

    /// The word this value is stored as, which is what a buffer holds.
    fn bits(self) -> i64 {
        match self {
            Scalar::Literal(value) | Scalar::Int(value) => value,
            Scalar::Float(value) => i64::from(value.to_bits()),
        }
    }

    /// Whether this value is a true condition: the bit pattern, not the value.
    fn is_true(self, class: ScalarClass) -> bool {
        match (self, class) {
            (Scalar::Literal(value), ScalarClass::Float) => (value as u32) != 0,
            (Scalar::Literal(value), ScalarClass::Int) => value != 0,
            (Scalar::Int(value), _) => value != 0,
            (Scalar::Float(value), _) => value.to_bits() != 0,
        }
    }
}

/// The class's `1`/`0` for a comparison's answer.
fn boolean(class: ScalarClass, holds: bool) -> Scalar {
    match class {
        ScalarClass::Int => Scalar::Int(i64::from(holds)),
        ScalarClass::Float => Scalar::Float(if holds { 1.0 } else { 0.0 }),
    }
}

/// An independent reading of the IR, hand-written from the `KernelInstr` docs.
///
/// # Invariant
/// A third reading — neither the wasm backend nor this crate's emitter — so a float compared against
/// it proves the shader agrees with this file and nothing about `lichen-compute`. The reading is per
/// class, read from the emitter rather than re-derived, and follows the language's rules: `==`/`!=`
/// compare `to_bits`, a condition is the bit pattern, and a comparison answers `1`/`0`.
fn reference(fragment: &KernelFragment, input: &[i64], count: usize) -> Vec<i64> {
    let class = lichen_compute_gpu::spirv::module_class(fragment)
        .expect("the reference reads the class the fragment declares");
    let index = fragment.param_shape.flat_arity() - 1;
    let mut output = vec![0i64; count];
    for element in 0..count {
        // The map that replaces the operand stack: every value the body defines is here
        // once its definition has run.
        let mut values: HashMap<lichen_kernel_ir::ValueId, Scalar> = HashMap::new();
        let entry = &fragment.body.blocks[fragment.body.entry];
        // The entry block's parameters are the ABI's leaves: the caller's input, then
        // the index.
        for (offset, parameter) in entry.params.iter().enumerate() {
            let read = if offset == index {
                element as i64
            } else {
                input[offset]
            };
            values.insert(*parameter, Scalar::Int(read));
        }
        for &definition in &entry.instrs {
            let Some(lichen_kernel_ir::ValueDef::Instr { op, args, .. }) =
                fragment.body.values.get(definition.0 as usize)
            else {
                continue;
            };
            let operand = |at: usize| -> Scalar { values[&args[at]] };
            let mut produced = Scalar::Int(0);
            let mut leaves_nothing = false;
            match *op {
                KernelInstr::Const(ScalarClass::Int, value) => produced = Scalar::Literal(value),
                // A float local takes an `f32`'s bits, which is the width the
                // payload holds in a value position (`Scalar::Float`).
                KernelInstr::Const(ScalarClass::Float, value) => {
                    produced = Scalar::Float(f32::from_bits(value as u32));
                }
                // A comparison yields 1 or 0 here, or 1.0 and 0.0 over floats; a
                // `select` only tests it.
                KernelInstr::Bin(_, operator) => {
                    let rhs = operand(1).as_class(class);
                    let lhs = operand(0).as_class(class);
                    produced = match (lhs, rhs) {
                        // Unsigned, all of it: `Div`/`Rem` and the order comparisons
                        // read the words as `u64`, as the shader does.
                        (Scalar::Int(lhs), Scalar::Int(rhs)) => {
                            let (left, right) = (lhs as u64, rhs as u64);
                            Scalar::Int(match operator {
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
                            })
                        }
                        // The language's relations, not IEEE's: `==`/`!=` compare
                        // `to_bits`, so `NaN == NaN` is `1`.
                        (Scalar::Float(lhs), Scalar::Float(rhs)) => match operator {
                            KernelBin::Add => Scalar::Float(lhs + rhs),
                            KernelBin::Sub => Scalar::Float(lhs - rhs),
                            KernelBin::Mul => Scalar::Float(lhs * rhs),
                            // IEEE here, undefined in SPIR-V: the language does not
                            // specify a kernel's float division.
                            KernelBin::Div => Scalar::Float(lhs / rhs),
                            KernelBin::Eq => boolean(class, lhs.to_bits() == rhs.to_bits()),
                            KernelBin::Neq => boolean(class, lhs.to_bits() != rhs.to_bits()),
                            KernelBin::Lt => boolean(class, lhs < rhs),
                            KernelBin::Gt => boolean(class, lhs > rhs),
                            KernelBin::Leq => boolean(class, lhs <= rhs),
                            KernelBin::Geq => boolean(class, lhs >= rhs),
                            KernelBin::Rem
                            | KernelBin::BitAnd
                            | KernelBin::BitOr
                            | KernelBin::BitXor => {
                                panic!("the language has no such operator over floats")
                            }
                        },
                        _ => panic!("the reference does not mix classes in one operation"),
                    };
                }
                // A no-op on this target, but not on the value: the walk still has to
                // record what it produced.
                KernelInstr::I32WrapI64 => produced = operand(0),
                // The language's class conversion: the number, not the bits.
                KernelInstr::Conv { from, to } => {
                    let seen = operand(0).as_class(from);
                    produced = match (seen, to) {
                        (Scalar::Int(value), ScalarClass::Float) => Scalar::Float(value as f32),
                        (Scalar::Float(value), ScalarClass::Int) => {
                            Scalar::Int(value.trunc() as i64)
                        }
                        (seen, _) => seen,
                    };
                }
                KernelInstr::Select => {
                    let condition = operand(2);
                    let otherwise = operand(1).as_class(class);
                    let then = operand(0).as_class(class);
                    produced = if condition.is_true(class) {
                        then
                    } else {
                        otherwise
                    };
                }
                KernelInstr::BufferReadCall(_) => {
                    let element_index = operand(1).as_index();
                    let position = operand(0).as_index();
                    let value = if position == 0 {
                        input[element_index]
                    } else {
                        output[element_index]
                    };
                    produced = match class {
                        ScalarClass::Int => Scalar::Int(value),
                        // This reading's own word layout, not the ABI's: a float
                        // element in an `i64` word is its low 32 bits.
                        ScalarClass::Float => Scalar::Float(f32::from_bits(value as u32)),
                    };
                }
                KernelInstr::BufferWriteCall(_) => {
                    let value = operand(2).as_class(class);
                    let element_index = operand(1).as_index();
                    let position = operand(0).as_index();
                    if position == 0 {
                        output[element_index] = value.bits();
                    }
                    leaves_nothing = true;
                }
                KernelInstr::CallKernel(_) => {
                    panic!("the reference emits no cross-kernel calls")
                }
            }
            if !leaves_nothing {
                values.insert(definition, produced);
            }
        }
    }
    output
}

/// One run, checked three ways; returns the device name it ran on.
///
/// # Invariant
/// The run hands back a resident id and the data comes home only on the `fetch`, so this pins the
/// split: a run that leaked results to the host, or a fetch that read something the dispatch did not
/// write, would differ. The context is the caller's, since the caller can name itself when there is
/// no device.
fn check(
    context: &GpuContext,
    fragment: &KernelFragment,
    input: &[i64],
    expected: &[i64],
) -> String {
    let count = input.len();
    let packed = pack(input);
    let resident = context
        .run(&only(fragment), &[BufferSlot::Host(&packed)], count)
        .expect("the dispatch completes");
    assert_eq!(
        resident.len(),
        fragment.outputs,
        "one resident buffer per declared output"
    );
    let from_gpu = words(
        context
            .fetch(resident[0], count)
            .expect("the result comes back off the device"),
    );
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
    // answering with zeroes.
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
    let Some(context) = common::context("an_arithmetic_map_matches_the_cpu_bit_for_bit") else {
        return;
    };
    let input: Vec<i64> = (0..100).map(|value| value - 40).collect();
    let expected: Vec<i64> = input.iter().map(|value| value + value + 1).collect();
    let device = check(&context, &adds(), &input, &expected);
    println!("ran on {device}");
}

#[test]
fn a_conditional_write_matches_the_cpu_bit_for_bit() {
    let Some(context) = common::context("a_conditional_write_matches_the_cpu_bit_for_bit") else {
        return;
    };
    let input: Vec<i64> = (0..100).collect();
    let expected: Vec<i64> = input
        .iter()
        .map(|value| if *value <= 3 { 7 } else { *value })
        .collect();
    let device = check(&context, &conditional(), &input, &expected);
    println!("ran on {device}");
}

/// Two runs in a row over data that never leaves the device.
///
/// # Invariant
/// The second run is handed the id, not the data: if the chain fell back to host data the values
/// would still be right, so the test also pins that a fetch of the intermediate is never what made
/// it correct.
#[test]
fn a_second_run_consumes_the_first_runs_id_without_a_round_trip() {
    let Some(context) =
        common::context("a_second_run_consumes_the_first_runs_id_without_a_round_trip")
    else {
        return;
    };
    let count = 100;
    let input: Vec<i64> = (0..count as i64).collect();

    // out = in + in + 1 twice: composed, the answer is known without the device.
    let first = context
        .run(&only(&adds()), &[BufferSlot::Host(&pack(&input))], count)
        .expect("the first run completes");
    let second = context
        .run(&only(&adds()), &[BufferSlot::Resident(first[0])], count)
        .expect("the second run consumes the first run's id");
    let result = words(
        context
            .fetch(second[0], count)
            .expect("the chained result comes back"),
    );

    let expected: Vec<i64> = input
        .iter()
        .map(|value| (value + value + 1) + (value + value + 1) + 1)
        .collect();
    assert_eq!(result, expected, "the chain composes correctly");

    // The intermediate is still on the device and readable: one download only.
    let intermediate = words(
        context
            .fetch(first[0], count)
            .expect("the intermediate is still resident after being consumed"),
    );
    assert_eq!(
        intermediate,
        reference(&adds(), &input, count),
        "and it is right"
    );

    context.release(first[0]);
    context.release(second[0]);
}

/// A released buffer goes back into the pool and is handed out again, uncleared.
///
/// # Invariant
/// What a run reads must never depend on what the previous run of the same size left there, and the
/// two fragments are alternated in one context so the second run of each is served from the pool the
/// first released.
#[test]
fn a_recycled_buffer_never_shows_the_previous_run_its_contents() {
    let Some(context) =
        common::context("a_recycled_buffer_never_shows_the_previous_run_its_contents")
    else {
        return;
    };
    let count = 100;
    let input: Vec<i64> = (0..count as i64).collect();
    let adds_expected: Vec<i64> = input.iter().map(|v| v + v + 1).collect();
    let select_expected: Vec<i64> = input.iter().map(|v| if *v <= 3 { 7 } else { *v }).collect();

    for round in 0..8 {
        let adds_run = context
            .run(&only(&adds()), &[BufferSlot::Host(&pack(&input))], count)
            .expect("the adds run completes");
        assert_eq!(
            words(context.fetch(adds_run[0], count).expect("adds comes back")),
            adds_expected,
            "round {round}: the adds run is right"
        );
        context.release(adds_run[0]);

        let select_run = context
            .run(
                &only(&conditional()),
                &[BufferSlot::Host(&pack(&input))],
                count,
            )
            .expect("the conditional run completes");
        assert_eq!(
            words(
                context
                    .fetch(select_run[0], count)
                    .expect("the conditional run comes back")
            ),
            select_expected,
            "round {round}: the conditional run is right, on a buffer the adds run just \
             released — a leaked byte would show here as a value the kernel never wrote"
        );
        context.release(select_run[0]);
    }
}

/// A count that is an exact multiple of the workgroup, so no padding is exercised.
#[test]
fn an_exact_workgroup_multiple_matches_too() {
    let Some(context) = common::context("an_exact_workgroup_multiple_matches_too") else {
        return;
    };
    let input: Vec<i64> = (0..(LOCAL_SIZE_X as usize * 2))
        .map(|value| value as i64)
        .collect();
    let expected: Vec<i64> = input.iter().map(|value| value + value + 1).collect();
    let device = check(&context, &adds(), &input, &expected);
    println!("ran on {device}");
}

#[test]
fn multiplication_division_and_remainder_match_the_cpu_bit_for_bit() {
    let Some(context) =
        common::context("multiplication_division_and_remainder_match_the_cpu_bit_for_bit")
    else {
        return;
    };
    let input: Vec<i64> = (0..100).collect();
    let expected: Vec<i64> = input
        .iter()
        .map(|value| (value * 3) % 7 + value / 5)
        .collect();
    let device = check(&context, &arithmetic(), &input, &expected);
    println!("ran on {device}");
}

#[test]
fn a_stored_predicate_matches_the_cpu_bit_for_bit() {
    let Some(context) = common::context("a_stored_predicate_matches_the_cpu_bit_for_bit") else {
        return;
    };
    let input: Vec<i64> = (0..100).collect();
    let expected: Vec<i64> = input
        .iter()
        .map(|value| i64::from((*value < 3 && *value > 0) || *value == 50))
        .collect();
    let device = check(&context, &predicates(), &input, &expected);
    println!("ran on {device}");
}

/// The signed reading and the unsigned one give different answers here, and the
/// unsigned one is what the language means.
#[test]
fn an_unsigned_reading_is_what_the_language_means() {
    // Every one of these is `2^63` or above as unsigned, which is a negative `i64`.
    let Some(context) = common::context("an_unsigned_reading_is_what_the_language_means") else {
        return;
    };
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
    let device = check(&context, &unsigned_reading(), &input, &expected);
    println!("ran on {device}");
}

/// Two submissions in flight at once, the second fed from the first.
///
/// # Invariant
/// Every other path waits before it returns, so this is the only place a command buffer is recorded
/// while a previous submission may still be running, and the only place a dispatch is recorded
/// against a buffer an earlier shader may not have written. Getting `4x + 3` back says both halves
/// held.
#[test]
fn a_submission_can_be_fed_to_one_that_is_still_in_flight() {
    let Some(context) = common::context("a_submission_can_be_fed_to_one_that_is_still_in_flight")
    else {
        return;
    };
    let count = 100;
    let input: Vec<i64> = (0..count as i64).collect();

    let first = context
        .submit(&only(&adds()), &[BufferSlot::Host(&pack(&input))], count)
        .expect("the first submission is recorded");
    // Deliberately not waited, and the second fed from the first: the id names an
    // unwritten buffer.
    let first_output = first.outputs()[0];
    let second = context
        .submit(&only(&adds()), &[BufferSlot::Resident(first_output)], count)
        .expect("the second submission is recorded into a different slot");

    let first_ids = Box::new(first)
        .wait()
        .expect("the first submission finishes");
    let second_ids = Box::new(second)
        .wait()
        .expect("the second submission finishes");

    let expected: Vec<i64> = input.iter().map(|value| 4 * value + 3).collect();
    assert_eq!(
        words(
            context
                .fetch(second_ids[0], count)
                .expect("the chained result comes back off the device")
        ),
        expected,
        "adds applied to adds, recorded before the first had finished"
    );
    // A consumer of the intermediate, fetched after its own wait, so this one is
    // a plain fetch of a waited buffer.
    assert_eq!(
        words(
            context
                .fetch(first_ids[0], count)
                .expect("the intermediate comes back")
        ),
        input
            .iter()
            .map(|value| value + value + 1)
            .collect::<Vec<i64>>(),
        "the intermediate is right too, and only once its submission was waited for"
    );
    context.release(first_ids[0]);
    context.release(second_ids[0]);
}

/// `out[i] = in[i] + a`, over `(n, a, index)` — the launch's runtime scalar.
///
/// # Invariant
/// Three leaves, not two: the extent, the scalar the host fixes beside it, and the index. The
/// fragment names its own class for each, and the second is the one the device used to refuse.
fn adds_a_runtime_scalar() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            3,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(2),                                        // the index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos
                FlatOp::Read(2),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Read(1),                                              // the runtime scalar
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// `out[i] = in[i] * alpha`, over `(n, alpha, index)` — a `Float` scalar beside an `Int` extent.
///
/// # Invariant
/// **A different width on each leaf**: this module is a float one, so its `Int` extent is a
/// 32-bit index where the integer module above had it at eight. A block written at the buffer
/// widths would leave `alpha` four bytes past the end.
fn scales_by_a_runtime_float() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            3,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(2),                                        // the index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos
                FlatOp::Read(2),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Float)), // in[i]
                FlatOp::Read(1), // the runtime scalar
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Float)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Float; 1],
        int_width: IntWidth::I64,
    }
}

/// The runtime scalar the host fixes reaches the shader, at its own width.
///
/// # Invariant
/// **Two values of one fragment, two answers.** A scalar the shader read from anywhere else would
/// give the same answer both rounds, which is the failure this rules out: a push constant read at
/// the wrong offset is a wrong number and not an error.
#[test]
fn a_launch_scalar_reaches_the_shader_at_its_own_width() {
    let Some(context) = common::context("a_launch_scalar_reaches_the_shader_at_its_own_width")
    else {
        return;
    };
    let count = 100;
    let input: Vec<i64> = (0..count as i64).map(|value| value - 40).collect();
    let packed = pack(&input);
    let fragment = adds_a_runtime_scalar();

    let mut answers = Vec::new();
    for scalar in [7i64, 1000] {
        let leaves = [ScalarLeaf {
            class: ScalarClass::Int,
            bits: scalar,
        }];
        let resident = context
            .run(
                &LaunchSet::single(&fragment).with_leaves(&leaves),
                &[BufferSlot::Host(&packed)],
                count,
            )
            .unwrap_or_else(|refusal| panic!("a run with a runtime scalar at {scalar}: {refusal}"));
        let got = words(
            context
                .fetch(resident[0], count)
                .expect("the result comes back off the device"),
        );
        assert_eq!(
            got,
            input
                .iter()
                .map(|value| value + scalar)
                .collect::<Vec<i64>>(),
            "the shader added the pushed scalar {scalar}, not a value it found elsewhere"
        );
        context.release(resident[0]);
        answers.push(got);
    }
    assert_ne!(
        answers[0], answers[1],
        "two launches of one fragment with two scalars are two answers"
    );

    // A launch fixing no leaves cannot fill the block.
    let refusal = context
        .run(
            &LaunchSet::single(&fragment),
            &[BufferSlot::Host(&packed)],
            count,
        )
        .expect_err("a launch that fixes no leaves cannot fill the block");
    assert!(
        refusal.to_string().contains("runtime scalar"),
        "the message names what the launch did not supply: {refusal}"
    );
}

/// The same feature in a float module, where the `Int` extent is a 32-bit member.
///
/// # Invariant
/// The extent leaf and the `Float` scalar are **both four bytes** here, so the scalar sits at
/// offset four. Reading it at eight would land on padding and multiply by a wrong number.
#[test]
fn a_float_scalar_is_pushed_beside_a_narrower_extent() {
    let Some(context) = common::context("a_float_scalar_is_pushed_beside_a_narrower_extent") else {
        return;
    };
    let count = 70;
    let input: Vec<f32> = (0..count).map(|value| value as f32 - 20.0).collect();
    let packed: Vec<u8> = input.iter().flat_map(|value| value.to_le_bytes()).collect();
    let alpha = 2.5f32;
    let leaves = [ScalarLeaf {
        class: ScalarClass::Float,
        bits: alpha.to_bits() as i64,
    }];
    let resident = context
        .run(
            &LaunchSet::single(&scales_by_a_runtime_float()).with_leaves(&leaves),
            &[BufferSlot::Host(&packed)],
            count,
        )
        .unwrap_or_else(|refusal| panic!("a float run with a runtime scalar: {refusal}"));
    let ScalarData::Float(got) = context
        .fetch(resident[0], count)
        .expect("the result comes back off the device")
    else {
        panic!("a float fragment's result came back as integers")
    };
    assert_eq!(
        got,
        input
            .iter()
            .map(|value| value * alpha)
            .collect::<Vec<f32>>(),
        "the shader scaled by the pushed float, read at its own offset"
    );
    context.release(resident[0]);
}

/// A submission nobody waited for is still released when it is dropped.
///
/// # Invariant
/// Nothing in a run relies on it, but the resources it holds are a claimed slot, a staging buffer the
/// device may still be copying out of, and upload targets nothing else can name. At depth two, a drop
/// that did not wait would hand the next acquisition a staging buffer with a copy in flight.
#[test]
fn dropping_a_submission_nobody_waited_for_still_frees_it() {
    let Some(context) = common::context("dropping_a_submission_nobody_waited_for_still_frees_it")
    else {
        return;
    };
    let count = 100;
    let input: Vec<i64> = (0..count as i64).collect();

    for round in 0..4 {
        drop(
            context
                .submit(&only(&adds()), &[BufferSlot::Host(&pack(&input))], count)
                .expect("the abandoned submission is recorded"),
        );
        // An id from an abandoned submission is still a live buffer, still readable.
        let pending = context
            .submit(&only(&adds()), &[BufferSlot::Host(&pack(&input))], count)
            .expect("the next submission is recorded");
        let ids = Box::new(pending)
            .wait()
            .expect("the next submission finishes");
        assert_eq!(
            words(context.fetch(ids[0], count).expect("comes back")),
            input
                .iter()
                .map(|value| value + value + 1)
                .collect::<Vec<i64>>(),
            "round {round}: a run after an abandoned one is right"
        );
        context.release(ids[0]);
    }
}
