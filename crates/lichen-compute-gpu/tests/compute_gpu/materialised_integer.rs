//! A boolean materialised into the integer class of a **float** module.
//!
//! # Invariant
//! `docs/notes/dispatch-crash.md` localised the sampler's `0xC0000005` to one emitted id: a float
//! module's constant pool declared `one_integer` and not `zero_integer`, so a boolean
//! materialised into the integer class named an id no instruction defined. Both the shape that
//! reaches it and the pool that owes the constant are pinned here.

use super::common;
use super::gpu_matches_cpu::{floats, pack_floats, reference};
use super::spirv_validation::validate;

use lichen_compute_gpu::spirv::{self, Binding};
use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X};
use lichen_kernel_ir::{
    Br, BufferSlot, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr,
    KernelRoles, KernelShape, LaneHeight, LaneWidth, LaunchSet, ScalarClass, TargetFacts,
    Terminator,
};

/// How many times [`masked_loop`] runs its body.
const ITERATIONS: i64 = 3;

/// A float module over `(config, index)`: one `Float` input and one `Float` output.
fn float_over_a_float_buffer(body: KernelBody) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        // The ABI's own result is an integer however the buffers are classed.
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(LOCAL_SIZE_X),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// `out[i] = float (the times in[i] > 0.0)` — the sampler's shape, in miniature.
///
/// # Invariant
/// The loop is the sampler's fold: an integer counter carried beside an integer running total and
/// a carried float element, compared with **float** order the way the sampler's `>` is. The
/// sampler's `1 *` is what this does: multiplied by an integer `1`, `bin_class` reads the declared
/// `Int` and `as_class` materialises the boolean into the integer class.
fn masked_loop() -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    body.add_param(entry); // the launch config, unread
    let seed = body.add_param(entry);
    let stop = body.add_const(entry, ScalarClass::Int, ITERATIONS);
    let zero = body.add_const(entry, ScalarClass::Int, 0);
    let one = body.add_const(entry, ScalarClass::Int, 1);
    // The one position, read in the input space and written in the output space.
    let position = body.add_const(entry, ScalarClass::Int, 0);
    // A float body's `Const` payload is the `f32`'s bits, and `0.0f32` is zero.
    let nothing = body.add_const(entry, ScalarClass::Float, 0);
    let element = body.add_op(
        entry,
        KernelInstr::BufferReadCall(ScalarClass::Float),
        vec![position, seed],
        vec![ScalarClass::Float],
    );

    let header = body.add_block();
    let counter = body.add_param(header);
    let running = body.add_param(header);
    let carried = body.add_param(header);
    let reached = body.add_op(
        header,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Lt),
        vec![counter, stop],
        vec![ScalarClass::Int],
    );

    let step = body.add_block();
    let current = body.add_param(step);
    let held = body.add_param(step);
    let lane = body.add_param(step);
    let holds = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Float, KernelBin::Gt),
        vec![lane, nothing],
        vec![ScalarClass::Float],
    );
    // **The feature**: `1 * holds`, whose `OpSelect` names the integer pair.
    let masked = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Mul),
        vec![one, holds],
        vec![ScalarClass::Int],
    );
    let advanced = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![held, masked],
        vec![ScalarClass::Int],
    );
    let stepped = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![current, one],
        vec![ScalarClass::Int],
    );

    let merged = body.add_block();
    let total = body.add_param(merged);
    body.add_param(merged);
    let widened = body.add_op(
        merged,
        KernelInstr::Conv {
            from: ScalarClass::Int,
            to: ScalarClass::Float,
        },
        vec![total],
        vec![ScalarClass::Float],
    );
    body.add_op(
        merged,
        KernelInstr::BufferWriteCall(ScalarClass::Float),
        vec![position, seed, widened],
        Vec::new(),
    );

    body.set_terminator(
        entry,
        Terminator::Br(Br {
            target: header,
            args: vec![zero, zero, element],
        }),
    );
    body.set_terminator(
        header,
        Terminator::CondBr {
            cond: reached,
            if_true: Br {
                target: step,
                args: vec![counter, running, carried],
            },
            if_false: Br {
                target: merged,
                args: vec![running, carried],
            },
        },
    );
    body.set_terminator(
        step,
        Terminator::Br(Br {
            target: header,
            args: vec![stepped, advanced, lane],
        }),
    );
    // The ABI's own result is an integer however the buffers are classed.
    body.set_terminator(
        merged,
        Terminator::Return {
            values: vec![total],
        },
    );
    float_over_a_float_buffer(body)
}

/// `out[i] = if 1 then in[i] else 0.0` — a float module whose condition is a **literal**.
///
/// # Invariant
/// This is the other road to the same constant: `as_condition` turns a non-`bool` into one with
/// `OpINotEqual … zero_integer`, and a float module's pool owed no such id. The sampler's body
/// never exercised this arm, so without it the sweep's second finding would stay a reading of the
/// source rather than a measured module.
fn integer_selector() -> KernelFragment {
    let read = |body: &mut Vec<FlatOp>| {
        body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
        body.push(FlatOp::Read(1));
        body.push(FlatOp::Instr(KernelInstr::BufferReadCall(
            ScalarClass::Float,
        )));
    };
    // A write's `[position, index]`, then its three operands in push order:
    // `then` arm, `otherwise` arm, selector.
    let mut body = vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        FlatOp::Read(1),
    ];
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Float, 0)));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)));
    body.push(FlatOp::Instr(KernelInstr::Select));
    body.push(FlatOp::Instr(KernelInstr::BufferWriteCall(
        ScalarClass::Float,
    )));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    float_over_a_float_buffer(KernelBody::from_flat(2, &body))
}

/// The module `fragment` is emitted as, checked before anything else asks for it.
fn emitted(what: &str, fragment: &KernelFragment) -> Vec<u32> {
    fragment
        .body
        .validate()
        .unwrap_or_else(|broken| panic!("{what} is well formed: {broken}"));
    spirv::compile(
        &LaunchSet::single(fragment),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .unwrap_or_else(|refusal| panic!("{what} is emitted: {refusal}"))
}

/// A float loop that materialises a boolean into the integer class emits a module a validator
/// accepts.
///
/// # Invariant
/// This is the whole claim: the missing `OpConstant` is what `spirv-val` rejected by name and both
/// this machine's drivers died on, so an accepted module is the fix made visible. The test asks
/// the validator rather than reading the words, because a malformed module is what the validator
/// is for.
#[test]
fn a_float_loop_materialising_a_boolean_into_the_integer_class_validates() {
    let fragment = masked_loop();
    let words = emitted("the masked loop", &fragment);
    assert!(
        validate(
            "a float loop materialising a boolean into the integer class",
            &words
        ),
        "`spirv-val` accepted the float module whose loop materialises a boolean into the integer \
         class; a skip means the module was not covered — put `spirv-val` on PATH"
    );
}

/// A float module whose condition is an integer names a defined id too.
///
/// # Invariant
/// The same constant, asked for from `as_condition` rather than from `as_class`: a float module
/// owes the integer `0` whichever of the two reads it, so one accepted module would not say the
/// other arm is covered.
#[test]
fn a_float_module_naming_the_integer_zero_as_a_condition_validates() {
    let fragment = integer_selector();
    let words = emitted("the integer-selector select", &fragment);
    assert!(
        validate("a float module whose condition is an integer", &words),
        "`spirv-val` accepted the float module whose `Select` condition is an integer; a skip means \
         the module was not covered — put `spirv-val` on PATH"
    );
}

/// Run `fragment` over `input` on the device and answer the `count` floats it writes.
fn on_device(
    context: &GpuContext,
    what: &str,
    fragment: &KernelFragment,
    input: &[f32],
) -> Vec<f32> {
    let count = input.len();
    let resident = context
        .run(
            &LaunchSet::single(fragment),
            &[BufferSlot::Host(&pack_floats(input))],
            count,
        )
        .unwrap_or_else(|refusal| panic!("{what} is dispatched: {refusal}"));
    let answer = floats(
        context
            .fetch(resident[0], count)
            .expect("the result comes back off the device"),
    );
    context.release(resident[0]);
    answer
}

/// The masked loop runs, and answers what its three trips compute.
///
/// # Invariant
/// A module a validator accepts is not yet an answer: the emitted `OpSelect` could carry the pair
/// the wrong way round and still validate, and then every trip would count as `0`. The answer is
/// therefore written out by hand — **not** taken from `reference`, which walks only the entry
/// block's instructions and reads a looped body as never having run.
#[test]
fn the_masked_loop_on_the_device_answers_what_its_trips_compute() {
    let Some(context) =
        common::context("the_masked_loop_on_the_device_answers_what_its_trips_compute")
    else {
        return;
    };
    let fragment = masked_loop();
    // Two positives and two not: the mask is the integer class's `1`/`0`, and
    // the total is how many of the three trips held.
    let input = [1.0f32, -1.0, 4.0, 0.0];
    let expected: Vec<f32> = input
        .iter()
        .map(|value| if *value > 0.0 { ITERATIONS as f32 } else { 0.0 })
        .collect();
    let from_device = on_device(&context, "the masked loop", &fragment, &input);
    println!(
        "masked loop: device {from_device:?} expected {expected:?} on {}",
        context.device_name()
    );
    assert_eq!(
        from_device, expected,
        "the device counts the trips whose condition held"
    );
}

/// The `Select` whose condition is an integer answers the same on both readings.
///
/// # Invariant
/// This body is one entry block, so `reference` **can** read it — and it is the reading a float
/// comparison against, so the device is compared bit-for-bit rather than against a hand-written
/// list. `1` is a non-zero bit pattern, so every element is passed through and any module that
/// dropped the branch would answer zeroes instead.
#[test]
fn the_integer_selector_on_the_device_is_what_the_cpu_reference_answers() {
    let Some(context) =
        common::context("the_integer_selector_on_the_device_is_what_the_cpu_reference_answers")
    else {
        return;
    };
    let fragment = integer_selector();
    let input = [1.5f32, -2.25, 0.0, 7.75, -0.5, 3.0];
    let count = input.len();
    let from_device = on_device(&context, "the integer selector", &fragment, &input);
    let words: Vec<i64> = input.iter().map(|v| i64::from(v.to_bits())).collect();
    let from_cpu: Vec<f32> = reference(&fragment, &words, count)
        .iter()
        .map(|word| f32::from_bits(*word as u32))
        .collect();
    println!(
        "integer selector: device {from_device:?} cpu {from_cpu:?} on {}",
        context.device_name()
    );
    assert_eq!(
        from_device,
        input.to_vec(),
        "`1` selects every element through"
    );
    let bits = |values: &[f32]| -> Vec<u32> { values.iter().map(|v| v.to_bits()).collect() };
    assert_eq!(
        bits(&from_device),
        bits(&from_cpu),
        "the device and the CPU reference are bit-for-bit alike"
    );
}
