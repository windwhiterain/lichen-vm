//! Attention over a cache of three positions, which is more than the two the layer was built for.

mod common;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{ATTEND_CACHE_STEP, Session};

/// The model's shapes, and the cache the test asks for.
const HEADS: usize = 16;
const KEY_HEADS: usize = 8;
const HEAD: usize = 128;
const POSITIONS: usize = 3;

/// The queries: `q[h][j] = sin((131 h + j) / 100) · 1.5`.
fn queries() -> Vec<f32> {
    (0..HEADS * HEAD)
        .map(|lane| {
            let head = lane / HEAD;
            let column = lane % HEAD;
            (((head * 131 + column) as f32) * 0.01).sin() * 1.5
        })
        .collect()
}

/// The cached keys or values: position `t` of head `p` starts at `t · 1024 + p · 128`.
fn cached(pattern: fn(usize) -> f32) -> Vec<f32> {
    (0..POSITIONS * KEY_HEADS * HEAD).map(pattern).collect()
}

/// Three positions are scored, softened and mixed, and the position count is the caller's.
///
/// # Invariant
/// The third position is the point: the two-position step answers nothing here, and a fold that stopped
/// one position short, or that mixed the wrong key head into a query, moves every element. The third
/// position's weight is `2.8e-6`, so a kernel that dropped it entirely would still pass a loose check —
/// which is why the last head's values, at `-0.76`, are asserted too.
#[test]
fn a_cache_of_three_positions_is_attended() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_cache_of_three_positions_is_attended: no device");
        return;
    };
    let prepared = session
        .prepare(ATTEND_CACHE_STEP)
        .unwrap_or_else(|refusal| panic!("the cache attention prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        4,
        "the held step is scoring, summing, weighting and mixing"
    );
    let keys = cached(|lane| {
        let position = lane / (KEY_HEADS * HEAD);
        let rest = lane % (KEY_HEADS * HEAD);
        let head = rest / HEAD;
        let column = rest % HEAD;
        (((position * 64 + head * 137 + column) as f32) * 0.01).cos() * 1.2
    });
    let values = cached(|lane| {
        let position = lane / (KEY_HEADS * HEAD);
        let rest = lane % (KEY_HEADS * HEAD);
        let head = rest / HEAD;
        let column = rest % HEAD;
        (((position * 64 + head * 173 + column) as f32) * 0.02).sin() * 0.9
    });
    let stepped = prepared
        .step(vec![
            RunArgument::Count((HEADS * POSITIONS) as i64),
            RunArgument::Count(POSITIONS as i64),
            floats(&queries()),
            floats(&keys),
            floats(&values),
        ])
        .unwrap_or_else(|refusal| panic!("the cache attention runs: {refusal}"));
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("the cache attention answers with one buffer, and this one answered otherwise");
    };
    let answered = read(&session, *resident);
    session.release(*resident);
    assert_eq!(
        answered.len(),
        HEADS * HEAD,
        "one value per element of one head"
    );
    let first = [0.001795483_f32, 0.019767117, 0.037730845];
    for (column, expected) in first.iter().enumerate() {
        assert!(
            (answered[column] - expected).abs() < 1.0e-5,
            "head 0 column {column} answered {} where the host answers {expected}",
            answered[column]
        );
    }
    let last = [-0.760047680_f32, -0.769534645, -0.778713806];
    for (offset, expected) in last.iter().enumerate() {
        let lane = 15 * HEAD + 125 + offset;
        assert!(
            (answered[lane] - expected).abs() < 1.0e-5,
            "head 15 column {} answered {} where the host answers {expected}",
            125 + offset,
            answered[lane]
        );
    }
    let total: f64 = answered.iter().map(|value| *value as f64).sum();
    assert!(
        (total + 17.361207547).abs() < 1.0e-3,
        "the answer sums to {total} where the host sums to -17.361207547"
    );
    println!(
        "three positions attended, head 0 column 0 at {}",
        answered[0]
    );
}
