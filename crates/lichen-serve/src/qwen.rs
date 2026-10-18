//! Qwen3-1.7B end to end: the weights a layer at a time, the cache, and the tokens.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lichen_compute::compute::graph::{RunArgument, RunResult};

use crate::{
    ATTEND_CACHE_STEP, HIDDEN, MATVEC_STEP, RMS_NORM_STEP, ROPE_STEP, SWIGLU_STEP, ServeRefusal,
    Session,
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

/// A layer's tensors, held at the width the model runs at.
enum Tensors {
    /// The f32 dumps, four bytes an element.
    Wide(Vec<f32>),
    /// The checkpoint's halves, two bytes an element, unconverted.
    Narrowed(Vec<u8>),
}

/// The numbers a run of halves holds, widened the way the device widens them.
fn halves_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|half| f32::from_bits(u32::from(u16::from_le_bytes([half[0], half[1]])) << 16))
        .collect()
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

/// The width a model runs at: the checkpoint's own halves, or the f32 dumps of them.
///
/// # Invariant
/// A narrowed model reads a `_bf16` file and hands its bytes on **unconverted**, so a layer costs its
/// read and not a conversion: the numbers are the checkpoint's own, at the width it states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    /// The f32 dumps, which the host stores four bytes an element.
    Wide,
    /// The checkpoint's `bf16` halves, two bytes an element, which fit in memory where f32 does not.
    Narrowed,
}

impl Width {
    /// The byte a file of this width spends on one element.
    fn bytes(self) -> usize {
        match self {
            Width::Wide => 4,
            Width::Narrowed => 2,
        }
    }
}

/// The bytes of one flat tensor, unconverted.
fn flat_bytes(name: &str, width: Width) -> Result<Vec<u8>, QwenRefusal> {
    let suffix = match width {
        Width::Wide => "",
        Width::Narrowed => "_bf16",
    };
    let path = root()?.join(format!("{name}{suffix}.bin"));
    std::fs::read(&path)
        .map_err(|error| QwenRefusal::Weights(format!("{}: {error}", path.display())))
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
    /// The width every payload this step hands over is stored at.
    width: Width,
}

impl<'session> Steps<'session> {
    fn prepare(session: &'session Session, width: Width) -> Result<Self, QwenRefusal> {
        let prepared = |source: &str| session.prepare(source).map_err(QwenRefusal::Step);
        Ok(Self {
            norm: prepared(RMS_NORM_STEP)?,
            projection: match width {
                Width::Wide => prepared(MATVEC_STEP)?,
                Width::Narrowed => crate::narrowed_projection(session)?,
            },
            rotation: prepared(ROPE_STEP)?,
            attention: prepared(ATTEND_CACHE_STEP)?,
            product: prepared(SWIGLU_STEP)?,
            width,
        })
    }

    /// A value the step reads, at the width this model runs at.
    ///
    /// # Invariant
    /// A narrowed model hands over the checkpoint's own halves, so a weight it holds is never converted
    /// on the way to the device; the values a layer computes are f32 here and become halves here.
    fn payload(&self, values: &[f32]) -> RunArgument {
        match self.width {
            Width::Wide => floats(values),
            Width::Narrowed => RunArgument::Buffer {
                class: lichen_kernel_ir::ScalarClass::BFloat16,
                data: values
                    .iter()
                    .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
                    .collect(),
            },
        }
    }

    fn normalised(
        &self,
        session: &Session,
        values: &[f32],
        gain: RunArgument,
        rows: i64,
        width: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let stepped = self
            .norm
            .step(vec![
                RunArgument::Count(width * rows),
                RunArgument::Count(rows),
                RunArgument::Count(width),
                self.payload(values),
                gain,
            ])
            .map_err(QwenRefusal::Step)?;
        answered(session, &stepped)
    }

    fn projected(
        &self,
        session: &Session,
        values: &[f32],
        weight: RunArgument,
        rows: i64,
        width: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let stepped = self
            .projection
            .step(vec![
                RunArgument::Count(rows),
                RunArgument::Count(width),
                self.payload(values),
                weight,
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
                self.payload(values),
            ])
            .map_err(QwenRefusal::Step)?;
        answered(session, &stepped)
    }

    fn mixed(&self, session: &Session, gate: &[f32], up: &[f32]) -> Result<Vec<f32>, QwenRefusal> {
        let stepped = self
            .product
            .step(vec![
                RunArgument::Count(INTERMEDIATE),
                self.payload(gate),
                self.payload(up),
            ])
            .map_err(QwenRefusal::Step)?;
        answered(session, &stepped)
    }

    /// The attention over the whole cache, whose length is the positions the keys hold.
    ///
    /// # Invariant
    /// One position is not a special case: the softmax over one weight is that weight, so the same
    /// dispatch answers the first token as the eleventh, and the query and the key are computed for
    /// both rather than skipped for the first.
    fn attended(
        &self,
        session: &Session,
        query: &[f32],
        keys: &[f32],
        values: &[f32],
    ) -> Result<Vec<f32>, QwenRefusal> {
        let positions = keys.len() / (KEY_HEADS as usize * HEAD as usize);
        let stepped = self
            .attention
            .step(vec![
                RunArgument::Count(HEADS * positions as i64),
                RunArgument::Count(positions as i64),
                self.payload(query),
                self.payload(keys),
                self.payload(values),
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
    /// What the weights are held and handed over at.
    width: Width,
    /// The embedding's own bytes, which the head reads whole and unconverted when narrowed.
    embedding_bytes: Option<Vec<u8>>,
}

impl<'session> Qwen<'session> {
    /// Read the tensors a forward needs in every token, and prepare the steps.
    pub fn open(session: &'session Session) -> Result<Self, QwenRefusal> {
        Self::open_with(session, Width::Wide)
    }

    /// Read the tensors at `width`: the f32 dumps, or the checkpoint's own halves.
    ///
    /// # Invariant
    /// A narrowed model keeps the embedding's bytes so its head hands them over unconverted, and reads
    /// the row it starts from out of those bytes rather than from a second copy of the tensor.
    pub fn open_with(session: &'session Session, width: Width) -> Result<Self, QwenRefusal> {
        let (final_norm, embedding, embedding_bytes) = match width {
            Width::Wide => (flat("final_norm")?, flat("layer0_embed")?, None),
            Width::Narrowed => {
                let bytes = flat_bytes("embed_tokens_weight", width)?;
                let norm = flat_bytes("norm_weight", width)?;
                (
                    halves_to_floats(&norm),
                    halves_to_floats(&bytes[..HIDDEN_SIZE * 2]),
                    Some(bytes),
                )
            }
        };
        Ok(Self {
            session,
            steps: Steps::prepare(session, width)?,
            final_norm,
            embedding,
            width,
            embedding_bytes,
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
            // The row the token starts from, read out of whichever copy the model holds.
            let row = match &self.embedding_bytes {
                None => self.embedding[token * HIDDEN_SIZE..(token + 1) * HIDDEN_SIZE].to_vec(),
                Some(bytes) => {
                    halves_to_floats(&bytes[token * HIDDEN_SIZE * 2..(token + 1) * HIDDEN_SIZE * 2])
                }
            };
            let mut hidden = row;
            for layer in 0..LAYERS {
                hidden = self.layer(layer, hidden, &mut caches[layer], position as i64)?;
            }
            let head = self.head_weight();
            let normalised = self.steps.normalised(
                self.session,
                &hidden,
                self.steps.payload(&self.final_norm),
                1,
                HIDDEN as i64,
            )?;
            let logits = self.steps.projected(
                self.session,
                &normalised,
                head,
                VOCABULARY as i64,
                HIDDEN as i64,
            )?;
            token = argmax(&logits);
            tokens.push(token);
            per_token.push(started.elapsed());
        }
        Ok(Generated { tokens, per_token })
    }

    /// The vocabulary head's weight, which is the embedding's own tensor.
    fn head_weight(&self) -> RunArgument {
        match &self.embedding_bytes {
            None => floats(&self.embedding),
            Some(bytes) => RunArgument::Buffer {
                class: lichen_kernel_ir::ScalarClass::BFloat16,
                data: bytes.clone(),
            },
        }
    }

    /// The argument one tensor of a layer is handed over as.
    ///
    /// # Invariant
    /// A narrowed layer keeps the file's bytes and hands a **slice** of them on, so a weight is never
    /// converted; the offsets the layer reads are elements and the file is two bytes to each.
    fn tensor(&self, tensors: &Tensors, offset: usize, length: usize) -> RunArgument {
        match tensors {
            Tensors::Wide(values) => floats(&values[offset..offset + length]),
            Tensors::Narrowed(bytes) => {
                let start = offset * self.width.bytes();
                let end = (offset + length) * self.width.bytes();
                RunArgument::Buffer {
                    class: lichen_kernel_ir::ScalarClass::BFloat16,
                    data: bytes[start..end].to_vec(),
                }
            }
        }
    }

    /// One layer, at one position, with the cache the position needs.
    fn layer(
        &self,
        layer: usize,
        mut hidden: Vec<f32>,
        cache: &mut Cache,
        position: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let tensors = match self.width {
            Width::Wide => Tensors::Wide(flat(&format!("layer{layer:02}"))?),
            Width::Narrowed => {
                Tensors::Narrowed(flat_bytes(&format!("layer{layer:02}"), self.width)?)
            }
        };
        let session = self.session;
        let steps = &self.steps;
        let normed = steps.normalised(
            session,
            &hidden,
            self.tensor(&tensors, GAIN, HIDDEN_SIZE),
            1,
            HIDDEN as i64,
        )?;
        let key = steps.projected(
            session,
            &normed,
            self.tensor(&tensors, K, 1024 * 2048),
            KEY_HEADS * HEAD,
            HIDDEN as i64,
        )?;
        let key = steps.normalised(
            session,
            &key,
            self.tensor(&tensors, K_NORM, 128),
            KEY_HEADS,
            HEAD,
        )?;
        let key = steps.rotated(session, &key, position)?;
        let value = steps.projected(
            session,
            &normed,
            self.tensor(&tensors, V, 1024 * 2048),
            KEY_HEADS * HEAD,
            HIDDEN as i64,
        )?;
        cache.keys.extend_from_slice(&key);
        cache.values.extend_from_slice(&value);
        let query = steps.projected(
            session,
            &normed,
            self.tensor(&tensors, Q, 2048 * 2048),
            HEADS * HEAD,
            HIDDEN as i64,
        )?;
        let query = steps.normalised(
            session,
            &query,
            self.tensor(&tensors, Q_NORM, 128),
            HEADS,
            HEAD,
        )?;
        let query = steps.rotated(session, &query, position)?;
        let attended = steps.attended(session, &query, &cache.keys, &cache.values)?;
        let out = steps.projected(
            session,
            &attended,
            self.tensor(&tensors, O, 2048 * 2048),
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
            self.tensor(&tensors, POST, HIDDEN_SIZE),
            1,
            HIDDEN as i64,
        )?;
        let gate = steps.projected(
            session,
            &prepared,
            self.tensor(&tensors, GATE, 6144 * 2048),
            INTERMEDIATE,
            HIDDEN as i64,
        )?;
        let up = steps.projected(
            session,
            &prepared,
            self.tensor(&tensors, UP, 6144 * 2048),
            INTERMEDIATE,
            HIDDEN as i64,
        )?;
        let mixed = steps.mixed(session, &gate, &up)?;
        let down = steps.projected(
            session,
            &mixed,
            self.tensor(&tensors, DOWN, 2048 * 6144),
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
