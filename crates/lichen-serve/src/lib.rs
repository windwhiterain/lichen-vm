//! A session that owns a compute backend and reports what one run asked the device for.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use lichen_compute::set_graph_policy;
use lichen_compute_gpu::GpuContext;
use lichen_graph_ir::Policy;
use lichen_kernel_ir::{ParallelBackend, clear_parallel_backend, install_parallel_backend};

/// What one run asked the device for.
///
/// # Invariant
/// The counts are deltas over the run, read from the context that is **installed**: `install_default`
/// deliberately hands none back, so a session built over it would report zero forever. `elapsed` is
/// host wall clock and includes the program's compilation, so it is not a per-dispatch cost.
#[derive(Debug, Clone, Copy)]
pub struct StepCost {
    /// Command buffers this run handed to the queue.
    pub submissions: u64,
    /// Fences this run waited on.
    pub fence_waits: u64,
    /// Host wall clock, compilation included.
    pub elapsed: Duration,
}

/// What one run answered, and what it cost.
#[derive(Debug)]
pub struct Step {
    /// The program's own rendering of its answer.
    pub answer: String,
    /// The device work behind it.
    pub cost: StepCost,
}

/// Why a session could not be opened, or a program not run.
#[derive(Debug)]
pub enum ServeRefusal {
    /// The device could not be opened, in the backend's own words.
    Device(String),
    /// The program was refused, one line per diagnostic the run reported.
    Program(Vec<String>),
}

impl std::fmt::Display for ServeRefusal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Device(reason) => write!(out, "no device: {reason}"),
            Self::Program(lines) => write!(out, "the program was refused: {}", lines.join(" | ")),
        }
    }
}

/// One device, installed for this session's lifetime, and the handle its counters are read from.
///
/// # Invariant
/// The context installed and the context counted are **one allocation**: the session wraps it and
/// installs a clone of that same `Arc`. The backend slot is process-wide, so a session holds
/// [`SESSION`] for its whole life — two would clear the slot from under each other — and drop
/// clears the backend, because one left installed hands its device to whatever runs next.
pub struct Session {
    context: Arc<GpuContext>,
    _slot: MutexGuard<'static, ()>,
}

/// The one session a process may hold, because the backend slot it installs into is process-wide.
static SESSION: Mutex<()> = Mutex::new(());

impl Session {
    /// Open the device and install it as this process's backend.
    ///
    /// # Invariant
    /// The slot is taken **before** the device is opened, so a second caller blocks rather than
    /// opening a device whose context the first would replace. The graph schedule is set to
    /// [`Policy::Batch`]: the language's own default is `Async`, whose submission count is
    /// `Serial`'s, so `Batch` is reachable only from a host that asks — a served step is one.
    pub fn open() -> Result<Self, ServeRefusal> {
        let slot = SESSION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let context =
            Arc::new(GpuContext::new().map_err(|error| ServeRefusal::Device(error.to_string()))?);
        let backend: Arc<dyn ParallelBackend> = context.clone();
        install_parallel_backend(backend);
        set_graph_policy(Policy::Batch);
        Ok(Self {
            context,
            _slot: slot,
        })
    }

    /// Run `source` and report what it answered and what the device did for it.
    ///
    /// # Invariant
    /// The store is fresh per run, so a second run compiles rather than reusing the first's modules.
    /// The counts are therefore this run's own and not a cache's.
    pub fn run(&self, source: &str) -> Result<Step, ServeRefusal> {
        let before = (self.context.submissions(), self.context.fence_waits());
        let started = Instant::now();
        let mut store = lichen_compute::language::new_store(None)
            .map_err(|error| ServeRefusal::Device(error.to_string()))?;
        let outcome = lichen_language::run::evaluate_raw(source, None, &mut store);
        let elapsed = started.elapsed();
        let cost = StepCost {
            submissions: self.context.submissions().saturating_sub(before.0),
            fence_waits: self.context.fence_waits().saturating_sub(before.1),
            elapsed,
        };
        match outcome {
            Ok(answer) => Ok(Step { answer, cost }),
            Err(diagnostics) => Err(ServeRefusal::Program(
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.clone())
                    .collect(),
            )),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        clear_parallel_backend();
        set_graph_policy(Policy::Async);
    }
}
