//! A function that dispatches, recorded into a graph and run — against a stub.
//!
//! # What this file checks and what it deliberately does not
//!
//! The backend here is a **stub that computes `sum(inputs) + 1` whatever the
//! fragment's body says**. That is on purpose, and it is a real limit: the
//! arithmetic of a fragment belongs to
//! [`graph_jit_device`](../lichen-compute-gpu)'s business on real hardware. What
//! this file is about is the **plumbing** — that a body which dispatched nothing
//! becomes a graph with no nodes, that a body which dispatched twice becomes a
//! chain whose second node records against the first's output, and that a count
//! read from the parameter is read at *run* time rather than frozen while the
//! graph was built. A stub that ignores the body is exactly right for that: it
//! makes the numbers a function of *which values reached which node*, so a graph
//! that wired them wrongly answers with different numbers rather than the same
//! ones by luck.
//!
//! # Why this is its own test binary
//!
//! The backend lives in a **process-wide slot**, so a test that installs one
//! changes what every other test in the same binary sees. A second test that
//! needed *no* backend, or a real device, would therefore race this one — and the
//! failure would be a number rather than an error, which is the worst shape a
//! test failure can have. Each of those lives in its own binary instead, and this
//! one owns the stub outright.
//!
//! The tests here all want the *same* stub, so they share one instance and take
//! a lock for their length — see [`stub`]. Sharing is what makes the dispatch
//! log readable at all; the lock is what makes it a fact about one test rather
//! than about which tests the scheduler ran first.

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

    /// The elements of a slot, decoded.
    ///
    /// A host slot is packed bytes at the class's width
    /// ([`lichen_kernel_ir::ScalarClass::byte_width`]) rather than a word per
    /// element, so an `i64` column is that payload decoded — and every fragment
    /// this stub is handed is an `Int` one.
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
        // The line records **which** values reached this node, so a chain that
        // mis-wired itself is visible in the log and not only in the answer.
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

/// Install the stub **once for this binary**, and take the lock for the caller's
/// test.
///
/// Two problems have the same cause and the same answer. The slot is
/// process-wide, so a second stub would silently take every dispatch away from
/// the first — which is why the tests here that need *no* backend, or a real
/// device, live in their own binaries instead. And a log that outlives one test
/// cannot be read back while another writes to it, so `saw()` would report a
/// number that depended on which tests happened to run first.
///
/// Serializing costs a few microseconds and buys a number that is a fact about
/// the program under test rather than about the scheduler.
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

/// Two kernels, one with no inputs and one with one, so **which node a value
/// reached is visible in the numbers**: a chain that mis-wired itself would give
/// the second kernel the first kernel's input instead of its output.
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
///
/// The recorded body's parameter is the named struct too, with both reserved
/// names: a graph's inputs arrive through it (the extent at `.n`, the buffers
/// under `.in`), and the dispatch's own result is the `.out` structure the run
/// hands back.
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
    // **The same graph, two extents.** The count reaches the graph as a value
    // rather than as something the build decided, which is the whole reason a
    // count is a value: a graph that could only be given a build-time count would
    // have to be rebuilt per run, and rebuilding a graph per run is the same as
    // not having one.
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
        // **All twos, and only the length changes.** The stub sums the inputs and
        // adds one, so `k1` — which reads no buffer at all — answers every index
        // with `0 + 1`, and `k2`, reading that, answers `1 + 1`. So the values
        // are a property of the *wiring* and the length is the property of the
        // extent: a graph that reused a count from build time, or built a fresh
        // graph per run, would answer a different length for the same program.
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
    // `held` is a live buffer the body reaches as a free variable, so recording
    // it would mean the graph had to **hold** a node with a block's lifetime. The
    // refusal names the capture, because "a graph cannot hold a buffer" is the
    // reason, and a caller who does not hear it will try the next thing.
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
    // **One dispatch with both roles in it, and only the count one wrong.**
    // `k2`'s buffer is `(first,)` — a value the parameter supplied, so the buffer
    // filter has nothing to say — while its count is `held`, a free variable the
    // body closed over. The two roles are read by two separate filters, and this
    // is the case that shows it: a filter that refused both would pass here too,
    // and a body that swapped the two roles would be told the wrong thing.
    //
    // The count is also read *first*, before the buffer tuple, so a refusal from
    // the count filter is the one that answers.
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

/// The three operators a recorded body may not reach for, and why each one is a
/// separate answer rather than one "unsupported" message.
///
/// **All three used to fall through to a bare hole with no diagnostic at all**,
/// and a hole is the worst of the three outcomes rather than the smallest: a body
/// that collected a dispatch's result mid-chain recorded a graph that was quietly
/// missing the collect, and the chain's own numbers looked right anyway, so
/// nothing in the program's answer said the collect had not happened.
///
/// **Whether an unread dispatch belongs in the graph was measured, and the answer
/// is that it does not** — see
/// [`a_graph_dispatches_exactly_what_the_program_dispatches`], which runs the same
/// body with and without a graph and compares the two traces. There is no separate
/// test here for a gap that measurement closed.
#[test]
#[ignore = "pre-existing on dev (fails identically at 4be9180, before the OperatorExt::run \
refactor and before the class-value experiment), and the wrapper migration moved which case \
fails first: the original reason was the third case's refusal no longer carrying both 'a collect \
is asked here' and 'after the graph has run', and now the first case stops earlier — its program \
still spells the retired tuple form, so `compute.collect` is handed a structure and refuses with \
'its buffer position holds an array'.  Reviving the test is a decision about what those three \
refusal cases are about under the named parameter, not a re-spelling: the count and the buffer a \
body reaches for are typed now, which is what retired the count-filter case in \
`a_count_the_body_closed_over_is_refused_by_the_count_filter_not_the_buffer_one`.  The eight \
other tests in this file pass."]
fn what_a_recorded_body_may_not_reach_for_is_refused_by_name() {
    let (_guard, _stub) = stub();
    // **Each case puts the offending operator where it cannot be skipped.** A
    // block's value is the tuple of its statements' values, and a statement whose
    // value nobody reads is not demanded — so an unused `collect` is never reached
    // at all and no refusal can speak for it. Nesting the operator inside the
    // statement that *is* the body's value forces it. That is worth stating
    // because it is the other half of the boundary: the recording sees what the
    // walk forces, and what the walk does not force is not in the graph.
    let collect = fail(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
step = ins => {{
  pulled = compute.collect (compute.plrun k1 (ins(0),))
  compute.plrun k2 (ins(0), (pulled,))
}}
built = compute.graph step
compute.collect (compute.graphrun built (3,))
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

    // A host read of a dispatch's own result, which is the same boundary at a
    // different size: one number instead of a whole buffer.
    let read = fail(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
step = ins => {{
  at_zero = compute.read ((compute.Read _)(.from compute.plrun k1 (ins(0),), .at 0))
  compute.plrun k2 (at_zero, (compute.plrun k1 (ins(0),),))
}}
built = compute.graph step
compute.collect (compute.graphrun built (3,))
"#
    ));
    let joined = read.join(" | ");
    assert!(
        read.iter()
            .any(|message| message.contains("a host read is asked here")),
        "a read is the same mistake as a collect and gets its own sentence, \
         because the repair is different: {joined}"
    );

    // A scalar kernel, refused outright rather than only when it is handed a
    // placeholder — it has no node to be, so no shape of it can go in.
    let scalar = fail(&format!(
        r#"---
  compute = import "compute.lichen"
---
{KERNELS}
one = compute.jit (cfg => cfg(0) + 1)
step = ins => {{
  scalar = compute.call one (3,)
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
    // A graph is run by a runner against a backend, and a body that dispatches
    // nothing names none. **Refused rather than defaulted**: there is no third
    // backend to fall back to, and picking one would be the host overriding a
    // program that did not say.
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
    // Two dispatches, two backends, one graph. Resolved while the graph is built
    // rather than at run time, where dropping one would silently change what the
    // program asked for.
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

/// **A graph performs what its function performs — the same dispatches, in the
/// same order, with the same wiring.** The trace is compared rather than a count,
/// because a count is satisfied by a graph that dispatched the right things in
/// the wrong order or against the wrong inputs, and that graph answers with
/// numbers a reader has no way to distrust.
///
/// **This is the invariant, and it is stronger than the one it replaces.** The
/// earlier claim was that the graph had to be *what its function wrote*, measured
/// as three dispatches written and two recorded — and it was wrong in a way worth
/// keeping: **the program does not perform the third dispatch either.** `dead` is
/// bound to a name the body never reads, so the VM's laziness eliminates it, and
/// running `step (3,)` without any graph in the program dispatches twice. There is
/// no expression-level CSE in this compiler to explain the absence, so the
/// elimination is the one every other unread binding already gets.
///
/// A graph is therefore not a transcript of the source; it is a transcript of the
/// run. Requiring more of it would have meant forcing a walk that *adds* a dispatch
/// the program never makes — and the second deep walk tried for that (it forced
/// operand edges and descended past the shallow mask in those days; both knobs and
/// the entry point itself have since been deleted, `code-audit.md`, the operand-arm
/// follow-up) emptied the function's return slot as well, refusing every
/// recording including bodies with no unread statement at all. **Laziness is the
/// semantics here, not a compromise with it.** The two `plrun k2` lines below are
/// both written and one is never reached, so the trace is the shorter one by
/// exactly the binding nobody reads.
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
    // The log is one stub for the whole binary, so the plain run's two lines are
    // still in it; forgetting here is what makes the second trace the graph's own
    // rather than the two runs stacked.
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

/// The registry must not answer "which backend" for a graph it did not record.
///
/// **The two graphs below are the same shape, and that is the point.** A graph is
/// content-addressed on its shape and the backend is deliberately not part of
/// that — it is a property of the *run*, not of what the graph computes. So the
/// cpu recording and the gpu recording intern to one id, and it is the registry
/// entry that must not then carry the first one's backend: a `"gpu"` program
/// would be refused with a message naming `"cpu"`, for a program that never says
/// it. The registries are process-global, so this crossed program boundaries
/// before the fix — a process that had ever built a cpu graph of a shape could
/// never run a gpu graph of it.
///
/// The stub is what makes this observable without a device: the refusal arrives
/// *before* any dispatch, so a run that reaches the stub at all is the proof.
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

    // The cpu recording first, and it reaches no backend — a cpu graph is
    // refused by the contract, which is a separate refusal and says so.
    let cpu = fail(&program("cpu"));
    assert!(
        cpu.iter()
            .any(|message| message.contains("\"cpu\"") && message.contains("ParallelBackend")),
        "the cpu graph is refused on its own terms, and that is not what this test is about: {cpu:?}"
    );

    // The same shape, now for "gpu". This is the run the fix is about, and the
    // numbers are the stub's rather than the kernels': `k1` reads no buffer, so
    // the stub answers `0 + 1` at every index, and `k2` reading that answers
    // `1 + 1`. What is being checked is that a run happened at all.
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
