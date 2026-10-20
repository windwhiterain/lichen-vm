//! A service whose step scores, samples and appends, with the weights resident for every step.

mod common;

use common::{BATCHED, CAPACITY, COLUMNS, ROWS, WEIGHTS};
use lichen_kernel_ir::ScalarData;
use lichen_serve::{Service, Session};

/// The keys two sequences append, and the values they append beside them.
const FIRST: [f32; 5] = [1.0, 2.0, -1.0, -1.0, -1.0];
const SECOND: [f32; 5] = [1.0, 1.0, 1.0, 1.0, 1.0];

/// The floats a resident holds.
fn read(session: &Session, resident: lichen_compute::compute::ResidentBuffer) -> Vec<f32> {
    match session
        .fetch(resident)
        .unwrap_or_else(|refusal| panic!("the value is readable: {refusal}"))
    {
        ScalarData::Float(values) => values,
        other => panic!("the step declared Float buffers, and this is {other:?}"),
    }
}

/// Tenfold `keys`, so the value cache is distinguishable from the key cache.
fn tenfold(keys: &[f32]) -> Vec<f32> {
    keys.iter().map(|word| word * 10.0).collect()
}

/// A served sequence samples the token it would have sampled alone, with the weights on the device.
///
/// # Invariant
/// Two services run the same prepared source over the same uploaded weights: one holds two slots and
/// one holds a single slot, so the assertion is that the first sequence's token does not depend on
/// what else is in its batch. The weights are uploaded once and both read the same resident, which
/// is what keeps the score out of the per-step upload path.
#[test]
fn a_served_decode_step_samples_and_keeps_its_weights_resident() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_served_decode_step_samples_and_keeps_its_weights_resident: no device");
        return;
    };
    let weights = session
        .upload(&WEIGHTS)
        .unwrap_or_else(|refusal| panic!("the weights load: {refusal}"));

    let mut pair = Service::open_decode(&session, BATCHED, 2, CAPACITY, COLUMNS, ROWS, weights)
        .unwrap_or_else(|refusal| panic!("the two-slot service opens: {refusal}"));
    assert_eq!(pair.active(), 0, "a batch begins empty");
    assert_eq!(pair.admit(), Some(0), "the first slot is free");
    assert_eq!(pair.admit(), Some(1), "and the second one follows it");
    let two_keys: Vec<f32> = FIRST.iter().chain(SECOND.iter()).copied().collect();
    let stepped = pair
        .step(&two_keys, &tenfold(&two_keys))
        .unwrap_or_else(|refusal| panic!("the two-sequence step runs: {refusal}"));
    assert_eq!(stepped.advanced, 2, "both sequences advanced");
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "a whole batched decode step is one submission, weights resident or not"
    );
    let tokens = pair
        .tokens()
        .expect("a decode step samples, so it answers with tokens");
    assert_eq!(tokens.count, 2, "one token per sequence");
    let sampled = read(&session, tokens);
    let keys = read(&session, pair.keys().expect("the step wrote a key cache"));
    assert_eq!(keys.len(), pair.lanes(), "the key cache is the whole batch");
    assert_eq!(
        &keys[0..COLUMNS],
        &FIRST,
        "the first sequence's position zero is the key it appended"
    );
    let values = read(
        &session,
        pair.values().expect("the step wrote a value cache"),
    );
    assert_eq!(
        &values[0..COLUMNS],
        &tenfold(&FIRST)[0..COLUMNS],
        "and its value cache is the value word, not the key word"
    );

    let mut alone = Service::open_decode(&session, BATCHED, 1, CAPACITY, COLUMNS, ROWS, weights)
        .unwrap_or_else(|refusal| panic!("the one-slot service opens: {refusal}"));
    assert_eq!(alone.admit(), Some(0), "the only slot is free");
    alone
        .step(&FIRST, &tenfold(&FIRST))
        .unwrap_or_else(|refusal| panic!("the one-sequence step runs: {refusal}"));
    let alone_tokens = read(
        &session,
        alone
            .tokens()
            .expect("a decode step samples, so it answers with tokens"),
    );
    assert_eq!(
        sampled[0], alone_tokens[0],
        "the first sequence sampled the same token as when it was the whole batch"
    );
    assert_ne!(
        sampled[1], sampled[0],
        "and its neighbour sampled a different one, because the two keys differ"
    );

    session.release(weights);
    println!("two sequences were served by one batched decode step, with the weights resident");
}
