//! A dispatching function recorded into a graph and run against a stub backend.
//! See compute-graph-jit.md.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use lichen_kernel_ir::{
    BufferSlot, KernelFragment, ParallelBackend, Pending, ResidentId, ScalarData,
    install_parallel_backend,
};
use lichen_language::package::PackageStore;
use lichen_language::program::{LangProgram, LangValue};
use lichen_lowlevel::{Module, NodeId};

mod common;

/// A backend that computes `sum(inputs)[i] + 1` and remembers what it saw.
#[derive(Clone, Default)]
struct Stub {
    /// One column per resident id, indexed by `id.0 - 1`.
    data: Arc<Mutex<Vec<Vec<i64>>>>,
    /// What each call was handed, in order, so a test can say what reached where.
    saw: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    /// What the stub was asked for, in order: one line per dispatch.
    fn saw(&self) -> Vec<String> {
        self.saw.lock().unwrap().clone()
    }

    /// Forget the log, so one test's dispatches are not read as another's.
    fn forget(&self) {
        self.saw.lock().unwrap().clear();
    }

    /// A host slot is packed bytes at the class's width; every fragment this stub
    /// is handed is an `Int` one.
    fn column(&self, slot: &BufferSlot<'_>) -> Vec<i64> {
        match slot {
            BufferSlot::Host(data) => data
                .chunks_exact(8)
                .map(|word| i64::from_le_bytes(word.try_into().unwrap_or_default()))
                .collect(),
            BufferSlot::Resident(id) => self.data.lock().unwrap()[id.0 as usize - 1].clone(),
        }
    }
}

impl ParallelBackend for Stub {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn run(
        &self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, String> {
        let submission = self.submit(fragment, inputs, count)?;
        Ok(submission.outputs().to_vec())
    }

    fn submit<'backend>(
        &'backend self,
        _fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Box<dyn Pending + 'backend>, String> {
        let columns: Vec<Vec<i64>> = inputs.iter().map(|slot| self.column(slot)).collect();
        // The line records which values reached this node, so a mis-wired chain shows.
        self.saw.lock().unwrap().push(format!(
            "over [0, {count}) with {} input(s) of length {}",
            columns.len(),
            columns.first().map(Vec::len).unwrap_or(0)
        ));
        let out: Vec<i64> = (0..count)
            .map(|i| columns.iter().map(|column| column[i]).sum::<i64>() + 1)
            .collect();
        let mut data = self.data.lock().unwrap();
        data.push(out);
        let id = ResidentId(data.len() as u64);
        drop(data);
        Ok(Box::new(StubPending { id }))
    }

    /// A host stub's buffers hold integers, so it answers with the class it
    /// holds them as.
    fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, String> {
        Ok(ScalarData::Int(
            self.data.lock().unwrap()[id.0 as usize - 1][..count].to_vec(),
        ))
    }

    fn release(&self, _id: ResidentId) {}
}

struct StubPending {
    id: ResidentId,
}

impl Pending for StubPending {
    fn outputs(&self) -> &[ResidentId] {
        std::slice::from_ref(&self.id)
    }

    fn wait(self: Box<Self>) -> Result<Vec<ResidentId>, String> {
        Ok(vec![self.id])
    }
}

/// Taken for the length of a test, so no two of them are inside the slot at once.
static BACKEND: Mutex<()> = Mutex::new(());

/// Install the stub once and lock it for the caller's test: a shared log
/// reports the scheduler's order, not the test's.
fn stub() -> (MutexGuard<'static, ()>, Stub) {
    static STUB: OnceLock<Stub> = OnceLock::new();
    let stub = STUB.get_or_init(|| {
        let stub = Stub::default();
        install_parallel_backend(Arc::new(stub.clone()));
        stub
    });
    let guard = BACKEND
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    stub.forget();
    (guard, stub.clone())
}

/// Compile and run `source`, returning the module, the evaluated root value,
/// and the root type node.
fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    common::run(source)
}

/// Compile `source` and return the rendered diagnostics.
fn fail(source: &str) -> Vec<String> {
    let mut store = PackageStore::<LangProgram>::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .expect_err("expected this program to fail")
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

/// Two kernels, one with no inputs and one with one, so which node a value
/// reached is visible in the numbers.
const KERNELS: &str = r#"
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "gpu"
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}
k2 = compute.parallel f2 "gpu"
"#;

/// A body that dispatches twice, the second over the first's output.
const CHAIN: &str = r#"
InS  = struct<.a Int>
OutS = struct<.unused (compute.Buf _)>
ParS = compute.P (compute.KT _)(.I InS, .O OutS)
step = (s : ParS) => {
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z)))
}
built = compute.graph step
"#;

#[test]
fn a_recording_produces_the_same_numbers_as_running_the_dispatches() {
    let (_guard, stub) = stub();
    // The direct answer first, from the same kernels run one at a time.
    let (direct_module, direct, _) = run(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
first = (compute.plrun k1 ((compute.A In1)(.n 3, .I In1(.a 0))) : Out1)
out = (compute.plrun k2 ((compute.A In2)(.n 3, .I In2(.b first.z))) : Out2)
compute.collect out.w
"#
    ));
    let (graph_module, through_a_graph, _) = run(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
{CHAIN}
compute.collect (compute.graphrun built 3)
"#
    ));
    assert_eq!(
        common::usize_array(&graph_module, &through_a_graph),
        common::usize_array(&direct_module, &direct),
        "a recorded chain is the chain, and the graph's numbers are the same numbers"
    );
    assert_eq!(
        stub.saw().len(),
        4,
        "two runs of two dispatches, and nothing was submitted that the body did not dispatch: \
         {:?}",
        stub.saw()
    );
}

#[test]
fn a_graph_runs_at_whichever_extent_its_argument_names() {
    let (_guard, _stub) = stub();
    // The count is a value the run supplies, not one the build decided.
    for count in [3, 5] {
        let (module, out, _) = run(&format!(
            r#"---
  compute = import "compute.lichen"
---
{KERNELS}
{CHAIN}
compute.collect (compute.graphrun built ({count},))
"#
        ));
        // The values are a property of the wiring; only the length is the extent's.
        assert_eq!(
            common::usize_array(&module, &out),
            vec![2; count],
            "at extent {count} the graph answered {count} twos"
        );
    }
}

#[test]
fn a_buffer_the_body_closed_over_is_refused_by_the_capture() {
    let (_guard, _stub) = stub();
    // `held` is a live buffer the body reaches as a free variable.
    let messages = fail(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
held = (compute.plrun k1 ((compute.A In1)(.n 3, .I In1(.a 0))) : Out1)
InS  = struct<.a Int>
OutS = struct<.unused (compute.Buf _)>
ParS = compute.P (compute.KT _)(.I InS, .O OutS)
step = (s : ParS) => {{
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b held.z)))
}}
built = compute.graph step
compute.collect (compute.graphrun built 3)
"#
    ));
    let joined = messages.join(" | ");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("compute.graph")),
        "the refusal is filed under the graph, not under the parallel launch: {joined}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("hold") && message.contains("parameter")),
        "and it names the cause and the way out: {joined}"
    );
}

#[test]
fn a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one() {
    let (_guard, _stub) = stub();
    // Only the count role is wrong, and the count filter is read first.
    let messages = fail(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
held = compute.plrun k1 (3,)
step = ins => {{
  first = compute.plrun k1 (ins(0),)
  compute.plrun k2 (held, (first,))
}}
built = compute.graph step
compute.collect (compute.graphrun built (3,))
"#
    ));
    let joined = messages.join(" | ");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("compute.graph")),
        "the refusal is filed under the graph, like the buffer role's: {joined}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("a dispatch's count is")
                && message.contains("a literal or one of this function's arguments")),
        "**and it is the count's own rule**, because the count is the role that \
         was wrong: {joined}"
    );
    assert!(
        !joined.contains("would have to hold it"),
        "and not the buffer role's message, which would send a caller after a \
         capture that did not happen — the buffer here came from the parameter: \
         {joined}"
    );
}

/// The three operators a recorded body may not reach for, each with its own
/// sentence.
#[test]
fn what_a_recorded_body_may_not_reach_for_is_refused_by_name() {
    let (_guard, _stub) = stub();
    // A statement nobody reads is never demanded, so the operator is nested in the
    // statement that is the body's value.
    let collect = fail(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
GArg = struct<.n Int, .in In1>
step = (s : GArg) => {{
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  pulled = compute.collect first.z
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b pulled)))
}}
built = compute.graph step
compute.collect (compute.graphrun built 3)
"#
    ));
    let joined = collect.join(" | ");
    assert!(
        collect
            .iter()
            .any(|message| message.contains("a collect is asked here")
                && message.contains("after the graph has run")),
        "and it names the cause and the way out: {joined}"
    );

    // A host read of a dispatch's result: the same boundary at one number's size.
    let read = fail(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
GArg = struct<.n Int, .in In1>
step = (s : GArg) => {{
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  at_zero = compute.read ((compute.Read _)(.from first.z, .at 0))
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b at_zero)))
}}
built = compute.graph step
compute.collect (compute.graphrun built 3)
"#
    ));
    let joined = read.join(" | ");
    assert!(
        read.iter()
            .any(|message| message.contains("a host read is asked here")),
        "a read is the same mistake as a collect and gets its own sentence, \
         because the repair is different: {joined}"
    );

    // A scalar kernel has no node to be, so no shape of it can go in.
    let scalar = fail(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
one = compute.jit (cfg : Int => cfg + 1)
step = ins => {{
  scalar = compute.call one 3
  compute.plrun k2 (ins(0), (scalar,))
}}
built = compute.graph step
compute.collect (compute.graphrun built (3,))
"#
    ));
    let joined = scalar.join(" | ");
    assert!(
        scalar
            .iter()
            .any(|message| message.contains("a graph has no node for one")
                && message.contains("nowhere for what it computes")),
        "**and this one is refused with no placeholder involved at all**, so \
         the message cannot be about a value it was handed: {joined}"
    );
    assert!(
        collect
            .iter()
            .any(|message| message.contains("compute.graph"))
            && read.iter().any(|message| message.contains("compute.graph"))
            && scalar
                .iter()
                .any(|message| message.contains("compute.graph")),
        "all three are filed under the graph, because all three are about what \
         a graph can hold and not about a launch that happened to be in flight: \
         collect={collect:#?} read={read:#?} scalar={scalar:#?}"
    );
}

#[test]
fn a_function_that_dispatches_nothing_has_no_backend_to_run_on() {
    let (_guard, _stub) = stub();
    // A body that dispatches nothing names no backend, and there is no third one
    // to default to.
    let messages = fail(
        r#"---
  compute = import "compute.lichen"
---
step = ins => ins(0)
built = compute.graph step
compute.graphrun built (3,)
"#,
    );
    let joined = messages.join(" | ");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("names no backend")),
        "and it says what is missing rather than picking a backend: {joined}"
    );
}

#[test]
fn one_backend_for_the_whole_graph_is_checked_while_it_is_built() {
    let (_guard, _stub) = stub();
    // Resolved while the graph is built: dropping one at run time would silently
    // change what the program asked for.
    let messages = fail(
        r#"---
  compute = import "compute.lichen"
---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i))
}
k1 = compute.parallel f1 "gpu"
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a))
}
k2 = compute.parallel f2 "cpu"
GArg = struct<.n Int, .in In1>
step = (s : GArg) => {
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z)))
}
built = compute.graph step
compute.graphrun built 3
"#,
    );
    let joined = messages.join(" | ");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("one runner against one backend")),
        "and it says why the second one cannot simply win: {joined}"
    );
}

/// A graph performs what its function performs: the same dispatches, in the
/// same order, with the same wiring.
#[test]
fn a_graph_dispatches_exactly_what_the_program_dispatches() {
    let (_guard, stub) = stub();
    let body = r#"InS  = struct<.a Int>
GArg = struct<.n Int, .in InS>
step = (s : GArg) => {
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  dead = (compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z))) : Out2)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z)))
}"#;
    run(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
{body}
out = step ((GArg)(.n 3, .in InS(.a 0)))
"#
    ));
    let plain = stub.saw();
    // The log is one stub for the binary, so the plain run's lines are still in it.
    stub.forget();
    run(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
{body}
built = compute.graph step
out = compute.collect (compute.graphrun built 3)
"#
    ));
    let recorded = stub.saw();
    assert_eq!(
        recorded, plain,
        "a graph is a transcript of the run, so recording and running the same \
         body have to reach the backend in the same order with the same inputs. \
         `dead` is bound to a name nothing reads, so the program never performs \
         it either and the graph must not: a recorded trace that is *longer* is \
         the graph running work the program did not ask for"
    );
    assert_eq!(
        plain.len(),
        2,
        "and the shorter trace is the two the body actually reaches — the first \
         `plrun k1` and the `plrun k2` that reads it. The unread `plrun k2` is \
         absent from the program's own run, which is the whole finding: \
         {plain:?}"
    );
}

/// A graph is content-addressed on its shape, so the registry entry must not
/// carry the first recording's backend.
#[test]
fn a_graph_recorded_for_one_backend_runs_on_another() {
    let (_guard, stub) = stub();
    let program = |backend: &str| {
        format!(
            r#"---
  compute = import "compute.lichen"
---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}}
k1 = compute.parallel f1 "{backend}"
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {{
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}}
k2 = compute.parallel f2 "{backend}"
GArg = struct<.n Int, .in In1>
step = (s : GArg) => {{
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z)))
}}
built = compute.graph step
compute.collect (compute.graphrun built 3)
"#
        )
    };

    // The cpu recording first; a cpu graph is refused by its own contract.
    let cpu = fail(&program("cpu"));
    assert!(
        cpu.iter()
            .any(|message| message.contains("\"cpu\"") && message.contains("ParallelBackend")),
        "the cpu graph is refused on its own terms, and that is not what this test is about: {cpu:?}"
    );

    // The same shape for "gpu": `k1` reads no buffer, so the stub answers `0 + 1`
    // and `k2` answers `1 + 1`.
    let (module, gpu, _) = run(&program("gpu"));
    assert_eq!(
        common::usize_array(&module, &gpu),
        vec![2, 2, 2],
        "a gpu graph of a shape a cpu graph already interned must run and answer"
    );
    assert_eq!(
        stub.saw().len(),
        2,
        "and the two dispatches it recorded are the two the body dispatches, so the cpu \
         recording contributed nothing to it: {:?}",
        stub.saw()
    );
}
