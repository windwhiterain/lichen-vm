//! How a fragment's reads and parameters are read as symbolic slots.

/// Which of a fragment's own reads hold the producer reduction's value.
///
/// # Invariant
/// A buffer position is the ordinal the lowering emitted — the buffer's place in the
/// parameter's `.in` group — and a parameter leaf is the position in the fragment's
/// flattened domain: the caller that knows the program says which one is the producer's,
/// because a fragment cannot know it. `None` is the producer's own body, where every read
/// is a datum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRoles {
    /// The input position whose read is the producer reduction's value.
    pub producer_input: Option<usize>,
    /// A scalar fragment's two parameter leaves, as the datum's and the producer's.
    ///
    /// # Invariant
    /// It is read only for a fragment that carries no buffers: there the parameters *are*
    /// the slots, and a projection that reads a buffer position instead is refused.
    pub parameter_leaves: Option<(usize, usize)>,
}

impl ReadRoles {
    /// The roles of a projection that reads no producer: every read is a datum.
    pub const DATUM_ONLY: ReadRoles = ReadRoles {
        producer_input: None,
        parameter_leaves: None,
    };

    /// The roles of a consumer's projection: the buffer at `position` is the producer's.
    pub const fn consumer(position: usize) -> ReadRoles {
        ReadRoles {
            producer_input: Some(position),
            parameter_leaves: None,
        }
    }

    /// The roles of a scalar projection: leaf `datum` is the datum and `producer` its value.
    pub const fn parameters(datum: usize, producer: usize) -> ReadRoles {
        ReadRoles {
            producer_input: None,
            parameter_leaves: Some((datum, producer)),
        }
    }
}

impl Default for ReadRoles {
    fn default() -> Self {
        ReadRoles::DATUM_ONLY
    }
}
