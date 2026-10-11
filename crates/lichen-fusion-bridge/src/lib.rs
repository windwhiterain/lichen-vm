//! The bridge from a marked subexpression of a kernel fragment to the symbolic core.
//! See docs/notes/graph-fusion.md §3.
//!
//! # Invariant
//! The bridge is total: a marked value it cannot read as one symbolic expression is a
//! [`BridgeRefusal`] naming the instruction it met, never a panic and never a guess. What
//! it reads is small on purpose — constants, the fragment's reads and its carried state,
//! and the arithmetic over them.

mod lower;
mod operators;
mod refusal;
mod roles;
mod tiled;

pub use lower::projection;
pub use refusal::BridgeRefusal;
pub use roles::ReadRoles;
pub use tiled::{Element, Folding, Identity, TileRefusal, Tiled, fold};
