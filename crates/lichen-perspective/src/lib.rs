//! The `lichen-perspective` plugin: the `# p` attribute, uniform over `p`
//! aligned threads, and its divisibility lattice.
//!
//! # Invariant
//!
//! The meet is `gcd`, the top is `0` (uniform over all threads, the `∞` fold),
//! and the bottom is `1`.  The crate is program-generic; the grammar, the
//! schema tail and the persist discriminator live in the host language layer.
//! See docs/notes/attributes.md.

pub mod perspective;

pub use perspective::{GcdOp, Perspective, divides, gcd, persp_attr_ext};
