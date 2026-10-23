//! How a fragment's reads and parameters are read as symbolic slots.

use lichen_fusion::{FAN_IN_INPUTS, Variable};

/// Which of a fragment's own reads hold the producer reduction's value.
///
/// # Invariant
/// A buffer position is the ordinal the lowering emitted — the buffer's place in the
/// parameter's `.in` group — and a parameter leaf is the position in the fragment's
/// flattened domain: the caller that knows the program says which is the producer's.
///
/// **One slot per named position**, in the order their producers are named, so two
/// producers read as [`Variable::producer`] `0` and `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRoles {
    /// The input positions holding a producer's value: the first `named` are named, and the
    /// rest are [`Self::UNNAMED`].
    producer_inputs: [usize; FAN_IN_INPUTS],
    /// How many entries of `producer_inputs` are named.
    named: usize,
    /// A scalar fragment's two parameter leaves, as the datum's and the producer's.
    ///
    /// # Invariant
    /// It is read only for a fragment that carries no buffers: there the parameters *are*
    /// the slots, and a projection that reads a buffer position instead is refused.
    pub parameter_leaves: Option<(usize, usize)>,
}

impl ReadRoles {
    /// A position no producer's value is named at.
    ///
    /// # Invariant
    /// It is not an ordinal a lowered body emits — those count a kernel's own inputs from
    /// zero — so a named position and an unnamed one cannot be confused.
    pub const UNNAMED: usize = usize::MAX;

    /// The roles of a projection that reads no producer: every read is a datum.
    pub const DATUM_ONLY: ReadRoles = ReadRoles {
        producer_inputs: [Self::UNNAMED; FAN_IN_INPUTS],
        named: 0,
        parameter_leaves: None,
    };

    /// The roles of a consumer's projection: the buffer at `position` is the producer's.
    ///
    /// # Invariant
    /// One position names one producer, so this reads as the expr's own [`Variable::PRODUCER`].
    pub const fn consumer(position: usize) -> ReadRoles {
        ReadRoles {
            producer_inputs: [
                position,
                Self::UNNAMED,
                Self::UNNAMED,
                Self::UNNAMED,
                Self::UNNAMED,
                Self::UNNAMED,
                Self::UNNAMED,
                Self::UNNAMED,
            ],
            named: 1,
            parameter_leaves: None,
        }
    }

    /// The roles of a consumer that reads several producers, in the order they are named.
    ///
    /// # Invariant
    /// A position past [`FAN_IN_INPUTS`] is refused rather than dropped: a reader that cannot
    /// name a producer's value would answer the datum where a producer's value is.
    pub fn consumers(positions: &[usize]) -> Option<ReadRoles> {
        if positions.len() > FAN_IN_INPUTS {
            return None;
        }
        let mut producer_inputs = [Self::UNNAMED; FAN_IN_INPUTS];
        producer_inputs[..positions.len()].copy_from_slice(positions);
        Some(ReadRoles {
            producer_inputs,
            named: positions.len(),
            parameter_leaves: None,
        })
    }

    /// The roles of a scalar projection: leaf `datum` is the datum and `producer` its value.
    pub const fn parameters(datum: usize, producer: usize) -> ReadRoles {
        ReadRoles {
            producer_inputs: [Self::UNNAMED; FAN_IN_INPUTS],
            named: 0,
            parameter_leaves: Some((datum, producer)),
        }
    }

    /// The input positions holding a producer's value, in the order their producers are named.
    fn producers(&self) -> &[usize] {
        &self.producer_inputs[..self.named]
    }

    /// The slot the read at `position` stands for, when that position holds a producer's value.
    ///
    /// # Invariant
    /// The slot is named by the position's **rank among the named positions**, so the
    /// correspondence is decided by the roles alone and two readers given the same roles
    /// cannot disagree about it.
    pub fn producer_at(&self, position: usize) -> Option<Variable> {
        let ordinal = self.producers().iter().position(|&at| at == position)?;
        Variable::producer(ordinal)
    }
}

impl Default for ReadRoles {
    fn default() -> Self {
        ReadRoles::DATUM_ONLY
    }
}
