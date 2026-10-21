//! A tiny inference service: prefill a prompt, then sample token by token.
//!
//! # Invariant
//! A prompt fills in one submission, and each token after it passes [`BLOCKS`] blocks and then the
//! decode step, with only residents crossing between them — the hidden state, the caches and the
//! model's tables — so the path brings back one value per token, the token itself. The toy model
//! scores only the key it is handed, so its tokens repeat; this demo is the serving path.

use std::process::ExitCode;

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::ScalarData;
use lichen_serve::{
    BLOCK, BLOCKS, CAPACITY, COLUMNS, PREFILL_STEP, PROJECTIONS, QUERIES, ROWS, SAMPLE_STEP,
    Service, Session, WEIGHTS, embedding_of,
};

/// The demo's batch: two sequences, a two-token prompt, then two tokens each.
const SEQUENCES: usize = 2;
const PROMPT: usize = 2;
const TOKENS: usize = CAPACITY - PROMPT;

fn main() -> ExitCode {
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// The floats of `values`, as the bytes an argument carries.
fn packed(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// The floats of `positions`, as the bytes an `Int` argument carries, eight bytes each.
fn packed_positions(values: &[i64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// One step's argument list for the block, whose six counts are the batch's own shape.
fn block_arguments(
    state: RunArgument,
    keys: ResidentBuffer,
    values: ResidentBuffer,
    query: ResidentBuffer,
    projection: ResidentBuffer,
    lengths: &[i64],
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
        RunArgument::Buffer {
            class: lichen_kernel_ir::ScalarClass::Int,
            data: packed_positions(lengths),
        },
    ]
}

/// The one resident a single-return step answered with.
fn only(stepped: &lichen_serve::Stepped) -> Result<ResidentBuffer, String> {
    match &stepped.results[..] {
        [RunResult::Resident(resident)] => Ok(*resident),
        _ => Err("a block answers with one buffer, and this one answered otherwise".into()),
    }
}

/// The two residents a prefill answered with.
fn caches(stepped: &lichen_serve::Stepped) -> Result<(ResidentBuffer, ResidentBuffer), String> {
    match &stepped.results[..] {
        [RunResult::Resident(keys), RunResult::Resident(values)] => Ok((*keys, *values)),
        _ => Err("a prefill answers with its two caches, and this one answered otherwise".into()),
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
    let prefill = session
        .prepare(PREFILL_STEP)
        .map_err(|refusal| format!("the prefill did not prepare: {refusal}"))?;
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

    let mut service = Service::open_sample(
        &session,
        SAMPLE_STEP,
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
        "serving {SEQUENCES} sequences over {CAPACITY} positions: a {PROMPT}-token prompt then \
         {TOKENS} tokens each, through {BLOCKS} blocks and the decode step"
    );

    let prompt: Vec<f32> = (0..SEQUENCES)
        .flat_map(|_| (0..PROMPT).flat_map(|offset| embedding_of(offset as f32).into_iter()))
        .collect();
    let empty = vec![0.0f32; SEQUENCES * CAPACITY * COLUMNS];
    let started = std::time::Instant::now();
    let filled = prefill
        .step(vec![
            RunArgument::Count((SEQUENCES * CAPACITY * COLUMNS) as i64),
            RunArgument::Count(PROMPT as i64),
            RunArgument::Buffer {
                class: lichen_kernel_ir::ScalarClass::Float,
                data: packed(&empty),
            },
            RunArgument::Buffer {
                class: lichen_kernel_ir::ScalarClass::Float,
                data: packed(&empty),
            },
            RunArgument::Buffer {
                class: lichen_kernel_ir::ScalarClass::Float,
                data: packed(&prompt),
            },
            RunArgument::Buffer {
                class: lichen_kernel_ir::ScalarClass::Float,
                data: packed(&prompt),
            },
            RunArgument::Buffer {
                class: lichen_kernel_ir::ScalarClass::Int,
                data: packed_positions(&vec![0; SEQUENCES]),
            },
        ])
        .map_err(|refusal| format!("the prefill did not run: {refusal}"))?;
    let (keys, values) = caches(&filled)?;
    service.adopt(keys, values, PROMPT);
    println!(
        "prefilled {PROMPT} positions in {} submission(s), {:.2} ms",
        filled.cost.submissions,
        started.elapsed().as_secs_f64() * 1.0e3
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
            _ => return Err(format!("step {step} had no cache to attend over")),
        };
        service.set_sampler(4.0, step as f32);
        let visible = (PROMPT + step + 1).min(CAPACITY) as i64;
        let lengths = vec![visible; SEQUENCES];
        let mut hidden: Option<ResidentBuffer> = None;
        let mut submissions = 0u64;
        let mut millis = 0.0;
        for depth in 0..BLOCKS {
            let state = match hidden {
                Some(resident) => RunArgument::Resident(resident),
                None => RunArgument::Buffer {
                    class: lichen_kernel_ir::ScalarClass::Float,
                    data: packed(&words),
                },
            };
            let stepped = block
                .step(block_arguments(
                    state,
                    keys,
                    values,
                    queries[depth],
                    projections[depth],
                    &lengths,
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
            "step {step}: tokens {sampled:?} advanced {}, {submissions} submission(s) at {millis:.2} ms",
            served.advanced
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

    for resident in [weights] {
        session.release(resident);
    }
    for resident in queries.into_iter().chain(projections) {
        session.release(resident);
    }
    Ok(())
}
