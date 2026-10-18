//! Qwen3-1.7B generating two tokens, with the cache the second one needs.

mod common;

use std::path::PathBuf;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{
    ATTEND_STEP, HIDDEN, MATVEC_STEP, RMS_NORM_STEP, ROPE_STEP, SWIGLU_STEP, Session,
};

/// The model's shape, and the flat file's order — the contract with `dump_layers.py`.
const LAYERS: usize = 28;
const HEADS: i64 = 16;
const KEY_HEADS: i64 = 8;
const HEAD: i64 = 128;
const INTERMEDIATE: i64 = 6144;
const HIDDEN_SIZE: usize = 2048;
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

/// The starting token, and the two the host's greedy loop names after it.
const START: usize = 9707;
const FIRST: usize = 25;
const SECOND: usize = 358;

/// The weights' home.
fn root() -> PathBuf {
    match std::env::var_os("LICHEN_MODEL_DIR") {
        Some(found) => PathBuf::from(found),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.model"),
    }
}

/// One flat file, read as f32 once.
fn flat(name: &str) -> Option<Vec<f32>> {
    let bytes = std::fs::read(root().join(format!("{name}.bin"))).ok()?;
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

/// Every step the loop uses, prepared once.
struct Steps<'a> {
    norm: lichen_serve::Prepared<'a>,
    projection: lichen_serve::Prepared<'a>,
    rotation: lichen_serve::Prepared<'a>,
    attention: lichen_serve::Prepared<'a>,
    product: lichen_serve::Prepared<'a>,
}

impl<'a> Steps<'a> {
    fn prepare(session: &'a Session) -> Self {
        let prepared = |source: &str| {
            session
                .prepare(source)
                .unwrap_or_else(|refusal| panic!("a step prepares: {refusal}"))
        };
        Self {
            norm: prepared(RMS_NORM_STEP),
            projection: prepared(MATVEC_STEP),
            rotation: prepared(ROPE_STEP),
            attention: prepared(ATTEND_STEP),
            product: prepared(SWIGLU_STEP),
        }
    }

    fn normalised(
        &self,
        session: &Session,
        values: &[f32],
        gain: &[f32],
        rows: i64,
        width: i64,
    ) -> Vec<f32> {
        let stepped = self
            .norm
            .step(vec![
                RunArgument::Count(width * rows),
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(gain),
            ])
            .unwrap_or_else(|refusal| panic!("the norm runs: {refusal}"));
        answered(session, &stepped)
    }

    fn projected(
        &self,
        session: &Session,
        values: &[f32],
        weight: &[f32],
        rows: i64,
        width: i64,
    ) -> Vec<f32> {
        let stepped = self
            .projection
            .step(vec![
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(weight),
            ])
            .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
        answered(session, &stepped)
    }

    fn rotated(&self, session: &Session, values: &[f32], position: i64) -> Vec<f32> {
        let stepped = self
            .rotation
            .step(vec![
                RunArgument::Count(values.len() as i64),
                RunArgument::Count(position),
                floats(values),
            ])
            .unwrap_or_else(|refusal| panic!("the rotation runs: {refusal}"));
        answered(session, &stepped)
    }
}

/// The cache a layer keeps: one key and one value per position seen.
#[derive(Default)]
struct Cache {
    keys: Vec<f32>,
    values: Vec<f32>,
}

/// The largest logit and where it is.
fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}

/// Two tokens generated through the whole model, the second one reading the cache the first filled.
///
/// # Invariant
/// The cache is what makes the second token different: a loop that ignored it would attend only to the
/// new position, which the host's own answer rules out — `358` against whatever the first token alone
/// names. The two implementations share no code, so agreeing on both tokens is the generation check.
#[test]
fn two_tokens_are_generated_through_the_cache() {
    let Some(embedding) = flat("layer0_embed") else {
        eprintln!("SKIPPED two_tokens_are_generated_through_the_cache: no weights");
        return;
    };
    let Some(final_norm) = flat("final_norm") else {
        eprintln!("SKIPPED two_tokens_are_generated_through_the_cache: no final norm");
        return;
    };
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED two_tokens_are_generated_through_the_cache: no device");
        return;
    };
    let steps = Steps::prepare(&session);

    let mut caches: Vec<Cache> = (0..LAYERS).map(|_| Cache::default()).collect();
    let mut token = START;
    let mut chosen = Vec::new();
    for position in 0..2 {
        let mut hidden = embedding[token * HIDDEN_SIZE..(token + 1) * HIDDEN_SIZE].to_vec();
        for layer in 0..LAYERS {
            let Some(flat_layer) = flat(&format!("layer{layer:02}")) else {
                eprintln!("SKIPPED two_tokens_are_generated_through_the_cache: no layer {layer}");
                return;
            };
            let normed = steps.normalised(
                &session,
                &hidden,
                &flat_layer[GAIN..GAIN + HIDDEN_SIZE],
                1,
                HIDDEN as i64,
            );
            let key = steps.projected(
                &session,
                &normed,
                &flat_layer[K..K + 1024 * 2048],
                KEY_HEADS * HEAD,
                HIDDEN as i64,
            );
            let key = steps.normalised(
                &session,
                &key,
                &flat_layer[K_NORM..K_NORM + 128],
                KEY_HEADS,
                HEAD,
            );
            let key = steps.rotated(&session, &key, position);
            let value = steps.projected(
                &session,
                &normed,
                &flat_layer[V..V + 1024 * 2048],
                KEY_HEADS * HEAD,
                HIDDEN as i64,
            );
            caches[layer].keys.extend_from_slice(&key);
            caches[layer].values.extend_from_slice(&value);

            let attended = if position == 0 {
                // One position exists, so each of the query heads takes its key head's value.
                (0..HIDDEN_SIZE)
                    .map(|index| {
                        value[(index / HEAD as usize / 2) * HEAD as usize + index % HEAD as usize]
                    })
                    .collect::<Vec<f32>>()
            } else {
                let query = steps.projected(
                    &session,
                    &normed,
                    &flat_layer[Q..Q + 2048 * 2048],
                    HEADS * HEAD,
                    HIDDEN as i64,
                );
                let query = steps.normalised(
                    &session,
                    &query,
                    &flat_layer[Q_NORM..Q_NORM + 128],
                    HEADS,
                    HEAD,
                );
                let query = steps.rotated(&session, &query, position);
                let stepped = steps
                    .attention
                    .step(vec![
                        RunArgument::Count(HEADS * 2),
                        floats(&query),
                        floats(&caches[layer].keys),
                        floats(&caches[layer].values),
                    ])
                    .unwrap_or_else(|refusal| panic!("the attention runs: {refusal}"));
                answered(&session, &stepped)
            };

            let out = steps.projected(
                &session,
                &attended,
                &flat_layer[O..O + 2048 * 2048],
                HIDDEN as i64,
                HIDDEN as i64,
            );
            hidden = hidden
                .iter()
                .zip(&out)
                .map(|(left, right)| left + right)
                .collect();
            let prepared = steps.normalised(
                &session,
                &hidden,
                &flat_layer[POST..POST + HIDDEN_SIZE],
                1,
                HIDDEN as i64,
            );
            let gate = steps.projected(
                &session,
                &prepared,
                &flat_layer[GATE..GATE + 6144 * 2048],
                INTERMEDIATE,
                HIDDEN as i64,
            );
            let up = steps.projected(
                &session,
                &prepared,
                &flat_layer[UP..UP + 6144 * 2048],
                INTERMEDIATE,
                HIDDEN as i64,
            );
            let stepped = steps
                .product
                .step(vec![
                    RunArgument::Count(INTERMEDIATE),
                    floats(&gate),
                    floats(&up),
                ])
                .unwrap_or_else(|refusal| panic!("the product runs: {refusal}"));
            let mixed = answered(&session, &stepped);
            let down = steps.projected(
                &session,
                &mixed,
                &flat_layer[DOWN..DOWN + 2048 * 6144],
                HIDDEN as i64,
                INTERMEDIATE,
            );
            hidden = hidden
                .iter()
                .zip(&down)
                .map(|(left, right)| left + right)
                .collect();
        }
        let normalised = steps.normalised(&session, &hidden, &final_norm, 1, HIDDEN as i64);
        let logits = steps.projected(&session, &normalised, &embedding, 151936, HIDDEN as i64);
        token = argmax(&logits);
        let wanted = flat(&format!("generate_logits{position}")).expect("the reference exists");
        let difference = logits
            .iter()
            .zip(&wanted)
            .map(|(left, right)| (left - right).abs() as f64)
            .fold(0.0_f64, f64::max);
        println!(
            "position {position}: chose {token} at {}, largest difference {difference:.3e}",
            logits[token]
        );
        chosen.push(token);
    }
    println!("tokens = {chosen:?}");
    assert_eq!(
        chosen,
        [FIRST, SECOND],
        "the loop names the tokens the host names"
    );
}
