//! A helper the device-backed tests in this directory share.
//!
//! # Invariant
//! Cargo takes each `.rs` file directly in `tests/` as its own binary, so a shared
//! module lives in a subdirectory.

use lichen_compute_gpu::GpuContext;

/// A compute context on a device carrying `shaderInt64`, or `None` when there is no
/// such device here.
///
/// # Invariant
/// A caller that gets `None` returns early and the skip is reported, never turned into
/// a failure: every `GpuContext::new` error counts as "no device here", and the reason
/// is printed in full.
pub fn context(test: &str) -> Option<GpuContext> {
    match GpuContext::new() {
        Ok(context) => Some(context),
        Err(reason) => {
            eprintln!("SKIPPED {test}: no device to run on: {reason}");
            None
        }
    }
}
