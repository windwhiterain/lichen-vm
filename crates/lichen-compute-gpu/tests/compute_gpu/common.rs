//! A helper the device-backed tests in this directory share.
//!
//! # Invariant
//! Cargo takes each `.rs` file directly in `tests/` as its own binary, so the shared
//! module is named by the suite's `mod.rs` and every area reaches it the same way, as
//! `super::common`. Each test opens its own context rather than sharing one: the recycling
//! and id assertions read one context's pools, and a test running on it alongside would be
//! what they saw.

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
