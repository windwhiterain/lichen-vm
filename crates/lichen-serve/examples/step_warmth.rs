//! What one step costs the first time and the third.

use std::process::ExitCode;

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::{MATVEC_STEP, Session};

/// A `Float` buffer argument of `values`.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

/// A projection is the same price every time once its pipeline exists.
///
/// # Invariant
/// The three runs carry the same arguments and the same graph, so a first run that costs more than the
/// third is compilation and not data: what a token pays per dispatch is this number times the dispatches.
fn measure(session: &Session) -> Result<(), String> {
    let prepared = session
        .prepare(MATVEC_STEP)
        .map_err(|refusal| format!("the projection did not prepare: {refusal}"))?;
    let vector: Vec<f32> = (0..2048).map(|index| (index % 31) as f32 * 0.02).collect();
    let tall: Vec<f32> = (0..6144 * 2048)
        .map(|index| (index % 97) as f32 * 0.01)
        .collect();
    for round in 1..=3 {
        let stepped = prepared
            .step(vec![
                RunArgument::Count(6144),
                RunArgument::Count(2048),
                floats(&vector),
                floats(&tall),
            ])
            .map_err(|refusal| format!("the projection did not run: {refusal}"))?;
        let [RunResult::Resident(resident)] = &stepped.results[..] else {
            return Err("one buffer".into());
        };
        let fetch_started = std::time::Instant::now();
        let answered = match session.fetch(*resident) {
            Ok(ScalarData::Float(values)) => values,
            Ok(other) => return Err(format!("the answer declared a Float buffer: {other:?}")),
            Err(refusal) => return Err(format!("the answer is not readable: {refusal}")),
        };
        let fetched = fetch_started.elapsed();
        session.release(*resident);
        println!("  fetch {:.4} s", fetched.as_secs_f64());
        println!(
            "round {round}: step {:.4} s, submissions {}, fences {}, first {}",
            stepped.cost.elapsed.as_secs_f64(),
            stepped.cost.submissions,
            stepped.cost.fence_waits,
            answered[0]
        );
    }
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
