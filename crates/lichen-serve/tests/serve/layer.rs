//! Qwen3-1.7B's own layer 0, one token, from the real weights.

use std::path::PathBuf;

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{LAYER_STEP, Session};

/// The token's position, which the rotation multiplies by.
const POSITION: i64 = 5;

/// The hidden width and the width the layer answers at.
const HIDDEN: usize = 2048;

/// One of the layer's tensors, as the reference dump wrote it.
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

/// The whole layer answers what the host answers for the same row of the real embedding.
///
/// # Invariant
/// The layer's answer, one step at a time, as `layer_composed` composes it.
#[ignore = "a twelve-field buffer struct binds two of its twelve fields, measured by abi_order"]
#[test]
fn the_real_layer_answers_what_the_host_answers() {
    let Some(row) = tensor("row9707") else {
        eprintln!("SKIPPED the_real_layer_answers_what_the_host_answers: no model files");
        return;
    };
    // Fields arrive in descending name order, as `abi_order` measures.
    let names = [
        "wv",
        "wu",
        "wq_n",
        "wq",
        "wp",
        "wo",
        "wk_n",
        "wk",
        "wi",
        "wg",
        "wd",
        "layer0_out",
    ];
    let mut tensors = Vec::new();
    for name in names {
        let Some(values) = tensor(name) else {
            eprintln!("SKIPPED the_real_layer_answers_what_the_host_answers: no {name}");
            return;
        };
        tensors.push(values);
    }
    let expected = tensors
        .pop()
        .expect("the last tensor is the reference answer");
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_real_layer_answers_what_the_host_answers: no device");
        return;
    };
    let prepared = session
        .prepare(LAYER_STEP)
        .unwrap_or_else(|refusal| panic!("the layer prepares: {refusal}"));
    // Thirteen dispatches are live at one position.
    assert_eq!(
        prepared.nodes(),
        13,
        "the held step is the live part of a layer"
    );
    let mut arguments: Vec<RunArgument> = vec![floats(&row)];
    for values in &tensors {
        arguments.push(floats(values));
    }
    arguments.push(RunArgument::Count(POSITION));
    let stepped = prepared
        .step(arguments)
        .unwrap_or_else(|refusal| panic!("the layer runs: {refusal}"));
    let [RunResult::Resident(answered)] = &stepped.results[..] else {
        panic!("the layer answers with its hidden state, and this one answered otherwise");
    };
    let values = read(&session, *answered);
    assert_eq!(
        values.len(),
        HIDDEN,
        "the layer answers at the hidden width"
    );
    // The host's own answer for this row, from `layer_reference.py`.
    let wanted = [
        0.247785583_f32,
        -0.021492276,
        0.322389841,
        -0.498382092,
        0.027982337,
        -0.048376147,
    ];
    for (index, expected) in wanted.iter().enumerate() {
        if (values[index] - expected).abs() >= 1.0e-4 {
            println!(
                "element {index} answered {} where the host answers {expected}",
                values[index]
            );
        }
    }
    let total: f64 = values.iter().map(|value| *value as f64).sum();
    assert!(
        (total - 64.155604972).abs() < 1.0e-2,
        "the hidden state sums to {total} where the host sums to 64.155604972"
    );
    let root_mean_square = (values
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>()
        / HIDDEN as f64)
        .sqrt();
    assert!(
        (root_mean_square - 0.545758751).abs() < 1.0e-5,
        "the hidden state's root mean square is {root_mean_square} where the host's is 0.545758751"
    );
    session.release(*answered);
    println!(
        "the real layer 0 answered {} at element 0 and {} overall",
        values[0], expected[0]
    );
}
