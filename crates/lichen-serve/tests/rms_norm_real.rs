//! Qwen3's RMSNorm over a row of the real model's own embedding.

mod common;

use std::path::PathBuf;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{HIDDEN, RMS_NORM_STEP, Session};

/// The weights the reference was computed from, if they are on this machine.
///
/// # Invariant
/// A row of `model.embed_tokens.weight` and `model.layers.0.input_layernorm.weight`, written as f32
/// by `.model/read_tensor.py`; the reference's numbers below are that script's answer for row 9707.
///
/// # Invariant
/// `LICHEN_MODEL_DIR` names the directory the weights are in, because they live beside the checkout
/// rather than in a worktree: a worktree's removal deletes what is inside it.
fn model_files() -> Option<(Vec<f32>, Vec<f32>)> {
    let root = match std::env::var_os("LICHEN_MODEL_DIR") {
        Some(found) => PathBuf::from(found),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.model"),
    };
    let row = std::fs::read(root.join("row9707.bin")).ok()?;
    let gain = std::fs::read(root.join("layer0_norm.bin")).ok()?;
    let as_floats = |bytes: Vec<u8>| -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|word| f32::from_le_bytes(word.try_into().unwrap_or_default()))
            .collect()
    };
    Some((as_floats(row), as_floats(gain)))
}

/// A row of the real embedding, through the norm, answers what the host answers.
///
/// # Invariant
/// The reference is `.model/read_tensor.py`'s own answer for row `9707`, computed in f64 from the
/// same two tensors: the device folds the squares in f32, so the two agree to `1e-5` rather than to
/// the bit. The row's root mean square is `0.032053217`, so every value here is small.
#[test]
fn a_real_embedding_row_normed_by_the_device_matches_the_host() {
    let Some((row, gain)) = model_files() else {
        eprintln!(
            "SKIPPED a_real_embedding_row_normed_by_the_device_matches_the_host: no model files"
        );
        return;
    };
    assert_eq!(row.len(), HIDDEN, "the row is the model's hidden width");
    assert_eq!(gain.len(), HIDDEN, "the gain is the model's hidden width");
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_real_embedding_row_normed_by_the_device_matches_the_host: no device");
        return;
    };
    let prepared = session
        .prepare(RMS_NORM_STEP)
        .unwrap_or_else(|refusal| panic!("the norm prepares: {refusal}"));
    let stepped = prepared
        .step(vec![
            RunArgument::Count(HIDDEN as i64),
            RunArgument::Count(1),
            RunArgument::Count(HIDDEN as i64),
            floats(&row),
            floats(&gain),
        ])
        .unwrap_or_else(|refusal| panic!("the norm runs: {refusal}"));
    let [RunResult::Resident(normed)] = &stepped.results[..] else {
        panic!("the norm answers with its row, and this one answered otherwise");
    };
    let answered = read(&session, *normed);
    let expected = [
        -0.023084413_f32,
        0.026700413,
        -0.07286645,
        -0.052993782,
        0.02772081,
        -0.054772507,
    ];
    for (index, wanted) in expected.iter().enumerate() {
        assert!(
            (answered[index] - wanted).abs() < 1.0e-5,
            "element {index} answered {} where the host answers {wanted}",
            answered[index]
        );
    }
    let total: f64 = answered.iter().map(|value| *value as f64).sum();
    assert!(
        (total - 1.907063760).abs() < 1.0e-4,
        "the row sums to {total} where the host sums to 1.907063760"
    );
    session.release(*normed);
    println!(
        "row 9707 of the real embedding normed to {} at element 0",
        answered[0]
    );
}
