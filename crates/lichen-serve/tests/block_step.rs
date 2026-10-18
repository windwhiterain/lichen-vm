//! A transformer block — attention, residual, reduce-and-broadcast, feed-forward — in one step.

mod common;

use common::{floats, read};
use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{BLOCK, Session};

/// The one resident a single-return step answered with.
fn only(stepped: &lichen_serve::Stepped) -> ResidentBuffer {
    match &stepped.results[..] {
        [RunResult::Resident(resident)] => *resident,
        _ => panic!("this step answers with one buffer, and this one answered otherwise"),
    }
}

/// The block, written again on the host, so the device's answer has an independent oracle.
///
/// # Invariant
/// It is the same twelve stages as the kernels: score, softmax, mix the values, add the residual,
/// reduce and broadcast, then spread by the weights and project. Every index is derived from the
/// three counts, so the same oracle reads a block of any width.
fn expected(
    state: &[f32],
    keys: &[f32],
    values: &[f32],
    query: &[f32],
    feed: &[f32],
    sequences: usize,
    positions: usize,
    units: usize,
) -> Vec<f32> {
    let mut answered = vec![0.0f32; sequences * units];
    for sequence in 0..sequences {
        let scores: Vec<f32> = (0..positions)
            .map(|position| {
                (0..units)
                    .map(|column| {
                        let weight = query[sequence * units + column];
                        weight * keys[(sequence * positions + position) * units + column]
                    })
                    .sum()
            })
            .collect();
        let top = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let total: f32 = scores.iter().map(|score| (score - top).exp()).sum();
        let attended: Vec<f32> = (0..units)
            .map(|column| {
                (0..positions)
                    .map(|position| {
                        let share = (scores[position] - top).exp() / total;
                        share * values[(sequence * positions + position) * units + column]
                    })
                    .sum()
            })
            .collect();
        let held: Vec<f32> = (0..units)
            .map(|unit| state[sequence * units + unit] + attended[unit])
            .collect();
        let summary: f32 = held.iter().sum();
        let normalised: Vec<f32> = held.iter().map(|unit| unit + summary).collect();
        for column in 0..units {
            answered[sequence * units + column] = (0..units)
                .map(|inner| normalised[inner] * feed[column * units + inner])
                .sum();
        }
    }
    answered
}

/// Every argument the block takes for a batch of `sequences` of `positions` of `units`.
fn arguments(
    sequences: usize,
    positions: usize,
    units: usize,
    state: &[f32],
    keys: ResidentBuffer,
    values: ResidentBuffer,
    query: &[f32],
    feed: &[f32],
) -> Vec<RunArgument> {
    vec![
        RunArgument::Count((sequences * positions * units) as i64),
        RunArgument::Count((sequences * positions) as i64),
        RunArgument::Count(sequences as i64),
        RunArgument::Count((sequences * units) as i64),
        RunArgument::Count((sequences * units * units) as i64),
        RunArgument::Count(units as i64),
        floats(state),
        RunArgument::Resident(keys),
        RunArgument::Resident(values),
        floats(query),
        floats(feed),
    ]
}

/// Two blocks of the same kind, and whether the device's answer is the host's.
fn both_match(
    session: &Session,
    sequences: usize,
    positions: usize,
    units: usize,
    state: &[f32],
    keys: &[f32],
    values: &[f32],
    query: &[f32],
    feed: &[f32],
) -> Vec<f32> {
    let cached_keys = session
        .upload(keys)
        .unwrap_or_else(|refusal| panic!("the key cache loads: {refusal}"));
    let cached_values = session
        .upload(values)
        .unwrap_or_else(|refusal| panic!("the value cache loads: {refusal}"));
    let prepared = session
        .prepare(BLOCK)
        .unwrap_or_else(|refusal| panic!("the block prepares: {refusal}"));
    assert_eq!(prepared.nodes(), 12, "the held step is twelve kernels");
    let stepped = prepared
        .step(arguments(
            sequences,
            positions,
            units,
            state,
            cached_keys,
            cached_values,
            query,
            feed,
        ))
        .unwrap_or_else(|refusal| panic!("the block runs: {refusal}"));
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "a whole block is one submission"
    );
    let answered = read(session, only(&stepped));
    let oracle = expected(
        state, keys, values, query, feed, sequences, positions, units,
    );
    for (index, (got, want)) in answered.iter().zip(oracle.iter()).enumerate() {
        assert!(
            (got - want).abs() <= 1.0e-4 * want.abs().max(1.0),
            "element {index}: the block answered {got} and the host's block {want}"
        );
    }
    session.release(only(&stepped));
    session.release(cached_keys);
    session.release(cached_values);
    answered
}

/// A block of twelve kernels runs as one submission, and its answer is the host's own block.
///
/// # Invariant
/// The state, the query and the feed-forward weights are all distinct from each other and from both
/// caches, so a stage that read the wrong buffer would answer other numbers.
#[test]
fn a_block_of_twelve_kernels_runs_in_one_submission() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_block_of_twelve_kernels_runs_in_one_submission: no device");
        return;
    };
    let keys = [
        1.0f32, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, 2.0, 1.0, -1.0,
    ];
    let values = [
        1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 1.0, 0.0, 0.0, 1.0, 2.0, 2.0,
    ];
    let answered = both_match(
        &session,
        2,
        3,
        2,
        &[1.0, 0.0, 0.0, 1.0],
        &keys,
        &values,
        &[1.0, 1.0, 1.0, 0.0],
        &[1.0, 2.0, 3.0, 1.0],
    );
    println!("the block answered {answered:?}");
}

/// The same source serves a wider model, so the width is a count and not a literal.
///
/// # Invariant
/// Five units instead of two changes every extent the host passes — words, columns and the spread —
/// while the program is the one the crate ships. A source whose width were written into it could not
/// answer this, so the host's own block is the oracle rather than a set of hand-picked numbers.
#[test]
fn the_same_block_serves_a_wider_model() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_same_block_serves_a_wider_model: no device");
        return;
    };
    let words = |length: usize, step: f32| -> Vec<f32> {
        (0..length)
            .map(|index| ((index as f32) * step).cos() + 0.5)
            .collect()
    };
    let answered = both_match(
        &session,
        2,
        3,
        5,
        &words(10, 0.7),
        &words(30, 0.3),
        &words(30, 0.9),
        &words(10, 1.1),
        &words(25, 0.5),
    );
    println!("the five-unit block answered {} words", answered.len());
}

/// Six blocks are six scoped steps, and the sixth crosses the dispatch cap of one submission.
///
/// # Invariant
/// The block is stepped six times with its own answer as the next residual, so six blocks cost six
/// submissions and seventy-two dispatches — more than one submission holds, which is why a stack is a
/// loop and not one graph. The answer is the host block iterated the same six times.
#[test]
fn a_stack_of_six_blocks_costs_six_submissions() {
    const DEPTH: usize = 6;
    const REPEATS: usize = 20;
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_stack_of_six_blocks_costs_six_submissions: no device");
        return;
    };
    let keys = [
        1.0f32, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, 2.0, 1.0, -1.0,
    ];
    let values = [
        1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 1.0, 0.0, 0.0, 1.0, 2.0, 2.0,
    ];
    let query = [1.0f32, 1.0, 1.0, 0.0];
    let feed = [1.0f32, 2.0, 3.0, 1.0];
    let held = session
        .upload(&keys)
        .unwrap_or_else(|refusal| panic!("the key cache loads: {refusal}"));
    let cached = session
        .upload(&values)
        .unwrap_or_else(|refusal| panic!("the value cache loads: {refusal}"));
    let prepared = session
        .prepare(BLOCK)
        .unwrap_or_else(|refusal| panic!("the block prepares: {refusal}"));

    let mut words = vec![1.0f32, 0.0, 0.0, 1.0];
    let mut submissions: u64 = 0;
    for block in 0..DEPTH {
        let stepped = prepared
            .step(arguments(2, 3, 2, &words, held, cached, &query, &feed))
            .unwrap_or_else(|refusal| panic!("block {block} runs: {refusal}"));
        assert_eq!(
            (stepped.cost.submissions, stepped.cost.fence_waits),
            (1, 1),
            "block {block} is one submission"
        );
        submissions += stepped.cost.submissions;
        let answered = only(&stepped);
        words = read(&session, answered);
        session.release(answered);
    }
    assert_eq!(
        submissions, DEPTH as u64,
        "six blocks over six submissions: seventy-two dispatches, past the sixty-four a step holds"
    );

    let mut oracle = vec![1.0f32, 0.0, 0.0, 1.0];
    for _ in 0..DEPTH {
        oracle = expected(&oracle, &keys, &values, &query, &feed, 2, 3, 2);
    }
    for (index, (got, want)) in words.iter().zip(oracle.iter()).enumerate() {
        assert!(
            (got - want).abs() <= 1.0e-4 * want.abs().max(1.0),
            "element {index}: six blocks answered {got} and the host's six {want}"
        );
    }

    let started = std::time::Instant::now();
    for _ in 0..REPEATS {
        let mut state = vec![1.0f32, 0.0, 0.0, 1.0];
        for _ in 0..DEPTH {
            let stepped = prepared
                .step(arguments(2, 3, 2, &state, held, cached, &query, &feed))
                .unwrap_or_else(|refusal| panic!("a stacked block runs: {refusal}"));
            let answered = only(&stepped);
            state = read(&session, answered);
            session.release(answered);
        }
        std::hint::black_box(&state);
    }
    let per_token = started.elapsed().as_secs_f64() * 1.0e3 / REPEATS as f64;
    println!(
        "six blocks: 72 dispatches over 6 submissions, {per_token:.2} ms a token, {:.2} ms a block",
        per_token / DEPTH as f64
    );
    session.release(held);
    session.release(cached);
}
