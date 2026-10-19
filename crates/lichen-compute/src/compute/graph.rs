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
    Count, Graph, GraphRefusal, KernelNode, Node, Policy, Runner, Scalar as GraphScalar, Value,
    ValueId,
};
use lichen_kernel_ir::{KernelFragment, ParallelBackend, ResidentId, ScalarClass, ScalarLeaf};
use lichen_lowlevel::LowValue;

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

/// Put a rewritten graph in the store under **its own** digest, and answer the id that names it.
///
/// # Invariant
/// A digest names the bytes it stores. A rewritten graph's bodies differ, so its digest does and
/// it takes an entry of its own: the recording's entry is left as the program wrote it.
///
/// Interning rather than writing over that entry is what keeps repeated runs from growing the
/// store — a rewrite that is the same bytes is the same id.
pub fn store(graph: &Graph) -> GraphId {
    intern(graph.clone())
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
    /// Which argument slots some dispatch read, one entry per slot up to the highest.
    ///
    /// # Invariant
    /// **Occupancy, not a value number.** The numbers are assigned once in [`finish`],
    /// in slot order, because a slot is reached when the recording reaches it and a
    /// parameter's extents are not at slots a body visits in order — numbering as they
    /// are reached would give slot 1 a higher number than slot 2. The width, the
    /// numbering and the edges are then three readings of this one list.
    inputs: Vec<bool>,
    /// Which values the source function returns, in the recording's own numbering.
    ///
    /// # Invariant
    /// `None` is "nobody has said", not "returns nothing" — a caller reads it as take everything,
    /// and an empty list is the real answer for a unit.
    returns: Option<Vec<Edge>>,
    backend: Option<Backend>,
    /// What refused while this recording was open, in the order it refused.
    ///
    /// # Invariant
    /// A refusal is a fact about the recording and not only about the module that
    /// reported it: `build_graph` refuses on the body's **return**, which is a
    /// symptom whenever one of these is present, so the reason has to travel here
    /// rather than stay a side effect at the dispatch that raised it.
    refusals: Vec<String>,
}

/// One recorded dispatch, with its edges in the recording's own numbering.
struct RecordedNode {
    fragment: KernelFragment,
    inputs: Vec<Edge>,
    count: RecordedCount,
    scalars: Vec<RecordedScalar>,
}

/// A recorded runtime scalar, in the recording's own numbering.
///
/// # Invariant
/// The same two arms [`RecordedCount`] has, for the same reason: the recording numbers an
/// edge only at [`finish`], and a scalar read from the parameter and one read from an
/// earlier node's output land in different id spaces.
#[derive(Debug, Clone, Copy)]
enum RecordedScalar {
    Constant(ScalarLeaf),
    Value(Edge),
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
            refusals: Vec::new(),
        })
    });
}

/// Note a refusal in the innermost recording, if one is open.
///
/// # Invariant
/// A refusal outside any recording is nothing to record here: there is no graph
/// whose return it could explain, and the module's own diagnostic already carries it.
pub fn record_refusal(reason: &str) {
    RECORDINGS.with(|recordings| {
        if let Some(recording) = recordings.borrow_mut().last_mut() {
            recording.refusals.push(reason.to_string());
        }
    })
}

/// Close the innermost recording without building a graph, and hand back what refused.
///
/// # Invariant
/// **Every path out of a recording closes it**, because an open one keeps intercepting
/// dispatches: a refused `compute.graph` would otherwise swallow every launch after it,
/// recording them into a graph that is never built. The refusals come out with it, since
/// closing the recording is the only handle `build_graph` has on them.
pub fn abandon() -> Vec<String> {
    RECORDINGS.with(|recordings| {
        recordings
            .borrow_mut()
            .pop()
            .map_or_else(Vec::new, |recording| recording.refusals)
    })
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
        let read = recording.inputs.iter().copied().collect::<Vec<bool>>();
        let width = read
            .iter()
            .rposition(|occupied| *occupied)
            .map_or(0, |highest| highest + 1);
        let mut graph = Graph::with_inputs(width);
        // One resolution for edges and counts alike, **dense over the read slots in
        // slot order** — see [`Recording::inputs`].
        let mut numbers: Vec<ValueId> = vec![0; read.len()];
        let mut next = 0;
        for (slot, occupied) in read.iter().enumerate() {
            if *occupied {
                numbers[slot] = next;
                next += 1;
            }
        }
        let resolve = |edge: &Edge| match edge {
            Edge::Input(slot) => numbers[*slot],
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
                        scalars: node
                            .scalars
                            .iter()
                            .map(|scalar| match scalar {
                                RecordedScalar::Constant(leaf) => GraphScalar::Constant(*leaf),
                                RecordedScalar::Value(edge) => GraphScalar::Value(resolve(edge)),
                            })
                            .collect(),
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

/// The runtime scalars a recorded dispatch fixes beside its extent: already known, or
/// read when the node runs.
///
/// # Invariant
/// The same two arms [`Extent`] has and for the same reason — a parameter's second leaf is
/// a value as often as a literal is — but the number a leaf carries is the leaf's own bits
/// at the leaf's own class, because a leaf is a word rather than an extent. A leaf's class is
/// read off the fragment when the graph is built, so the two cannot disagree.
#[derive(Debug, Clone, Copy)]
pub enum Scalar {
    /// A literal the build already had.
    Constant(ScalarLeaf),
    /// A value in the table, read when the node runs.
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
             the caller's arguments — a buffer, or a number that is an integer or a float — and \
             the values the graph's own nodes produced",
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

/// What a low-level value is, for a refusal that has to say.
///
/// # Invariant
/// The same job [`describe`] does for a compute value and never a `Debug` render: a
/// handle's address differs between runs and names nothing a reader can act on, while
/// what the value **is** is the same sentence on every run. A number and a string carry
/// the number and the text, because those are the ones a reader can act on.
pub fn describe_low_value(value: &LowValue) -> String {
    match value {
        LowValue::USize(number) => format!("the number {number}"),
        LowValue::Float(number) => format!("the float {number:?}"),
        LowValue::Str(text) => format!("the text {text:?}"),
        LowValue::Array(_) => "a tuple or a struct value".to_string(),
        LowValue::Table(_) => "a table".to_string(),
        LowValue::Function(_) => "a function".to_string(),
        LowValue::None => "the empty value".to_string(),
        LowValue::Error => "the value of a read that failed".to_string(),
        LowValue::Any => "a value this run has not decided".to_string(),
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
    scalars: &[Scalar],
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
        // An input slot is marked on first read, so the graph's width is what the body
        // read, not the placeholder cells.

        // A dispatch's count marks through the same allocator.
        let allocate = |recording: &mut Recording, slot: usize| {
            if recording.inputs.len() <= slot {
                recording.inputs.resize(slot + 1, false);
            }
            recording.inputs[slot] = true;
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
        // Each runtime scalar marks through the same allocator as a count, in the
        // parameter's own leaf order.
        let scalars: Vec<RecordedScalar> = scalars
            .iter()
            .map(|scalar| match scalar {
                Scalar::Constant(leaf) => RecordedScalar::Constant(*leaf),
                Scalar::Value(placed) => RecordedScalar::Value(edge(recording, placed)),
            })
            .collect();
        recording.nodes.push(RecordedNode {
            fragment,
            inputs: edges,
            count,
            scalars,
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
/// only the language can answer it. What the table ends up holding is a **buffer or a number, an
/// integer or a float**, and a number's class is the arm it arrives in.
pub enum RunArgument {
    /// Data the host already has, which a dispatch records against as host data.
    ///
    /// # Invariant
    /// Owned, not borrowed, because the caller still holds the module that owns the arena. The
    /// class travels with the bytes: what an element occupies is [`ScalarClass::byte_width`] and
    /// not a constant, so a run that dropped the class would hand a device integers where the
    /// program computed floats.
    Buffer { class: ScalarClass, data: Vec<u8> },
    /// The same, sharing bytes the caller keeps: a reference count rather than a copy.
    BufferShared {
        class: ScalarClass,
        data: std::sync::Arc<[u8]>,
    },
    /// A buffer a previous run left on a device, handed over as the id it is.
    Resident(ResidentBuffer),
    /// A number, which a dispatch may read as its extent.
    ///
    /// # Invariant
    /// An extent has no class to carry, so this arm is [`ScalarClass::Int`] and nothing else:
    /// a `Float` is its own arm below rather than a flag here.
    Count(i64),
    /// A machine float, which a launch may read into a leaf its parameter declared `Float`.
    ///
    /// # Invariant
    /// Its own arm rather than a class beside [`Self::Count`], so the class travels with the
    /// value the whole way into the table and a run reads a leaf at the class it declared
    /// rather than at whichever one the argument carried.
    Float(f32),
}

/// Run a built graph on the backend its value names, and hand back the source
/// function's return, separated by role.
///
/// # Invariant
/// The result is left where it is: a value the device wrote stays a resident id and the caller
/// downloads it when it asks, because a `Value` can be a submission and a wait consumes it.
///
/// **The graph is the caller's**, looked up and possibly rewritten before this point: a graph a
/// pass handed back is the one that has to run.
pub fn run(
    graph: &Graph,
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
    if let Backend::Cpu = backend_name {
        return Err(
            "this graph was compiled for the \"cpu\" backend, and a graph runs through the \
             ParallelBackend contract — which the cpu path is not: it is this crate's own wasm \
             engine, driven by a compiled module rather than by a submitted run"
                .into(),
        );
    }
    let graph = &aligned(graph, &arguments)?;
    // The ids the caller brought in, taken before the arguments move: a run never gives back a
    // buffer it was handed.
    let mut kept: Vec<ResidentId> = arguments
        .iter()
        .filter_map(|argument| match argument {
            RunArgument::Resident(resident) => Some(resident.id),
            _ => None,
        })
        .collect();
    // The arguments are this call's, so their bytes move: a clone here copies every payload.
    let inputs: Vec<Value<'_>> = arguments
        .into_iter()
        .map(|argument| match argument {
            RunArgument::Buffer { class, data } => Value::host_data(class, data),
            RunArgument::BufferShared { class, data } => Value::host_shared(class, data),
            RunArgument::Resident(resident) => {
                Value::device(resident.id, resident.count, resident.class)
            }
            RunArgument::Count(count) => Value::int(count),
            RunArgument::Float(number) => Value::float(number),
        })
        .collect();
    let table = Runner::new(&*backend, policy)
        .run(graph, inputs)
        .map_err(|refusal| format!("this graph could not be run: {refusal}"))?;
    let returned: Vec<ValueId> = graph
        .returns()
        .map(<[ValueId]>::to_vec)
        .unwrap_or_else(|| (0..table.len()).collect());
    let results: Vec<RunResult> = returned
        .iter()
        .map(|value| {
            let entry = table.get(*value).ok_or(GraphRefusal::UnknownValue {
                node: usize::MAX,
                value: *value,
            })?;
            returned_role(entry)
        })
        .collect::<Result<_, GraphRefusal>>()
        .map_err(|refusal| format!("this graph could not be read back: {refusal}"))?;
    kept.extend(results.iter().filter_map(|result| match result {
        RunResult::Resident(resident) => Some(resident.id),
        _ => None,
    }));
    given_back(&*backend, &table, &kept);
    Ok(results)
}

/// Hand back every buffer the run produced that the caller is not holding.
///
/// # Invariant
/// **A value nothing returns and the caller did not bring in is a temporary**, and a temporary's
/// device memory is the run's alone: keeping it would make what a run costs the device a function
/// of how many intermediates it had rather than of what it returned. An id in `kept` is the
/// caller's, and each id is released once however many entries carry it.
fn given_back(backend: &dyn ParallelBackend, table: &[Value<'_>], kept: &[ResidentId]) {
    let mut released: Vec<ResidentId> = Vec::new();
    for value in table {
        let Ok(id) = value.device_buffer() else {
            continue;
        };
        if kept.contains(&id) || released.contains(&id) {
            continue;
        }
        released.push(id);
        backend.release(id);
    }
}

/// State one element class along each chain of values a graph runs, or refuse a crossing.
///
/// # Invariant
/// A class is the **buffer's**: a value has the class its producer wrote, and a reader naming another
/// moves a different number of bytes per element. Within the real family the reader is re-classed, body
/// accesses included; across the families the run is refused, because nothing converts.
fn aligned(graph: &Graph, arguments: &[RunArgument]) -> Result<Graph, String> {
    let mut classes: Vec<Option<ScalarClass>> = vec![None; graph.value_count()];
    for (index, argument) in arguments.iter().enumerate().take(graph.input_count()) {
        if let Some(entry) = classes.get_mut(index) {
            *entry = match argument {
                RunArgument::Buffer { class, .. } | RunArgument::BufferShared { class, .. } => {
                    Some(*class)
                }
                RunArgument::Resident(resident) => Some(resident.class),
                RunArgument::Count(_) | RunArgument::Float(_) => None,
            };
        }
    }
    let mut aligned = Graph::with_inputs(graph.input_count());
    let mut next = aligned.input_count();
    for position in 0..graph.nodes().len() {
        let mut node = graph.nodes()[position].clone();
        let Node::Kernel(kernel) = &node else {
            continue;
        };
        let mut stated: Option<ScalarClass> = None;
        for (slot, value) in kernel.inputs.iter().enumerate() {
            let held = classes.get(*value).copied().flatten();
            let declared = kernel.fragment.input_classes.get(slot).copied();
            let (Some(held), Some(declared)) = (held, declared) else {
                continue;
            };
            if held == declared {
                continue;
            }
            if !(held.is_real() && declared.is_real()) {
                return Err(format!(
                    "node {position} reads input {slot} as {declared:?}, and the value it reads was \
                     written as {held:?}: a class is the buffer's, and nothing converts between the real \
                     and integer families"
                ));
            }
            stated = Some(held);
        }
        if let (Some(class), Node::Kernel(kernel)) = (stated, &mut node) {
            kernel.fragment.states_the_class(class);
        }
        let Node::Kernel(kernel) = &node else {
            continue;
        };
        for (slot, class) in kernel.fragment.output_classes.iter().enumerate() {
            if let Some(entry) = classes.get_mut(next + slot) {
                *entry = Some(*class);
            }
        }
        next += kernel.fragment.outputs;
        let outputs = node.outputs();
        aligned
            .push(node, outputs)
            .map_err(|refusal| format!("this graph could not be rebuilt: {refusal}"))?;
    }
    if let Some(returns) = graph.returns() {
        aligned
            .returning(returns.to_vec())
            .map_err(|refusal| format!("this graph could not be rebuilt: {refusal}"))?;
    }
    Ok(aligned)
}

/// What one entry of a finished run is, for the caller to turn into a value.
///
/// # Invariant
/// Matched rather than asked, because the difference is who asks: [`Value::slot`] and
/// [`Value::as_number`] answer a *demand* and refuse, which is right in the runner and wrong here,
/// since a return is not a demand any node made. A number is therefore one of the four, and the
/// extent is decided here — a float is handed back as the float the table held.
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
        Value::Float(number) => RunResult::Float(*number),
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
    /// A float the function returned, with the class it was held at.
    Float(f32),
}
