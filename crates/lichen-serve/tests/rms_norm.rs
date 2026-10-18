//! Qwen3's RMSNorm at the width a real layer computes at.

mod common;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{HIDDEN, RMS_NORM_STEP, Session};

/// The row and the gain: a deterministic pattern whose root mean square is not one.
///
/// # Invariant
/// The answer is compared against the reference computed for **this** pattern, so the values here and
/// the constants below are one measurement: `x[i] = sin(0.01 i) · 1.5` and `w[i] = 0.5 + i/4096`.
fn row() -> Vec<f32> {
    (0..HIDDEN).map(|i| (i as f32 * 0.01).sin() * 1.5).collect()
}

fn gain() -> Vec<f32> {
    (0..HIDDEN).map(|i| 0.5 + i as f32 / 4096.0).collect()
}

/// The answer is the host's, at every index the reference states.
///
/// # Invariant
/// The device folds the row's squares in f32 and the reference folds them in f64, so the two agree to
/// the tolerance rather than to the bit: the largest value here is `1.41`, and `1e-5` is well below
/// the `1e-6` relative difference a reordered sum can make.
#[test]
fn the_real_width_norm_answers_what_the_host_answers() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_real_width_norm_answers_what_the_host_answers: no device");
        return;
    };
    let prepared = session
        .prepare(RMS_NORM_STEP)
        .unwrap_or_else(|refusal| panic!("the norm prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is the sum and the scale"
    );
    let stepped = prepared
        .step(vec![
            RunArgument::Count(HIDDEN as i64),
            RunArgument::Count(1),
            RunArgument::Count(HIDDEN as i64),
            floats(&row()),
            floats(&gain()),
        ])
        .unwrap_or_else(|refusal| panic!("the norm runs: {refusal}"));
    let [RunResult::Resident(normed)] = &stepped.results[..] else {
        panic!("the norm answers with its row, and this one answered otherwise");
    };
    let answered = read(&session, *normed);
    assert_eq!(
        answered.len(),
        HIDDEN,
        "the answer is one value per element"
    );
    // The reference for this pattern, computed in f64 and rounded once.
    let expected = [
        0.000000000_f32,
        0.007065859,
        0.014137907,
        0.021215437,
        0.028297741,
        0.035384104,
    ];
    for (index, wanted) in expected.iter().enumerate() {
        assert!(
            (answered[index] - wanted).abs() < 1.0e-5,
            "element {index} answered {} where the host answers {wanted}",
            answered[index]
        );
    }
    assert!(
        (answered[HIDDEN - 1] - 1.410420656).abs() < 1.0e-5,
        "the last element answered {} where the host answers 1.410420656",
        answered[HIDDEN - 1]
    );
    let total: f64 = answered.iter().map(|value| *value as f64).sum();
    assert!(
        (total - 81.782303497).abs() < 1.0e-3,
        "the row sums to {total} where the host sums to 81.782303497"
    );
    session.release(*normed);
    println!(
        "the {HIDDEN}-wide norm answered {} at element 1",
        answered[1]
    );
}
