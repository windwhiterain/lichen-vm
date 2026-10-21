//! What a chain of projections costs when each one hands its answer back through the host.

use std::process::ExitCode;
use std::time::Instant;

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::{MATVEC_ROW_STEP, Session};

/// The projection's shape, which is the router's: 2048 outputs over the hidden width.
const ROWS: usize = 2048;
const DIM: usize = 2048;

/// The stages in the chain, which is enough for the hand-off to dominate one dispatch.
const STAGES: usize = 4;

/// The chains each run repeats.
const REPEATS: usize = 10;

/// A chain whose answers cross the bus, against one whose answers stay where they were written.
///
/// # Invariant
/// Both chains run the same programs over the same shapes, so the only difference is whether each stage's
/// answer is fetched and handed back as a buffer or passed on as the resident the device already wrote.
fn measure(session: &Session) -> Result<(), String> {
    let prepared = session
        .prepare(MATVEC_ROW_STEP)
        .map_err(|refusal| format!("the projection did not prepare: {refusal}"))?;
    let weights: Vec<u8> = (0..ROWS * DIM)
        .flat_map(|index| ((index % 251) as f32 * 0.001).to_le_bytes())
        .collect();
    let placed = session
        .place(&weights, ScalarClass::Float)
        .map_err(|refusal| format!("the weights are not placed: {refusal}"))?;
    let values: Vec<u8> = (0..DIM)
        .flat_map(|index| ((index % 97) as f32 * 0.01).to_le_bytes())
        .collect();

    let arguments = |input: RunArgument| {
        vec![
            RunArgument::Count(ROWS as i64),
            RunArgument::Count(DIM as i64),
            RunArgument::Count(ROWS as i64),
            input,
            RunArgument::Resident(placed),
        ]
    };

    // Through the host: each stage's answer is fetched, and the next stage uploads it again.
    let started = Instant::now();
    for _ in 0..REPEATS {
        let mut input = RunArgument::Buffer {
            class: ScalarClass::Float,
            data: values.clone(),
        };
        for _ in 0..STAGES {
            let stepped = prepared
                .step(arguments(input))
                .map_err(|refusal| format!("the chain did not run: {refusal}"))?;
            let [RunResult::Resident(resident)] = &stepped.results[..] else {
                return Err("a projection answers with one resident".into());
            };
            let answered = session
                .fetch(*resident)
                .map_err(|refusal| format!("the answer is not fetched: {refusal}"))?;
            let ScalarData::Float(floats) = answered else {
                return Err("a float projection answers with floats".into());
            };
            input = RunArgument::Buffer {
                class: ScalarClass::Float,
                data: floats
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect(),
            };
            let _ = session.release(*resident);
        }
    }
    let through_host = started.elapsed().as_secs_f64() / REPEATS as f64;

    // On the device: each stage's answer is the next stage's input where the device wrote it.
    let started = Instant::now();
    for _ in 0..REPEATS {
        let mut input = session
            .place(&values, ScalarClass::Float)
            .map_err(|refusal| format!("the input is not placed: {refusal}"))?;
        let mut kept = vec![input];
        for _ in 0..STAGES {
            let stepped = prepared
                .step(arguments(RunArgument::Resident(input)))
                .map_err(|refusal| format!("the chain did not run: {refusal}"))?;
            let [RunResult::Resident(resident)] = &stepped.results[..] else {
                return Err("a projection answers with one resident".into());
            };
            input = *resident;
            kept.push(input);
        }
        for resident in kept {
            let _ = session.release(resident);
        }
    }
    let on_device = started.elapsed().as_secs_f64() / REPEATS as f64;
    println!(
        "{} stages: through the host {:.3} s, on the device {:.3} s, the bus costs {:.2}x",
        STAGES,
        through_host,
        on_device,
        through_host / on_device
    );
    Ok(())
}

fn main() -> ExitCode {
    match Session::open() {
        Ok(session) => match measure(&session) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        },
        Err(refusal) => {
            eprintln!("this reading needs a device: {refusal}");
            ExitCode::FAILURE
        }
    }
}
