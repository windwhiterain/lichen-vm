//! A deterministic source of random values, for the numeric cross-check and the example.
//!
//! # Invariant
//! Reproducible from its seed on every machine: the check this crate runs numerically is
//! evidence about the derivation, and evidence that changes when the run does is not.

/// A SplitMix64 sequence, read as `f64` samples.
#[derive(Debug, Clone)]
pub struct Random {
    state: u64,
}

impl Random {
    /// The sequence this seed starts.
    pub fn from_seed(seed: u64) -> Random {
        Random { state: seed }
    }

    /// The next value of the sequence.
    ///
    /// # Invariant
    /// One round of SplitMix64 per call, so a sample costs a fixed number of operations and
    /// a value never repeats with a period short enough to see.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    /// A sample in `[0, 1)`, from the top fifty-three bits.
    pub fn next_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A sample in `[low, low + span)`.
    pub fn next_bounded(&mut self, low: f64, span: f64) -> f64 {
        low + span * self.next_unit()
    }
}
