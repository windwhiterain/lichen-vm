//! Running a parallel launch: the workers, the host functions, the call.
//!
//! # Invariant
//!
//! The engine is the runtime's and the words are the IR's. A crate that owns the
//! kernel IR implements [`HostWords`] and hands the engine indices; a crate that
//! owns no IR never resolves this module, because a backend that runs no wasm
//! takes the runtime with `default-features = false`.

use lichen_kernel_ir::{KernelId, ScalarClass};

use super::{buffer_import_name, to_wasmi, value_word};

use crate::module_cache::cached_module;

/// The words a `read`/`write` import reads and writes.
///
/// # Invariant
///
/// Everything crossing this boundary is an index and a word: the engine holds
/// neither the roles nor the buffers. An atomic add is the state's own, because
/// only the state holds the word it read-modify-writes and knows whether that
/// word is an integer or a float's bits.
pub trait HostWords {
    /// The word the input at `position` holds at the global `index`.
    fn read(&self, position: usize, index: usize) -> i64;

    /// The word the output at `position` holds at the global `index`.
    ///
    /// # Invariant
    /// A write past the end is dropped rather than grown into, and the store is
    /// the whole of a write: disjointness is the kernel's contract, so the engine
    /// asks for no stronger ordering than the caller's.
    fn write(&self, position: usize, index: usize, value: i64);

    /// The word the output at `position` holds at the global `index`, added to.
    ///
    /// # Invariant
    /// The add is read-modify-write over the word the state owns, so a lane that
    /// added twice has both adds.
    fn atomic_add(&self, position: usize, index: usize, value: i64);
}

/// The element count at or above which a parallel run spreads over worker threads.
///
/// # Invariant
/// Below it the run keeps its own latency: every worker costs a spawn plus its own
/// store, linker and instantiation, repaid only once a worker owns enough indices for
/// the interpreted calls to dominate. A few thousand indices cost milliseconds of
/// interpretation against a spawn measured in tens of microseconds.
pub const SEQUENTIAL_PARALLEL_ELEMENTS: usize = 1 << 12;

/// Run the parallel launch of `id` over `count` indices and answer the workers it used.
///
/// # Invariant
/// The module is the one `assemble` writes, cached by the kernel's id and shared by
/// every worker, and a refusal to assemble is reported before any worker starts. The
/// first chunk is the calling thread's own work, and each worker builds its own state
/// over the buffers the caller keeps, which is what lets the lanes write into one set
/// of outputs without sharing a store.
pub fn run_parallel<State, Make>(
    id: KernelId,
    assemble: impl FnOnce() -> Result<Vec<u8>, String>,
    state: Make,
    leaves: &[i64],
    leaf_classes: &[ScalarClass],
    class: ScalarClass,
    count: usize,
) -> Result<usize, String>
where
    State: HostWords,
    Make: Fn() -> State + Send + Sync,
{
    let (engine, module) = cached_module(id, assemble)?;
    let workers = parallel_worker_count(count);
    if workers == 1 {
        // The sequential run *is* the one-worker run: the calling thread runs every lane,
        // and nothing is spawned.
        run_range(
            &engine,
            &module,
            state(),
            leaves,
            leaf_classes,
            class,
            0,
            count,
        )?;
        return Ok(workers);
    }
    // Lane bounds, not storage bounds: the worker count partitions indices only.
    let bounds = chunk_bounds(count, workers);
    let mut failures: Vec<Option<Result<(), String>>> = (0..workers).map(|_| None).collect();
    // Scoped, not detached: the scope joins every worker before the shared
    // buffers are read back.
    std::thread::scope(|scope| {
        for (failure, window) in failures.iter_mut().zip(bounds.windows(2)) {
            let (base, end) = (window[0], window[1]);
            // The engine, the module and the factory outlive the scope, so each
            // worker borrows them rather than taking them.
            let engine = &engine;
            let module = &module;
            let state = &state;
            let run = move || {
                run_range(
                    &engine,
                    &module,
                    state(),
                    leaves,
                    leaf_classes,
                    class,
                    base,
                    end,
                )
            };
            // The first chunk is the calling thread's own work: it runs here
            // and joins only the others.
            if base == 0 {
                *failure = Some(run());
                continue;
            }
            scope.spawn(move || *failure = Some(run()));
        }
    });
    for (worker, failure) in failures.into_iter().enumerate() {
        if let Some(Err(message)) = failure {
            return Err(format!(
                "parallel launch of kernel {id} failed in worker {worker}, \
                 over indices [{}, {}): {message}",
                bounds[worker],
                bounds[worker + 1]
            ));
        }
    }
    Ok(workers)
}

/// The chunk end of each of `workers` contiguous chunks of `count` indices.
///
/// # Invariant
/// A function of its two arguments and nothing else, so a partition is reproducible
/// and the result does not depend on how the indices are grouped. Chunk `k` is
/// `count / workers` long with the first `count % workers` one longer, and the calling
/// thread takes chunk 0 so the later workers are off the shortest chunk.
fn chunk_bounds(count: usize, workers: usize) -> Vec<usize> {
    let (length, extra) = (count / workers, count % workers);
    (0..=workers)
        .map(|index| index * length + index.min(extra))
        .collect()
}

/// How many workers a parallel run of `count` indices uses, the caller included.
///
/// # Invariant
/// Never below [`SEQUENTIAL_PARALLEL_ELEMENTS`] — the fan-out must pay for itself —
/// and never above what the machine has or what there are indices for. An unavailable
/// `available_parallelism` reads as one, the sequential run.
fn parallel_worker_count(count: usize) -> usize {
    if count < SEQUENTIAL_PARALLEL_ELEMENTS {
        return 1;
    }
    let available = std::thread::available_parallelism().map_or(1, |count| count.get());
    available.clamp(1, count)
}

/// Run one worker's index range `[base, end)`: the store, linker, instance and the
/// per-index call loop.
///
/// # Invariant
/// The extent is the first of `leaves` and is the whole launch's: the index
/// function is a function of the full extent, not of the chunk. The host functions
/// read the store's own state and capture nothing, which is what lets one module
/// run under every worker.
fn run_range<State: HostWords>(
    engine: &wasmi::Engine,
    module: &wasmi::Module,
    state: State,
    leaves: &[i64],
    leaf_classes: &[ScalarClass],
    class: ScalarClass,
    base: usize,
    end: usize,
) -> Result<(), String> {
    let mut store = wasmi::Store::new(engine, state);
    let mut linker = wasmi::Linker::<State>::new(engine);

    // One `read`/`write` expr per class: both are defined even when the module
    // declares only one.
    for (element_class, value_type) in [
        (ScalarClass::Int, wasmi::ValType::I64),
        (ScalarClass::Float, wasmi::ValType::F32),
    ] {
        let read_ty =
            wasmi::FuncType::new([wasmi::ValType::I64, wasmi::ValType::I64], [value_type]);
        let write_ty =
            wasmi::FuncType::new([wasmi::ValType::I64, wasmi::ValType::I64, value_type], []);
        linker
            .func_new(
                "env",
                &buffer_import_name(element_class, "read"),
                read_ty,
                move |caller: wasmi::Caller<'_, State>,
                      params: &[wasmi::Val],
                      results: &mut [wasmi::Val]| {
                    let pos = position_of(params.first());
                    let idx = position_of(params.get(1));
                    // The index is global, so no rebase: the word is the element's
                    // own bits.
                    let value = caller.data().read(pos, idx);
                    results[0] = to_wasmi(element_class, value);
                    Ok(())
                },
            )
            .map_err(|e| e.to_string())?;
        linker
            .func_new(
                "env",
                &buffer_import_name(element_class, "write"),
                write_ty.clone(),
                move |caller: wasmi::Caller<'_, State>,
                      params: &[wasmi::Val],
                      _results: &mut [wasmi::Val]| {
                    let out_pos = position_of(params.first());
                    let idx = position_of(params.get(1));
                    let value = params.get(2).map_or(0, |v| value_word(element_class, v));
                    caller.data().write(out_pos, idx, value);
                    Ok(())
                },
            )
            .map_err(|e| e.to_string())?;
        linker
            .func_new(
                "env",
                &buffer_import_name(element_class, "atomic"),
                write_ty.clone(),
                move |caller: wasmi::Caller<'_, State>,
                      params: &[wasmi::Val],
                      _results: &mut [wasmi::Val]| {
                    let out_pos = position_of(params.first());
                    let idx = position_of(params.get(1));
                    let value = params.get(2).map_or(0, |v| value_word(element_class, v));
                    caller.data().atomic_add(out_pos, idx, value);
                    Ok(())
                },
            )
            .map_err(|e| e.to_string())?;
    }

    let instance = linker
        .instantiate_and_start(&mut store, module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_func(&store, "main")
        .ok_or_else(|| "parallel kernel has no export `main`".to_string())?;
    // The leaves are fixed for the whole range and only the index moves, so the
    // list is built once.
    let mut args: Vec<wasmi::Val> = {
        let state = store.data();
        leaf_classes
            .iter()
            .zip(leaves)
            .map(|(class, word)| to_wasmi(*class, *word))
            .collect()
    };
    args.push(to_wasmi(ScalarClass::Int, base as i64));
    let mut results = [to_wasmi(class, 0)];
    for i in base..end {
        // The ABI's arguments are the scalar leaves then the index: only the index
        // moves per element.

        // The extent and the index are not data: `i64` whatever class the body
        // computes in.
        if let Some(index) = args.last_mut() {
            *index = to_wasmi(ScalarClass::Int, i as i64);
        }
        main.call(&mut store, &args, &mut results)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The position or the index a `read`/`write` import's argument carries.
///
/// # Invariant
/// The ABI is `i64` whatever class the body computes in, and a module is
/// validated against these signatures before it runs, so an argument that is not
/// an integer reads as position zero rather than refusing the whole run.
fn position_of(value: Option<&wasmi::Val>) -> usize {
    value.and_then(|value| value.i64()).unwrap_or(0) as usize
}

#[cfg(test)]
mod tests {
    use super::{SEQUENTIAL_PARALLEL_ELEMENTS, chunk_bounds, parallel_worker_count};

    /// A count below the threshold runs on the calling thread alone.
    #[test]
    fn a_small_run_is_sequential() {
        assert_eq!(parallel_worker_count(SEQUENTIAL_PARALLEL_ELEMENTS - 1), 1);
    }

    /// A count at the threshold fans out, and never past the indices there are.
    #[test]
    fn a_large_run_fans_out_up_to_the_machine() {
        let count = SEQUENTIAL_PARALLEL_ELEMENTS;
        let workers = parallel_worker_count(count);
        let available = std::thread::available_parallelism().map_or(1, |count| count.get());
        assert!(workers >= 2 || available < 2);
        assert!(workers <= available.min(count));
    }

    /// The chunks partition the indices: every index is in exactly one, and the
    /// first chunk is never the shortest.
    #[test]
    fn the_chunks_partition_the_indices() {
        let bounds = chunk_bounds(10, 3);
        assert_eq!(bounds, vec![0, 4, 7, 10]);
        assert!(bounds.windows(2).all(|w| w[1] > w[0]));
    }
}
