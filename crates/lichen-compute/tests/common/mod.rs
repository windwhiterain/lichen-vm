//! Shared end-to-end helpers bound to the compute sub-language.
//! See tests-do-not-render.md.

#![allow(dead_code, unused_imports)]

use std::sync::{Mutex, MutexGuard};

use lichen_compute::language::{LangProgram, LangValue, new_store};
use lichen_compute_gpu::{GpuContext, install_default};
use lichen_lowlevel::{Module, NodeId};

pub use lichen_language::testing::{
    array_values, float_array, float_of, run_at, type_head, type_is_float, type_is_int,
    type_is_undecided, usize_array, usize_of, values_eq,
};

/// Compile and run `source` (no imports, no store).
pub fn evaluate(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    lichen_language::testing::evaluate::<LangProgram>(source)
}

/// Compile and run `source` through a fresh store with `compute.lichen`
/// registered.
pub fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    let mut store = new_store(None).expect("the embedded compute package registers");
    run_at(source, None, &mut store)
}

/// The device slot, which is process-global, so it is spelled once here.
///
/// # Invariant
/// `install_default` replaces the process's backend, so a test that installs one while another
/// runs leaves the first's buffers on a device that is no longer installed. Every test here that
/// installs a backend holds this slot first, through [`device_slot`] or [`device`].
static DEVICE: Mutex<()> = Mutex::new(());

/// The device slot alone, for a test that installs its own backends.
///
/// # Invariant
/// The lock is taken before the caller touches a device and is released only when the guard is
/// dropped, which is the hold [`device`] takes and for the same reason: the slot it guards is the
/// process's, not the caller's.
pub fn device_slot() -> MutexGuard<'static, ()> {
    DEVICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A context on a device, or `None` when there is none here, with the device held for the run.
///
/// # Invariant
/// The skip is reported and never turned into a failure. The context is installed as the
/// process's backend, because a graph run reaches a device only through `ParallelBackend`. The
/// guard travels back with the context, so a caller that keeps both keeps the device for as long
/// as it runs.
pub fn device(test: &str) -> Option<(GpuContext, MutexGuard<'static, ()>)> {
    let guard = device_slot();
    match install_default() {
        Ok(()) => Some((
            GpuContext::new().expect("a context was just installed"),
            guard,
        )),
        Err(reason) => {
            eprintln!("SKIPPED {test}: no device to run on: {reason}");
            None
        }
    }
}
