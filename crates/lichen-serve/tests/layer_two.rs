//! Qwen3-1.7B's layer 0 over two tokens, where the second one's query decides its own answer.

mod common;

use std::path::PathBuf;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{
    ATTEND_STEP, HIDDEN, MATVEC_STEP, RMS_NORM_STEP, ROPE_STEP, SWIGLU_STEP, Session,
};

/// The model's shapes.
const HEADS: i64 = 16;
const KEY_HEADS: i64 = 8;
const HEAD: i64 = 128;
const INTERMEDIATE: i64 = 6144;

/// One of the reference's f32 tensors, or `None` when the model is not on this machine.
fn tensor(name: &str) -> Option<Vec<f32>> {
    let root = match std::env::var_os("LICHEN_MODEL_DIR") {
        Some(found) => PathBuf::from(found),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.model"),
    };
    let bytes = std::fs::read(root.join(format!("layer0_{name}.bin"))).ok()?;
    Some(
        bytes
            .chunks_exact(4)
            .map(|word| f32::from_le_bytes(word.try_into().unwrap_or_default()))
            .collect(),
    )
}

/// One of the two rows, or `None` when the model is not on this machine.
fn row(name: &str) -> Option<Vec<f32>> {
    tensor(name)
}

/// The one resident a step answered with, read and released.
fn answered(session: &Session, stepped: &lichen_serve::Stepped) -> Vec<f32> {
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("a step answers with one buffer, and this one answered otherwise");
    };
    let values = read(session, *resident);
    session.release(*resident);
    values
}

/// Every step the layer is composed of, prepared once.
struct Steps<'a> {
    norm: lichen_serve::Prepared<'a>,
    projection: lichen_serve::Prepared<'a>,
    rotation: lichen_serve::Prepared<'a>,
    attention: lichen_serve::Prepared<'a>,
    product: lichen_serve::Prepared<'a>,
}

impl<'a> Steps<'a> {
    fn prepare(session: &'a Session) -> Self {
        let prepared = |source: &str| {
            session
                .prepare(source)
                .unwrap_or_else(|refusal| panic!("a step prepares: {refusal}"))
        };
        Self {
            norm: prepared(RMS_NORM_STEP),
            projection: prepared(MATVEC_STEP),
            rotation: prepared(ROPE_STEP),
            attention: prepared(ATTEND_STEP),
            product: prepared(SWIGLU_STEP),
        }
    }

    fn normalised(
        &self,
        session: &Session,
        values: &[f32],
        gain: &[f32],
        rows: i64,
        width: i64,
    ) -> Vec<f32> {
        let stepped = self
            .norm
            .step(vec![
                RunArgument::Count(width * rows),
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(gain),
            ])
            .unwrap_or_else(|refusal| panic!("the norm runs: {refusal}"));
        answered(session, &stepped)
    }

    fn projected(
        &self,
        session: &Session,
        values: &[f32],
        weight: &[f32],
        rows: i64,
        width: i64,
    ) -> Vec<f32> {
        let stepped = self
            .projection
            .step(vec![
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(weight),
            ])
            .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
        answered(session, &stepped)
    }

    fn rotated(&self, session: &Session, values: &[f32], position: i64) -> Vec<f32> {
        let stepped = self
            .rotation
            .step(vec![
                RunArgument::Count(values.len() as i64),
                RunArgument::Count(position),
                floats(values),
            ])
            .unwrap_or_else(|refusal| panic!("the rotation runs: {refusal}"));
        answered(session, &stepped)
    }
}

/// The second token's answer is the host's, which means its query read both cached positions.
///
/// # Invariant
/// The first token alone cannot tell a correct attention from one that ignores its query, so this test
/// is the first that fails when the query, the key, the rotation or the grouping is wrong: the answer
/// is compared against the host's computation of the same two rows and the same twelve tensors.
#[test]
fn the_second_token_attends_to_both_and_answers_the_host() {
    let (Some(first), Some(second)) = (row("row9707"), row("row9708")) else {
        eprintln!("SKIPPED the_second_token_attends_to_both_and_answers_the_host: no rows");
        return;
    };
    let (
        Some(wi),
        Some(wq),
        Some(wk),
        Some(wv),
        Some(wqn),
        Some(wkn),
        Some(wo),
        Some(wp),
        Some(wg),
        Some(wu),
        Some(wd),
        Some(wanted),
    ) = (
        tensor("wi"),
        tensor("wq"),
        tensor("wk"),
        tensor("wv"),
        tensor("wq_n"),
        tensor("wk_n"),
        tensor("wo"),
        tensor("wp"),
        tensor("wg"),
        tensor("wu"),
        tensor("wd"),
        tensor("two_out"),
    )
    else {
        eprintln!(
            "SKIPPED the_second_token_attends_to_both_and_answers_the_host: a tensor is missing"
        );
        return;
    };
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_second_token_attends_to_both_and_answers_the_host: no device");
        return;
    };
    let steps = Steps::prepare(&session);

    // The cache: each token's key and value, rotated and grouped as the model does.
    let mut keys = Vec::new();
    let mut values = Vec::new();
    let mut query = Vec::new();
    for (position, source) in [&first, &second].iter().enumerate() {
        let normed = steps.normalised(&session, source, &wi, 1, HIDDEN as i64);
        let key = steps.projected(&session, &normed, &wk, KEY_HEADS * HEAD, HIDDEN as i64);
        let key = steps.normalised(&session, &key, &wkn, KEY_HEADS, HEAD);
        let key = steps.rotated(&session, &key, position as i64);
        keys.extend_from_slice(&key);
        let value = steps.projected(&session, &normed, &wv, KEY_HEADS * HEAD, HIDDEN as i64);
        values.extend_from_slice(&value);
        if position == 1 {
            let own = steps.projected(&session, &normed, &wq, HEADS * HEAD, HIDDEN as i64);
            let own = steps.normalised(&session, &own, &wqn, HEADS, HEAD);
            query = steps.rotated(&session, &own, position as i64);
        }
    }

    let stepped = steps
        .attention
        .step(vec![
            RunArgument::Count(HEADS * 2),
            floats(&query),
            floats(&keys),
            floats(&values),
        ])
        .unwrap_or_else(|refusal| panic!("the attention runs: {refusal}"));
    let attended = answered(&session, &stepped);
    let first_wanted = [-0.011448570_f32, 0.104194318, 0.121950299];
    for (column, expected) in first_wanted.iter().enumerate() {
        assert!(
            (attended[column] - expected).abs() < 1.0e-5,
            "the attended answer at {column} is {} where the host answers {expected}",
            attended[column]
        );
    }

    let out = steps.projected(&session, &attended, &wo, HIDDEN as i64, HIDDEN as i64);
    let hidden: Vec<f32> = second
        .iter()
        .zip(&out)
        .map(|(left, right)| left + right)
        .collect();
    let prepared = steps.normalised(&session, &hidden, &wp, 1, HIDDEN as i64);
    let gate = steps.projected(&session, &prepared, &wg, INTERMEDIATE, HIDDEN as i64);
    let up = steps.projected(&session, &prepared, &wu, INTERMEDIATE, HIDDEN as i64);
    let stepped = steps
        .product
        .step(vec![
            RunArgument::Count(INTERMEDIATE),
            floats(&gate),
            floats(&up),
        ])
        .unwrap_or_else(|refusal| panic!("the product runs: {refusal}"));
    let mixed = answered(&session, &stepped);
    let down = steps.projected(&session, &mixed, &wd, HIDDEN as i64, INTERMEDIATE);
    let answer: Vec<f32> = hidden
        .iter()
        .zip(&down)
        .map(|(left, right)| left + right)
        .collect();

    let mut wrong = 0;
    for (index, expected) in wanted.iter().enumerate().take(6) {
        let difference = (answer[index] - expected).abs();
        println!(
            "out[{index}] = {} (host {expected}, difference {difference:.3e})",
            answer[index]
        );
        if difference >= 1.0e-4 {
            wrong += 1;
        }
    }
    let total: f64 = answer.iter().map(|value| *value as f64).sum();
    println!("the second token's hidden state sums to {total} (host 1.145791696)");
    assert_eq!(
        wrong, 0,
        "{wrong} of the first six elements differ from the host"
    );
    assert!(
        (total - 1.145791696).abs() < 1.0e-2,
        "the hidden state sums to {total} where the host sums to 1.145791696"
    );
    println!("the second token answered the host");
}
