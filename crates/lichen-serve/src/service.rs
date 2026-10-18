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

/// A batch of sequences sharing one cache, admitted and retired by slot.
///
/// # Invariant
/// A slot's position is its place in its own cache, and a retired slot holds `capacity`, which no
/// position inside the cache equals, so its lanes copy themselves and its memory waits where it is.
/// `capacity` and `dim` are the source's own literals, since the kernel divides a lane by them.
pub struct Service<'session> {
    session: &'session Session,
    prepared: Prepared<'session>,
    sequences: usize,
    capacity: usize,
    dim: usize,
    keys: Option<ResidentBuffer>,
    values: Option<ResidentBuffer>,
    positions: Vec<i64>,
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
        Ok(Self {
            session,
            prepared: session.prepare(source)?,
            sequences,
            capacity,
            dim,
            keys: None,
            values: None,
            positions: vec![capacity as i64; sequences],
        })
    }

    /// How many lanes one step covers, which is the whole cache and not the live sequences.
    pub fn lanes(&self) -> usize {
        self.sequences * self.capacity * self.dim
    }

    /// How many sequences are live.
    pub fn active(&self) -> usize {
        self.positions
            .iter()
            .filter(|position| **position < self.capacity as i64)
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
            .position(|position| *position >= self.capacity as i64)?;
        self.positions[free] = 0;
        Some(free)
    }

    /// Free `slot`, leaving its cache where it is and its lanes copying themselves.
    pub fn retire(&mut self, slot: usize) {
        if let Some(position) = self.positions.get_mut(slot) {
            *position = self.capacity as i64;
        }
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
        let lanes = self.lanes();
        let previous = (self.keys, self.values);
        let mut arguments = vec![RunArgument::Count(lanes as i64)];
        for cache in [previous.0, previous.1] {
            arguments.push(match cache {
                Some(resident) => RunArgument::Resident(resident),
                None => RunArgument::Buffer {
                    class: ScalarClass::Float,
                    data: vec![0; lanes * ScalarClass::Float.byte_width()],
                },
            });
        }
        for fresh in [keys, values] {
            arguments.push(RunArgument::Buffer {
                class: ScalarClass::Float,
                data: packed(fresh),
            });
        }
        arguments.push(RunArgument::Buffer {
            class: ScalarClass::Int,
            data: positions_of(&self.positions),
        });
        let stepped = self.prepared.step(arguments)?;
        let [RunResult::Resident(keys), RunResult::Resident(values)] = &stepped.results[..] else {
            return Err(ServeRefusal::Program(vec![
                "a step answers with the two caches it wrote, and this one answered otherwise"
                    .into(),
            ]));
        };
        for cache in [previous.0, previous.1].into_iter().flatten() {
            self.session.release(cache);
        }
        self.keys = Some(*keys);
        self.values = Some(*values);
        let advanced = self.active();
        for position in &mut self.positions {
            if *position < self.capacity as i64 {
                *position += 1;
            }
        }
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
