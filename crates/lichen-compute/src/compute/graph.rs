//! Building a graph by recording an evaluation that has already happened.
//! # The recording *is* the walk
//!
//! A graph is not read out of a function's body and it is not compiled from it.
//! `$graph(f)` **applies** `f` to a tuple of placeholders, through the VM's own
//! `Apply` path, and every dispatch the body performs is intercepted on the way
//! past. Nothing runs: an intercepted dispatch appends a node to the graph under
//! construction and hands back a value number instead of a buffer. So the
//! sequence of nodes **is** the sequence the body performed, and a chain is a
//! chain because the second dispatch's operand already held the first dispatch's
//! placeholder.
//!
//! That is why there is no `produced_by` side table anywhere in this file. The
//! placeholder is sitting in the very node the next `cfg` reads, carrying the
//! value number with it, and a second copy of that fact is a second copy that can
//! disagree with the first.
//!
//! # The inputs arrive through the parameter, and nowhere else
//!
//! The placeholder tuple is one [`ComputeValue::GraphInput`] per **parameter
//! cell**, and the parameter cell is a tuple with one cell per read — so its
//! length *is* the arity, and it is read with no evaluation at all. Every lichen
//! function has exactly one parameter, so there is exactly one channel a value
//! can arrive through that the graph does not have to capture. A buffer the body
//! reads as a free variable is refused **by the capture**, not by a vague
//! "unresolved value": a graph that captured a buffer would have to hold it, and
//! holding a node is a block-lifetime obligation this design does not take on.
//!
//! # What the graph holds
//!
//! **Nothing that belongs to the program.** A built graph is a `usize` naming a
//! registry entry of kernel ids, edge numbers and counts — all plain data, no
//! block, no arena, no device lifetime. So there is nothing to trace, no host
//! obligation to state, and no copy to pay.

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
/// **A number, not a reference**, exactly like a kernel id: the registry is
/// process-local and its ids mean nothing outside the process that issued them.
pub type GraphId = usize;

/// What the registry stores: a graph, and nothing else.
///
/// **The backend is not here, and that is the fix.** It used to sit beside the
/// graph in a `BuiltGraph` while deliberately *not* being part of
/// [`graph_digest`], so the first build of a shape in a process decided the
/// backend for every later build of that shape: a `"cpu"` graph was handed back
/// to a `"gpu"` program, which is then refused for a backend it never named. The
/// registries are process-global, so that crossed program boundaries.
///
/// The backend rides on the **value** instead — [`ComputeValue::Graph`] carries
/// it, exactly as [`ComputeValue::ParKernel`] carries a parallel kernel's — which
/// is what the digest's reasoning already assumed. What a graph *is* is
/// content-addressed; which device runs it is a property of the run, so it is
/// read off the value at run time and the registry stores no answer to that
/// question.
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
/// **`Debug` is the canonical form**, the same choice
/// [`lichen_kernel_ir::fragment_digest`] makes and for the same reason: it is a
/// total, deterministic function of every field, so two graphs differing in any
/// of them cannot collapse into one identity and have the registry serve one
/// graph's shape for another's.
///
/// The backend is **not** hashed, for the same reason it is not part of a
/// fragment: it is a property of how the graph is *run*, not of what it computes.
/// **And it is not stored either**, which is what makes the reasoning true — a
/// key that left it out while the entry kept it would let the first build of a
/// shape answer for every later one. It rides on [`ComputeValue::Graph`] instead.
fn graph_digest(graph: &Graph) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{graph:?}").hash(&mut hasher);
    hasher.finish()
}

/// Put a built graph in the registry, or hand back the id it already has.
///
/// Content-addressed like a kernel, and for the same reason: `compute.graph`
/// reached in a hot path would otherwise re-record the same body every time, and
/// the recording is the expensive half.
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

/// A recording in progress, reached by a dispatch without an argument to be given
/// one.
///
/// **It holds the nodes itself and builds the `Graph` at the end**, and that is
/// the whole reason a recording does not build one as it goes. The graph's
/// declared width is the number of argument slots its nodes read, and that number
/// is not knowable until the last dispatch has been recorded: a function's read
/// positions are invisible before it is applied, so the placeholder tuple is built
/// at [`MAX_GRAPH_INPUTS`] and the real width only exists once the body is done.
/// A `Graph` built during the walk would have to declare the ceiling, and then
/// either lie about its arity or be renumbered afterwards — and renumbering a
/// finished graph means moving every produced value, which is exactly the kind of
/// edit that can be got subtly wrong.
struct Recording {
    /// One entry per recorded dispatch, in the order the body performed them.
    nodes: Vec<RecordedNode>,
    /// Ceiling slot to the value-table index that slot became, or `None` for a
    /// slot no dispatch read. **Allocated on first use**, so a body that reads
    /// only `ins(0)` leaves a one-input graph rather than a sixty-four-input one
    /// with two edges.
    inputs: Vec<Option<ValueId>>,
    /// Which values the source function returns, in the recording's own
    /// numbering. `None` until it says, and `None` is **not** "returns nothing":
    /// it is "nobody has said", which a caller reads as *take everything*. The
    /// distinction matters because an empty list is a real answer — a function
    /// returning a unit — and silently meaning it by forgetting to record would
    /// make a forgotten call indistinguishable from a deliberate one.
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
/// **The value form is an [`Edge`], and that is the whole point.** A count read
/// from a parameter and a count read from an earlier node's output land in
/// different id spaces at the end — one is already final, the other has the input
/// count added in front of it. Resolving either during the walk therefore makes
/// the later shift wrong for the other, and the walk cannot tell which it
/// resolved. So this carries the same two kinds an edge does and
/// [`finish`] is the one place that turns either into a value number.
#[derive(Debug, Clone, Copy)]
enum RecordedCount {
    Constant(usize),
    Value(Edge),
}

/// An edge, in the two kinds a recorded dispatch can have.
///
/// **Kept apart because the two get different numbers at the end**: an input is
/// whatever slot the body read, which the recording only discovers as it goes,
/// while a produced value is numbered from zero and has the input count added in
/// front of it once the input count is known. A single `usize` would have to
/// guess which of the two a given number is, and the guess is wrong the moment a
/// body reads its eleventh argument and produces its tenth value.
#[derive(Debug, Clone, Copy)]
enum Edge {
    Input(usize),
    Produced(ValueId),
}

thread_local! {
    /// A **stack**, not one slot, because a body may build a graph of its own:
    /// `$graph f` where `f` calls `$graph g` must record `g`'s dispatches into
    /// `g`. With a single slot the inner build would clobber the outer and the
    /// outer graph would come back holding another graph's nodes.
    static RECORDINGS: RefCell<Vec<Recording>> = const { RefCell::new(Vec::new()) };
}

/// Is a recording in progress **on this thread**?
///
/// That word is load-bearing. A dispatch is intercepted on the thread that
/// evaluates the body, and one graph is built by one evaluation, so a dispatch on
/// another thread is not part of this graph and has to run normally. A
/// process-wide flag would let an unrelated dispatch be recorded into a graph it
/// has no edge in — a graph whose value table would then be off by whatever that
/// dispatch produced.
pub fn is_recording() -> bool {
    RECORDINGS.with(|recordings| !recordings.borrow().is_empty())
}

/// Start a recording whose argument tuple is built at [`MAX_GRAPH_INPUTS`].
///
/// **A ceiling, and it is not a design preference — it is forced.** The width a
/// function's argument tuple needs is not knowable before the function is
/// applied: a read of `ins(i)` compiles to a bare cell with no operation and no
/// subscript, so the unapplied body contains nothing that says which slot a read
/// wants. The parameter's own value node cannot answer it either, because in a
/// template that value is still an undecided cell rather than a tuple. So the
/// tuple has to exist before the read is resolved, and its width is a guess with
/// a bound on it.
///
/// The bound is **Rust-side only**, like the submission pool's depth: it is a
/// property of how a recording works, not something a program chooses, and
/// putting it in the language would let a program's correctness depend on a
/// number nobody reading it can see. Reading past it is the VM's own index
/// refusal, and the graph the recording produced is trimmed to the width the body
/// actually used.
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

/// Finish the innermost recording and hand back what it built: the graph, and the
/// one backend every dispatch in it named.
///
/// **The backend comes out of the recording rather than out of the registry**,
/// because a recording is the only place the agreement is knowable: the check
/// that every dispatch named the same backend runs here, and its answer is what
/// the caller puts on the value.
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
        // The width, from the slots that were actually read. A `None` at the top
        // would mean a read past the ceiling, which the VM's own index refusal
        // catches first — so the last slot is the highest one there is.
        let width = recording
            .inputs
            .iter()
            .rposition(Option::is_some)
            .map_or(0, |highest| highest + 1);
        let mut graph = Graph::with_inputs(width);
        // **One resolution, for edges and counts alike.** An `Input` edge is
        // already final — it is the number its slot was allocated — while a
        // `Produced` one is numbered from zero and has `width` put in front of
        // it here. Doing this in two places is how a count read from a parameter
        // ends up shifted as though it were a node's output, which is a value
        // number that exists, so the graph runs and answers with a buffer's
        // length instead of its extent.
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
        // The return is the last thing recorded and the first thing resolved: its
        // edges are the recording's own, so they move with everything else — and
        // through the same `resolve`, so a returned extent keeps the number its
        // slot was allocated.
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
    /// The caller's `slot`-th argument. **The slot is the number**, so `ins(i)`
    /// is the `i`-th argument and no ordering has to be guessed.
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
    /// **Carries the same [`Placed`] an edge does, and for the same reason.** A
    /// count is read from the function's parameter just as often as a buffer is,
    /// so a body that takes its extent as its first argument has an *input* and
    /// the graph has to take one. An extent that only knew "a value" would
    /// quietly produce a graph with no inputs and a `graphrun` call that cannot
    /// be made.
    ///
    /// **Kept apart from `Constant` because a count that depends on data is
    /// ordinary** — a length off a `collect` is the common case — and collapsing
    /// the two would mean inventing a node that produces a number for free,
    /// doing no work.
    Value(Placed),
}

/// Read a value the body produced as a graph value, or name what it was.
///
/// **This is the filter, and it is where arbitrary values are sorted into roles.**
/// A jit'd function may be handed any lichen value; what a graph can place in its
/// value table is an input slot or one of its own nodes' outputs. Anything else is
/// refused here by name, and a live buffer gets its own message because it is a
/// *capture* rather than a value in the wrong slot.
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
/// **One value per output the fragment declares**, and several are the tuple of
/// them, which is the same shape a real launch produces — so the body downstream
/// cannot tell a recording from a run, and the code that reads a result is the
/// code that already worked.
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
        // The first value this recording has produced, numbered from zero: the
        // input count goes in front of it at the end, and it cannot be known now.
        let first = recording
            .nodes
            .iter()
            .map(|node| node.fragment.outputs)
            .sum::<usize>();
        // An input slot is **allocated the first time a dispatch reads it**, so
        // the graph's width is the number of arguments the body actually read and
        // not the ceiling the placeholder tuple had to be built at. A slot read
        // twice keeps the number it was first given, which is what makes `ins(0)`
        // in two dispatches the same argument.
        // An input slot is **allocated the first time anything reads it**, by a
        // dispatch's buffer list or by its count. The two go through the same
        // allocator because a body that takes its extent as its first argument has
        // an input just as much as one that takes a buffer, and a graph that only
        // counted the buffers would take no arguments and could not be run.
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
/// **Once, from the function's own return, and never inferred from the tail.** A
/// graph whose last node is dead cannot express its return at all if "the last
/// node" is the rule, and what a function returns is a decision about what the
/// program computes rather than about what happened to run last. Recording twice
/// is refused rather than merged, because a lowering with two opinions about its
/// own function's return is a wrong answer wearing a working graph.
///
/// **The return is a list of [`Placed`]s for the reason a node's inputs are.** A
/// function is allowed to return its own extent, and an extent is an *input*
/// value — so a return that names a placeholder has to be able to say which of
/// the two it is. Resolving either kind during the walk is what made a returned
/// count come back as a node's output, and leaving it unresolved is what made
/// the whole value table stand in for a return nobody recorded.
pub fn record_return(values: Vec<Placed>) -> Result<(), String> {
    RECORDINGS.with(|recordings| {
        let mut recordings = recordings.borrow_mut();
        let Some(recording) = recordings.last_mut() else {
            return Err("a return was recorded with no recording in progress".into());
        };
        if recording.returns.is_some() {
            // **Refused, not merged**: a lowering that answers "what does this
            // function return" twice has two opinions, and letting the second one
            // win is a wrong answer that still runs.
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
/// **Sorted by the caller, not by this module**, because deciding which is which
/// is a question about a lichen value and only the language can answer it. What
/// arrives here is one role per value, and a value that is neither of these has
/// been refused by name already.
pub enum RunArgument {
    /// Data the host already has, which a dispatch records against as host data.
    ///
    /// **Owned, not borrowed**, because the caller is still holding the module
    /// that owns the payload's arena and has to be able to keep using it — the
    /// alternative is a borrow that would make every refusal in the calling loop
    /// unreachable to write.
    ///
    /// **The class travels with the bytes**, because what an element occupies is
    /// [`ScalarClass::byte_width`] and not a constant: a run that dropped the
    /// class would hand a device integers where the program computed floats.
    Buffer { class: ScalarClass, data: Vec<u8> },
    /// A buffer a previous run left on a device, handed over as the id it is.
    Resident(ResidentBuffer),
    /// A number, which a dispatch may read as its extent.
    Count(i64),
}

/// Run a built graph on the backend its **value** names, and hand back what its
/// source function returned, already separated by the role each value is in.
///
/// **The result is left where it is.** A value the device wrote stays a resident
/// id and the caller downloads it when it asks, which is the same discipline a
/// single launch follows; a run that fetched everything on the way out would put
/// the download back on the path of every result.
///
/// Everything happens in here, and the value table never leaves, because a
/// `Value` can be a submission and a submission is consumed by its wait — so
/// there is no honest way to hand one out and read it after the backend handle
/// that owns it is gone.
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
/// **Matched rather than asked, and the difference is who is doing the asking.**
/// [`Value::slot`] and [`Value::as_number`] are the runner's two filters: they
/// exist to answer a *demand* — node 7 wanted a buffer, node 8 wanted an extent —
/// and so they refuse, which is right there and wrong here. A return is not a
/// demand any node made, so the two filters have nothing to refuse and a third
/// question is the honest one: a value is a number, host data, or a device
/// buffer, and which one it is has to become a `RunResult` either way.
///
/// The old version asked for the buffer and caught the refusal, so a number was
/// recognised by the fact that it had been *refused*. That made the readback
/// depend on a diagnostic it was about to throw away, and it would have broken
/// outright the day a refusal wanted to say which node asked — a number
/// classified here has no node, because no node asked for it.
///
/// **A number is one of the three rather than a refusal**, and that is what makes
/// the "nobody recorded a return, so take every value" answer above actually
/// work. A graph whose extent is one of its own arguments has that number in its
/// value table, so a graph that takes everything meets a number — and a function
/// is allowed to return its own extent. Refusing here would make the permissive
/// answer unreachable for exactly the graphs that most need it.
///
/// **The extent is decided here, because this is the one place a count is not
/// asked for by a node.** A node's count becomes a `usize` in the runner, which
/// is about to dispatch over it; a returned number becomes one here, on its way
/// to the caller's value table. One rule, both callers, and the `i64` never
/// reaches a `usize` slot without the conversion in between.
/// **The class is read off the value, and that is the whole point of carrying it
/// there.**  A `Value` says which class its elements are, whether it came from a
/// host input, a device the backend wrote, or a submission that has since been
/// waited for — so a float graph result comes home as a float buffer rather than
/// as the bits of one read as integers
/// (`docs/notes/floating-point.md` §3.8, §4.4).
fn returned_role(value: &Value<'_>) -> Result<RunResult, GraphRefusal> {
    Ok(match value {
        Value::Host { data, class } => RunResult::Buffer {
            class: *class,
            data: data.to_vec(),
        },
        // The pending arm is a shape the match has to name rather than one it
        // expects: the runner settles every value before it hands the table back,
        // so a resident here has been waited for. Naming it anyway costs one arm
        // and means a readback that somehow met a submission would name the
        // buffer rather than disagree with the runner about what it is.
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
    /// Data on the host, which becomes an arena buffer — with the class its
    /// elements are, so the buffer the caller builds says what it holds, and the
    /// bytes its class's width packs.
    Buffer { class: ScalarClass, data: Vec<u8> },
    /// A buffer still on a device, which stays a resident id.
    Resident(ResidentBuffer),
    /// A number, which a function is allowed to have returned — its own extent,
    /// most often, since that is the one number a graph always has.
    ///
    /// **`usize` rather than the `i64` the value table holds**, because this is
    /// the last point at which a count can be checked and the next one is a
    /// `LowValue::USize`. Handing an `i64` to that slot is a `as usize` in
    /// somebody's `match`, and an `as usize` is how `-1` becomes a very large
    /// number that looks like a length.
    Count(usize),
}
