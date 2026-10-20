//! The vocabulary projection: 151936 rows over the model's own hidden width.

use std::path::PathBuf;

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{MATVEC_STEP, Session};

/// Qwen3-1.7B's vocabulary, which is also the head, because its embeddings are tied.
const VOCABULARY: usize = 151936;
const HIDDEN: usize = 2048;

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

/// The largest logit, which is the token a greedy sampler hands back.
fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}

/// The head answers the host's logits, and its largest names the next token.
///
/// # Invariant
/// The head is the tied embedding matrix, so a lane is one of the model's 151936 vocabulary entries and
/// folds the whole hidden width. The reference is the host's own dot products over the same two inputs,
/// so a wrong head — the wrong row, the wrong width, or a folded sum — moves the argmax, which is the
/// one number a sampler acts on.
#[test]
fn the_vocabulary_head_answers_the_host_and_names_the_next_token() {
    let Some(hidden) = tensor("two_out") else {
        eprintln!(
            "SKIPPED the_vocabulary_head_answers_the_host_and_names_the_next_token: no hidden state"
        );
        return;
    };
    let Some(embedding) = tensor("embed") else {
        eprintln!(
            "SKIPPED the_vocabulary_head_answers_the_host_and_names_the_next_token: no weights"
        );
        return;
    };
    let Some(wanted) = tensor("logits") else {
        eprintln!(
            "SKIPPED the_vocabulary_head_answers_the_host_and_names_the_next_token: no logits"
        );
        return;
    };
    assert_eq!(
        embedding.len(),
        VOCABULARY * HIDDEN,
        "the head is the whole vocabulary"
    );
    assert_eq!(
        wanted.len(),
        VOCABULARY,
        "the reference is one logit per entry"
    );
    let Ok(session) = Session::open() else {
        eprintln!(
            "SKIPPED the_vocabulary_head_answers_the_host_and_names_the_next_token: no device"
        );
        return;
    };
    let prepared = session
        .prepare(MATVEC_STEP)
        .unwrap_or_else(|refusal| panic!("the head prepares: {refusal}"));
    let stepped = prepared
        .step(vec![
            RunArgument::Count(VOCABULARY as i64),
            RunArgument::Count(HIDDEN as i64),
            floats(&hidden),
            floats(&embedding),
        ])
        .unwrap_or_else(|refusal| panic!("the head runs: {refusal}"));
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("the head answers with one buffer, and this one answered otherwise");
    };
    let answered = read(&session, *resident);
    session.release(*resident);
    assert_eq!(answered.len(), VOCABULARY, "one logit per vocabulary entry");
    let probes = [
        (0usize, 0.805703325_f32),
        (9707, 0.304524364),
        (151935, 0.030139117),
    ];
    for (index, expected) in probes {
        println!(
            "logit[{index}] = {} (host {expected}, difference {:.3e})",
            answered[index],
            (answered[index] - expected).abs()
        );
        assert!(
            (answered[index] - expected).abs() < 1.0e-4,
            "logit {index} answered {} where the host answers {expected}",
            answered[index]
        );
    }
    println!("the host's largest is {} at 9708", wanted[9708]);
    println!("the largest is {} at {}", answered[9708], argmax(&answered));
    assert_eq!(
        argmax(&answered),
        9708,
        "the head names the token the host names"
    );
}
