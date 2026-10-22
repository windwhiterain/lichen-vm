//! Shared end-to-end helpers bound to the compute sub-language.
//! See tests-do-not-render.md.

#![allow(dead_code, unused_imports)]

use std::sync::{Arc, Mutex, MutexGuard};

use lichen_compute::language::{LangProgram, LangValue, new_store};
use lichen_kernel_ir::{
    BufferSlot, KernelFragment, LaunchSet, ParallelBackend, ResidentId, ScalarClass, ScalarData,
    install_parallel_backend,
};
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
pub fn buf_element_reads(module: &Module<LangProgram>) -> Vec<(NodeId, String)> {
    use lichen_highlevel::shape::{TypeRef, field_names, field_type, low_type_of_slot};
    use lichen_lowlevel::AnyNodeId;

    let mut reads: Vec<(NodeId, String)> = Vec::new();
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
        reads.push((id, read));
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

/// The stub backend the suite installs where a device would go.
///
/// # Invariant
/// A stub runs no kernel and computes nothing, so the answers it hands back are its own: a
/// test that reads a value from it reads the stub's. What it does answer is the routing —
/// which fragment reached a backend and over how many elements — and that is this crate's
/// own subject.
pub struct Stub {
    /// The calls, each with the thread that made it: the tests in one binary run in
    /// parallel, so a record is the test's own.
    calls: Mutex<Vec<(std::thread::ThreadId, Call)>>,
    /// The class each resident was issued as, so a fetch answers the buffer it was
    /// given and not one class for every buffer.
    residents: Mutex<std::collections::HashMap<u64, ScalarClass>>,
    /// The next resident id, so two dispatches are two residents.
    next: Mutex<u64>,
}

/// One dispatch the stub was handed.
#[derive(Clone)]
pub struct Call {
    /// The fragment the launch set's root named.
    pub fragment: KernelFragment,
    /// The extent the host asked for.
    pub count: usize,
}

impl ParallelBackend for Stub {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn run(
        &self,
        launch: &LaunchSet<'_>,
        _inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, String> {
        let root = launch.root();
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((
                std::thread::current().id(),
                Call {
                    fragment: root.clone(),
                    count,
                },
            ));
        let mut next = self
            .next
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut residents = self
            .residents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ids = (0..root.outputs)
            .map(|position| {
                let id = ResidentId(*next);
                *next += 1;
                let class = root
                    .output_classes
                    .get(position)
                    .copied()
                    .unwrap_or(ScalarClass::Int);
                residents.insert(id.0, class);
                id
            })
            .collect();
        Ok(ids)
    }

    fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, String> {
        let class = self
            .residents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id.0)
            .copied()
            .unwrap_or(ScalarClass::Int);
        Ok(match class {
            ScalarClass::Int => ScalarData::Int(vec![0; count]),
            ScalarClass::Float | ScalarClass::BFloat16 => ScalarData::Float(vec![0.0; count]),
        })
    }

    /// Zeros over the shape the trait states, so the routing is what a test reads.
    fn cooperative_matrix(&self, _a: &[i8], _b: &[i8], _c: &[i32]) -> Result<Vec<i32>, String> {
        let m = lichen_kernel_ir::COOPERATIVE_MATRIX_M as usize;
        let n = lichen_kernel_ir::COOPERATIVE_MATRIX_N as usize;
        Ok(vec![0; m * n])
    }

    fn release(&self, id: ResidentId) {
        self.residents
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id.0);
    }
}

/// The slot the installed backend lives in, which is the process's.
///
/// # Invariant
/// The lock is taken before the caller installs a backend and released only when the guard
/// is dropped, so a second test that installs one while another runs cannot leave the
/// first's residents on a backend that is no longer installed.
static BACKEND_SLOT: Mutex<()> = Mutex::new(());

/// The slot alone, for a test that installs a backend of its own.
///
/// # Invariant
/// The lock is taken before the caller installs a backend and released only when the guard
/// is dropped, so the slot it guards is the process's and not the caller's.
pub fn slot() -> MutexGuard<'static, ()> {
    BACKEND_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The stub, installed as this process's backend, with the slot held.
///
/// # Invariant
/// The guard travels back with the stub, so a caller that keeps both keeps the slot for as
/// long as it runs.
pub fn stub() -> (Arc<Stub>, MutexGuard<'static, ()>) {
    let guard = BACKEND_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let stub = Arc::new(Stub {
        calls: Mutex::new(Vec::new()),
        residents: Mutex::new(std::collections::HashMap::new()),
        next: Mutex::new(1),
    });
    install_parallel_backend(stub.clone());
    // The pass table is the process's, so a stub starts from none installed.
    lichen_compute::compute::graph_pass::clear_graph_pass();
    (stub, guard)
}

/// The calls the stub was handed, in the order they came.
impl Stub {
    pub fn calls(&self) -> Vec<Call> {
        let thread = std::thread::current().id();
        let calls = self
            .calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        calls
            .iter()
            .filter(|(made_by, _)| *made_by == thread)
            .map(|(_, call)| call.clone())
            .collect()
    }
}

/// The lane count one row of a workgroup runs, which is the width the tests take.
///
/// # Invariant
/// A launch the tile divides is a whole number of these rows, so a count that is not a
/// multiple of it covers a last row that is short, and a test that wants that says so.
pub const LANES: u32 = 64;
