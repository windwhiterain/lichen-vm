//! Attention over two positions: the first place the query and the key decide the answer.

mod common;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{ATTEND_STEP, Session};

/// The layer's shapes: sixteen query heads over eight key/value heads of this width.
const HEADS: usize = 16;
const KEY_HEADS: usize = 8;
const HEAD: usize = 128;

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

/// The cached keys and values: position `t` of key head `p` starts at `t · 1024 + p · 128`.
fn cached(pattern: fn(usize) -> f32) -> Vec<f32> {
    (0..2 * KEY_HEADS * HEAD).map(pattern).collect()
}

/// Two positions are scored, softened and mixed, and the answer is the host's.
///
/// # Invariant
/// The queries make the softmax sharp — the first head weights the two positions
/// `0.998` against `0.002` — so an answer that mixed them evenly, scored the wrong cached position, or
/// ignored the grouping of two query heads onto one key head moves every element by far more than the
/// tolerance. The reference is the host's f64 computation of the same three inputs.
#[test]
fn two_positions_are_scored_softened_and_mixed() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED two_positions_are_scored_softened_and_mixed: no device");
        return;
    };
    let prepared = session
        .prepare(ATTEND_STEP)
        .unwrap_or_else(|refusal| panic!("the attention prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        3,
        "the held step is scoring, softening and mixing"
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
            RunArgument::Count((HEADS * 2) as i64),
            floats(&queries()),
            floats(&keys),
            floats(&values),
        ])
        .unwrap_or_else(|refusal| panic!("the attention runs: {refusal}"));
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("the attention answers with one buffer, and this one answered otherwise");
    };
    let answered = read(&session, *resident);
    assert_eq!(
        answered.len(),
        HEADS * HEAD,
        "one value per element of one head"
    );
    let first = [0.001794103_f32, 0.019765830, 0.037729652];
    for (column, expected) in first.iter().enumerate() {
        assert!(
            (answered[column] - expected).abs() < 1.0e-5,
            "head 0 column {column} answered {} where the host answers {expected}",
            answered[column]
        );
    }
    let last = [0.243842557_f32, 0.226468750, 0.209004358];
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
        (total - 82.973584579).abs() < 1.0e-3,
        "the answer sums to {total} where the host sums to 82.973584579"
    );
    session.release(*resident);
    println!("two positions attended, head 0 column 0 at {}", answered[0]);
}
