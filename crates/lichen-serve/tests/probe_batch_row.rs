//! Which row of the weight a batched projection reads, on the plain and the pack-narrowed path.
//!
//! # Invariant
//! The weight carries **one row per output row one input row produces** — `per` rows, as every
//! tensor in the model does — while a launch of `n` outputs covers `n / per` input rows. The two
//! index expressions the kernel must carry are therefore different: `row / per` is the input row
//! and `row % per` is the weight row, and the second one is the one a batch shares.

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::{MATVEC_BATCH_STEP, MATVEC_STEP, Session};

/// Three input rows of two, exact in f32 and in a half.
const XS: [f32; 6] = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
/// **Three** rows of two, one per output row one input row produces.
const WS: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 2.0, 0.0];
/// The width every row of both is.
const DIM: i64 = 2;
/// The outputs one input row produces, which is the weight's own row count.
const PER: i64 = 3;
/// The launch: two input rows, each projected to `per` outputs.
const ROWS: i64 = 6;

/// What each output row has to be: the weight's row `row % per` over the input row `row / per`.
const EXPECTED: [f32; ROWS as usize] = [1.0, 2.0, 2.0, 3.0, 4.0, 6.0];

/// A `Float` buffer argument of `values`.
fn wide(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

/// The same buffer at the width a narrowed model hands one over.
fn narrow(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::BFloat16,
        data: values
            .iter()
            .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
            .collect(),
    }
}

/// What one step answered, read back from the device.
fn answered(session: &Session, stepped: &lichen_serve::Stepped) -> Vec<f32> {
    let [RunResult::Resident(rows)] = &stepped.results[..] else {
        panic!("the projection answers with one resident, and this one answered otherwise");
    };
    let values = match session
        .fetch(*rows)
        .unwrap_or_else(|refusal| panic!("the answer is readable: {refusal}"))
    {
        ScalarData::Float(values) => values,
        other => panic!("the answer declared a Float buffer, and this is {other:?}"),
    };
    session.release(*rows);
    values
}

#[test]
fn which_weight_row_a_batch_reads() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED which_weight_row_a_batch_reads: no device");
        return;
    };
    let plain = session
        .prepare(MATVEC_BATCH_STEP)
        .unwrap_or_else(|refusal| panic!("the batched projection prepares: {refusal}"));
    println!("[plain] nodes {}", plain.nodes());
    let stepped = plain
        .step(vec![
            RunArgument::Count(ROWS),
            RunArgument::Count(DIM),
            RunArgument::Count(PER),
            wide(&XS),
            wide(&WS),
        ])
        .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
    println!(
        "[plain] rows={ROWS} per={PER} -> {:?}, wanted {EXPECTED:?}",
        answered(&session, &stepped)
    );
    lichen_compute::compute::clear_graph_pass();
    let narrowed = lichen_serve::narrowed_projection(&session)
        .unwrap_or_else(|refusal| panic!("{refusal}"));
    let stepped = narrowed
        .step(vec![
            RunArgument::Count(ROWS),
            RunArgument::Count(DIM),
            RunArgument::Count(PER),
            narrow(&XS),
            narrow(&WS),
        ])
        .unwrap_or_else(|refusal| panic!("the narrowed projection runs: {refusal}"));
    println!(
        "[narrowed] rows={ROWS} per={PER} -> {:?}, wanted {EXPECTED:?}",
        answered(&session, &stepped)
    );
    lichen_compute::compute::clear_graph_pass();
    // The single-row projection over the same launch: every output reads input row zero, and the
    // weight's own rows are `rows`.
    let single = session
        .prepare(MATVEC_STEP)
        .unwrap_or_else(|refusal| panic!("the single-row projection prepares: {refusal}"));
    let stepped = single
        .step(vec![
            RunArgument::Count(ROWS),
            RunArgument::Count(DIM),
            wide(&XS),
            wide(&[1.0, 0.0, 0.0, 1.0, 2.0, 0.0, 3.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
        ])
        .unwrap_or_else(|refusal| panic!("the single-row projection runs: {refusal}"));
    println!(
        "[single] rows={ROWS} -> {:?}, wanted every output over input row zero",
        answered(&session, &stepped)
    );
}
