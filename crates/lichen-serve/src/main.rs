//! A tiny inference service: load a model, admit sequences, and sample token by token.
//!
//! # Invariant
//! The toy model scores only the key it is handed, so a sequence's tokens repeat once its key
//! repeats: this demo shows the *serving* mechanics — one submission a step, the cache on the device,
//! a slot retired and reused — and not a language model's behaviour.

use std::process::ExitCode;

use lichen_kernel_ir::ScalarData;
use lichen_serve::{CAPACITY, COLUMNS, DECODE_STEP, ROWS, Service, Session, WEIGHTS, embedding_of};

/// How many sequences the demo admits, and how many tokens each of them generates before the cache
/// is full.
const SEQUENCES: usize = 2;
const TOKENS: usize = CAPACITY;

fn main() -> ExitCode {
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("this demo needs a device: {refusal}");
            return ExitCode::FAILURE;
        }
    };
    let weights = match session.upload(&WEIGHTS) {
        Ok(resident) => resident,
        Err(refusal) => {
            eprintln!("the weights did not load: {refusal}");
            return ExitCode::FAILURE;
        }
    };
    let mut service = match Service::open_decode(
        &session,
        DECODE_STEP,
        SEQUENCES,
        CAPACITY,
        COLUMNS,
        ROWS,
        weights,
    ) {
        Ok(service) => service,
        Err(refusal) => {
            eprintln!("the model did not prepare: {refusal}");
            return ExitCode::FAILURE;
        }
    };
    for _ in 0..SEQUENCES {
        if service.admit().is_none() {
            eprintln!("the model ran out of slots before the demo was admitted");
            return ExitCode::FAILURE;
        }
    }
    println!(
        "serving {SEQUENCES} sequences, {TOKENS} tokens each, over {} slots of {CAPACITY} positions",
        service.lanes() / (CAPACITY * COLUMNS)
    );
    println!("the first step carries the cold start, so its clock is not a step's");

    let mut tokens: Vec<f32> = (0..SEQUENCES).map(|sequence| sequence as f32).collect();
    for step in 0..TOKENS {
        tokens = match step_once(&session, &mut service, &tokens, step) {
            Ok(sampled) => sampled,
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::FAILURE;
            }
        };
    }

    service.retire(0);
    println!("retired slot 0, and the cache waits where it was");
    match service.admit() {
        Some(slot) => println!("admitted a new sequence in slot {slot}, at position zero"),
        None => {
            eprintln!("a retired slot was not free again");
            return ExitCode::FAILURE;
        }
    }
    let fresh = vec![3.0f32; SEQUENCES];
    if let Err(message) = step_once(&session, &mut service, &fresh, TOKENS) {
        eprintln!("{message}");
        return ExitCode::FAILURE;
    }
    println!(
        "{} of {} slots are live after the swap, and every step was one submission",
        service.active(),
        SEQUENCES
    );

    session.release(weights);
    ExitCode::SUCCESS
}

/// One step of the batch, and the tokens it sampled.
fn step_once(
    session: &Session,
    service: &mut Service<'_>,
    tokens: &[f32],
    step: usize,
) -> Result<Vec<f32>, String> {
    let words: Vec<f32> = tokens
        .iter()
        .flat_map(|token| embedding_of(*token))
        .collect();
    let served = service
        .step(&words, &words)
        .map_err(|refusal| format!("step {step} did not run: {refusal}"))?;
    let sampled = match service.tokens() {
        Some(resident) => match session.fetch(resident) {
            Ok(ScalarData::Float(sampled)) => sampled,
            Ok(other) => return Err(format!("step {step} answered with {other:?}, not tokens")),
            Err(refusal) => return Err(format!("step {step} could not be read: {refusal}")),
        },
        None => {
            return Err(format!(
                "step {step} sampled nothing, so the step does not decode"
            ));
        }
    };
    println!(
        "step {step}: tokens {sampled:?} advanced {}, {} submission(s), {:.2} ms",
        served.advanced,
        served.cost.submissions,
        served.cost.elapsed.as_secs_f64() * 1.0e3
    );
    Ok(sampled)
}
