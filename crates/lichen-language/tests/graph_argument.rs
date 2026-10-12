//! A graph step's argument may name more than one buffer, under `.in`.
//! See docs/notes/compute-graph-jit.md.

mod common;

use std::sync::{Mutex, MutexGuard};

use lichen_compute::ComputeValue;
use lichen_compute::compute::graph;
use lichen_compute_gpu::{GpuContext, install_default};
use lichen_graph_ir::{Graph, Node};
use lichen_language::package::PackageStore;
use lichen_language::program::{LangProgram, LangValue};
use lichen_utils::extend::AsEnum;

/// The elements the chain runs over.
const COUNT: usize = 8;

/// The device, held for the whole of a test that runs on one.
///
/// # Invariant
/// `install_default` replaces the process's backend, so a second test installing one while
/// the first runs leaves the first's buffers on a device that is no longer installed.
static DEVICE: Mutex<()> = Mutex::new(());

/// The chain every case here records, with the argument's declaration and the step's body.
///
/// # Invariant
/// The two kernels and their claims are `fuse_licence.rs`'s byte for byte except that
/// `offset` also reads the buffer it was handed at `.d`, so an answer moves with whichever
/// argument buffers the step names rather than with the chain's shape alone.
const HEAD: &str = r#"
---
  compute = import "compute.lichen"
---
Pair = struct{.a (compute.Buf Float), .b (compute.Buf Float)}
Side = struct{.p (compute.Buf Float)}
In   = struct{.d (compute.Buf Float)}
OutE = struct{.e (compute.Buf Float)}
ParE = compute.P (compute.KT Float)(.I In, .O OutE)
scale_raw = (k : ParE) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) * 2.0) ~ "element")))
}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu"
In2  = struct{.d (compute.Buf Float), .p (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
offset_raw = (k : Par2) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) + compute.read ((compute.Read _)(.from k.in.p, .at i))) ~ "element")))
}
offset = compute.parallel (offset_raw ~ "claim:Elementwise(element)") "gpu"
"#;

/// The step `fuse_licence.rs` records, reading the one buffer its argument names.
const ONE_BUFFER: &str = r#"
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun offset ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.e)))
}
built = compute.graph step
built
"#;

/// The same step over an argument naming **two** buffers, both read.
const TWO_BUFFERS: &str = r#"
GArg = struct{.n Int, .in Pair}
step = (s : GArg) => {
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.a))) : OutE)
  compute.plrun offset ((compute.A In2)(.n s.n, .I In2(.d s.in.b, .p first.e)))
}
built = compute.graph step
built
"#;

/// The same step over an argument whose second buffer sits **outside** `.in`, unread.
const MISPLACED_BUFFER: &str = r#"
GArg = struct{.n Int, .in In, .q Side}
step = (s : GArg) => {
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun offset ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.e)))
}
built = compute.graph step
built
"#;

/// The recording, run over two buffers whose elements are `1 … COUNT` and `10 … COUNT + 9`.
fn running() -> String {
    format!(
        "{}\ncount = ({count})\nfirst = (compute.buffer ([{one}] : array{{Float, {count}}}) : \
         (compute.Buf Float))\nsecond = (compute.buffer ([{ten}] : array{{Float, {count}}}) : \
         (compute.Buf Float))\nout = compute.collect (compute.graphrun built (count, first, \
         second))\nout\n",
        chain(TWO_BUFFERS),
        count = COUNT,
        one = values(1, COUNT),
        ten = values(10, COUNT),
    )
}

/// `head` followed by one `GArg` declaration, its step and the recording the root names.
fn chain(declaration: &str) -> String {
    let mut source = String::from(HEAD);
    source.push_str(declaration);
    source
}

/// The `elements` float literals `first.0 … (first + elements - 1).0`.
fn values(first: usize, elements: usize) -> String {
    (1..=elements)
        .map(|position| format!("{}.0", position + first - 1))
        .collect::<Vec<String>>()
        .join(", ")
}

/// The answer `z[i] = 2·a[i] + e[i] + b[i]` has over `a = 1 … COUNT` and `b = 10 … COUNT + 9`.
///
/// # Invariant
/// `e = 2a`, so the three terms carry the coefficients 2, 1 and 1 and no two of them can
/// be exchanged for one another without a different answer at every position.
fn answer() -> Vec<f32> {
    (0..COUNT)
        .map(|at| 2.0 * (at + 1) as f32 + (at + 10) as f32)
        .collect()
}

/// The graph the program's root value recorded, or `None` when it recorded nothing.
///
/// # Invariant
/// A decided `Graph` value whose id the registry does not hold is not a recording, so
/// registration is checked as well as the variant: both are what "recorded" means here.
fn recorded(value: &LangValue) -> Option<Graph> {
    let ComputeValue::Graph(id, _) = AsEnum::<ComputeValue>::as_enum(value)? else {
        return None;
    };
    graph::lookup(id).ok()
}

/// Whether `source` records a graph, without saying why when it does not.
///
/// # Invariant
/// The diagnostics are read and dropped: this asks only whether a graph came out, so the
/// wording of a refusal is not part of what any test here pins — except
/// [`a_buffer_beside_the_in_group_is_refused_by_the_line_that_names_it`], which reads the
/// same run's report.
fn records(source: &str) -> bool {
    reported_run(source).1.as_ref().and_then(recorded).is_some()
}

/// `source` compiled and run: every line its report carried, and the root value.
///
/// # Invariant
/// The lines are the ones a user reads, in both channels and in the order they were
/// produced: the check report's own, then the run's refusals as `render_build` prints
/// them (`category: message`). A run is not entered for a program the checker already
/// refused, so a check-stage failure stays check-stage.
fn reported_run(source: &str) -> (Vec<String>, Option<LangValue>) {
    let mut store = PackageStore::<LangProgram>::new();
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    assert!(diags.is_empty(), "the program's imports resolve: {diags:?}");
    let report = lichen_language::compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &lichen_language::lex::line_starts(source),
        lichen_highlevel::no_native_ops(),
    );
    let mut lines: Vec<String> = report
        .diagnostics
        .iter()
        .map(|line| line.message.clone())
        .collect();
    if !report.ok() {
        return (lines, None);
    }
    let build = report.build.expect("an accepted program has a build");
    let mut module = build.module;
    let root = module.evaluate_node_deep(build.root_val, None);
    lines.extend(
        module
            .extension_diagnostics
            .iter()
            .map(|entry| format!("{}: {}", entry.category, entry.message)),
    );
    (lines, root)
}

/// The extent and two buffers are three separate graph inputs, wired to three separate ports.
///
/// # Invariant
/// The slots are numbered in field order over the parameter's cells, so `.n` is 0 and
/// `.in.a` and `.in.b` are 1 and 2; a second buffer that aliased the first would sit at 1
/// beside the producer, and the second node would then name the producer's own output twice.
#[test]
fn a_step_whose_argument_names_two_buffers_records_three_inputs() {
    let (_module, value, _root) = common::run(&chain(TWO_BUFFERS));
    let graph = recorded(&value).expect("the two-buffer argument records");
    assert_eq!(
        (graph.len(), graph.input_count()),
        (2, 3),
        "the chain is two nodes over the extent and two buffers"
    );
    println!(
        "the two-buffer recording: {} node(s), {} input(s), {} value(s)",
        graph.len(),
        graph.input_count(),
        graph.value_count()
    );
    let Node::Kernel(producer) = &graph.nodes()[0];
    assert_eq!(
        producer.inputs,
        vec![1],
        "the producer names the first argument buffer at port 0, not the extent"
    );
    let Node::Kernel(consumer) = &graph.nodes()[1];
    assert_eq!(
        consumer.inputs,
        vec![2, 3],
        "the consumer names the **second** argument buffer, which is not the first again"
    );
}

/// The device runs the two-buffer chain and answers over both buffers.
#[test]
fn the_two_buffer_chain_runs_on_the_device_and_answers_over_both() {
    let test = "the_two_buffer_chain_runs_on_the_device_and_answers_over_both";
    let Some((_context, _device)) = device(test) else {
        return;
    };
    let (module, value, _root) = common::run(&running());
    let found = common::float_array(&module, &value);
    assert_eq!(
        found,
        answer(),
        "the run answers 2·a[i] + e[i] + b[i] element for element, so both buffers reached it"
    );
    println!("the device ran the two-buffer chain: {found:?}");
}

/// The rule: a second buffer records under `.in` and does not record beside it.
///
/// # Invariant
/// Only the outcome is pinned, never the refusal's wording. The misplaced case differs
/// from the accepted one by a single field and by nothing the body reads — its second
/// buffer is never read — so what separates them is where the buffer sits, not the read.
#[test]
fn a_buffer_beside_the_in_group_records_and_the_same_buffer_under_it_does_not() {
    assert!(
        records(&chain(ONE_BUFFER)),
        "the control records, which is what the other two are measured against"
    );
    assert!(
        records(&chain(TWO_BUFFERS)),
        "two buffers under `.in` record: the walk numbers them in field order"
    );
    assert!(
        !records(&chain(MISPLACED_BUFFER)),
        "a buffer outside `.in`/`.out` has no place in the parameter, and the step does \
         not record — unread or not, this is where the buffer sits"
    );
}

/// The misplaced buffer's report is the one line that is the cause.
///
/// # Invariant
/// The four lines the positional fallback used to answer with — two `this value is not a
/// container`, `argument 0 of this dispatch is not a buffer`, and the return a graph
/// cannot hand back — are symptoms of reading a struct parameter as a flat tuple, so
/// none names what the program did. The refusal names its subject and its position, and
/// it is the only line the run reports.
#[test]
fn a_buffer_beside_the_in_group_is_refused_by_the_line_that_names_it() {
    let (lines, value) = reported_run(&chain(MISPLACED_BUFFER));
    assert!(
        value.as_ref().and_then(recorded).is_none(),
        "the step still records nothing: a refused parameter has no placeholders to apply"
    );
    assert_eq!(
        lines.len(),
        1,
        "one cause, one line:\n  {}",
        lines.join("\n  ")
    );
    let refusal = &lines[0];
    assert!(
        refusal.contains("a buffer outside `.in`/`.out`"),
        "the line names the buffer and the rule it broke:\n  {refusal}"
    );
    assert!(
        refusal.contains("position 2.0"),
        "the line names where in the parameter the buffer sits:\n  {refusal}"
    );
    for symptom in [
        "is not a container",
        "is not a buffer",
        "is not a value a graph can hand back",
    ] {
        assert!(
            !lines.iter().any(|line| line.contains(symptom)),
            "{symptom:?} is a symptom of the positional fallback, not this program's cause, \
             and the report was:\n  {}",
            lines.join("\n  ")
        );
    }
    println!("the misplaced buffer is refused by:\n  {refusal}");
}

/// A context on a device, or `None` when there is none here, with the device held for the run.
///
/// # Invariant
/// The skip is reported and never turned into a failure, and the guard travels back with
/// the context, so a caller that keeps both keeps the device for as long as it runs.
fn device(test: &str) -> Option<(GpuContext, MutexGuard<'static, ()>)> {
    let guard = DEVICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
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
