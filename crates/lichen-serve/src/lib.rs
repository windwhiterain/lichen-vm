//! A session that owns a compute backend and reports what one run asked the device for.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use lichen_compute::compute::graph::{self, RunArgument, RunResult};
use lichen_compute::compute::{Backend, ComputeValue, ResidentBuffer};
use lichen_compute::set_graph_policy;
use lichen_compute_gpu::GpuContext;
use lichen_graph_ir::{Graph, Policy};
use lichen_kernel_ir::{
    ParallelBackend, ScalarData, clear_parallel_backend, install_parallel_backend,
};
use lichen_utils::extend::AsEnum;

mod model;
mod narrowed;
mod qwen;
mod service;

pub use model::{
    ATTEND_CACHE_STEP, ATTEND_STEP, BLOCK, BLOCKS, CAPACITY, COLUMNS, DECODE_STEP, EMBEDDING,
    HIDDEN, LAYER_STEP, MATVEC_STEP, PREFILL_STEP, PROJECTIONS, QUERIES, RMS_NORM_STEP, ROPE_STEP,
    ROWS, SAMPLE_STEP, SWIGLU_STEP, WEIGHTS, embedding_of,
};

mod upload;

pub use narrowed::narrowed_projection;
pub use qwen::{Generated, Qwen, QwenRefusal, Width};
pub use service::{Served, Service};

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

/// A recorded step, held so that a step does not compile its program again.
///
/// # Invariant
/// The graph is looked up **once** and run many times, and [`Prepared::step`] passes the session's
/// policy as an argument rather than reading a thread-local. What a session does **not** do is
/// consult a graph pass: `graph::run` documents that the graph handed to it is *"the one a pass
/// handed back"*, so a caller that wants a pass runs it before preparing.
pub struct Prepared<'session> {
    session: &'session Session,
    graph: Graph,
    backend: Backend,
}

/// What one step answered, and what it cost.
pub struct Stepped {
    /// The graph's returns in the source function's order, device values left resident.
    pub results: Vec<RunResult>,
    /// The device work behind them.
    pub cost: StepCost,
}

impl Prepared<'_> {
    /// Run the held graph once under this session's schedule, and leave what it wrote where it is.
    ///
    /// # Invariant
    /// The counts are this call's alone, and a `RunResult::Resident` names memory the caller still
    /// has to fetch: a step is one submission and a readback is a separate act with a cost of its
    /// own, which is what [`Session::fetch`] exists for.
    pub fn step(&self, arguments: Vec<RunArgument>) -> Result<Stepped, ServeRefusal> {
        self.step_with(Policy::Batch, arguments)
    }

    /// Run the held graph once under `policy`, for a host that wants to compare schedules.
    ///
    /// # Invariant
    /// The only thing `policy` changes is how many submissions the run takes: `Batch` records every
    /// node into one and waits once, while `Async` takes one per node and does not idle the host
    /// between them. So the same graph run twice is the same work at two submission counts.
    pub fn step_with(
        &self,
        policy: Policy,
        arguments: Vec<RunArgument>,
    ) -> Result<Stepped, ServeRefusal> {
        let before = (
            self.session.context.submissions(),
            self.session.context.fence_waits(),
        );
        let started = Instant::now();
        let outcome = graph::run(&self.graph, self.backend, arguments, policy);
        let elapsed = started.elapsed();
        let cost = StepCost {
            submissions: self.session.context.submissions().saturating_sub(before.0),
            fence_waits: self.session.context.fence_waits().saturating_sub(before.1),
            elapsed,
        };
        match outcome {
            Ok(results) => Ok(Stepped { results, cost }),
            Err(reason) => Err(ServeRefusal::Program(vec![reason])),
        }
    }

    /// How many nodes the held graph dispatches.
    pub fn nodes(&self) -> usize {
        self.graph.nodes().len()
    }
}

impl Session {
    /// Compile `source` once and hold the step its tail recorded.
    ///
    /// # Invariant
    /// The program's tail must be the graph value — `compute.graph step` and nothing after it —
    /// because preparing records a step and dispatches nothing. It compiles twice: the refusal is
    /// asked for first, since `run_at` panics on a refused program, and `line_starts` is not
    /// public, so the one-pass form is unreachable
    /// (`docs/notes/inference-service.md` §8).
    pub fn prepare(&self, source: &str) -> Result<Prepared<'_>, ServeRefusal> {
        self.prepare_with_packs(source, &[])
    }

    /// Compile `source` once, with `packs` installed as graph passes **before** it is evaluated.
    ///
    /// # Invariant
    /// A graph is recorded while the program's root is evaluated, so a pass installed after
    /// [`Session::prepare`] would be applied to a graph that is already recorded. This composes the
    /// language's compile and evaluate around the install, which is the only order that reaches the
    /// recording — and with no packs nothing is installed, so [`Session::prepare`] is this call.
    pub fn prepare_with_packs(
        &self,
        source: &str,
        packs: &[lichen_compute::placement_pack::PlacementPack],
    ) -> Result<Prepared<'_>, ServeRefusal> {
        let mut store = lichen_compute::language::new_store(None)
            .map_err(|error| ServeRefusal::Device(error.to_string()))?;
        if let Err(diagnostics) = lichen_language::run::evaluate_raw(source, None, &mut store) {
            return Err(ServeRefusal::Program(
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.clone())
                    .collect(),
            ));
        }
        let mut store = lichen_compute::language::new_store(None)
            .map_err(|error| ServeRefusal::Device(error.to_string()))?;
        let build = lichen_language::testing::compile_at(source, None, &mut store);
        if !packs.is_empty() {
            lichen_compute::automatic_tiling::install_tiling_with_packs(&build.module, packs);
        }
        let (_module, value, _root) = lichen_language::testing::finish(build);
        let Some(ComputeValue::Graph(id, backend)) = AsEnum::<ComputeValue>::as_enum(&value) else {
            return Err(ServeRefusal::Program(vec![
                "this program's tail is not a recorded graph: preparing wants `compute.graph step` \
                 as the last binding, so that a step is recorded and nothing is dispatched"
                    .into(),
            ]));
        };
        let mut graph = graph::lookup(id).map_err(|reason| ServeRefusal::Program(vec![reason]))?;
        // A `compute.graph` tail is recorded rather than run, so the pass is asked here.
        let extent = |node: usize| -> Option<usize> {
            match graph.nodes().get(node)? {
                lichen_graph_ir::Node::Kernel(kernel) => match kernel.count {
                    lichen_graph_ir::Count::Constant(count) => Some(count),
                    _ => None,
                },
                _ => None,
            }
        };
        if let Some(rewritten) = lichen_compute::compute::consult_graph_pass(&graph, &extent) {
            graph::store(&rewritten);
            graph = rewritten;
        }
        Ok(Prepared {
            session: self,
            graph,
            backend,
        })
    }

    /// Read a resident the device wrote, so a caller fetches only what it asks for.
    pub fn fetch(&self, resident: ResidentBuffer) -> Result<ScalarData, ServeRefusal> {
        self.context
            .fetch(resident.id, resident.count)
            .map_err(|error| ServeRefusal::Device(error.to_string()))
    }

    /// Hand a resident back to the device, so a service that steps for hours does not leak it.
    ///
    /// # Invariant
    /// A resident a step produced is the caller's to release: the session cannot know whether it is
    /// still an input to the next step, and a released id is one the backend may hand out again.
    pub fn release(&self, resident: ResidentBuffer) {
        self.context.release(resident.id);
    }
}
