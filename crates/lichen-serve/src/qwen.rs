//! Qwen3-1.7B end to end: the weights a layer at a time, the cache, and the tokens.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lichen_compute::compute::graph::{RunArgument, RunResult};

use crate::{
    ATTEND_STEP, HIDDEN, MATVEC_STEP, RMS_NORM_STEP, ROPE_STEP, SWIGLU_STEP, ServeRefusal, Session,
};

/// The model's shape, and the flat file's order — the contract with `.model/dump_layers.py`.
const LAYERS: usize = 28;
const HEADS: i64 = 16;
const KEY_HEADS: i64 = 8;
const HEAD: i64 = 128;
const INTERMEDIATE: i64 = 6144;
const HIDDEN_SIZE: usize = 2048;
const VOCABULARY: usize = 151936;
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

/// Where the model's tensors are, and why a caller cannot run without them.
#[derive(Debug)]
pub enum QwenRefusal {
    /// The directory the tensors live in is not there or lacks one.
    Weights(String),
    /// A step the run needed did not prepare.
    Step(ServeRefusal),
}

impl std::fmt::Display for QwenRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QwenRefusal::Weights(what) => write!(formatter, "the weights are not readable: {what}"),
            QwenRefusal::Step(refusal) => write!(formatter, "a step did not prepare: {refusal}"),
        }
    }
}

/// What one generation produced, and what it cost.
pub struct Generated {
    /// The starting token, then every token the loop chose.
    pub tokens: Vec<usize>,
    /// How long each forward asked the host to wait.
    pub per_token: Vec<Duration>,
}

/// The cache a layer keeps: one key and one value per position seen.
#[derive(Default)]
struct Cache {
    keys: Vec<f32>,
    values: Vec<f32>,
}

/// The directory the tensors live in, or a refusal naming what a caller should set.
fn root() -> Result<PathBuf, QwenRefusal> {
    if let Some(found) = std::env::var_os("LICHEN_MODEL_DIR") {
        return Ok(PathBuf::from(found));
    }
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.model"))
}

/// One flat file, read as f32 once.
fn flat(name: &str) -> Result<Vec<f32>, QwenRefusal> {
    let path = root()?.join(format!("{name}.bin"));
    let bytes = std::fs::read(&path)
        .map_err(|error| QwenRefusal::Weights(format!("{}: {error}", path.display())))?;
    Ok(bytes
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes(word.try_into().unwrap_or_default()))
        .collect())
}

/// The steps a forward uses, prepared once per session.
struct Steps<'session> {
    norm: crate::Prepared<'session>,
    projection: crate::Prepared<'session>,
    rotation: crate::Prepared<'session>,
    attention: crate::Prepared<'session>,
    product: crate::Prepared<'session>,
}

impl<'session> Steps<'session> {
    fn prepare(session: &'session Session) -> Result<Self, QwenRefusal> {
        let prepared = |source: &str| session.prepare(source).map_err(QwenRefusal::Step);
        Ok(Self {
            norm: prepared(RMS_NORM_STEP)?,
            projection: prepared(MATVEC_STEP)?,
            rotation: prepared(ROPE_STEP)?,
            attention: prepared(ATTEND_STEP)?,
            product: prepared(SWIGLU_STEP)?,
        })
    }

    fn normalised(
        &self,
        session: &Session,
        values: &[f32],
        gain: &[f32],
        rows: i64,
        width: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let stepped = self
            .norm
            .step(vec![
                RunArgument::Count(width * rows),
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(gain),
            ])
            .map_err(QwenRefusal::Step)?;
        answered(session, &stepped)
    }

    fn projected(
        &self,
        session: &Session,
        values: &[f32],
        weight: &[f32],
        rows: i64,
        width: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let stepped = self
            .projection
            .step(vec![
                RunArgument::Count(rows),
                RunArgument::Count(width),
                floats(values),
                floats(weight),
            ])
            .map_err(QwenRefusal::Step)?;
        answered(session, &stepped)
    }

    fn rotated(
        &self,
        session: &Session,
        values: &[f32],
        position: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let stepped = self
            .rotation
            .step(vec![
                RunArgument::Count(values.len() as i64),
                RunArgument::Count(position),
                floats(values),
            ])
            .map_err(QwenRefusal::Step)?;
        answered(session, &stepped)
    }

    fn mixed(&self, session: &Session, gate: &[f32], up: &[f32]) -> Result<Vec<f32>, QwenRefusal> {
        let stepped = self
            .product
            .step(vec![
                RunArgument::Count(INTERMEDIATE),
                floats(gate),
                floats(up),
            ])
            .map_err(QwenRefusal::Step)?;
        answered(session, &stepped)
    }

    fn attended(
        &self,
        session: &Session,
        query: &[f32],
        keys: &[f32],
        values: &[f32],
    ) -> Result<Vec<f32>, QwenRefusal> {
        let stepped = self
            .attention
            .step(vec![
                RunArgument::Count(HEADS * 2),
                floats(query),
                floats(keys),
                floats(values),
            ])
            .map_err(QwenRefusal::Step)?;
        answered(session, &stepped)
    }
}

/// A `Float` buffer argument of `values`.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: lichen_kernel_ir::ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

/// The one resident a step answered with, read and released.
fn answered(session: &Session, stepped: &crate::Stepped) -> Result<Vec<f32>, QwenRefusal> {
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        return Err(QwenRefusal::Weights(
            "a step answered with something other than one buffer".to_string(),
        ));
    };
    let values = session.fetch(*resident).map_err(QwenRefusal::Step)?;
    session.release(*resident);
    match values {
        lichen_kernel_ir::ScalarData::Float(values) => Ok(values),
        other => Err(QwenRefusal::Weights(format!(
            "a step answered {other:?} where one float buffer was expected"
        ))),
    }
}

/// The model, ready to generate from a session.
///
/// # Invariant
/// The weights are **streamed a layer at a time**: the whole model is 8.13 GB at f32 and the card is
/// smaller, so a forward reads the layer it is about to use and drops it. That is why a token costs the
/// bytes it moves rather than the arithmetic it does.
pub struct Qwen<'session> {
    session: &'session Session,
    steps: Steps<'session>,
    final_norm: Vec<f32>,
    embedding: Vec<f32>,
}

impl<'session> Qwen<'session> {
    /// Read the tensors a forward needs in every token, and prepare the steps.
    pub fn open(session: &'session Session) -> Result<Self, QwenRefusal> {
        Ok(Self {
            session,
            steps: Steps::prepare(session)?,
            final_norm: flat("final_norm")?,
            embedding: flat("layer0_embed")?,
        })
    }

    /// Generate `count` tokens greedily after `start`, keeping the cache the loop fills.
    pub fn generate(&self, start: usize, count: usize) -> Result<Generated, QwenRefusal> {
        let mut caches: Vec<Cache> = (0..LAYERS).map(|_| Cache::default()).collect();
        let mut tokens = vec![start];
        let mut per_token = Vec::new();
        let mut token = start;
        for position in 0..count {
            let started = Instant::now();
            let mut hidden =
                self.embedding[token * HIDDEN_SIZE..(token + 1) * HIDDEN_SIZE].to_vec();
            for layer in 0..LAYERS {
                hidden = self.layer(layer, hidden, &mut caches[layer], position as i64)?;
            }
            let normalised =
                self.steps
                    .normalised(self.session, &hidden, &self.final_norm, 1, HIDDEN as i64)?;
            let logits = self.steps.projected(
                self.session,
                &normalised,
                &self.embedding,
                VOCABULARY as i64,
                HIDDEN as i64,
            )?;
            token = argmax(&logits);
            tokens.push(token);
            per_token.push(started.elapsed());
        }
        Ok(Generated { tokens, per_token })
    }

    /// One layer, at one position, with the cache the position needs.
    fn layer(
        &self,
        layer: usize,
        mut hidden: Vec<f32>,
        cache: &mut Cache,
        position: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let weights = flat(&format!("layer{layer:02}"))?;
        let session = self.session;
        let steps = &self.steps;
        let normed = steps.normalised(
            session,
            &hidden,
            &weights[GAIN..GAIN + HIDDEN_SIZE],
            1,
            HIDDEN as i64,
        )?;
        let key = steps.projected(
            session,
            &normed,
            &weights[K..K + 1024 * 2048],
            KEY_HEADS * HEAD,
            HIDDEN as i64,
        )?;
        let key = steps.normalised(
            session,
            &key,
            &weights[K_NORM..K_NORM + 128],
            KEY_HEADS,
            HEAD,
        )?;
        let key = steps.rotated(session, &key, position)?;
        let value = steps.projected(
            session,
            &normed,
            &weights[V..V + 1024 * 2048],
            KEY_HEADS * HEAD,
            HIDDEN as i64,
        )?;
        cache.keys.extend_from_slice(&key);
        cache.values.extend_from_slice(&value);
        let attended = if position == 0 {
            // One position exists, so each query head takes its key head's value.
            (0..HIDDEN_SIZE)
                .map(|index| {
                    value[(index / HEAD as usize / 2) * HEAD as usize + index % HEAD as usize]
                })
                .collect::<Vec<f32>>()
        } else {
            let query = steps.projected(
                session,
                &normed,
                &weights[Q..Q + 2048 * 2048],
                HEADS * HEAD,
                HIDDEN as i64,
            )?;
            let query =
                steps.normalised(session, &query, &weights[Q_NORM..Q_NORM + 128], HEADS, HEAD)?;
            let query = steps.rotated(session, &query, position)?;
            steps.attended(session, &query, &cache.keys, &cache.values)?
        };
        let out = steps.projected(
            session,
            &attended,
            &weights[O..O + 2048 * 2048],
            HIDDEN as i64,
            HIDDEN as i64,
        )?;
        hidden = hidden
            .iter()
            .zip(&out)
            .map(|(left, right)| left + right)
            .collect();
        let prepared = steps.normalised(
            session,
            &hidden,
            &weights[POST..POST + HIDDEN_SIZE],
            1,
            HIDDEN as i64,
        )?;
        let gate = steps.projected(
            session,
            &prepared,
            &weights[GATE..GATE + 6144 * 2048],
            INTERMEDIATE,
            HIDDEN as i64,
        )?;
        let up = steps.projected(
            session,
            &prepared,
            &weights[UP..UP + 6144 * 2048],
            INTERMEDIATE,
            HIDDEN as i64,
        )?;
        let mixed = steps.mixed(session, &gate, &up)?;
        let down = steps.projected(
            session,
            &mixed,
            &weights[DOWN..DOWN + 2048 * 6144],
            HIDDEN as i64,
            INTERMEDIATE,
        )?;
        Ok(hidden
            .iter()
            .zip(&down)
            .map(|(left, right)| left + right)
            .collect())
    }
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
