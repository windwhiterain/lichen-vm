//! A whole decode step over three sequences: score, sample and append, all batched by slot.

use super::common::{BATCHED, CAPACITY, COLUMNS, ROWS, WEIGHTS, floats, positions, read, returns};
use lichen_compute::compute::graph::RunArgument;
use lichen_serve::Session;

/// One sequence's step, as every argument a batched step takes with one sequence in the batch.
fn arguments_for(sequences: usize, keys: &[f32], values: &[f32]) -> Vec<RunArgument> {
    vec![
        RunArgument::Count(sequences as i64),
        RunArgument::Count((sequences * ROWS) as i64),
        RunArgument::Count((sequences * CAPACITY * COLUMNS) as i64),
        floats(&WEIGHTS),
        floats(&vec![0.0; sequences * CAPACITY * COLUMNS]),
        floats(&vec![0.0; sequences * CAPACITY * COLUMNS]),
        floats(keys),
        floats(values),
        positions(&vec![0; sequences]),
    ]
}

/// A sequence's token and cache slot do not depend on what else is in the batch.
///
/// # Invariant
/// The one-sequence run and the three-sequence run use the same prepared graph and differ only in
/// the counts and the buffer lengths, so a difference in the first sequence's token or its twenty
/// cache words is the batch crossing sequences. The three keys are distinct, so a slot that read
/// another sequence's key would score differently.
#[test]
fn a_batched_step_answers_a_sequence_as_if_it_were_alone() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_batched_step_answers_a_sequence_as_if_it_were_alone: no device");
        return;
    };
    let prepared = session
        .prepare(BATCHED)
        .unwrap_or_else(|refusal| panic!("the batched decode step prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        3,
        "the held step is score, sample and append"
    );

    let alone = [1.0f32, 2.0, -1.0, -1.0, -1.0];
    let single = prepared
        .step(arguments_for(
            1,
            &alone,
            &alone.iter().map(|w| w * 10.0).collect::<Vec<_>>(),
        ))
        .unwrap_or_else(|refusal| panic!("the one-sequence step runs: {refusal}"));
    assert_eq!(
        (single.cost.submissions, single.cost.fence_waits),
        (1, 1),
        "a whole batched decode step is one submission"
    );
    let [single_token, single_keys, single_values] = returns(&single);
    let one_token = read(&session, single_token);
    let one_keys = read(&session, single_keys);
    assert_eq!(
        one_keys.len(),
        CAPACITY * COLUMNS,
        "one sequence owns four positions of five columns"
    );
    assert_eq!(
        &one_keys[0..5],
        &alone,
        "and its first position is the key this step appended"
    );

    let three: Vec<[f32; 5]> = vec![
        [1.0, 2.0, -1.0, -1.0, -1.0],
        [1.0, 1.0, 1.0, 1.0, 1.0],
        [0.0, 1.0, 0.0, 1.0, 0.0],
    ];
    let flat: Vec<f32> = three.iter().flatten().copied().collect();
    let tenfold: Vec<f32> = flat.iter().map(|word| word * 10.0).collect();
    let batched = prepared
        .step(arguments_for(3, &flat, &tenfold))
        .unwrap_or_else(|refusal| panic!("the three-sequence step runs: {refusal}"));
    assert_eq!(
        batched.cost.submissions, 1,
        "three sequences are still one submission"
    );
    let [batched_tokens, batched_keys, batched_values] = returns(&batched);
    let tokens = read(&session, batched_tokens);
    assert_eq!(tokens.len(), 3, "one token per sequence");
    assert_eq!(
        tokens[0], one_token[0],
        "the first sequence sampled the same token as when it was alone"
    );
    let keys = read(&session, batched_keys);
    let slot = &keys[0..CAPACITY * COLUMNS];
    assert_eq!(
        slot,
        &one_keys[..],
        "and its whole cache slot is the one it had when it was alone"
    );
    let values = read(&session, batched_values);
    assert_eq!(
        &values[0..COLUMNS],
        &tenfold[0..COLUMNS],
        "the value cache took the value word, which is not the key word"
    );
    assert_ne!(
        tokens[1], tokens[0],
        "a different key is a different token, and the two are distinct keys"
    );
    for resident in [
        single_token,
        single_keys,
        single_values,
        batched_tokens,
        batched_keys,
        batched_values,
    ] {
        session.release(resident);
    }
    println!(
        "one batched step sampled three sequences at one submission, and the first one's answer \
         matched its own run alone"
    );
}
