//! Which row of the weight a batched projection reads, on the plain and the pack-narrowed path.
//!
//! # Invariant
//! The weight carries **one row per output row one input row produces** — `per` rows, as every
//! tensor in the model does — while a launch of `n` outputs covers `n / per` input rows. The two
//! index expressions the kernel must carry are therefore different: `row / per` is the input row
//! and `row % per` is the weight row, and the second one is the one a batch shares.

use std::time::Instant;

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::{MATVEC_BATCH_STEP, MATVEC_STEP, Qwen, Session, Width};

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

/// The two starts the batch carries, and how many tokens each generates.
const STARTS: [usize; 2] = [9707, 785];
const TOKENS: usize = 3;

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
    let narrowed =
        lichen_serve::narrowed_projection(&session).unwrap_or_else(|refusal| panic!("{refusal}"));
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
    // The single-row projection over the same launch reads input row zero throughout.
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

/// What the batch of two buys against the two generations it replaces.
///
/// # Invariant
/// Both sides are measured warm — one token each way first, so neither pays the page cache — and in
/// both orders, because the first measurement of a pair carries whatever the card is still doing.
#[test]
fn what_a_batch_buys() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED what_a_batch_buys: no device");
        return;
    };
    let mut model = Qwen::open_with(&session, Width::Narrowed)
        .unwrap_or_else(|refusal| panic!("the narrowed model opens: {refusal}"));
    model
        .hold_in_memory()
        .unwrap_or_else(|refusal| panic!("the halves are held: {refusal}"));
    let batched = || {
        let started = Instant::now();
        let tokens = model
            .generate_batch(&STARTS, TOKENS)
            .unwrap_or_else(|refusal| panic!("the batch generates: {refusal}"));
        (started.elapsed(), tokens)
    };
    let alone = || {
        let started = Instant::now();
        let tokens = STARTS
            .iter()
            .map(|start| {
                model
                    .generate(*start, TOKENS)
                    .unwrap_or_else(|refusal| panic!("{start} alone generates: {refusal}"))
                    .tokens
            })
            .collect::<Vec<_>>();
        (started.elapsed(), tokens)
    };
    // One token each way, so neither measurement below pays the page cache.
    let _ = model.generate_batch(&STARTS, 1);
    let _ = model.generate(STARTS[0], 1);
    let (first_batch, batch_tokens) = batched();
    let (first_alone, alone_tokens) = alone();
    let (second_alone, _) = alone();
    let (second_batch, _) = batched();
    println!("[cost] batch first  {first_batch:?}",);
    println!("[cost] alone second {first_alone:?}");
    println!("[cost] alone first  {second_alone:?}");
    println!("[cost] batch second {second_batch:?}");
    println!("[cost] batched {batch_tokens:?}");
    println!("[cost] alone   {alone_tokens:?}");
}
