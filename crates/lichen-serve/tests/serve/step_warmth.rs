//! What one step costs the first time and the third.

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{MATVEC_STEP, Session};

/// A projection is the same price every time once its pipeline exists.
///
/// # Invariant
/// The three runs carry the same arguments and the same graph, so a first run that costs more than the
/// third is compilation and not data: what a token pays per dispatch is this number times the dispatches.
#[test]
fn a_step_costs_the_same_the_second_time() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_step_costs_the_same_the_second_time: no device");
        return;
    };
    let prepared = session
        .prepare(MATVEC_STEP)
        .unwrap_or_else(|refusal| panic!("the projection prepares: {refusal}"));
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
            .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
        let [RunResult::Resident(resident)] = &stepped.results[..] else {
            panic!("one buffer");
        };
        let fetch_started = std::time::Instant::now();
        let answered = read(&session, *resident);
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
}
