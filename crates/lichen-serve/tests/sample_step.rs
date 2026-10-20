//! A step that samples from the logits on the device, reproducible from its seed.

mod common;

use common::{floats, positions, read};
use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{DECODE_STEP, ROWS, SAMPLE_STEP, Session, WEIGHTS};

/// How many positions and columns the toy score spans.
const CAPACITY: usize = 4;
const COLUMNS: usize = 5;
const SEQUENCES: usize = 2;

/// The token, the key cache and the value cache a decode step answered with.
fn returns(stepped: &lichen_serve::Stepped) -> [ResidentBuffer; 3] {
    let [
        RunResult::Resident(token),
        RunResult::Resident(keys),
        RunResult::Resident(values),
    ] = &stepped.results[..]
    else {
        panic!("a decode step answers with its token and its two caches");
    };
    [*token, *keys, *values]
}

/// The arguments a step takes before the words it appends.
fn counts() -> Vec<RunArgument> {
    vec![
        RunArgument::Count(SEQUENCES as i64),
        RunArgument::Count((SEQUENCES * ROWS) as i64),
        RunArgument::Count((SEQUENCES * CAPACITY * COLUMNS) as i64),
    ]
}

/// The buffers every decode step reads, as an empty cache and the keys two sequences append.
fn buffers() -> Vec<RunArgument> {
    let empty = vec![0.0f32; SEQUENCES * CAPACITY * COLUMNS];
    let keys = [1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    vec![
        floats(&WEIGHTS),
        floats(&empty),
        floats(&empty),
        floats(&keys),
        floats(&keys),
        positions(&[0, 0]),
    ]
}

/// Sampling at a temperature near zero answers what the largest logit answers.
///
/// # Invariant
/// The two keys give the logits `2, 1, 3, -6` and `2, 4, 2, 3`, whose largest rows are distinct, so the
/// collapse of the distribution at a small temperature is the same token the greedy kernel takes. A
/// tie would be the one input that tells the two apart, which is why neither key has one.
#[test]
fn a_cold_sample_answers_what_greedy_answers() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_cold_sample_answers_what_greedy_answers: no device");
        return;
    };
    let greedy = session
        .prepare(DECODE_STEP)
        .unwrap_or_else(|refusal| panic!("the greedy step prepares: {refusal}"));
    let mut arguments = counts();
    arguments.extend(buffers());
    let stepped = greedy
        .step(arguments)
        .unwrap_or_else(|refusal| panic!("the greedy step runs: {refusal}"));
    let [token, keys, values] = returns(&stepped);
    let decided = read(&session, token);

    let sampling = session
        .prepare(SAMPLE_STEP)
        .unwrap_or_else(|refusal| panic!("the sampling step prepares: {refusal}"));
    assert_eq!(sampling.nodes(), 7, "the held step is seven kernels");
    let mut arguments = counts();
    arguments.push(RunArgument::Float(0.01));
    arguments.push(RunArgument::Float(11.0));
    arguments.extend(buffers());
    let stepped = sampling
        .step(arguments)
        .unwrap_or_else(|refusal| panic!("the sampling step runs: {refusal}"));
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "sampling on the device is one submission"
    );
    let [sampled, sampled_keys, sampled_values] = returns(&stepped);
    assert_eq!(
        read(&session, sampled),
        decided,
        "a temperature near zero samples the largest logit, which is what greedy took"
    );
    for resident in [token, keys, values, sampled, sampled_keys, sampled_values] {
        session.release(resident);
    }
    println!("a cold sample answered {decided:?}, as greedy did");
}

/// At a real temperature the seed decides, and every token is one the score spans.
///
/// # Invariant
/// The same words and temperature with different seeds must not all answer the same token, or the
/// threshold the pick draws is not reaching the distribution. Every answer must also be a row the score
/// has, which is what bounds it to the vocabulary.
#[test]
fn a_warm_sample_follows_its_seed() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_warm_sample_follows_its_seed: no device");
        return;
    };
    let sampling = session
        .prepare(SAMPLE_STEP)
        .unwrap_or_else(|refusal| panic!("the sampling step prepares: {refusal}"));
    let mut drawn = Vec::new();
    for step in 0..8 {
        let mut arguments = counts();
        arguments.push(RunArgument::Float(4.0));
        arguments.push(RunArgument::Float(step as f32));
        arguments.extend(buffers());
        let stepped = sampling
            .step(arguments)
            .unwrap_or_else(|refusal| panic!("seed {step} runs: {refusal}"));
        let [token, keys, values] = returns(&stepped);
        let answered = read(&session, token);
        for word in &answered {
            assert!(
                *word >= 0.0 && *word < ROWS as f32,
                "seed {step} answered {word}, which is not one of the {ROWS} rows"
            );
        }
        drawn.push(answered[0]);
        for resident in [token, keys, values] {
            session.release(resident);
        }
    }
    let distinct: std::collections::BTreeSet<u32> = drawn.iter().map(|word| *word as u32).collect();
    assert!(
        distinct.len() > 1,
        "eight seeds all answered {drawn:?}, so the threshold never reached the distribution"
    );
    println!("eight seeds drew {drawn:?}");
}
