//! The three named transcendentals, spliced and run on a device.
//! See `docs/notes/floating-point.md` §7, §8.

use super::common;
use super::gpu_matches_cpu::{floats, pack_floats, reference};

use lichen_compute_gpu::GpuContext;
use lichen_kernel_ir::{
    BufferSlot, FlatOp, IntWidth, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaneHeight, LaneWidth, LaunchSet, ScalarClass, TargetFacts,
};

/// One `Float` input and one `Float` output, over `(input, index)`.
fn float_fragment(body: KernelBody) -> KernelFragment {
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
        result_classes: vec![ScalarClass::Float; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// `out[i] = operator (in[i])`, expanded the way the lowering expands it.
fn unary(operator: KernelInstr) -> KernelFragment {
    let body = KernelBody::from_flat(
        2,
        &[
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // the write's position
            FlatOp::Read(1),                                        // the write's index
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // the read's position
            FlatOp::Read(1),                                        // the read's index
            FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Float)),
            FlatOp::Instr(operator),
            FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Float)),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // the ABI's one result
        ],
    );
    let mut spliced = float_fragment(body);
    spliced
        .body
        .expand_transcendentals()
        .expect("the operator is named in `TRANSCENDENTALS`, so its definition inlines");
    spliced
}

/// How many units in the last place two `f32`s sit apart.
fn ulps_apart(from: f32, to: f32) -> i64 {
    // A negative float's word is large unsigned, so the key is reflected down.
    let key = |value: f32| {
        let bits = value.to_bits() as i32;
        i64::from(if bits < 0 {
            i32::MIN.wrapping_sub(bits)
        } else {
            bits
        })
    };
    (key(from) - key(to)).abs()
}

/// Runs `operator` over `arguments`; `within` is a measured bound, held per element.
fn agrees(
    context: &GpuContext,
    name: &str,
    operator: KernelInstr,
    arguments: &[f32],
    cpu: fn(f32) -> f32,
    within: i64,
) {
    let count = arguments.len();
    let fragment = unary(operator);
    let resident = context
        .run(
            &LaunchSet::single(&fragment),
            &[BufferSlot::Host(&pack_floats(arguments))],
            count,
        )
        .expect("a spliced body is dispatched");
    let from_device = floats(
        context
            .fetch(resident[0], count)
            .expect("the result comes back off the device"),
    );
    context.release(resident[0]);
    assert_eq!(from_device.len(), count, "{name} answered every element");

    let words: Vec<i64> = arguments
        .iter()
        .map(|value| i64::from(value.to_bits()))
        .collect();
    let read: Vec<f32> = reference(&fragment, &words, count)
        .iter()
        .map(|word| f32::from_bits(*word as u32))
        .collect();

    let mut worst = 0;
    for element in 0..count {
        let argument = arguments[element];
        let answer = from_device[element];
        let expected = cpu(argument);
        assert_eq!(
            read[element].to_bits(),
            expected.to_bits(),
            "{name}({argument}): this file's reference and the definition part company"
        );
        let ulps = ulps_apart(answer, expected);
        println!(
            "{name}({argument}): device {answer:e} ({device:#010x}) cpu {expected:e} \
             ({reference:#010x}) {ulps} ulp",
            device = answer.to_bits(),
            reference = expected.to_bits(),
        );
        worst = worst.max(ulps);
    }
    println!("{name}: device against cpu is {worst} ulp over {count} element(s)");
    assert!(
        worst <= within,
        "{name}: the device is {worst} ulp from the definition, and the bound is {within}"
    );
}

/// `ln` is asked over one argument per path its reduction has.
#[test]
fn a_spliced_ln_is_the_definition_the_cpu_runs() {
    let Some(context) = common::context("a_spliced_ln_is_the_definition_the_cpu_runs") else {
        return;
    };
    agrees(
        &context,
        "ln",
        KernelInstr::Ln,
        &[0.5, 1.0, 1.5, 3.0, 10.0, 1000.0],
        lichen_kernel_ir::ln_value,
        0,
    );
    println!("ran on {}", context.device_name());
}

/// `sin` is asked over all four residues of the reduction and over a negative.
///
/// # Invariant
/// The bound is **zero**, and it was 5 before `NoContraction` was emitted: the
/// decoration is what made the device reproduce `sin_value` here rather than
/// within five of it. See `docs/notes/operator-reverification.md`.
#[test]
fn a_spliced_sin_is_the_definition_the_cpu_runs() {
    let Some(context) = common::context("a_spliced_sin_is_the_definition_the_cpu_runs") else {
        return;
    };
    agrees(
        &context,
        "sin",
        KernelInstr::Sin,
        &[0.25, 1.0, 3.0, 4.0, 7.0, -1.5],
        lichen_kernel_ir::sin_value,
        0,
    );
    println!("ran on {}", context.device_name());
}

/// `cos` is asked over the same arguments, so the pair reads beside each other.
///
/// # Invariant
/// The bound is **zero**, and it was 6 before `NoContraction` was emitted — the
/// same change as `sin`'s above, on the same six arguments.
#[test]
fn a_spliced_cos_is_the_definition_the_cpu_runs() {
    let Some(context) = common::context("a_spliced_cos_is_the_definition_the_cpu_runs") else {
        return;
    };
    agrees(
        &context,
        "cos",
        KernelInstr::Cos,
        &[0.25, 1.0, 3.0, 4.0, 7.0, -1.5],
        lichen_kernel_ir::cos_value,
        0,
    );
    println!("ran on {}", context.device_name());
}
