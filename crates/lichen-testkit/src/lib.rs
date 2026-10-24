//! One test harness: a named IR writer and a named inspector; see
//! test-harness.md.
//!
//! # Invariant
//! A builder **requires** the name its expression is written under, so no test
//! can build an unnamed one, and a failure never has to be re-derived from the
//! writer to be read.
//!
//! # Invariant
//! A reading reports a semantic fact — a value, a node's structure, a diagnostic's
//! kind — never a rendered string; a test that asserts a string is testing the
//! printer, not the program (`docs/notes/test-harness.md`).

pub mod probe;
pub mod writer;

pub use probe::{Probe, dyn_node};
pub use writer::{Program, Writer};
