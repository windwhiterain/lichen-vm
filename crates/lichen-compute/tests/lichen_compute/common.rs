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

/// The one fragment a module compiled, whatever value of its own the root carries.
///
/// # Invariant
///
/// The multiset, not a named kernel: which kernel compiled first is a property of the
/// module and not of the source, so this returns the fragment rather than the kernel it
/// came from, and refuses a module that holds more than one rather than picking.
pub fn sole_fragment(module: &Module<LangProgram>) -> lichen_kernel_ir::KernelFragment {
    use lichen_compute::fusion_claim::{fragment_of, kernel_id};

    let mut found: Vec<lichen_kernel_ir::KernelFragment> = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    for (id, _node) in module.nodes.iter() {
        let Ok(kernel) = kernel_id(module, id) else {
            continue;
        };
        if seen.contains(&kernel) {
            continue;
        }
        let Ok(fragment) = fragment_of(kernel) else {
            continue;
        };
        seen.push(kernel);
        found.push(fragment);
    }
    assert_eq!(
        found.len(),
        1,
        "the module compiles exactly one parallel kernel: {found:?}"
    );
    found.pop().expect("one fragment was just counted")
}

/// The `compute.Buf` element slots a module holds, and what each one reads.
///
/// # Invariant
///
/// This is `walk_role`'s read restated: a `Buf`-shaped type term's `.element` field
/// through `low_type_of_slot`, which is the call the element class is taken by.
pub fn buf_element_reads(module: &Module<LangProgram>) -> Vec<(String, String)> {
    use lichen_highlevel::shape::{TypeRef, field_names, field_type, low_type_of_slot};
    use lichen_lowlevel::AnyNodeId;

    let mut reads: Vec<(String, String)> = Vec::new();
    for (id, _node) in module.nodes.iter() {
        let slot = AnyNodeId::Dynamic(id);
        let Some(ty) = [TypeRef::Term(slot), TypeRef::Slot(slot)]
            .into_iter()
            .find(|ty| field_names(module, *ty).is_some())
        else {
            continue;
        };
        if !field_names(module, ty)
            .unwrap_or_default()
            .iter()
            .filter_map(|name| *name)
            .eq(["native", "element"])
        {
            continue;
        }
        let read = match field_type(module, ty, 1) {
            Some(element) => format!("{:?}", low_type_of_slot(module, element)),
            None => "<no element field>".to_string(),
        };
        reads.push((format!("{id:?}"), read));
    }
    reads
}

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

/// `source` compiled and checked, never run.
///
/// # Invariant
/// The store is fresh per call, so the embedded compute package registers once for this
/// program and the report is the program's own: a test that asserts a refusal reads the
/// diagnostics without panicking on the check.
pub fn check(source: &str) -> lichen_language::Report<LangProgram> {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    assert!(diags.is_empty(), "the program's imports resolve: {diags:?}");
    lichen_language::compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &lichen_language::lex::line_starts(source),
        lichen_highlevel::no_native_ops(),
    )
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
