//! Qwen3-1.7B end to end: the weights a layer at a time, the cache, and the tokens.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};

use crate::{
    ATTEND_CACHE_STEP, ATTEND_ROWS_STEP, HIDDEN, MATVEC_BATCH_STEP, RMS_NORM_STEP, ROPE_ROWS_STEP,
    ROPE_STEP, SWIGLU_STEP, ServeRefusal, Session,
};

/// The model's shape, and the flat file's order — the contract with `scripts/dump_bf16.py`.
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
///
/// # Invariant
/// The halves are **borrowed** from the copy the model keeps: a layer is 96 MB and a token reads
/// twenty-eight of them, so cloning one here costs 2.7 GB of `memcpy` a token, which was measured at half
/// of a layer's time.
enum Tensors<'a> {
    /// The f32 dumps, four bytes an element, read for this layer alone when the model streams them.
    Wide(Vec<f32>),
    /// The checkpoint's halves, two bytes an element, unconverted, one shared slice a tensor.
    Narrowed(&'a [std::sync::Arc<[u8]>]),
}

/// One layer's bytes as one shared slice a tensor, in the order `SHAPES` names them.
///
/// # Invariant
/// The split copies the bytes once per load and never per token: a dispatch is handed a reference, so a
/// weight a token reads is never copied again.
fn split_layer(bytes: &[u8], width: Width) -> Vec<std::sync::Arc<[u8]>> {
    SHAPES
        .iter()
        .enumerate()
        .map(|(index, (offset, length))| {
            let start = offset * width.bytes();
            let end = (offset + length) * width.bytes();
            std::sync::Arc::from(&bytes[start..end])
        })
        .collect()
}

/// One layer's bytes as one shared slice a tensor, every projection turned the way the held path reads it.
fn held_layer(bytes: &[u8], width: Width) -> Vec<std::sync::Arc<[u8]>> {
    SHAPES
        .iter()
        .enumerate()
        .map(|(index, (offset, length))| {
            let start = offset * width.bytes();
            let end = (offset + length) * width.bytes();
            match COLUMN_MAJOR[index] {
                Some((rows, columns)) => {
                    std::sync::Arc::from(by_column(&bytes[start..end], rows, columns, width))
                }
                None => std::sync::Arc::from(&bytes[start..end]),
            }
        })
        .collect()
}

/// Every layer tensor a projection reads, and the shape it is stored in.
///
/// # Invariant
/// A projection reads its weight a row an output, so a lane's neighbours read addresses a whole row apart.
/// These are stored by column instead, paid for at load: the head is the one projection read by row, because
/// `embed_row` reads it a row at a time and 593 MB is not worth paying twice.
const COLUMN_MAJOR: [Option<(usize, usize)>; TENSORS] = [
    None,
    Some((2048, 2048)),
    Some((1024, 2048)),
    Some((1024, 2048)),
    None,
    None,
    Some((2048, 2048)),
    None,
    Some((6144, 2048)),
    Some((6144, 2048)),
    Some((2048, 6144)),
];

/// `bytes`, a `rows` by `columns` matrix, read by column instead of by row.
///
/// # Invariant
/// An element keeps its width and its bytes, and the result is `columns` rows of `rows`, so a read at
/// `column * rows + row` names what `row * columns + column` named before.
fn by_column(bytes: &[u8], rows: usize, columns: usize, width: Width) -> Vec<u8> {
    let element = width.bytes();
    let mut turned = vec![0u8; bytes.len()];
    for row in 0..rows {
        for column in 0..columns {
            let from = (row * columns + column) * element;
            let to = (column * rows + row) * element;
            turned[to..to + element].copy_from_slice(&bytes[from..from + element]);
        }
    }
    turned
}

/// A layer's f32 tensors, every one a projection reads turned the way the kernel reads it.
///
/// # Invariant
/// The narrow path stores a layer's projections by column, so this path turns them too: two paths that
/// disagree about a layout disagree about the answer.
fn wide_layer(name: &str, held: bool) -> Result<Vec<f32>, QwenRefusal> {
    let values = flat(name)?;
    if !held {
        return Ok(values);
    }
    let mut turned = values.clone();
    for (index, (offset, length)) in SHAPES.iter().enumerate() {
        if let Some((rows, columns)) = COLUMN_MAJOR[index] {
            assert_eq!(rows * columns, *length);
            for row in 0..rows {
                for column in 0..columns {
                    turned[offset + column * rows + row] = values[offset + row * columns + column];
                }
            }
        }
    }
    Ok(turned)
}

/// The numbers a run of halves holds, widened the way the device widens them.
fn halves_to_floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|half| f32::from_bits(u32::from(u16::from_le_bytes([half[0], half[1]])) << 16))
        .collect()
}

/// `count` rows of `width` elements each, as a count.
///
/// # Invariant
/// The multiplication is here rather than at each site, so a site cannot multiply the wrong two numbers.
fn rows(count: i64, width: i64) -> i64 {
    count * width
}

/// One request inside a batch: where it stands and what it has answered.
struct Request {
    /// What the caller knows it by, which a retirement does not change.
    id: usize,
    /// The state its next token is read from.
    hidden: Vec<f32>,
    /// The position that state is at.
    position: i64,
    /// The tokens it has answered with.
    answer: Vec<usize>,
    /// How many it may answer with.
    limit: usize,
    /// The prompt, whose tokens feed the first positions a step at a time.
    prompt: Vec<usize>,
    /// How many prompt tokens are in the cache already, which is when it answers.
    filled: usize,
    /// The token the next step feeds this row, chosen once its prompt is in.
    pending: Option<usize>,
    /// How its tokens are chosen, which a request states for itself.
    sampling: crate::Sampling,
    /// Its draw, which moves only when its own token is chosen.
    state: u64,
    /// How many of its rivals a client asked to see beside each token.
    rivals: usize,
}

/// Several requests advanced together, each at its own position.
///
/// # Invariant
/// The caches are held layer by layer across the live requests, so a layer is one contiguous run and one
/// step is one dispatch a layer. A request's position travels with its row, which is what lets a batch hold
/// requests of different lengths; retiring one moves the last live request into its place and keeps its id,
/// so a caller that held one holds the same request.
pub struct Batched<'model, 'session> {
    model: &'model Qwen<'session>,
    caches: Vec<Vec<Cache>>,
    live: Vec<Request>,
    next: usize,
}

impl<'model, 'session> Batched<'model, 'session> {
    /// A batch over `model`, empty, whose tokens are chosen under `sampling`.
    pub fn new(model: &'model Qwen<'session>) -> Self {
        Self {
            model,
            caches: (0..LAYERS).map(|_| Vec::new()).collect(),
            live: Vec::new(),
            next: 0,
        }
    }

    /// Fill `prompt`'s cache and hold it for at most `limit` answers, answering its identifier.
    pub fn admit(
        &mut self,
        prompt: &[usize],
        limit: usize,
        sampling: crate::Sampling,
        rivals: usize,
    ) -> Result<usize, QwenRefusal> {
        if prompt.is_empty() {
            return Err(QwenRefusal::Weights("a request with no prompt".into()));
        }
        for caches in self.caches.iter_mut() {
            caches.push(Cache::default());
        }
        let id = self.next;
        self.next += 1;
        self.live.push(Request {
            id,
            hidden: Vec::new(),
            position: 0,
            answer: Vec::new(),
            limit,
            prompt: prompt.to_vec(),
            filled: 0,
            pending: None,
            sampling,
            state: sampling.seed ^ (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            rivals,
        });
        Ok(id)
    }

    /// How many requests are live.
    pub fn live(&self) -> usize {
        self.live.len()
    }

    /// One step: every live row advances one position, and the rows whose prompt is in answer a token.
    ///
    /// # Invariant
    /// A filling row feeds its next prompt token and an answering row the token it chose last, so one layer
    /// pass carries both and a long prompt fills beside the requests already answering. A row answers only
    /// once its whole prompt is in the cache, and its state is the one at its own position.
    pub fn step(&mut self) -> Result<Vec<(usize, crate::sampling::Chosen)>, QwenRefusal> {
        let live = self.live.len();
        if live == 0 {
            return Ok(Vec::new());
        }
        let mut fed: Vec<f32> = Vec::with_capacity(live * HIDDEN_SIZE);
        for request in self.live.iter() {
            let token = match request.pending {
                Some(token) => token,
                None => request.prompt[request.filled],
            };
            fed.extend(self.model.embed_row(token));
        }
        let positions: Vec<i64> = self.live.iter().map(|request| request.position).collect();
        let mut state = fed;
        for layer in 0..LAYERS {
            state = self
                .model
                .layer_batch(layer, &state, &mut self.caches[layer], &positions)?;
        }
        for (sequence, request) in self.live.iter_mut().enumerate() {
            request.position += 1;
            if request.filled < request.prompt.len() {
                request.filled += 1;
            }
            request.pending = None;
            request.hidden = state[sequence * HIDDEN_SIZE..(sequence + 1) * HIDDEN_SIZE].to_vec();
        }
        let answering: Vec<usize> = (0..live)
            .filter(|sequence| self.live[*sequence].filled >= self.live[*sequence].prompt.len())
            .collect();
        if answering.is_empty() {
            return Ok(Vec::new());
        }
        let joined: Vec<f32> = answering
            .iter()
            .flat_map(|sequence| self.live[*sequence].hidden.iter().copied())
            .collect();
        let normalised = self.model.steps.normalised(
            self.model.session,
            &joined,
            self.model.steps.payload(&self.model.final_norm),
            answering.len() as i64,
            HIDDEN as i64,
        )?;
        let logits = self.model.steps.head(
            self.model.session,
            &normalised,
            self.model.head_weight(),
            answering.len() as i64,
        )?;
        let mut answered = Vec::with_capacity(answering.len());
        for (at, sequence) in answering.iter().copied().enumerate() {
            let row = &logits[at * VOCABULARY..(at + 1) * VOCABULARY];
            let sampling = self.live[sequence].sampling;
            let mut state = self.live[sequence].state;
            let rivals = self.live[sequence].rivals;
            let chosen = crate::sampling::choose_with(
                row,
                &sampling,
                &mut || crate::sampling::next(&mut state),
                rivals,
            );
            let request = &mut self.live[sequence];
            request.state = state;
            request.answer.push(chosen.token);
            request.pending = Some(chosen.token);
            answered.push((request.id, chosen));
        }
        Ok(answered)
    }

    /// What request `id` has answered so far.
    pub fn answer(&self, id: usize) -> Option<&[usize]> {
        self.live
            .iter()
            .find(|request| request.id == id)
            .map(|request| request.answer.as_slice())
    }

    /// Whether request `id` has answered as many tokens as it may, which it is not live to do.
    pub fn done(&self, id: usize) -> bool {
        match self.live.iter().find(|request| request.id == id) {
            Some(request) => request.answer.len() >= request.limit,
            None => true,
        }
    }

    /// Let request `id` go, moving the last live one into its place.
    pub fn retire(&mut self, id: usize) {
        let Some(at) = self.live.iter().position(|request| request.id == id) else {
            return;
        };
        for caches in self.caches.iter_mut() {
            caches.swap_remove(at);
        }
        self.live.swap_remove(at);
    }
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
    /// Nanoseconds spent inside the stages, which is what the device was asked for.
    stage_nanos: std::cell::Cell<u64>,
    norm: crate::Prepared<'session>,
    projection: crate::Prepared<'session>,
    /// The projection for a layer read from the file, whose weight is still stored by row.
    streamed: crate::Prepared<'session>,
    /// Whether the layers are held, which is what decides which of the two reads them.
    held: std::cell::Cell<bool>,
    /// The head's projection, which reads a weight stored by row.
    head: crate::Prepared<'session>,
    rotation: crate::Prepared<'session>,
    /// The rotation over a whole batch's rows, its positions in a buffer.
    rotation_rows: crate::Prepared<'session>,
    /// The attention over a batch whose rows share a position, the caches packed.
    attention_rows: crate::Prepared<'session>,
    attention: crate::Prepared<'session>,
    product: crate::Prepared<'session>,
    /// The width every payload this step hands over is stored at.
    width: Width,
}

impl<'session> Steps<'session> {
    /// Add the time a stage took, so a caller can tell the device's work from the work around it.
    fn add_stage(&self, started: std::time::Instant) {
        self.stage_nanos
            .set(self.stage_nanos.get() + started.elapsed().as_nanos() as u64);
    }

    /// How long the stages have held the device, which a caller resets by noting it before a step.
    pub fn stage_nanos(&self) -> u64 {
        self.stage_nanos.get()
    }

    fn prepare(session: &'session Session, width: Width) -> Result<Self, QwenRefusal> {
        let prepared = |source: &str| session.prepare(source).map_err(QwenRefusal::Step);
        Ok(Self {
            norm: prepared(RMS_NORM_STEP)?,
            projection: match width {
                Width::Wide => prepared(MATVEC_BATCH_STEP)?,
                Width::Narrowed => crate::narrowed_projection(session)?,
            },
            streamed: match width {
                Width::Wide => prepared(crate::model::MATVEC_ROW_STEP)?,
                Width::Narrowed => crate::narrowed(session, crate::model::MATVEC_ROW_STEP)?,
            },
            held: std::cell::Cell::new(false),
            stage_nanos: std::cell::Cell::new(0),
            head: match width {
                Width::Wide => prepared(crate::model::MATVEC_ROW_STEP)?,
                Width::Narrowed => crate::narrowed(session, crate::model::MATVEC_ROW_STEP)?,
            },
            rotation: prepared(ROPE_STEP)?,
            rotation_rows: prepared(ROPE_ROWS_STEP)?,
            attention_rows: prepared(ATTEND_ROWS_STEP)?,
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
        let resident = self.normalised_from(session, self.payload(values), gain, rows, width)?;
        fetched(session, resident)
    }

    /// The same, from an argument a previous step left on the device, answering with its resident.
    fn normalised_from(
        &self,
        session: &Session,
        input: RunArgument,
        gain: RunArgument,
        rows: i64,
        width: i64,
    ) -> Result<ResidentBuffer, QwenRefusal> {
        let started = std::time::Instant::now();
        let stepped = self
            .norm
            .step(vec![
                RunArgument::Count(width * rows),
                RunArgument::Count(rows),
                RunArgument::Count(width),
                input,
                gain,
            ])
            .map_err(QwenRefusal::Step)?;
        self.add_stage(started);
        resident(&stepped)
    }

    /// The projection of one or more input rows, `per` outputs a row.
    ///
    /// # Invariant
    /// `per` is the outputs one input row produces, which is also the weight's own row count: an
    /// output reads the input row `rows / per` and the weight row `rows % per`, so every sequence
    /// shares the one weight. A single input row passes its own output count, and both are then
    /// row zero's input and the output's own weight.
    fn projected(
        &self,
        session: &Session,
        values: &[f32],
        weight: RunArgument,
        rows: i64,
        width: i64,
        per: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let resident =
            self.projected_from(session, self.payload(values), weight, rows, width, per)?;
        fetched(session, resident)
    }

    /// The same, from an argument a previous step left on the device, answering with its resident.
    fn projected_from(
        &self,
        session: &Session,
        input: RunArgument,
        weight: RunArgument,
        rows: i64,
        width: i64,
        per: i64,
    ) -> Result<ResidentBuffer, QwenRefusal> {
        let projection = if self.held.get() {
            &self.projection
        } else {
            &self.streamed
        };
        let started = std::time::Instant::now();
        let stepped = projection
            .step(vec![
                RunArgument::Count(rows),
                RunArgument::Count(width),
                RunArgument::Count(per),
                input,
                weight,
            ])
            .map_err(QwenRefusal::Step)?;
        self.add_stage(started);
        resident(&stepped)
    }

    /// The head's projection over `count` rows of values, read from a weight stored by row.
    ///
    /// # Invariant
    /// The layers are stored by column and this weight is not, so it runs the row-major program: the same
    /// program would read a different matrix of it.
    fn head(
        &self,
        session: &Session,
        values: &[f32],
        weight: RunArgument,
        count: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let started = std::time::Instant::now();
        let stepped = self
            .head
            .step(vec![
                RunArgument::Count(rows(count, VOCABULARY as i64)),
                RunArgument::Count(HIDDEN as i64),
                RunArgument::Count(VOCABULARY as i64),
                self.payload(values),
                weight,
            ])
            .map_err(QwenRefusal::Step)?;
        self.add_stage(started);
        answered(session, &stepped)
    }

    fn rotated(
        &self,
        session: &Session,
        values: &[f32],
        position: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let started = std::time::Instant::now();
        let stepped = self
            .rotation
            .step(vec![
                RunArgument::Count(values.len() as i64),
                RunArgument::Count(position),
                self.payload(values),
            ])
            .map_err(QwenRefusal::Step)?;
        self.add_stage(started);
        answered(session, &stepped)
    }

    /// The rotation of every row in `values`, its `positions` one a row.
    ///
    /// # Invariant
    /// `row_len` is the values a row turns; both buffers ride as floats because a dispatch reads
    /// its buffers at one class, and a position has to be exact.
    fn rotated_rows(
        &self,
        session: &Session,
        values: &[f32],
        positions: &[f32],
        row_len: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let started = std::time::Instant::now();
        let stepped = self
            .rotation_rows
            .step(vec![
                RunArgument::Count(values.len() as i64),
                RunArgument::Count(row_len),
                floats(values),
                floats(positions),
            ])
            .map_err(QwenRefusal::Step)?;
        self.add_stage(started);
        answered(session, &stepped)
    }

    fn mixed(
        &self,
        session: &Session,
        gate: &[f32],
        up: &[f32],
        count: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let resident = self.mixed_from(session, self.payload(gate), self.payload(up), count)?;
        fetched(session, resident)
    }

    /// The same, from arguments a previous step left on the device, answering with its resident.
    fn mixed_from(
        &self,
        session: &Session,
        gate: RunArgument,
        up: RunArgument,
        count: i64,
    ) -> Result<ResidentBuffer, QwenRefusal> {
        let started = std::time::Instant::now();
        let stepped = self
            .product
            .step(vec![RunArgument::Count(count), gate, up])
            .map_err(QwenRefusal::Step)?;
        self.add_stage(started);
        resident(&stepped)
    }

    /// The attention over `rows` rows that share `p` positions, the caches packed row after row.
    ///
    /// # Invariant
    /// Every row holds `p` positions, so the packed key and value blocks are `p * 1024` elements
    /// and the query's rows are `2048`; a batch whose rows differ takes the per-sequence path.
    fn attended_rows(
        &self,
        session: &Session,
        query: &[f32],
        keys: &[f32],
        values: &[f32],
        p: i64,
        rows: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        let started = std::time::Instant::now();
        let stepped = self
            .attention_rows
            .step(vec![
                RunArgument::Count(rows * p * HEADS),
                RunArgument::Count(rows * HEADS),
                RunArgument::Count(rows * HEADS * HEAD),
                RunArgument::Count(p),
                self.payload(query),
                self.payload(keys),
                self.payload(values),
            ])
            .map_err(QwenRefusal::Step)?;
        self.add_stage(started);
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
        let started = std::time::Instant::now();
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
        self.add_stage(started);
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

/// The one resident a step answered with, left where the device wrote it.
fn resident(stepped: &crate::Stepped) -> Result<ResidentBuffer, QwenRefusal> {
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        return Err(QwenRefusal::Weights(
            "a step answered with something other than one buffer".to_string(),
        ));
    };
    Ok(*resident)
}

/// A resident read as floats and handed back, which is the one readback a stage costs.
fn fetched(session: &Session, resident: ResidentBuffer) -> Result<Vec<f32>, QwenRefusal> {
    let values = session.fetch(resident).map_err(QwenRefusal::Step)?;
    session.release(resident);
    match values {
        lichen_kernel_ir::ScalarData::Float(values) => Ok(values),
        other => Err(QwenRefusal::Weights(format!(
            "a step answered {other:?} where one float buffer was expected"
        ))),
    }
}

/// The one resident a step answered with, read and released.
fn answered(session: &Session, stepped: &crate::Stepped) -> Result<Vec<f32>, QwenRefusal> {
    fetched(session, resident(stepped)?)
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
    embedding_bytes: Option<std::sync::Arc<[u8]>>,
    /// The layers' bytes read once, in the order they are used, when the model holds them.
    ///
    /// # Invariant
    /// The page cache made a warm token 2.26 s cheaper than a cold one, which is the operating system
    /// doing this job for 3.2 GB of halves: holding them here is the same saving made deliberate rather
    /// than hoped for, and it is the one that needs no device memory.
    held: Vec<Vec<std::sync::Arc<[u8]>>>,
    /// The vocabulary head's weight, when it is on the device.
    head: Option<crate::ResidentBuffer>,
    /// The tensors held on the device, one entry per layer that does not stream.
    ///
    /// # Invariant
    /// A resident is uploaded once and handed to every later token: what a layer costs then is its
    /// dispatches and not its 96 MB, which is the difference between four and a half seconds and three.
    resident: Vec<Option<Vec<crate::ResidentBuffer>>>,
}

/// How many tensors a layer holds, in the flat file's order.
const TENSORS: usize = 11;

/// The eleven tensors' element offsets and lengths, in the flat file's order.
const SHAPES: [(usize, usize); TENSORS] = [
    (GAIN, HIDDEN_SIZE),
    (Q, 2048 * 2048),
    (K, 1024 * 2048),
    (V, 1024 * 2048),
    (Q_NORM, 128),
    (K_NORM, 128),
    (O, 2048 * 2048),
    (POST, HIDDEN_SIZE),
    (GATE, 6144 * 2048),
    (UP, 6144 * 2048),
    (DOWN, 2048 * 6144),
];

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
                    Some(std::sync::Arc::from(bytes)),
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
            held: Vec::new(),
            head: None,
            resident: (0..LAYERS).map(|_| None).collect(),
        })
    }

    /// Generate `count` tokens greedily after `start`, keeping the cache the loop fills.
    pub fn generate(&self, start: usize, count: usize) -> Result<Generated, QwenRefusal> {
        self.generate_with(start, count, |_| {})
    }

    /// Generate under a request's sampling, handing each token over as it is chosen.
    ///
    /// # Invariant
    /// A greedy request and a sampled one take the same path, so a temperature of zero cannot answer
    /// differently from the largest logit: the difference is the draw, which greedy never consults.
    pub fn generate_sampled<F: FnMut(usize)>(
        &self,
        start: usize,
        count: usize,
        sampling: crate::Sampling,
        on_token: F,
    ) -> Result<Generated, QwenRefusal> {
        self.generate_under(start, count, sampling, on_token)
    }

    /// Generate from a whole prompt, which is what a request hands over.
    ///
    /// # Invariant
    /// The prompt fills the cache before any token is chosen: every prompt token runs through the layers at
    /// its own position without sampling, and the last one's state is what the first answer comes from. A
    /// model started from the prompt's last token alone would answer as if the request were one word.
    pub fn generate_from<F: FnMut(usize)>(
        &self,
        prompt: &[usize],
        limit: usize,
        sampling: crate::Sampling,
        mut on_token: F,
    ) -> Result<Generated, QwenRefusal> {
        let mut caches: Vec<Cache> = (0..LAYERS).map(|_| Cache::default()).collect();
        let mut tokens: Vec<usize> = prompt.to_vec();
        let mut hidden = Vec::new();
        for (position, token) in prompt.iter().enumerate() {
            hidden = self.embed_row(*token);
            for layer in 0..LAYERS {
                hidden = self.layer(layer, hidden, &mut caches[layer], position as i64)?;
            }
        }
        let mut draw = crate::sampling::randomness(sampling.seed);
        let mut per_token = Vec::new();
        let start = prompt.len() as i64;
        let mut position = start;
        for step in 0..limit {
            let started = std::time::Instant::now();
            let normalised = self.steps.normalised(
                self.session,
                &hidden,
                self.steps.payload(&self.final_norm),
                1,
                HIDDEN as i64,
            )?;
            let logits = self
                .steps
                .head(self.session, &normalised, self.head_weight(), 1)?;
            let chosen = crate::sampling::choose(&logits, &sampling, &mut draw);
            tokens.push(chosen);
            per_token.push(started.elapsed());
            on_token(chosen);
            if step + 1 == limit {
                break;
            }
            hidden = self.embed_row(chosen);
            for layer in 0..LAYERS {
                hidden = self.layer(layer, hidden, &mut caches[layer], position)?;
            }
            position += 1;
        }
        Ok(Generated { tokens, per_token })
    }

    /// The row a token starts from, read out of whichever copy the model holds.
    fn embed_row(&self, token: usize) -> Vec<f32> {
        match &self.embedding_bytes {
            None => self.embedding[token * HIDDEN_SIZE..(token + 1) * HIDDEN_SIZE].to_vec(),
            Some(bytes) => {
                halves_to_floats(&bytes[token * HIDDEN_SIZE * 2..(token + 1) * HIDDEN_SIZE * 2])
            }
        }
    }

    /// Generate, handing each token to `on_token` as it is chosen.
    ///
    /// # Invariant
    /// The token is handed over **after** the forward that chose it and before the next one starts, so a
    /// caller printing it sees the output arrive at the rate the model produces it rather than at the rate
    /// the whole run finishes.
    pub fn generate_with<F: FnMut(usize)>(
        &self,
        start: usize,
        count: usize,
        on_token: F,
    ) -> Result<Generated, QwenRefusal> {
        self.generate_under(start, count, crate::Sampling::greedy(), on_token)
    }

    /// The loop both entries share.
    fn generate_under<F: FnMut(usize)>(
        &self,
        start: usize,
        count: usize,
        sampling: crate::Sampling,
        mut on_token: F,
    ) -> Result<Generated, QwenRefusal> {
        let mut draw = crate::sampling::randomness(sampling.seed);
        let mut caches: Vec<Cache> = (0..LAYERS).map(|_| Cache::default()).collect();
        let mut tokens = vec![start];
        let mut per_token = Vec::new();
        let mut token = start;
        for position in 0..count {
            let started = Instant::now();
            // The row the token starts from, read out of whichever copy the model holds.
            let row = self.embed_row(token);
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
            let logits = self.steps.head(self.session, &normalised, head, 1)?;
            token = crate::sampling::choose(&logits, &sampling, &mut draw);
            tokens.push(token);
            per_token.push(started.elapsed());
            on_token(token);
        }
        Ok(Generated { tokens, per_token })
    }

    /// Generate for several sequences at once, all starting at the same position.
    ///
    /// # Invariant
    /// Every step after the first starts from the row of the token it chose, never from the state that chose
    /// it: carrying the state forward answers the next position as if the model were its own input. The
    /// rows share a dispatch wherever the step names no position and no cache, and the batch pays the
    /// weights once rather than once a sequence.
    pub fn generate_batch(
        &self,
        starts: &[usize],
        count: usize,
    ) -> Result<Vec<Vec<usize>>, QwenRefusal> {
        let sequences = starts.len();
        if sequences == 0 {
            return Ok(Vec::new());
        }
        let mut caches: Vec<Vec<Cache>> = (0..LAYERS)
            .map(|_| (0..sequences).map(|_| Cache::default()).collect())
            .collect();
        let mut hidden: Vec<f32> = starts
            .iter()
            .flat_map(|start| self.embed_row(*start))
            .collect();
        let mut tokens: Vec<Vec<usize>> = starts.iter().map(|start| vec![*start]).collect();
        for position in 0..count {
            for layer in 0..LAYERS {
                let positions = vec![position as i64; sequences];
                hidden = self.layer_batch(layer, &hidden, &mut caches[layer], &positions)?;
            }
            let normalised = self.steps.normalised(
                self.session,
                &hidden,
                self.steps.payload(&self.final_norm),
                sequences as i64,
                HIDDEN as i64,
            )?;
            let logits = self.steps.head(
                self.session,
                &normalised,
                self.head_weight(),
                sequences as i64,
            )?;
            let mut next: Vec<f32> = Vec::with_capacity(sequences * HIDDEN_SIZE);
            for (sequence, chosen) in tokens.iter_mut().enumerate() {
                let row = &logits[sequence * VOCABULARY..(sequence + 1) * VOCABULARY];
                let token = crate::sampling::argmax(row);
                chosen.push(token);
                next.extend(self.embed_row(token));
            }
            hidden = next;
        }
        Ok(tokens)
    }

    /// Fill a prompt's cache and hand back the state its first answer is read from.
    ///
    /// # Invariant
    /// The prompt runs through every layer at its own position without producing a token, so the state
    /// returned is the one the position after the prompt is read at.
    fn filled(&self, prompt: &[usize]) -> Result<(Vec<Cache>, Vec<f32>, i64), QwenRefusal> {
        let mut caches: Vec<Cache> = (0..LAYERS).map(|_| Cache::default()).collect();
        let mut hidden = Vec::new();
        for (position, token) in prompt.iter().enumerate() {
            hidden = self.embed_row(*token);
            for layer in 0..LAYERS {
                hidden = self.layer(layer, hidden, &mut caches[layer], position as i64)?;
            }
        }
        Ok((caches, hidden, prompt.len() as i64))
    }

    /// Read every layer's bytes once, split into one shared slice a tensor, and keep them.
    ///
    /// # Invariant
    /// The split is what a dispatch is handed, so a token hands over eleven references rather than copying
    /// a layer: the bytes are the width's own and nothing converts them on the way in or out, and copying
    /// them once here is what keeps a token from copying them every time.
    pub fn hold_in_memory(&mut self) -> Result<(), QwenRefusal> {
        self.steps.held.set(true);
        for layer in 0..LAYERS {
            let bytes = flat_bytes(&format!("layer{layer:02}"), self.width)?;
            self.held.push(held_layer(&bytes, self.width));
        }
        Ok(())
    }

    /// Hold the first `layers` layers' tensors on the device instead of streaming them.
    ///
    /// # Invariant
    /// A layer held here is read once for the life of the model: every token after the first hands the
    /// residents straight to the dispatches. What it costs is device memory, so how many layers a card can
    /// hold is the caller's to decide: what a card can hold is measured (`resident_token`), not assumed.
    pub fn reside(&mut self, layers: usize) -> Result<(), QwenRefusal> {
        for layer in 0..layers.min(LAYERS) {
            let read: Vec<std::sync::Arc<[u8]>>;
            let tensors = match self.width {
                Width::Wide => Tensors::Wide(wide_layer(
                    &format!("layer{layer:02}"),
                    !self.held.is_empty(),
                )?),
                Width::Narrowed => {
                    let bytes = flat_bytes(&format!("layer{layer:02}"), self.width)?;
                    // The placed bytes must be the layout the chosen program reads.
                    read = if self.held.is_empty() {
                        split_layer(&bytes, self.width)
                    } else {
                        held_layer(&bytes, self.width)
                    };
                    Tensors::Narrowed(&read)
                }
            };
            let mut held: Vec<crate::ResidentBuffer> = Vec::with_capacity(TENSORS);
            for (slot, (offset, length)) in SHAPES.iter().enumerate() {
                held.push(match &tensors {
                    Tensors::Wide(values) => self
                        .session
                        .upload(&values[*offset..*offset + *length])
                        .map_err(QwenRefusal::Step)?,
                    Tensors::Narrowed(tensors) => self
                        .session
                        .place(&tensors[slot], lichen_kernel_ir::ScalarClass::BFloat16)
                        .map_err(QwenRefusal::Step)?,
                });
            }
            self.resident[layer] = Some(held);
        }
        Ok(())
    }

    /// One tensor of a layer, from the device when it is held there and from the host when it is not.
    fn weight(
        &self,
        layer: usize,
        tensors: &Tensors,
        slot: usize,
        offset: usize,
        length: usize,
    ) -> RunArgument {
        match &self.resident[layer] {
            Some(held) => RunArgument::Resident(held[slot]),
            None => self.tensor(tensors, slot, offset, length),
        }
    }

    /// How long this model's stages have held the device, in nanoseconds.
    pub fn stage_nanos(&self) -> u64 {
        self.steps.stage_nanos()
    }

    /// The vocabulary head's weight, which is the embedding's own tensor.
    fn head_weight(&self) -> RunArgument {
        if let Some(head) = self.head {
            return RunArgument::Resident(head);
        }
        match &self.embedding_bytes {
            None => floats(&self.embedding),
            Some(bytes) => RunArgument::BufferShared {
                class: lichen_kernel_ir::ScalarClass::BFloat16,
                data: std::sync::Arc::clone(bytes),
            },
        }
    }

    /// Place the vocabulary head's weight, which is the embedding's own tensor and the largest one.
    ///
    /// # Invariant
    /// The head is read once a token and is 593 MB, so a token that streams it pays a staging buffer of the
    /// same size beside the weights already on the device: placed, it is one reference count.
    pub fn place_head(&mut self) -> Result<(), QwenRefusal> {
        let Some(bytes) = &self.embedding_bytes else {
            return Err(QwenRefusal::Weights(
                "the wide model's head is not a half-width tensor".to_string(),
            ));
        };
        let placed = self
            .session
            .place(bytes, lichen_kernel_ir::ScalarClass::BFloat16)
            .map_err(QwenRefusal::Step)?;
        self.head = Some(placed);
        Ok(())
    }

    /// The argument one tensor of a layer is handed over as.
    ///
    /// # Invariant
    /// A narrowed layer keeps the file's bytes and hands a **slice** of them on, so a weight is never
    /// converted; the offsets the layer reads are elements and the file is two bytes to each.
    fn tensor(
        &self,
        tensors: &Tensors<'_>,
        slot: usize,
        offset: usize,
        length: usize,
    ) -> RunArgument {
        match tensors {
            Tensors::Wide(values) => floats(&values[offset..offset + length]),
            Tensors::Narrowed(tensors) => RunArgument::BufferShared {
                class: lichen_kernel_ir::ScalarClass::BFloat16,
                data: std::sync::Arc::clone(&tensors[slot]),
            },
        }
    }

    /// One layer, at one position, with the cache the position needs.
    fn layer(
        &self,
        layer: usize,
        hidden: Vec<f32>,
        cache: &mut Cache,
        position: i64,
    ) -> Result<Vec<f32>, QwenRefusal> {
        self.layer_batch(layer, &hidden, std::slice::from_mut(cache), &[position])
    }

    /// One layer over `caches.len()` sequences, all at `position`.
    ///
    /// # Invariant
    /// A row's steps that do not name a position or a cache are independent, so they carry `n` rows in one
    /// dispatch; the rotation names one position and the attention one cache, so those loop per sequence.
    fn layer_batch(
        &self,
        layer: usize,
        hidden: &[f32],
        caches: &mut [Cache],
        positions: &[i64],
    ) -> Result<Vec<f32>, QwenRefusal> {
        let n = caches.len() as i64;
        let mut hidden: Vec<f32> = hidden.to_vec();
        let read: Vec<std::sync::Arc<[u8]>>;
        let tensors = match self.width {
            Width::Wide => Tensors::Wide(flat(&format!("layer{layer:02}"))?),
            Width::Narrowed => match self.held.get(layer) {
                Some(tensors) => Tensors::Narrowed(tensors),
                None => {
                    let bytes = flat_bytes(&format!("layer{layer:02}"), self.width)?;
                    read = split_layer(&bytes, self.width);
                    Tensors::Narrowed(&read)
                }
            },
        };
        let session = self.session;
        let steps = &self.steps;
        let normed = steps.normalised_from(
            session,
            steps.payload(&hidden),
            self.weight(layer, &tensors, 0, GAIN, HIDDEN_SIZE),
            n,
            HIDDEN as i64,
        )?;
        let key = steps.projected_from(
            session,
            RunArgument::Resident(normed),
            self.weight(layer, &tensors, 2, K, 1024 * 2048),
            rows(n, KEY_HEADS * HEAD),
            HIDDEN as i64,
            KEY_HEADS * HEAD,
        )?;
        let key = steps.normalised_from(
            session,
            RunArgument::Resident(key),
            self.weight(layer, &tensors, 5, K_NORM, 128),
            rows(n, KEY_HEADS),
            HEAD,
        )?;
        let key = fetched(session, key)?;
        let angles: Vec<f32> = positions.iter().map(|position| *position as f32).collect();
        let key = steps.rotated_rows(session, &key, &angles, (KEY_HEADS * HEAD) as i64)?;
        let value = steps.projected_from(
            session,
            RunArgument::Resident(normed),
            self.weight(layer, &tensors, 3, V, 1024 * 2048),
            rows(n, KEY_HEADS * HEAD),
            HIDDEN as i64,
            KEY_HEADS * HEAD,
        )?;
        let value = fetched(session, value)?;
        for (sequence, cache) in caches.iter_mut().enumerate() {
            let span = sequence * (KEY_HEADS * HEAD) as usize
                ..(sequence + 1) * (KEY_HEADS * HEAD) as usize;
            cache.keys.extend_from_slice(&key[span.clone()]);
            cache.values.extend_from_slice(&value[span]);
        }
        let query = steps.projected_from(
            session,
            RunArgument::Resident(normed),
            self.weight(layer, &tensors, 1, Q, 2048 * 2048),
            rows(n, HEADS * HEAD),
            HIDDEN as i64,
            HEADS * HEAD,
        )?;
        session.release(normed);
        let query = steps.normalised_from(
            session,
            RunArgument::Resident(query),
            self.weight(layer, &tensors, 4, Q_NORM, 128),
            rows(n, HEADS),
            HEAD,
        )?;
        let query = fetched(session, query)?;
        let query = steps.rotated_rows(session, &query, &angles, (HEADS * HEAD) as i64)?;
        // Rows at one position have one cache length: each group is one packed step.
        let mut groups: Vec<(i64, Vec<usize>)> = Vec::new();
        for (sequence, position) in positions.iter().enumerate() {
            match groups.iter_mut().find(|(seen, _)| seen == position) {
                Some((_, indices)) => indices.push(sequence),
                None => groups.push((*position, vec![sequence])),
            }
        }
        let mut attended: Vec<f32> = vec![0.0; caches.len() * HIDDEN_SIZE];
        for (_, indices) in &groups {
            let answer = if indices.len() == 1 {
                let sequence = indices[0];
                let span =
                    sequence * (HEADS * HEAD) as usize..(sequence + 1) * (HEADS * HEAD) as usize;
                steps.attended(
                    session,
                    &query[span],
                    &caches[sequence].keys,
                    &caches[sequence].values,
                )?
            } else {
                let keys: Vec<f32> = indices
                    .iter()
                    .flat_map(|sequence| caches[*sequence].keys.iter().copied())
                    .collect();
                let values: Vec<f32> = indices
                    .iter()
                    .flat_map(|sequence| caches[*sequence].values.iter().copied())
                    .collect();
                let queries: Vec<f32> = indices
                    .iter()
                    .flat_map(|sequence| {
                        let span = sequence * (HEADS * HEAD) as usize
                            ..(sequence + 1) * (HEADS * HEAD) as usize;
                        query[span].iter().copied()
                    })
                    .collect();
                let p = (keys.len() / (indices.len() * (KEY_HEADS * HEAD) as usize)) as i64;
                steps.attended_rows(session, &queries, &keys, &values, p, indices.len() as i64)?
            };
            for (at, sequence) in indices.iter().enumerate() {
                let span =
                    sequence * (HEADS * HEAD) as usize..(sequence + 1) * (HEADS * HEAD) as usize;
                attended[span].copy_from_slice(
                    &answer[at * (HEADS * HEAD) as usize..(at + 1) * (HEADS * HEAD) as usize],
                );
            }
        }
        let out = steps.projected(
            session,
            &attended,
            self.weight(layer, &tensors, 6, O, 2048 * 2048),
            rows(n, HIDDEN as i64),
            HIDDEN as i64,
            HIDDEN as i64,
        )?;
        hidden = hidden
            .iter()
            .zip(&out)
            .map(|(left, right)| left + right)
            .collect();
        let prepared = steps.normalised_from(
            session,
            steps.payload(&hidden),
            self.weight(layer, &tensors, 7, POST, HIDDEN_SIZE),
            n,
            HIDDEN as i64,
        )?;
        let gate = steps.projected_from(
            session,
            RunArgument::Resident(prepared),
            self.weight(layer, &tensors, 8, GATE, 6144 * 2048),
            rows(n, INTERMEDIATE),
            HIDDEN as i64,
            INTERMEDIATE,
        )?;
        let up = steps.projected_from(
            session,
            RunArgument::Resident(prepared),
            self.weight(layer, &tensors, 9, UP, 6144 * 2048),
            rows(n, INTERMEDIATE),
            HIDDEN as i64,
            INTERMEDIATE,
        )?;
        session.release(prepared);
        let mixed = steps.mixed_from(
            session,
            RunArgument::Resident(gate),
            RunArgument::Resident(up),
            rows(n, INTERMEDIATE),
        )?;
        session.release(gate);
        session.release(up);
        let down = steps.projected_from(
            session,
            RunArgument::Resident(mixed),
            self.weight(layer, &tensors, 10, DOWN, 2048 * 6144),
            rows(n, HIDDEN as i64),
            INTERMEDIATE,
            HIDDEN as i64,
        )?;
        session.release(mixed);
        let down = fetched(session, down)?;
        Ok(hidden
            .iter()
            .zip(&down)
            .map(|(left, right)| left + right)
            .collect())
    }
}
