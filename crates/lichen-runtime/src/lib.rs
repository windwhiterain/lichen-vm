//! `lichen-runtime`: an artifact read and run on IR alone, with no language in it.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! **Nothing here takes a `Module`.** `Module` is `lichen-lowlevel`'s, so it is the
//! line between this crate and `lichen-compute`: an IR type belongs here, and anything
//! that also wants a `Module` belongs to the language side. `cargo tree -p
//! lichen-runtime` naming no language crate *is* the check.

pub mod fused;
pub mod module_cache;

pub use fused::{FusedKernel, FusedRefusal, malformed_at, read_fused_kernel, write_fused_kernel};
pub use module_cache::{cached_module, module_cache_misses};
