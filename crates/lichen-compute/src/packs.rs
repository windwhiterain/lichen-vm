//! The packs a command line names, read before any program is compiled.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! Reading a pack from a path is file and text work with no language in it, so the loader is
//! `lichen-runtime`'s. This module is the path `lichen-compute` callers already named, so
//! moving the code did not move a public path.

pub use lichen_runtime::packs::{PackLoadRefusal, load_packs};
