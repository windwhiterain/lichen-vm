//! A helper the device-backed tests in this directory share.
//!
//! Cargo takes each `.rs` file directly in `tests/` as its own test binary, so a
//! module two binaries share has to live in a subdirectory — `tests/common/` —
//! and each binary pulls it in with `mod common;`. A `tests/common.rs` would be
//! built as a third binary instead, and a `#[path]` include would put a path in
//! each test file that has to keep agreeing with the other one's.

use lichen_compute_gpu::GpuContext;

/// A compute context on a device carrying `shaderInt64`, or `None` when there is
/// no such device here.
///
/// These tests assert what a device *computes*, so on a machine without one
/// there is nothing to assert: a caller that gets `None` returns early, and the
/// skip is reported rather than turned into a failure. Every error
/// [`GpuContext::new`] can fail with counts as "no device here" — no loader, no
/// physical device, a device without `shaderInt64`, or a loader with no driver
/// installed behind it — and the reason is printed in full, because a skip must
/// never hide *why* it skipped.
///
/// `test` is the caller's name, passed rather than read off the running thread,
/// so the message names the coverage that was not paid for without depending on
/// how the test harness names its threads.
pub fn context(test: &str) -> Option<GpuContext> {
    match GpuContext::new() {
        Ok(context) => Some(context),
        Err(reason) => {
            eprintln!("SKIPPED {test}: no device to run on: {reason}");
            None
        }
    }
}
