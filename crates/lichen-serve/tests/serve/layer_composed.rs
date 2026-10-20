//! Qwen3-1.7B's layer 0, one token, composed from the steps that are known to answer the host.

use std::path::PathBuf;

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{HIDDEN, MATVEC_STEP, RMS_NORM_STEP, SWIGLU_STEP, Session};

/// The token's row in the embedding and the head blocks.
const ROW: &str = "row9707";
const HEAD: usize = 128;
const KEY_HEADS: usize = 8;
const INTERMEDIATE: usize = 6144;

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

/// The one resident a step answered with, read and released.
fn answered(session: &Session, stepped: &lichen_serve::Stepped) -> Vec<f32> {
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("a step answers with one buffer, and this one answered otherwise");
    };
    let values = read(session, *resident);
    session.release(*resident);
    values
}

/// The layer's answer, one verified step at a time.
///
/// # Invariant
/// Every step here is one the crate already answers the host with — the norm, the projection and the
/// SwiGLU product — so the composition's only new content is the order the model uses and the grouped
/// spread between the value projection and the output projection.
#[test]
fn the_layer_composed_from_its_steps_answers_the_host() {
    let (Some(row), Some(wi), Some(_wq), Some(_wk), Some(wv), Some(_wqn), Some(_wkn)) = (
        tensor(ROW),
        tensor("wi"),
        tensor("wq"),
        tensor("wk"),
        tensor("wv"),
        tensor("wq_n"),
        tensor("wk_n"),
    ) else {
        eprintln!("SKIPPED the_layer_composed_from_its_steps_answers_the_host: no model files");
        return;
    };
    let (Some(wo), Some(wp), Some(wg), Some(wu), Some(wd), Some(wanted)) = (
        tensor("wo"),
        tensor("wp"),
        tensor("wg"),
        tensor("wu"),
        tensor("wd"),
        tensor("layer0_out"),
    ) else {
        eprintln!(
            "SKIPPED the_layer_composed_from_its_steps_answers_the_host: a tensor is missing"
        );
        return;
    };
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_layer_composed_from_its_steps_answers_the_host: no device");
        return;
    };
    let norm = session
        .prepare(RMS_NORM_STEP)
        .unwrap_or_else(|refusal| panic!("the norm prepares: {refusal}"));
    let projection = session
        .prepare(MATVEC_STEP)
        .unwrap_or_else(|refusal| panic!("the projection prepares: {refusal}"));
    let product = session
        .prepare(SWIGLU_STEP)
        .unwrap_or_else(|refusal| panic!("the product prepares: {refusal}"));

    let normalised = |session: &Session, values: &[f32], gain: &[f32], rows: i64, width: i64| {
        let stepped = norm
            .step(vec![
                RunArgument::Count(width),
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(gain),
            ])
            .unwrap_or_else(|refusal| panic!("the norm runs: {refusal}"));
        answered(session, &stepped)
    };
    let project = |session: &Session, values: &[f32], weight: &[f32], rows: i64, width: i64| {
        let stepped = projection
            .step(vec![
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(weight),
            ])
            .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
        answered(session, &stepped)
    };

    // The attention half, up to the first residual.
    let normed = normalised(&session, &row, &wi, 1, HIDDEN as i64);
    let value = project(
        &session,
        &normed,
        &wv,
        (KEY_HEADS * HEAD) as i64,
        HIDDEN as i64,
    );
    let attended: Vec<f32> = (0..HIDDEN)
        .map(|i| value[(i / HEAD / 2) * HEAD + i % HEAD])
        .collect();
    let out = project(&session, &attended, &wo, HIDDEN as i64, HIDDEN as i64);
    let hidden: Vec<f32> = row
        .iter()
        .zip(&out)
        .map(|(left, right)| left + right)
        .collect();

    // The feed-forward half, up to the second residual.
    let prepared = normalised(&session, &hidden, &wp, 1, HIDDEN as i64);
    let gate = project(&session, &prepared, &wg, INTERMEDIATE as i64, HIDDEN as i64);
    let up = project(&session, &prepared, &wu, INTERMEDIATE as i64, HIDDEN as i64);
    let stepped = product
        .step(vec![
            RunArgument::Count(INTERMEDIATE as i64),
            floats(&gate),
            floats(&up),
        ])
        .unwrap_or_else(|refusal| panic!("the product runs: {refusal}"));
    let mixed = answered(&session, &stepped);
    let down = project(&session, &mixed, &wd, HIDDEN as i64, INTERMEDIATE as i64);
    let final_hidden: Vec<f32> = hidden
        .iter()
        .zip(&down)
        .map(|(left, right)| left + right)
        .collect();

    let mut wrong = 0;
    for (index, expected) in wanted.iter().enumerate().take(6) {
        let difference = (final_hidden[index] - expected).abs();
        println!(
            "out[{index}] = {} (host {expected}, difference {difference:.3e})",
            final_hidden[index]
        );
        if difference >= 1.0e-4 {
            wrong += 1;
        }
    }
    let total: f64 = final_hidden.iter().map(|value| *value as f64).sum();
    println!("the composed hidden state sums to {total} (host 64.155604972)");
    assert_eq!(
        wrong, 0,
        "{wrong} of the first six elements differ from the host"
    );
    assert!(
        (total - 64.155604972).abs() < 1.0e-2,
        "the composed hidden state sums to {total} where the host sums to 64.155604972"
    );
    println!("the composed layer answers the host at every element read");
}
