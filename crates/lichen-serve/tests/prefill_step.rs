//! One submission fills a run of positions, which is what a prompt costs.

mod common;

use common::{floats, positions, read};
use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{PREFILL_STEP, Session};

/// How many positions a sequence may reach and how many words a position holds.
const CAPACITY: usize = 4;
const COLUMNS: usize = 5;
const SEQUENCES: usize = 2;

/// The two residents a prefill answered with: the filled key and value caches.
fn returns(stepped: &lichen_serve::Stepped) -> [ResidentBuffer; 2] {
    let [RunResult::Resident(keys), RunResult::Resident(values)] = &stepped.results[..] else {
        panic!("a prefill answers with its two caches, and this one answered otherwise");
    };
    [*keys, *values]
}

/// Two positions of prompt are filled in one submission, and the slots after them are untouched.
///
/// # Invariant
/// Each sequence's run is `1..5, 6..10` for the first and `11..15, 16..20` for the second, so the
/// assertion says which sequence's run reached which slot. The two positions past the run must stay
/// what they were, which is what tells a run from a fill of the whole cache.
#[test]
fn a_run_of_positions_fills_in_one_submission() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_run_of_positions_fills_in_one_submission: no device");
        return;
    };
    let prepared = session
        .prepare(PREFILL_STEP)
        .unwrap_or_else(|refusal| panic!("the prefill prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is the fill and the copy"
    );
    let run: Vec<f32> = (1..=20).map(|value| value as f32).collect();
    let empty = vec![0.0f32; SEQUENCES * CAPACITY * COLUMNS];
    let stepped = prepared
        .step(vec![
            RunArgument::Count((SEQUENCES * CAPACITY * COLUMNS) as i64),
            RunArgument::Count(2),
            floats(&empty),
            floats(&empty),
            floats(&run),
            floats(&run),
            positions(&[0, 0]),
        ])
        .unwrap_or_else(|refusal| panic!("the prefill runs: {refusal}"));
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "two positions of prompt are one submission"
    );
    let [keys, values] = returns(&stepped);
    let filled = read(&session, keys);
    assert_eq!(
        &filled[0..10],
        &run[0..10],
        "the first sequence's two positions are the first ten words of its run"
    );
    assert_eq!(
        &filled[20..30],
        &run[10..20],
        "and the second sequence's are the other ten, in its own slots"
    );
    assert_eq!(
        [filled[10], filled[15], filled[35]],
        [0.0; 3],
        "the positions past the run are still what they were"
    );
    assert_eq!(
        read(&session, values),
        filled,
        "the value cache took the same run, which the assertion above already checked once"
    );
    for resident in [keys, values] {
        session.release(resident);
    }
    println!("a run of two positions filled in one submission");
}
