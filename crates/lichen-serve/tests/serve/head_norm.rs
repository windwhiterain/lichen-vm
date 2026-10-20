//! One norm kernel over sixteen heads of the model's own head width.

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{RMS_NORM_STEP, Session};

/// Qwen3-1.7B's head width, and the heads one attention block has.
const HEAD: usize = 128;
const HEADS: usize = 16;

/// The vector: sixteen heads of `x[i] = sin(0.01 i) · 1.5`, over the whole block.
fn rows() -> Vec<f32> {
    (0..HEADS * HEAD)
        .map(|i| (i as f32 * 0.01).sin() * 1.5)
        .collect()
}

/// The gain, shared by every head because the model normalises each head with the same weight.
fn gain() -> Vec<f32> {
    (0..HEAD).map(|i| 0.5 + i as f32 / 256.0).collect()
}

/// Each head is normalised on its own, which is what a per-row lane buys.
///
/// # Invariant
/// The reference is the host's own per-row computation, so a kernel that folded the whole block into
/// one sum would answer a different number in every head — and the last head's values, near `1.23`,
/// are far from what a shared sum gives. `1e-5` covers the f32 fold against the f64 reference.
#[test]
fn every_head_is_normalised_on_its_own() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED every_head_is_normalised_on_its_own: no device");
        return;
    };
    let prepared = session
        .prepare(RMS_NORM_STEP)
        .unwrap_or_else(|refusal| panic!("the norm prepares: {refusal}"));
    let stepped = prepared
        .step(vec![
            RunArgument::Count((HEADS * HEAD) as i64),
            RunArgument::Count(HEADS as i64),
            RunArgument::Count(HEAD as i64),
            floats(&rows()),
            floats(&gain()),
        ])
        .unwrap_or_else(|refusal| panic!("the norm runs: {refusal}"));
    let [RunResult::Resident(normed)] = &stepped.results[..] else {
        panic!("the norm answers with its rows, and this one answered otherwise");
    };
    let answered = read(&session, *normed);
    assert_eq!(answered.len(), HEADS * HEAD, "one value per element");
    let first = [0.000000000_f32, 0.008077912, 0.016280249, 0.024606172];
    for (index, wanted) in first.iter().enumerate() {
        assert!(
            (answered[index] - wanted).abs() < 1.0e-5,
            "head 0 element {index} answered {} where the host answers {wanted}",
            answered[index]
        );
    }
    let last = [1.224727988_f32, 1.229284883, 1.233716011, 1.238019943];
    for (offset, wanted) in last.iter().enumerate() {
        let index = 15 * HEAD + 124 + offset;
        assert!(
            (answered[index] - wanted).abs() < 1.0e-5,
            "head 15 element {} answered {} where the host answers {wanted}",
            124 + offset,
            answered[index]
        );
    }
    let total: f64 = answered.iter().map(|value| *value as f64).sum();
    assert!(
        (total - 28.524640736).abs() < 1.0e-3,
        "the block sums to {total} where the host sums to 28.524640736"
    );
    session.release(*normed);
    println!(
        "sixteen heads normalised, head 15 near {}",
        answered[15 * HEAD + 127]
    );
}
