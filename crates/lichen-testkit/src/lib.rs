//! The one test harness: write an IR by hand with every expression named, then
//! ask the built module about it by that name. See docs/notes/test-harness.md.
//!
//! A name is the whole point. `ExprId(17)` says nothing about which expression
//! failed, and a failure that has to be re-derived from the writer to be read
//! is a failure that costs a rebuild. So a builder **requires** the name it
//! writes under, [`Probe`] takes that name back, and a node answers with the
//! name of the expression it lowered from.

pub mod probe;
pub mod writer;

pub use probe::{Probe, dyn_node};
pub use writer::{Program, Writer};
