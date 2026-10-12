//! The fused kernel's artifact and its landing, re-exported from the runtime they live in.
//!
//! # Invariant
//! The artifact is read, written and landed on IR alone, so it is `lichen-runtime`'s. This
//! module is the path `lichen-compute` callers already named, so moving the code did not move
//! a public path.

pub use lichen_runtime::fused::schema;
pub use lichen_runtime::fused::{
    FusedKernel, FusedRefusal, LandingRefusal, land_fused_kernel, malformed_at, read_fused_kernel,
    write_fused_kernel,
};
