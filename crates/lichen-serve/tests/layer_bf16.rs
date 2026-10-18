//! A projection at the model's own width, run at f32 and at bf16.

mod common;

use std::time::Duration;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{MATVEC_STEP, Session, narrowed_projection};

/// The width of every projection in the model: `2048 -> 2048`.
const ROWS: usize = 2048;
const DIM: usize = 2048;

/// A weight matrix whose elements are **not** exact in bf16, so the narrowing shows in the numbers.
fn weights() -> Vec<f32> {
    (0..ROWS * DIM)
        .map(|index| ((index % 97) as f32 - 48.0) * 0.01)
        .collect()
}

/// The vector a projection multiplies.
fn vector() -> Vec<f32> {
    (0..DIM)
        .map(|index| ((index % 31) as f32 - 15.0) * 0.02)
        .collect()
}

/// The half of each `f32`, little-endian, which is one `bf16` element.
fn halves(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: lichen_kernel_ir::ScalarClass::BFloat16,
        data: values
            .iter()
            .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
            .collect(),
    }
}

/// Run the projection once, answering its rows and how long the step took.
fn run(
    session: &Session,
    prepared: &lichen_serve::Prepared<'_>,
    vector: RunArgument,
    weight: RunArgument,
) -> (Vec<f32>, Duration) {
    let stepped = prepared
        .step(vec![
            RunArgument::Count(ROWS as i64),
            RunArgument::Count(DIM as i64),
            vector,
            weight,
        ])
        .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("the projection answers with one buffer, and this one answered otherwise");
    };
    let values = read(session, *resident);
    session.release(*resident);
    (values, stepped.cost.elapsed)
}

/// Eight mantissa bits answer a two-thousand-term dot product to a few parts in a thousand.
///
/// # Invariant
/// The tolerance is relative to the widest row: a fixed one would pass a projection that answered zeros
/// near the origin, and the weights are chosen so a meaningful share of the sum comes from elements
/// whose low bits bf16 drops.
#[test]
fn a_projection_at_the_model_width_answers_the_same_rows_at_half_width() {
    let Ok(session) = Session::open() else {
        eprintln!(
            "SKIPPED a_projection_at_the_model_width_answers_the_same_rows_at_half_width: no device"
        );
        return;
    };
    let plain = session
        .prepare(MATVEC_STEP)
        .unwrap_or_else(|refusal| panic!("the projection prepares: {refusal}"));
    let narrowed = narrowed_projection(&session)
        .unwrap_or_else(|refusal| panic!("the narrowed projection prepares: {refusal}"));
    let (wide, wide_time) = run(&session, &plain, floats(&vector()), floats(&weights()));
    let (narrow, narrow_time) = run(&session, &narrowed, halves(&vector()), halves(&weights()));
    let widest = wide
        .iter()
        .fold(0.0_f32, |held, value| held.max(value.abs()));
    let difference = wide
        .iter()
        .zip(&narrow)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f32, f32::max);
    println!(
        "f32 {:.2} s, bf16 {:.2} s, widest row {widest:.6}, largest difference {difference:.6} ({:.2e} relative)",
        wide_time.as_secs_f64(),
        narrow_time.as_secs_f64(),
        difference as f64 / widest as f64
    );
    assert!(
        difference <= widest * 2.0e-2,
        "halving the width moves a row by {difference} where the widest is {widest}"
    );
}
