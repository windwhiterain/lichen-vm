//! Qwen3-1.7B's twenty-eight layers, streamed a layer at a time, and the token they name.

use std::path::PathBuf;

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{HIDDEN, MATVEC_STEP, RMS_NORM_STEP, SWIGLU_STEP, Session};

/// The model's shape, and the flat file's order — the contract with `dump_layers.py`.
const LAYERS: usize = 28;
const KEY_HEADS: i64 = 8;
const HEAD: i64 = 128;
const INTERMEDIATE: i64 = 6144;
const GAIN: usize = 0;
const Q: usize = 2048;
const K: usize = Q + 2048 * 2048;
const V: usize = K + 1024 * 2048;
const Q_NORM: usize = V + 1024 * 2048;
const K_NORM: usize = Q_NORM + 128;
const O: usize = K_NORM + 128;
const POST: usize = O + 2048 * 2048;
const GATE: usize = POST + 2048;
const UP: usize = GATE + 6144 * 2048;
const DOWN: usize = UP + 6144 * 2048;

/// The weights' home, or `None` when the model is not on this machine.
fn weights() -> Option<PathBuf> {
    let root = match std::env::var_os("LICHEN_MODEL_DIR") {
        Some(found) => PathBuf::from(found),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.model"),
    };
    Some(root)
}

/// One flat file, read as f32 once.
fn flat(root: &PathBuf, name: &str) -> Option<Vec<f32>> {
    let bytes = std::fs::read(root.join(format!("{name}.bin"))).ok()?;
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

/// The whole model answers the host's logits, and names the host's token.
///
/// # Invariant
/// The two implementations share no code — one is twenty-eight layers of device kernels, the other the
/// host's own arithmetic — so agreeing on the argmax after twenty-eight layers is evidence that the
/// pipeline is right. It is not evidence that the architecture is right: both follow the same reading
/// of the model, which is why the layer notes record that reading.
#[test]
fn twenty_eight_layers_answer_the_host_and_name_a_token() {
    let Some(root) = weights() else {
        eprintln!("SKIPPED twenty_eight_layers_answer_the_host_and_name_a_token: no model");
        return;
    };
    let (Some(mut hidden), Some(final_norm), Some(embedding), Some(wanted)) = (
        flat(&root, "layer0_row9707"),
        flat(&root, "final_norm"),
        flat(&root, "layer0_embed"),
        flat(&root, "full_logits"),
    ) else {
        eprintln!(
            "SKIPPED twenty_eight_layers_answer_the_host_and_name_a_token: a tensor is missing"
        );
        return;
    };
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED twenty_eight_layers_answer_the_host_and_name_a_token: no device");
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

    let normalised = |values: &[f32], gain: &[f32], rows: i64| {
        let stepped = norm
            .step(vec![
                RunArgument::Count(HIDDEN as i64 * rows),
                RunArgument::Count(rows),
                RunArgument::Count(HIDDEN as i64),
                floats(values),
                floats(gain),
            ])
            .unwrap_or_else(|refusal| panic!("the norm runs: {refusal}"));
        answered(&session, &stepped)
    };
    let projected = |values: &[f32], weight: &[f32], rows: i64, width: i64| {
        let stepped = projection
            .step(vec![
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(weight),
            ])
            .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
        answered(&session, &stepped)
    };

    for layer in 0..LAYERS {
        let Some(flat_layer) = flat(&root, &format!("layer{layer:02}")) else {
            eprintln!(
                "SKIPPED twenty_eight_layers_answer_the_host_and_name_a_token: no layer {layer}"
            );
            return;
        };
        let gain = &flat_layer[GAIN..GAIN + HIDDEN];
        let value_weight = &flat_layer[V..V + 1024 * 2048];
        let out_weight = &flat_layer[O..O + 2048 * 2048];
        let post = &flat_layer[POST..POST + HIDDEN];
        let gate_weight = &flat_layer[GATE..GATE + 6144 * 2048];
        let up_weight = &flat_layer[UP..UP + 6144 * 2048];
        let down_weight = &flat_layer[DOWN..DOWN + 2048 * 6144];

        // The attention half: one position exists, so the answer is the value, spread over its heads.
        let normed = normalised(&hidden, gain, 1);
        let value = projected(&normed, value_weight, KEY_HEADS * HEAD, HIDDEN as i64);
        let attended: Vec<f32> = (0..HIDDEN)
            .map(|index| value[(index / HEAD as usize / 2) * HEAD as usize + index % HEAD as usize])
            .collect();
        let out = projected(&attended, out_weight, HIDDEN as i64, HIDDEN as i64);
        hidden = hidden
            .iter()
            .zip(&out)
            .map(|(left, right)| left + right)
            .collect();

        // The feed-forward half.
        let prepared = normalised(&hidden, post, 1);
        let gate = projected(&prepared, gate_weight, INTERMEDIATE, HIDDEN as i64);
        let up = projected(&prepared, up_weight, INTERMEDIATE, HIDDEN as i64);
        let stepped = product
            .step(vec![
                RunArgument::Count(INTERMEDIATE),
                floats(&gate),
                floats(&up),
            ])
            .unwrap_or_else(|refusal| panic!("the product runs: {refusal}"));
        let mixed = answered(&session, &stepped);
        let down = projected(&mixed, down_weight, HIDDEN as i64, INTERMEDIATE);
        hidden = hidden
            .iter()
            .zip(&down)
            .map(|(left, right)| left + right)
            .collect();
        if layer % 7 == 0 || layer == LAYERS - 1 {
            let rms =
                (hidden.iter().map(|value| value * value).sum::<f32>() / HIDDEN as f32).sqrt();
            println!("layer {layer}: rms {rms:.9}");
        }
    }

    let normalised = normalised(&hidden, &final_norm, 1);
    let logits = projected(&normalised, &embedding, 151936, HIDDEN as i64);
    let mut best = 0;
    for (index, value) in logits.iter().enumerate() {
        if *value > logits[best] {
            best = index;
        }
    }
    let differences = logits
        .iter()
        .zip(&wanted)
        .map(|(left, right)| (left - right).abs() as f64)
        .fold(0.0_f64, f64::max);
    println!("the largest is {} at {best}", logits[best]);
    println!("the host's largest is {} at {}", wanted[25], 25);
    println!("the largest difference from the host is {differences:.3e}");
    assert_eq!(best, 25, "the model names the token the host names");
    assert!(
        differences < 1.0e-2,
        "the logits differ from the host by {differences:.3e}"
    );
}
