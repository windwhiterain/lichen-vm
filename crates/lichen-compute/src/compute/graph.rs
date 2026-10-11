//! Building a graph by recording an evaluation that has already happened.
//! The graph-recording extension: `$graph`.
//!
//! # Invariant
//! `$graph(f)` applies `f` to placeholders through the VM's own `Apply` path and intercepts each
//! dispatch, so the node sequence is the sequence the body performed — nothing runs, and a
//! dispatch appends a node and hands back a value number. The placeholder sits in the very node
//! the next dispatch reads, so no side table exists to disagree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use lichen_graph_ir::{
    Count, Graph, GraphRefusal, KernelNode, Node, Policy, Runner, Value, ValueId,
};
use lichen_kernel_ir::{KernelFragment, ScalarClass};

use super::{Backend, ComputeValue, ResidentBuffer};

/// A built graph's slot in the process registry.
///
/// # Invariant
/// A number, not a reference: the registry is process-local.
pub type GraphId = usize;

/// What the registry stores: a graph, and nothing else.
///
/// # Invariant
/// The backend is not here: it used to sit beside the graph while not being part of
/// [`graph_digest`], so the first build of a shape decided the backend for every later one. It
/// rides on the value instead, exactly as a parallel kernel's does — what a graph *is* is
/// content-addressed; which device runs it is a property of the run.
fn graphs() -> &'static Mutex<HashMap<GraphId, Graph>> {
    static GRAPHS: OnceLock<Mutex<HashMap<GraphId, Graph>>> = OnceLock::new();
    GRAPHS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn digests() -> &'static Mutex<HashMap<u64, GraphId>> {
    static DIGESTS: OnceLock<Mutex<HashMap<u64, GraphId>>> = OnceLock::new();
    DIGESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A content digest of a graph's shape, which is what makes two builds one id.
///
/// # Invariant
/// `Debug` is the canonical form, as in [`lichen_kernel_ir::fragment_digest`]: a total,
/// deterministic function of every field, so two graphs differing in one cannot collapse. The
/// backend is neither hashed nor stored — a key that left it out while the entry kept it would
/// let the first build of a shape answer for every later one.
fn graph_digest(graph: &Graph) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{graph:?}").hash(&mut hasher);
    hasher.finish()
}

/// Put a built graph in the registry, or hand back the id it already has.
///
/// # Invariant
/// Content-addressed like a kernel: otherwise `compute.graph` in a hot path re-records the same
/// body every time, and the recording is the expensive half.
pub fn intern(graph: Graph) -> GraphId {
    let digest = graph_digest(&graph);
    let mut known = digests().lock().unwrap();
    if let Some(id) = known.get(&digest) {
        return *id;
    }
    let mut graphs = graphs().lock().unwrap();
    let id = graphs.len() as GraphId;
    graphs.insert(id, graph);
    known.insert(digest, id);
    id
}

/// Read a graph back, or say that this process never built it.
pub fn lookup(id: GraphId) -> Result<Graph, String> {
    graphs()
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("graph {id} is not registered, so another process built it"))
}

// --- the recording -----------------------------------------------------------

/// A recording in progress, reached by a dispatch with no argument to give it.
///
/// # Invariant
/// It holds the nodes itself and builds the `Graph` at the end, because the declared width is the
/// number of argument slots its nodes read and that is not knowable until the last dispatch: a
/// function's read positions are invisible before it is applied, so the cells are built at the
/// parameter's own paths and the real width exists only once the body is done.
struct Recording {
    /// One entry per recorded dispatch, in the order the body performed them.
    nodes: Vec<RecordedNode>,
    /// Ceiling slot to the value-table index it became, or `None` for a slot never read.
    inputs: Vec<Option<ValueId>>,
    /// Which values the source function returns, in the recording's own numbering.
    ///
    /// # Invariant
    /// `None` is "nobody has said", not "returns nothing" — a caller reads it as take everything,
    /// and an empty list is the real answer for a unit.
    returns: Option<Vec<Edge>>,
    backend: Option<Backend>,
}

/// One recorded dispatch, with its edges in the recording's own numbering.
struct RecordedNode {
    fragment: KernelFragment,
    inputs: Vec<Edge>,
    count: RecordedCount,
}

/// A recorded extent, in the recording's own numbering.
///
/// # Invariant
/// The value form is an [`Edge`]: a count read from a parameter and one read from an earlier
/// node's output land in different id spaces at the end, so resolving either during the walk
/// makes the later shift wrong and the walk cannot tell which it resolved. [`finish`] is the one
/// place that turns either into a value number.
#[derive(Debug, Clone, Copy)]
enum RecordedCount {
    Constant(usize),
    Value(Edge),
}

/// An edge, in the two kinds a recorded dispatch can have.
///
/// # Invariant
/// Kept apart because the two get different numbers at the end: an input is whatever slot the
/// body read, while a produced value is numbered from zero with the input count added in front. A
/// single `usize` would have to guess which it is.
#[derive(Debug, Clone, Copy)]
enum Edge {
    Input(usize),
    Produced(ValueId),
}

thread_local! {
    /// A stack, not one slot, because a body may build a graph of its own.
    ///
    /// # Invariant
    /// With a single slot an inner `$graph` would clobber the outer and the outer graph would come
    /// back holding another graph's nodes.
    static RECORDINGS: RefCell<Vec<Recording>> = const { RefCell::new(Vec::new()) };
}

/// Is a recording in progress on this thread?
///
/// # Invariant
/// The thread is load-bearing: one graph is built by one evaluation, so a dispatch on another
/// thread is not part of it and has to run normally. A process-wide flag would let an unrelated
/// dispatch be recorded into a graph it has no edge in.
pub fn is_recording() -> bool {
    RECORDINGS.with(|recordings| !recordings.borrow().is_empty())
}

/// The ceiling a parameter that is not the named struct has its argument tuple built at.
///
/// # Invariant
/// A ceiling, and forced rather than preferred: a read of `ins(i)` compiles to a bare cell with
/// no operation and no subscript, so an unapplied body says nothing about which slot a read wants,
/// and the parameter's own value is an undecided cell rather than a tuple. The bound is Rust-side
/// only, like the submission pool's depth, and reading past it is the VM's own index refusal.
pub const MAX_GRAPH_INPUTS: usize = 64;

/// Start a recording.
pub fn begin() {
    RECORDINGS.with(|recordings| {
        recordings.borrow_mut().push(Recording {
            nodes: Vec::new(),
            inputs: Vec::new(),
            returns: None,
            backend: None,
        })
    });
}

/// Finish the innermost recording and hand back what it built.
///
/// # Invariant
/// The backend comes out of the recording rather than the registry, because a recording is the
/// only place the agreement is knowable: the check that every dispatch named the same backend runs
/// here.
pub fn finish() -> Result<(Graph, Backend), String> {
    RECORDINGS.with(|recordings| {
        let mut recordings = recordings.borrow_mut();
        let recording = recordings
            .pop()
            .ok_or_else(|| "a graph was finished with no recording in progress".to_string())?;
        let Some(backend) = recording.backend else {
            return Err(format!(
                "this function recorded {} dispatch(es) and names no backend to run them on. A \
                 graph is run by a runner against a backend, and there is no third value to fall \
                 back to",
                recording.nodes.len()
            ));
        };
        // The width, from the slots actually read; a read past the ceiling is the VM's
        // own index refusal.
        let width = recording
            .inputs
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |highest| highest + 1);
        let mut graph = Graph::with_inputs(width);
        // One resolution for edges and counts alike: an `Input` edge is final, a `Produced`
        // one gets `width` put in front.

        // Resolving in two places is how a count read from a parameter ends up shifted as
        // though it were a node's output.
        let resolve = |edge: &Edge| match edge {
            Edge::Input(slot) => {
                recording.inputs[*slot].expect("a recorded input was allocated when it was read")
            }
            Edge::Produced(id) => width + id,
        };
        for node in &recording.nodes {
            let inputs: Vec<ValueId> = node.inputs.iter().map(resolve).collect();
            graph
                .push(
                    Node::Kernel(KernelNode {
                        fragment: node.fragment.clone(),
                        inputs,
                        count: match node.count {
                            RecordedCount::Constant(count) => Count::Constant(count),
                            RecordedCount::Value(edge) => Count::Value(resolve(&edge)),
                        },
                    }),
                    node.fragment.outputs,
                )
                .map_err(|refusal| format!("this graph is malformed: {refusal}"))?;
        }
        // The return is recorded last and resolved first: its edges are the recording's
        // own, so they move with everything else.
        if let Some(returns) = recording.returns {
            graph
                .returning(returns.iter().map(resolve).collect())
                .map_err(|refusal| {
                    format!("this graph's return could not be recorded: {refusal}")
                })?;
        }
        Ok((graph, backend))
    })
}

/// A value the recording has to be able to place in the graph's value table.
#[derive(Debug, Clone, Copy)]
pub enum Placed {
    /// The caller's `slot`-th argument: `ins(i)` is the `i`-th argument.
    Input(usize),
    /// A value an earlier node of this recording produced.
    Value(ValueId),
}

impl Placed {
    /// The edge this is, which for an input *is* its number.
    pub fn edge(self) -> ValueId {
        match self {
            Placed::Input(slot) => slot,
            Placed::Value(id) => id,
        }
    }
}

/// The extent of a recorded dispatch: already known, or read when the node runs.
#[derive(Debug, Clone, Copy)]
pub enum Extent {
    /// A literal, or a value nothing in the graph produced.
    Constant(usize),
    /// A value in the table, read when the node runs.
    ///
    /// # Invariant
    /// It carries the same [`Placed`] an edge does, because a count is read from the parameter as
    /// often as a buffer is: an extent that only knew "a value" would produce a graph with no
    /// inputs. Kept apart from `Constant` because a count that depends on data is ordinary, and
    /// collapsing them would invent a node that produces a number for free.
    Value(Placed),
}

/// Read a value the body produced as a graph value, or name what it was.
///
/// # Invariant
/// The filter, and where arbitrary values are sorted into roles: what a graph can place is an
/// input slot or one of its own outputs, and anything else is refused by name — a live buffer
/// with its own message, because it is a *capture* rather than a value in the wrong slot.
pub fn place(value: &ComputeValue, position: usize) -> Result<Placed, String> {
    match value {
        ComputeValue::GraphInput(slot) => Ok(Placed::Input(*slot)),
        ComputeValue::GraphValue(id) => Ok(Placed::Value(*id)),
        ComputeValue::Buffer(..) | ComputeValue::DeviceBuffer(_) => Err(format!(
            "argument {position} of this dispatch is {} and was not read from this function's \
             parameter, so the graph would have to hold it. A graph holds nothing but kernel \
             ids, edge numbers and counts: a value reaches a graph through the parameter, so \
             pass this one in (ins({position})) rather than closing over it",
            describe(value)
        )),
        other => Err(format!(
            "argument {position} of this dispatch is {}, and a graph's value table holds only \
             the caller's arguments and the values the graph's own nodes produced",
            describe(other)
        )),
    }
}

/// What a value is, for a refusal that has to say.
pub fn describe(value: &ComputeValue) -> &'static str {
    match value {
        ComputeValue::Kernel(_) => "a single-invocation kernel",
        ComputeValue::ParKernel(..) => "a parallel kernel",
        ComputeValue::Buffer(..) => "a buffer",
        ComputeValue::DeviceBuffer(_) => "a buffer that is still on a device",
        ComputeValue::Graph(..) => "a graph",
        ComputeValue::GraphInput(_) => "a placeholder for a graph's own input",
        ComputeValue::GraphValue(_) => "a placeholder for a value the graph has produced",
    }
}

/// Record one dispatch, and hand back what the body should see in its place.
///
/// # Invariant
/// One value per output the fragment declares, assembled into the result shape the parameter
/// declares — the shape a real launch produces, so the body downstream cannot tell a recording
/// from a run.
pub fn record_dispatch(
    fragment: KernelFragment,
    backend: Backend,
    extent: Extent,
    inputs: &[Placed],
) -> Result<Vec<Placed>, String> {
    RECORDINGS.with(|recordings| {
        let mut recordings = recordings.borrow_mut();
        let Some(recording) = recordings.last_mut() else {
            return Err("a dispatch was recorded with no recording in progress".into());
        };
        match recording.backend {
            None => recording.backend = Some(backend),
            Some(seen) if seen == backend => {}
            Some(seen) => {
                return Err(format!(
                    "this graph dispatches on \"{}\" and then on \"{}\", and a graph is run by one \
                     runner against one backend — so the second one would have to be dropped \
                     silently at run time",
                    seen.as_str(),
                    backend.as_str()
                ));
            }
        }
        let outputs = fragment.outputs;
        // The first value this recording produced, numbered from zero; the input count
        // goes in front of it at the end.
        let first = recording
            .nodes
            .iter()
            .map(|node| node.fragment.outputs)
            .sum::<usize>();
        // An input slot is allocated on first read, so the graph's width is what the body
        // read, not the placeholder cells.

        // A dispatch's count allocates through the same allocator.
        let allocate = |recording: &mut Recording, slot: usize| {
            if recording.inputs.len() <= slot {
                recording.inputs.resize_with(slot + 1, || None);
            }
            let next = recording.inputs.iter().flatten().count() as ValueId;
            if recording.inputs[slot].is_none() {
                recording.inputs[slot] = Some(next);
            }
        };
        let edge = |recording: &mut Recording, placed: &Placed| match *placed {
            Placed::Value(id) => Edge::Produced(id),
            Placed::Input(slot) => {
                allocate(recording, slot);
                Edge::Input(slot)
            }
        };
        let count = match extent {
            Extent::Constant(count) => RecordedCount::Constant(count),
            Extent::Value(placed) => RecordedCount::Value(edge(recording, &placed)),
        };
        let edges: Vec<Edge> = inputs
            .iter()
            .map(|placed| edge(recording, placed))
            .collect();
        recording.nodes.push(RecordedNode {
            fragment,
            inputs: edges,
            count,
        });
        Ok((0..outputs)
            .map(|offset| Placed::Value(first + offset))
            .collect())
    })
}

/// Record which values the source function returns.
///
/// # Invariant
/// Once, from the function's own return, never inferred from the tail: a graph whose last node is
/// dead cannot express its return if "the last node" is the rule. The return is a list of
/// [`Placed`]s because a function may return its own extent, which is an *input* value. Recording
/// twice is refused rather than merged.
pub fn record_return(values: Vec<Placed>) -> Result<(), String> {
    RECORDINGS.with(|recordings| {
        let mut recordings = recordings.borrow_mut();
        let Some(recording) = recordings.last_mut() else {
            return Err("a return was recorded with no recording in progress".into());
        };
        if recording.returns.is_some() {
            // Refused, not merged: a lowering with two opinions about its own return.
            return Err(
                "this graph's source function already had its return recorded, and it \
                        has been recorded a second time"
                    .into(),
            );
        }
        recording.returns = Some(
            values
                .into_iter()
                .map(|placed| match placed {
                    Placed::Input(slot) => Edge::Input(slot),
                    Placed::Value(id) => Edge::Produced(id),
                })
                .collect(),
        );
        Ok(())
    })
}
// --- running one -------------------------------------------------------------

/// What a caller hands a run, already separated by role.
///
/// # Invariant
/// Sorted by the caller, not here: deciding which is which is a question about a lichen value, and
/// only the language can answer it.
pub enum RunArgument {
    /// Data the host already has, which a dispatch records against as host data.
    ///
    /// # Invariant
    /// Owned, not borrowed, because the caller still holds the module that owns the arena. The
    /// class travels with the bytes: what an element occupies is [`ScalarClass::byte_width`] and
    /// not a constant, so a run that dropped the class would hand a device integers where the
    /// program computed floats.
    Buffer { class: ScalarClass, data: Vec<u8> },
    /// A buffer a previous run left on a device, handed over as the id it is.
    Resident(ResidentBuffer),
    /// A number, which a dispatch may read as its extent.
    Count(i64),
}

/// Run a built graph on the backend its value names, and hand back the source
/// function's return, separated by role.
///
/// # Invariant
/// The result is left where it is: a value the device wrote stays a resident id and the caller
/// downloads it when it asks. Everything happens in here and the value table never leaves, because
/// a `Value` can be a submission and a wait consumes it.
pub fn run(
    id: GraphId,
    backend_name: Backend,
    arguments: Vec<RunArgument>,
    policy: Policy,
) -> Result<Vec<RunResult>, String> {
    let Some(backend) = lichen_kernel_ir::parallel_backend() else {
        return Err(
            "this graph dispatches to a device, but no compute backend is installed: a host \
             program has to install one before a run can be dispatched"
                .into(),
        );
    };
    let graph = lookup(id)?;
    if let Backend::Cpu = backend_name {
        return Err(
            "this graph was compiled for the \"cpu\" backend, and a graph runs through the \
             ParallelBackend contract — which the cpu path is not: it is this crate's own wasm \
             engine, driven by a compiled module rather than by a submitted run"
                .into(),
        );
    }
    let inputs: Vec<Value<'_>> = arguments
        .iter()
        .map(|argument| match argument {
            RunArgument::Buffer { class, data } => Value::host_data(*class, data.clone()),
            RunArgument::Resident(resident) => {
                Value::device(resident.id, resident.count, resident.class)
            }
            RunArgument::Count(count) => Value::int(*count),
        })
        .collect();
    let table = Runner::new(&*backend, policy)
        .run(&graph, inputs)
        .map_err(|refusal| format!("this graph could not be run: {refusal}"))?;
    let returned: Vec<ValueId> = graph
        .returns()
        .map(<[ValueId]>::to_vec)
        .unwrap_or_else(|| (0..table.len()).collect());
    returned
        .iter()
        .map(|value| {
            let entry = table.get(*value).ok_or(GraphRefusal::UnknownValue {
                node: usize::MAX,
                value: *value,
            })?;
            returned_role(entry)
        })
        .collect::<Result<_, GraphRefusal>>()
        .map_err(|refusal| format!("this graph could not be read back: {refusal}"))
}

/// What one entry of a finished run is, for the caller to turn into a value.
///
/// # Invariant
/// Matched rather than asked, because the difference is who asks: [`Value::slot`] and
/// [`Value::as_number`] answer a *demand* and refuse, which is right in the runner and wrong here,
/// since a return is not a demand any node made. A number is therefore one of the three, and the
/// extent is decided here.
fn returned_role(value: &Value<'_>) -> Result<RunResult, GraphRefusal> {
    Ok(match value {
        Value::Host { data, class } => RunResult::Buffer {
            class: *class,
            data: data.to_vec(),
        },
        // The pending arm is named rather than expected: the runner settles every value
        // before handing the table back.
        Value::Device { id, count, class }
        | Value::Pending {
            id, count, class, ..
        } => RunResult::Resident(ResidentBuffer {
            id: *id,
            count: *count,
            class: *class,
        }),
        Value::Int(number) => usize::try_from(*number)
            .map(RunResult::Count)
            .map_err(|_| GraphRefusal::CountNegative { number: *number })?,
    })
}

/// A returned value, already separated by the role it is in.
pub enum RunResult {
    /// Data on the host, which becomes an arena buffer with the class it holds.
    Buffer { class: ScalarClass, data: Vec<u8> },
    /// A buffer still on a device, which stays a resident id.
    Resident(ResidentBuffer),
    /// A number, which a function is allowed to have returned.
    ///
    /// # Invariant
    /// `usize` rather than the `i64` the value table holds, because this is the last point a count
    /// can be checked: handing an `i64` on is an `as usize` in somebody's `match`, and that is how
    /// `-1` becomes a very large number that looks like a length.
    Count(usize),
}
