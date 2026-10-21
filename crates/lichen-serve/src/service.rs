//! A served batch: the sequences it holds, the cache they share, and the step that advances them.

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::ScalarClass;

use crate::{Prepared, ServeRefusal, Session, StepCost};

/// What one step of a served batch did.
pub struct Served {
    /// How many sequences were live when the step ran.
    pub advanced: usize,
    /// The device work behind them.
    pub cost: StepCost,
}

/// The argument list the held step takes, which is what a serving step's shape decides.
///
/// # Invariant
/// The two are not a flag apart: an append step is `count, k, v, keys, values, positions`, while a
/// decode step is `sequences, sequences × rows, sequences × capacity × columns, weights, k, v, keys,
/// values, positions` — three counts because its three dispatches have three different extents, and
/// the weights first because the score reads them before the append writes any cache.
enum Shape {
    /// One dispatch per cache slot, with the position each sequence holds.
    Append,
    /// Score, sample and append over the batch, with the score's weights resident.
    Decode {
        weights: ResidentBuffer,
        rows: usize,
    },
    /// Decode, but sampling from the distribution a temperature and a seed make.
    Sample {
        weights: ResidentBuffer,
        rows: usize,
    },
}

/// The slots a batch of sequences is served from, admitted and retired by the host.
///
/// # Invariant
/// A slot's position is its place in its own cache, and a retired slot holds `capacity`, which no
/// position inside the cache equals, so its lanes copy themselves. `capacity` and `dim` are the
/// **source's own literals**, since the kernel divides a lane by them. The bookkeeping is a host's
/// own, so it needs no device: a caller that only admits and retires never opens one.
pub struct Slots {
    sequences: usize,
    capacity: i64,
    positions: Vec<i64>,
}

impl Slots {
    /// The slots a batch of `sequences` sequences starts with, every one of them free.
    pub fn new(sequences: usize, capacity: usize) -> Self {
        Self {
            sequences,
            capacity: capacity as i64,
            positions: vec![capacity as i64; sequences],
        }
    }

    /// How many lanes one step covers, which is the whole cache and not the live sequences.
    pub fn lanes(&self, dim: usize) -> usize {
        self.sequences * self.capacity as usize * dim
    }

    /// How many sequences are live.
    pub fn active(&self) -> usize {
        self.positions
            .iter()
            .filter(|position| **position < self.capacity)
            .count()
    }

    /// Take a free slot for a new sequence, at position zero.
    ///
    /// # Invariant
    /// A reused slot keeps the words its previous sequence wrote past position zero, which is safe
    /// only because a reader reads positions before its own.
    pub fn admit(&mut self) -> Option<usize> {
        let free = self
            .positions
            .iter()
            .position(|position| *position >= self.capacity)?;
        self.positions[free] = 0;
        Some(free)
    }

    /// Free `slot`, leaving its cache where it is and its lanes copying themselves.
    pub fn retire(&mut self, slot: usize) {
        if let Some(position) = self.positions.get_mut(slot) {
            *position = self.capacity;
        }
    }

    /// The positions, one per slot, in slot order.
    pub fn positions(&self) -> &[i64] {
        &self.positions
    }

    /// Every live slot's position becomes `after`, so the next step appends past the prompt.
    pub fn adopt(&mut self, after: usize) {
        let after = after.min(self.capacity as usize) as i64;
        for position in &mut self.positions {
            if *position < self.capacity {
                *position = after;
            }
        }
    }

    /// Every live slot's position moves on by one.
    pub fn advance(&mut self) {
        for position in &mut self.positions {
            if *position < self.capacity {
                *position += 1;
            }
        }
    }
}

/// A batch of sequences sharing one cache, admitted and retired by slot.
///
/// # Invariant
/// A slot's position is its place in its own cache, and a retired slot holds `capacity`, which no
/// position inside the cache equals, so its lanes copy themselves and its memory waits where it is.
/// `capacity` and `dim` are the **source's own literals**, since the kernel divides a lane by them.
pub struct Service<'session> {
    session: &'session Session,
    prepared: Prepared<'session>,
    shape: Shape,
    sequences: usize,
    capacity: usize,
    dim: usize,
    temperature: f32,
    seed: f32,
    tokens: Option<ResidentBuffer>,
    keys: Option<ResidentBuffer>,
    values: Option<ResidentBuffer>,
    slots: Slots,
}

impl<'session> Service<'session> {
    /// Prepare `source` as the step every call runs, over `sequences` slots.
    ///
    /// # Invariant
    /// Every slot starts retired, so a batch begins empty and only `admit` puts a sequence in it.
    pub fn open(
        session: &'session Session,
        source: &str,
        sequences: usize,
        capacity: usize,
        dim: usize,
    ) -> Result<Self, ServeRefusal> {
        Ok(Self::opened(
            session,
            source,
            Shape::Append,
            sequences,
            capacity,
            dim,
        )?)
    }

    /// Prepare a step that scores, samples and appends, over `sequences` slots.
    ///
    /// # Invariant
    /// `rows` is the score's row count and `weights` its coefficient table, both the **source's own**
    /// numbers: the weights are a resident the caller uploaded and still owns, so this releases
    /// neither, and every step reads them from the device rather than uploading them again.
    pub fn open_decode(
        session: &'session Session,
        source: &str,
        sequences: usize,
        capacity: usize,
        dim: usize,
        rows: usize,
        weights: ResidentBuffer,
    ) -> Result<Self, ServeRefusal> {
        Self::opened(
            session,
            source,
            Shape::Decode { weights, rows },
            sequences,
            capacity,
            dim,
        )
    }

    /// Prepare a step that samples from the distribution instead of taking the largest row.
    ///
    /// # Invariant
    /// The temperature and the seed are the caller's per step, so one batch can be served hotter and
    /// then colder without a new step: `set_sampler` is what changes them, and the device derives the
    /// threshold from the seed, so no random number reaches the host.
    pub fn open_sample(
        session: &'session Session,
        source: &str,
        sequences: usize,
        capacity: usize,
        dim: usize,
        rows: usize,
        weights: ResidentBuffer,
    ) -> Result<Self, ServeRefusal> {
        Self::opened(
            session,
            source,
            Shape::Sample { weights, rows },
            sequences,
            capacity,
            dim,
        )
    }

    /// Set the temperature and the seed the next sampled step uses.
    pub fn set_sampler(&mut self, temperature: f32, seed: f32) {
        self.temperature = temperature;
        self.seed = seed;
    }

    /// The one place a batch is built, so both shapes start retired and empty.
    fn opened(
        session: &'session Session,
        source: &str,
        shape: Shape,
        sequences: usize,
        capacity: usize,
        dim: usize,
    ) -> Result<Self, ServeRefusal> {
        Ok(Self {
            session,
            prepared: session.prepare(source)?,
            shape,
            sequences,
            capacity,
            dim,
            temperature: 1.0,
            seed: 0.0,
            tokens: None,
            keys: None,
            values: None,
            slots: Slots::new(sequences, capacity),
        })
    }

    /// How many lanes one step covers, which is the whole cache and not the live sequences.
    pub fn lanes(&self) -> usize {
        self.slots.lanes(self.dim)
    }

    /// How many sequences are live.
    pub fn active(&self) -> usize {
        self.slots.active()
    }

    /// Take a free slot for a new sequence, at position zero.
    ///
    /// # Invariant
    /// A reused slot keeps the words its previous sequence wrote past position zero, which is safe
    /// only because a reader reads positions before its own.
    pub fn admit(&mut self) -> Option<usize> {
        self.slots.admit()
    }

    /// Free `slot`, leaving its cache where it is and its lanes copying themselves.
    pub fn retire(&mut self, slot: usize) {
        self.slots.retire(slot);
    }

    /// Take the caches a prefill wrote as this batch's own, starting every live sequence past them.
    ///
    /// # Invariant
    /// A live slot's position becomes `positions`, so its next step appends past the prompt, while a
    /// retired slot stays retired. The residents become the batch's, so the service releases them when
    /// it replaces them, and `positions` is clamped to the capacity the cache holds.
    pub fn adopt(&mut self, keys: ResidentBuffer, values: ResidentBuffer, positions: usize) {
        self.slots.adopt(positions);
        for previous in [
            self.tokens.take(),
            self.keys.replace(keys),
            self.values.replace(values),
        ]
        .into_iter()
        .flatten()
        {
            self.session.release(previous);
        }
    }

    /// The tokens the last step sampled, when the step samples any.
    pub fn tokens(&self) -> Option<ResidentBuffer> {
        self.tokens
    }

    /// The key cache the last step wrote, which is the one the next step reads.
    pub fn keys(&self) -> Option<ResidentBuffer> {
        self.keys
    }

    /// The value cache the last step wrote.
    pub fn values(&self) -> Option<ResidentBuffer> {
        self.values
    }

    /// Advance every live sequence one position, appending `keys` and `values` as their new words.
    ///
    /// # Invariant
    /// Each slice holds one word per sequence per column, so its length is `sequences × dim`
    /// whatever the live count is, and they are separate because a model's key and value projections
    /// differ. The step's count is the whole cache's lanes, retired slots included, and the positions
    /// advance only after the run succeeded, so a refusal leaves the batch where it was.
    pub fn step(&mut self, keys: &[f32], values: &[f32]) -> Result<Served, ServeRefusal> {
        let words = self.sequences * self.dim;
        if keys.len() != words || values.len() != words {
            return Err(ServeRefusal::Program(vec![format!(
                "a step writes one key word and one value word per sequence per column, which is {} \
                 of each here, and it was given {} and {}",
                words,
                keys.len(),
                values.len()
            )]));
        }
        self.step_arguments(
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: packed(keys),
            },
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: packed(values),
            },
        )
    }

    /// Advance every live sequence one position, appending the words the two arguments carry.
    ///
    /// # Invariant
    /// A `Resident` keeps a word on the device and a `Buffer` uploads it, so a step can take its input
    /// from the step before it without a host round trip. The step's count is the whole cache's lanes,
    /// retired slots included, and the positions advance only after the run succeeded.
    pub fn step_arguments(
        &mut self,
        keys: RunArgument,
        values: RunArgument,
    ) -> Result<Served, ServeRefusal> {
        let lanes = self.lanes();
        let previous = (self.tokens, self.keys, self.values);
        let mut arguments = match &self.shape {
            Shape::Append => {
                let mut arguments = vec![RunArgument::Count(lanes as i64)];
                for cache in [previous.1, previous.2] {
                    arguments.push(match cache {
                        Some(resident) => RunArgument::Resident(resident),
                        None => RunArgument::Buffer {
                            class: ScalarClass::Float,
                            data: vec![0; lanes * ScalarClass::Float.byte_width()],
                        },
                    });
                }
                arguments
            }
            Shape::Sample { weights, rows } => vec![
                RunArgument::Count(self.sequences as i64),
                RunArgument::Count((self.sequences * rows) as i64),
                RunArgument::Count(lanes as i64),
                RunArgument::Float(self.temperature),
                RunArgument::Float(self.seed),
                RunArgument::Resident(*weights),
                match previous.1 {
                    Some(resident) => RunArgument::Resident(resident),
                    None => RunArgument::Buffer {
                        class: ScalarClass::Float,
                        data: vec![0; lanes * ScalarClass::Float.byte_width()],
                    },
                },
                match previous.2 {
                    Some(resident) => RunArgument::Resident(resident),
                    None => RunArgument::Buffer {
                        class: ScalarClass::Float,
                        data: vec![0; lanes * ScalarClass::Float.byte_width()],
                    },
                },
            ],
            Shape::Decode { weights, rows } => vec![
                RunArgument::Count(self.sequences as i64),
                RunArgument::Count((self.sequences * rows) as i64),
                RunArgument::Count(lanes as i64),
                RunArgument::Resident(*weights),
                match previous.1 {
                    Some(resident) => RunArgument::Resident(resident),
                    None => RunArgument::Buffer {
                        class: ScalarClass::Float,
                        data: vec![0; lanes * ScalarClass::Float.byte_width()],
                    },
                },
                match previous.2 {
                    Some(resident) => RunArgument::Resident(resident),
                    None => RunArgument::Buffer {
                        class: ScalarClass::Float,
                        data: vec![0; lanes * ScalarClass::Float.byte_width()],
                    },
                },
            ],
        };
        for fresh in [keys, values] {
            arguments.push(fresh);
        }
        arguments.push(RunArgument::Buffer {
            class: ScalarClass::Int,
            data: positions_of(self.slots.positions()),
        });
        let stepped = self.prepared.step(arguments)?;
        let (tokens, keys, values) = match &stepped.results[..] {
            [RunResult::Resident(keys), RunResult::Resident(values)] => (None, *keys, *values),
            [
                RunResult::Resident(tokens),
                RunResult::Resident(keys),
                RunResult::Resident(values),
            ] => (Some(*tokens), *keys, *values),
            _ => {
                return Err(ServeRefusal::Program(vec![
                    "a step answers with its caches, or with a token and its caches, and this one \
                     answered otherwise"
                        .into(),
                ]));
            }
        };
        for cache in [previous.0, previous.1, previous.2].into_iter().flatten() {
            self.session.release(cache);
        }
        self.tokens = tokens;
        self.keys = Some(keys);
        self.values = Some(values);
        let advanced = self.active();
        self.slots.advance();
        Ok(Served {
            advanced,
            cost: stepped.cost,
        })
    }
}

/// `values` as the packed bytes a `Float` argument carries.
fn packed(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// `positions` as the packed bytes an `Int` argument carries, which is eight bytes each.
fn positions_of(positions: &[i64]) -> Vec<u8> {
    positions
        .iter()
        .flat_map(|position| position.to_le_bytes())
        .collect()
}
