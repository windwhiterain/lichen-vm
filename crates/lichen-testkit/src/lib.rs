//! One test harness: a named IR writer and a named inspector; see
//! test-harness.md.
//!
//! # Invariant
//! A builder **requires** the name its expression is written under, so no test
//! can build an unnamed one, and a failure never has to be re-derived from the
//! writer to be read.

pub mod probe;
pub mod writer;

pub use probe::{Probe, dyn_node};
pub use writer::{Program, Writer};
