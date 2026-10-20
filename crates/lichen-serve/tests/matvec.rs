//! A projection at the model's own width: 2048 rows over 2048 inputs.

mod common;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{HIDDEN, MATVEC_STEP, Session};

/// The input: a pattern that is exact in f32, so a reordered sum is the only difference there is.
fn input() -> Vec<f32> {
    (0..HIDDEN)
        .map(|i| ((i % 13) as f32 - 6.0) * 0.25)
        .collect()
}

/// The weight: `w[j][i] = ((7i + j) mod 11 - 5) · 0.125`, likewise exact.
fn weight() -> Vec<f32> {
    let mut values = Vec::with_capacity(HIDDEN * HIDDEN);
    for j in 0..HIDDEN {
        for i in 0..HIDDEN {
            values.push((((i * 7 + j) % 11) as f32 - 5.0) * 0.125);
        }
    }
    values
}

/// Every row is the host's dot product, at the width a real projection has.
///
/// # Invariant
/// The patterns are integers over small denominators, so each product is exact in f32 and the only
/// difference a device can have is the order it adds in: the reference below is the exact sum, and
/// `1e-4` is far above the `2048 · 2^-24` a reordered f32 fold can drift.
#[test]
fn a_projection_of_the_model_width_answers_the_hosts_dot_products() {
    let Ok(session) = Session::open() else {
        eprintln!(
            "SKIPPED a_projection_of_the_model_width_answers_the_hosts_dot_products: no device"
        );
        return;
    };
    let prepared = session
        .prepare(MATVEC_STEP)
        .unwrap_or_else(|refusal| panic!("the projection prepares: {refusal}"));
    assert_eq!(prepared.nodes(), 2, "the held step is the row and the copy");
    let stepped = prepared
        .step(vec![
            RunArgument::Count(HIDDEN as i64),
            RunArgument::Count(HIDDEN as i64),
            floats(&input()),
            floats(&weight()),
        ])
        .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
    let [RunResult::Resident(rows)] = &stepped.results[..] else {
        panic!("the projection answers with its rows, and this one answered otherwise");
    };
    let answered = read(&session, *rows);
    assert_eq!(answered.len(), HIDDEN, "the answer is one value per row");
    // The exact dot products, computed in f64 from the same two patterns.
    let expected = [(0usize, -0.5_f32), (1, -1.84375), (2047, -1.84375)];
    for (row, wanted) in expected {
        assert!(
            (answered[row] - wanted).abs() < 1.0e-4,
            "row {row} answered {} where the host answers {wanted}",
            answered[row]
        );
    }
    session.release(*rows);
    println!(
        "a {HIDDEN}x{HIDDEN} projection answered {} at row 1",
        answered[1]
    );
}
