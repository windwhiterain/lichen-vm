//! Which row of the weight a batched projection reads, on the plain and the pack-narrowed path.
//!
//! # Invariant
//! The weight carries **one row per output row one input row produces** — `per` rows, as every
//! tensor in the model does — while a launch of `n` outputs covers `n / per` input rows. The two
//! index expressions the kernel must carry are therefore different: `row / per` is the input row
//! and `row % per` is the weight row, and the second one is the one a batch shares.

use std::process::ExitCode;
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
fn answered(session: &Session, stepped: &lichen_serve::Stepped) -> Result<Vec<f32>, String> {
    let [RunResult::Resident(rows)] = &stepped.results[..] else {
        return Err(
            "the projection answers with one resident, and this one answered otherwise".into(),
        );
    };
    let values = match session.fetch(*rows) {
        Ok(ScalarData::Float(values)) => values,
        Ok(other) => return Err(format!("the answer declared a Float buffer: {other:?}")),
        Err(refusal) => return Err(format!("the answer is not readable: {refusal}")),
    };
    session.release(*rows);
    Ok(values)
}

/// Which row of the weight the plain, the narrowed and the single-row projection each read.
fn weight_rows(session: &Session) -> Result<(), String> {
    let plain = session
        .prepare(MATVEC_BATCH_STEP)
        .map_err(|refusal| format!("the batched projection did not prepare: {refusal}"))?;
    println!("[plain] nodes {}", plain.nodes());
    let stepped = plain
        .step(vec![
            RunArgument::Count(ROWS),
            RunArgument::Count(DIM),
            RunArgument::Count(PER),
            wide(&XS),
            wide(&WS),
        ])
        .map_err(|refusal| format!("the projection did not run: {refusal}"))?;
    println!(
        "[plain] rows={ROWS} per={PER} -> {:?}, wanted {EXPECTED:?}",
        answered(session, &stepped)?
    );
    lichen_compute::compute::clear_graph_pass();
    let narrowed = lichen_serve::narrowed_projection(session)
        .map_err(|refusal| format!("the narrowed projection did not prepare: {refusal}"))?;
    let stepped = narrowed
        .step(vec![
            RunArgument::Count(ROWS),
            RunArgument::Count(DIM),
            RunArgument::Count(PER),
            narrow(&XS),
            narrow(&WS),
        ])
        .map_err(|refusal| format!("the narrowed projection did not run: {refusal}"))?;
    println!(
        "[narrowed] rows={ROWS} per={PER} -> {:?}, wanted {EXPECTED:?}",
        answered(session, &stepped)?
    );
    lichen_compute::compute::clear_graph_pass();
    // The single-row projection over the same launch reads input row zero throughout.
    let single = session
        .prepare(MATVEC_STEP)
        .map_err(|refusal| format!("the single-row projection did not prepare: {refusal}"))?;
    let stepped = single
        .step(vec![
            RunArgument::Count(ROWS),
            RunArgument::Count(DIM),
            wide(&XS),
            wide(&[1.0, 0.0, 0.0, 1.0, 2.0, 0.0, 3.0, 0.0, 0.0, 0.0, 0.0, 1.0]),
        ])
        .map_err(|refusal| format!("the single-row projection did not run: {refusal}"))?;
    println!(
        "[single] rows={ROWS} -> {:?}, wanted every output over input row zero",
        answered(session, &stepped)?
    );
    Ok(())
}

/// What the batch of two buys against the two generations it replaces.
///
/// # Invariant
/// Both sides are measured warm — one token each way first, so neither pays the page cache — and in
/// both orders, because the first measurement of a pair carries whatever the card is still doing.
fn what_a_batch_buys(session: &Session) -> Result<(), String> {
    let mut model = Qwen::open_with(session, Width::Narrowed)
        .map_err(|refusal| format!("the narrowed model did not open: {refusal}"))?;
    model
        .hold_in_memory()
        .map_err(|refusal| format!("the halves are not held: {refusal}"))?;
    let batched = || -> Result<(std::time::Duration, Vec<Vec<usize>>), String> {
        let started = Instant::now();
        let tokens = model
            .generate_batch(&STARTS, TOKENS)
            .map_err(|refusal| format!("the batch generates: {refusal}"))?;
        Ok((started.elapsed(), tokens))
    };
    let alone = || -> Result<(std::time::Duration, Vec<Vec<usize>>), String> {
        let started = Instant::now();
        let mut tokens = Vec::with_capacity(STARTS.len());
        for start in STARTS {
            tokens.push(
                model
                    .generate(start, TOKENS)
                    .map_err(|refusal| format!("{start} alone generates: {refusal}"))?
                    .tokens,
            );
        }
        Ok((started.elapsed(), tokens))
    };
    // One token each way, so neither measurement below pays the page cache.
    let _ = model.generate_batch(&STARTS, 1);
    let _ = model.generate(STARTS[0], 1);
    let (first_batch, batch_tokens) = batched()?;
    let (first_alone, alone_tokens) = alone()?;
    let (second_alone, _) = alone()?;
    let (second_batch, _) = batched()?;
    println!("[cost] batch first  {first_batch:?}");
    println!("[cost] alone second {first_alone:?}");
    println!("[cost] alone first  {second_alone:?}");
    println!("[cost] batch second {second_batch:?}");
    println!("[cost] batched {batch_tokens:?}");
    println!("[cost] alone   {alone_tokens:?}");
    Ok(())
}

fn main() -> ExitCode {
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("this reading needs a device: {refusal}");
            return ExitCode::FAILURE;
        }
    };
    for reading in [weight_rows(&session), what_a_batch_buys(&session)] {
        if let Err(message) = reading {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
