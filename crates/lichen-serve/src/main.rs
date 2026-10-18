//! A tiny inference service: load a model, admit sequences, and sample token by token.
//!
//! # Invariant
//! A token passes [`BLOCKS`] blocks and then the decode step, and only residents cross between them
//! — the hidden state, the two caches and the model's tables — so the path brings back one value per
//! token, the token itself. The toy model scores only the key it is handed, so its tokens repeat;
//! this demo is the serving path, not a language model's behaviour.

use std::process::ExitCode;

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::ScalarData;
use lichen_serve::{
    BLOCK, BLOCKS, CAPACITY, COLUMNS, DECODE_STEP, PROJECTIONS, QUERIES, ROWS, Service, Session,
    WEIGHTS, embedding_of,
};

/// How many sequences the demo admits, and how many tokens each of them generates before the cache
/// is full.
const SEQUENCES: usize = 2;
const TOKENS: usize = CAPACITY;

fn main() -> ExitCode {
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// One step's argument list for the block, whose six counts are the batch's own shape.
fn block_arguments(
    state: RunArgument,
    keys: ResidentBuffer,
    values: ResidentBuffer,
    query: ResidentBuffer,
    projection: ResidentBuffer,
) -> Vec<RunArgument> {
    vec![
        RunArgument::Count((SEQUENCES * CAPACITY * COLUMNS) as i64),
        RunArgument::Count((SEQUENCES * CAPACITY) as i64),
        RunArgument::Count(SEQUENCES as i64),
        RunArgument::Count((SEQUENCES * COLUMNS) as i64),
        RunArgument::Count((SEQUENCES * COLUMNS * COLUMNS) as i64),
        RunArgument::Count(COLUMNS as i64),
        state,
        RunArgument::Resident(keys),
        RunArgument::Resident(values),
        RunArgument::Resident(query),
        RunArgument::Resident(projection),
    ]
}

/// The one resident a single-return step answered with.
fn only(stepped: &lichen_serve::Stepped) -> Result<ResidentBuffer, String> {
    match &stepped.results[..] {
        [RunResult::Resident(resident)] => Ok(*resident),
        _ => Err("a block answers with one buffer, and this one answered otherwise".into()),
    }
}

/// The floats a resident holds.
fn read(session: &Session, resident: ResidentBuffer) -> Result<Vec<f32>, String> {
    match session.fetch(resident) {
        Ok(ScalarData::Float(values)) => Ok(values),
        Ok(other) => Err(format!("a Float buffer was read as {other:?}")),
        Err(refusal) => Err(format!("the answer is not readable: {refusal}")),
    }
}

fn serve() -> Result<(), String> {
    let session =
        Session::open().map_err(|refusal| format!("this demo needs a device: {refusal}"))?;
    let weights = session
        .upload(&WEIGHTS)
        .map_err(|refusal| format!("the weights did not load: {refusal}"))?;
    let block = session
        .prepare(BLOCK)
        .map_err(|refusal| format!("the block did not prepare: {refusal}"))?;
    let zero = vec![0.0f32; SEQUENCES * CAPACITY * COLUMNS];
    let empty_keys = session
        .upload(&zero)
        .map_err(|refusal| format!("the empty key cache did not load: {refusal}"))?;
    let empty_values = session
        .upload(&zero)
        .map_err(|refusal| format!("the empty value cache did not load: {refusal}"))?;
    let mut queries = Vec::with_capacity(BLOCKS);
    let mut projections = Vec::with_capacity(BLOCKS);
    for layer in 0..BLOCKS {
        queries.push(
            session
                .upload(&QUERIES[layer])
                .map_err(|refusal| format!("layer {layer}'s query did not load: {refusal}"))?,
        );
        projections.push(
            session
                .upload(&PROJECTIONS[layer])
                .map_err(|refusal| format!("layer {layer}'s projection did not load: {refusal}"))?,
        );
    }

    let mut service = Service::open_decode(
        &session,
        DECODE_STEP,
        SEQUENCES,
        CAPACITY,
        COLUMNS,
        ROWS,
        weights,
    )
    .map_err(|refusal| format!("the model did not prepare: {refusal}"))?;
    for _ in 0..SEQUENCES {
        service
            .admit()
            .ok_or("the model ran out of slots before the demo was admitted")?;
    }
    println!(
        "serving {SEQUENCES} sequences, {TOKENS} tokens each, through {BLOCKS} blocks and the \
         decode step over {CAPACITY} positions"
    );
    println!("the first token carries the cold start, so its clock is not a token's");

    let mut tokens: Vec<f32> = (0..SEQUENCES).map(|sequence| sequence as f32).collect();
    for step in 0..TOKENS {
        let words: Vec<f32> = tokens
            .iter()
            .flat_map(|token| embedding_of(*token))
            .collect();
        let (keys, values) = match (service.keys(), service.values()) {
            (Some(keys), Some(values)) => (keys, values),
            _ => (empty_keys, empty_values),
        };
        let mut hidden: Option<ResidentBuffer> = None;
        let mut submissions = 0u64;
        let mut millis = 0.0;
        for depth in 0..BLOCKS {
            let state = match hidden {
                Some(resident) => RunArgument::Resident(resident),
                None => RunArgument::Buffer {
                    class: lichen_kernel_ir::ScalarClass::Float,
                    data: words.iter().flat_map(|word| word.to_le_bytes()).collect(),
                },
            };
            let stepped = block
                .step(block_arguments(
                    state,
                    keys,
                    values,
                    queries[depth],
                    projections[depth],
                ))
                .map_err(|refusal| format!("step {step} block {depth} did not run: {refusal}"))?;
            submissions += stepped.cost.submissions;
            millis += stepped.cost.elapsed.as_secs_f64() * 1.0e3;
            let answered = only(&stepped)?;
            if let Some(previous) = hidden {
                session.release(previous);
            }
            hidden = Some(answered);
        }
        let hidden = hidden.ok_or("a token with no blocks has no hidden state")?;
        let served = service
            .step_arguments(RunArgument::Resident(hidden), RunArgument::Resident(hidden))
            .map_err(|refusal| format!("step {step} did not sample: {refusal}"))?;
        submissions += served.cost.submissions;
        millis += served.cost.elapsed.as_secs_f64() * 1.0e3;
        session.release(hidden);
        let sampled = match service.tokens() {
            Some(resident) => read(&session, resident)?,
            None => return Err(format!("step {step} sampled nothing")),
        };
        println!(
            "step {step}: tokens {sampled:?} advanced {}, {submissions} submission(s) over \
             {} blocks, {millis:.2} ms",
            served.advanced, BLOCKS
        );
        tokens = sampled;
    }

    service.retire(0);
    println!("retired slot 0, and the cache waits where it was");
    match service.admit() {
        Some(slot) => println!("admitted a new sequence in slot {slot}, at position zero"),
        None => return Err("a retired slot was not free again".into()),
    }
    println!(
        "{} of {} slots are live after the swap",
        service.active(),
        SEQUENCES
    );

    for resident in [weights, empty_keys, empty_values] {
        session.release(resident);
    }
    for resident in queries.into_iter().chain(projections) {
        session.release(resident);
    }
    Ok(())
}
