//! A session installs the backend, counts what its runs ask for, and reports refusals by name.

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_graph_ir::Policy;
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::{ServeRefusal, Session};

/// One kernel, so that only the schedule can differ between two runs of the same work.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
Inc = struct{.d (compute.Buf Float)}
Out = struct{.e (compute.Buf Float)}
Par = struct{.n Int, .in Inc, .out Out}
double_raw = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (compute.read ((compute.Read _)(.from k.in.d, .at i)) * 2.0)))
}
double = compute.parallel double_raw "gpu"
data = (compute.buffer ([1.0, 2.0, 3.0] : array{Float, 3}) : (compute.Buf Float))
"#;

/// One dispatch, to which the readback adds its own share.
const ONE: &str = r#"
a = (compute.plrun double ((compute.A Inc)(.n 3, .I Inc(.d data))) : Out)
compute.collect a.e : array{Float, 3}
"#;

/// Three dispatches, which is one submission and one fence per node.
const CHAINED: &str = r#"
a = (compute.plrun double ((compute.A Inc)(.n 3, .I Inc(.d data))) : Out)
b = (compute.plrun double ((compute.A Inc)(.n 3, .I Inc(.d a.e))) : Out)
c = (compute.plrun double ((compute.A Inc)(.n 3, .I Inc(.d b.e))) : Out)
compute.collect c.e : array{Float, 3}
"#;

/// The same three kernels as one recorded graph, so the whole step is one submission.
const GRAPHED: &str = r#"
GArg = struct{.n Int, .in Inc}
step = (s : GArg) => {
  a = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d s.in.d))) : Out)
  b = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d a.e))) : Out)
  compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d b.e)))
}
built = compute.graph step
compute.collect (compute.graphrun built (3, data)) : array{Float, 3}
"#;

/// A dispatch whose count is neither a literal nor an argument, which the recorder refuses.
const REFUSED: &str = r#"
a = (compute.plrun double ((compute.A Inc)(.n 3, .I Inc(.d data))) : Out)
b = (compute.plrun double ((compute.A Inc)(.n (compute.read ((compute.Read _)(.from a.e, .at 0))), .I Inc(.d a.e))) : Out)
compute.collect b.e : array{Float, 3}
"#;

/// A readback is a submission and a fence of its own, and dispatches are one each.
///
/// # Invariant
/// The counts are deltas read from the installed context, so a session over `install_default`
/// reports zero and this fails rather than passing quietly. The two programs differ only in how
/// many dispatches precede the same collect, so the difference is the dispatches' and the shared
/// part is the readback's.
#[test]
fn a_readback_is_a_submission_and_dispatches_are_one_each() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_readback_is_a_submission_and_dispatches_are_one_each: no device");
        return;
    };
    let one = session
        .run(&format!("{KERNEL}{ONE}"))
        .unwrap_or_else(|refusal| panic!("one dispatch runs: {refusal}"));
    let three = session
        .run(&format!("{KERNEL}{CHAINED}"))
        .unwrap_or_else(|refusal| panic!("the chain runs: {refusal}"));
    assert_eq!(
        one.answer, "[2.0, 4.0, 6.0]: array{Float, 3}",
        "one doubling of 1, 2 and 3"
    );
    assert_eq!(
        three.answer, "[8.0, 16.0, 24.0]: array{Float, 3}",
        "three doublings of 1, 2 and 3"
    );
    assert_eq!(
        (one.cost.submissions, one.cost.fence_waits),
        (2, 2),
        "one dispatch and the readback that hands its answer over are two of each"
    );
    assert_eq!(
        (three.cost.submissions, three.cost.fence_waits),
        (4, 4),
        "three dispatches and the same readback are four, so each dispatch is one and the \
         readback is the other"
    );
    println!(
        "one dispatch + readback {}/{}, three + readback {}/{}",
        one.cost.submissions, one.cost.fence_waits, three.cost.submissions, three.cost.fence_waits
    );
}

/// The same three dispatches recorded as one graph cost one submission, not three.
///
/// # Invariant
/// The step's body reads its own extent argument and its parameter's `.in` is the dispatch's own
/// struct: a graph step that breaks either rule is handed the extent in its first buffer slot.
#[test]
fn a_recorded_step_is_one_submission_where_the_chain_is_three() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_recorded_step_is_one_submission_where_the_chain_is_three: no device");
        return;
    };
    let chained = session
        .run(&format!("{KERNEL}{CHAINED}"))
        .unwrap_or_else(|refusal| panic!("the chain runs: {refusal}"));
    let graphed = session
        .run(&format!("{KERNEL}{GRAPHED}"))
        .unwrap_or_else(|refusal| panic!("the recorded step runs: {refusal}"));
    assert_eq!(
        graphed.answer, chained.answer,
        "the recorded step answers what the chain answers: the schedule changed and the numbers \
         did not"
    );
    assert_eq!(
        (chained.cost.submissions, chained.cost.fence_waits),
        (4, 4),
        "three dispatches and a readback"
    );
    assert_eq!(
        (graphed.cost.submissions, graphed.cost.fence_waits),
        (2, 2),
        "and one recorded step plus the same readback is two, so a step is one submission and the \
         readback is the other"
    );
    println!(
        "chained {}/{}, recorded step {}/{}",
        chained.cost.submissions,
        chained.cost.fence_waits,
        graphed.cost.submissions,
        graphed.cost.fence_waits
    );
}

/// A refused program comes back with the run's own lines, not with a count or a guess.
#[test]
fn a_refused_program_answers_with_its_diagnostics() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_refused_program_answers_with_its_diagnostics: no device");
        return;
    };
    match session.run(&format!("{KERNEL}{REFUSED}")) {
        Err(ServeRefusal::Program(lines)) => {
            assert!(
                !lines.is_empty(),
                "a refusal carries at least the line that caused it"
            );
            println!(
                "refused with {} line(s): {}",
                lines.len(),
                lines.join(" | ")
            );
        }
        Err(other) => panic!("a refused program is a program refusal: {other}"),
        Ok(step) => panic!(
            "a count that is neither a literal nor an argument does not record, and this run \
             answered {:?}",
            step.answer
        ),
    }
}

/// `steps` recorded steps in one program, the answer threaded through the recursion's state.
fn looped(steps: usize) -> String {
    format!(
        "{KERNEL}
GArg = struct{{.n Int, .in Inc}}
step = (s : GArg) => {{
  a = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d s.in.d))) : Out)
  b = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d a.e))) : Out)
  compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d b.e)))
}}
built = compute.graph step
@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.graphrun built (3, s(1))))) : tuple{{Int, compute.Buf Float}} -> compute.Buf Float)
compute.collect (march ({steps}, data)) : array{{Float, 3}}
"
    )
}

/// Each further recorded step is one more submission, and a program's readback is one.
///
/// # Invariant
/// The two runs differ only in how many times the same recorded step is run, so the difference is
/// the steps' and the shared part is the readback's. Each step is three doublings of `1, 2, 3`, so
/// the answer says the loop ran as many times as it was asked to.
#[test]
fn each_further_recorded_step_is_one_more_submission() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED each_further_recorded_step_is_one_more_submission: no device");
        return;
    };
    let one = session
        .run(&looped(1))
        .unwrap_or_else(|refusal| panic!("one step runs: {refusal}"));
    let three = session
        .run(&looped(3))
        .unwrap_or_else(|refusal| panic!("three steps run: {refusal}"));
    assert_eq!(
        one.answer, "[8.0, 16.0, 24.0]: array{Float, 3}",
        "one step is three doublings of 1, 2 and 3"
    );
    assert_eq!(
        three.answer, "[512.0, 1024.0, 1536.0]: array{Float, 3}",
        "three steps are nine doublings, which is what says the loop ran three times"
    );
    assert_eq!(
        (one.cost.submissions, one.cost.fence_waits),
        (2, 2),
        "one step and the program's own readback"
    );
    assert_eq!(
        (three.cost.submissions, three.cost.fence_waits),
        (4, 4),
        "three steps and the same readback, so each further step is one submission and one fence"
    );
    println!(
        "one step {}/{}, three steps {}/{}",
        one.cost.submissions, one.cost.fence_waits, three.cost.submissions, three.cost.fence_waits
    );
}

/// A step recorded as a graph value, which is what preparing wants as a program's tail.
const STEP_GRAPH: &str = r#"
GArg = struct{.n Int, .in Inc}
step = (s : GArg) => {
  a = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d s.in.d))) : Out)
  compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d a.e)))
}
built = compute.graph step
built
"#;

/// Three `Float` elements as the packed bytes a `RunArgument::Buffer` carries.
fn packed(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// One `prepare`, two `step`s, and the answer fetched afterwards as the caller's own act.
///
/// # Invariant
/// The second step costs one submission and no compilation, which is what says the program was
/// compiled once. The readback is not in the step's count because `graph::run` leaves what the
/// device wrote where it is: `Session::fetch` is what reads it back, and it pays its own submission.
#[test]
fn a_prepared_step_runs_without_compiling_again() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_prepared_step_runs_without_compiling_again: no device");
        return;
    };
    let prepared = session
        .prepare(&format!("{KERNEL}{STEP_GRAPH}"))
        .unwrap_or_else(|refusal| panic!("the step prepares: {refusal}"));
    let arguments = || {
        vec![
            RunArgument::Count(3),
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: packed(&[1.0, 2.0, 3.0]),
            },
        ]
    };
    let first = prepared
        .step(arguments())
        .unwrap_or_else(|refusal| panic!("the first step runs: {refusal}"));
    let second = prepared
        .step(arguments())
        .unwrap_or_else(|refusal| panic!("the second step runs: {refusal}"));

    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is two dispatches, and a submission carries both"
    );
    for (which, stepped) in [("first", &first), ("second", &second)] {
        assert_eq!(
            (stepped.cost.submissions, stepped.cost.fence_waits),
            (1, 1),
            "the {which} step is one submission and one fence, with no compilation in it"
        );
    }
    assert_eq!(
        first.results.len(),
        1,
        "the step returns the tail node's buffer, and nothing else"
    );
    let RunResult::Resident(resident) = &first.results[0] else {
        panic!("a device wrote this value, so it stays a resident id");
    };
    assert_eq!(
        session
            .fetch(*resident)
            .unwrap_or_else(|refusal| panic!("the resident is readable: {refusal}")),
        ScalarData::Float(vec![4.0, 8.0, 12.0]),
        "two doublings of 1, 2 and 3, fetched only because the caller asked"
    );
    println!(
        "prepared step {}/{} twice, {} node(s)",
        first.cost.submissions,
        first.cost.fence_waits,
        prepared.nodes()
    );
}

/// A step of `nodes` dispatches, each doubling the one before, recorded as a graph value.
fn layer(nodes: usize) -> String {
    let mut body = String::new();
    let mut previous: Option<String> = None;
    for ordinal in 0..nodes {
        let source = previous
            .as_deref()
            .map(|name| format!("{name}.e"))
            .unwrap_or_else(|| "s.in.d".to_string());
        if ordinal + 1 == nodes {
            body.push_str(&format!(
                "  compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d {source})))\n"
            ));
        } else {
            let name = if ordinal == 0 {
                "a".to_string()
            } else {
                format!("n{ordinal}")
            };
            body.push_str(&format!(
                "  {name} = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d {source}))) : Out)\n"
            ));
            previous = Some(name);
        }
    }
    format!(
        "{KERNEL}
GArg = struct{{.n Int, .in Inc}}
step = (s : GArg) => {{
{body}}}
built = compute.graph step
built
"
    )
}

/// Whether a step's clock is its submissions or its dispatches, on one graph run two ways.
///
/// # Invariant
/// The clock covers `REPEATS` steps of an already-prepared graph, so no compilation is inside it,
/// and both schedules run **the same graph** — so the only difference is how many submissions the
/// run takes. The counts are asserted per step, so a schedule change cannot pass as timing noise.
#[test]
fn a_schedule_trades_submissions_for_clock() {
    const REPEATS: usize = 200;
    const NODES: usize = 16;
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_schedule_trades_submissions_for_clock: no device");
        return;
    };
    let arguments = || {
        vec![
            RunArgument::Count(3),
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: packed(&[1.0, 2.0, 3.0]),
            },
        ]
    };
    let prepared = session
        .prepare(&layer(NODES))
        .unwrap_or_else(|refusal| panic!("the step prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        NODES,
        "the held step has the nodes asked for"
    );

    let batches = prepared
        .step_with(Policy::Batch, arguments())
        .unwrap_or_else(|refusal| panic!("a batched step runs: {refusal}"));
    assert_eq!(
        (batches.cost.submissions, batches.cost.fence_waits),
        (1, 1),
        "the whole step is one submission and one fence"
    );
    let pipelined = prepared
        .step_with(Policy::Async, arguments())
        .unwrap_or_else(|refusal| panic!("an async step runs: {refusal}"));
    assert_eq!(
        pipelined.cost.submissions, NODES as u64,
        "under `Async` the same step is one submission per node"
    );
    assert!(
        pipelined.cost.fence_waits >= pipelined.cost.submissions,
        "`Async` waits at least as many fences as it submits, and this run waited {} for {} \
         submissions",
        pipelined.cost.fence_waits,
        pipelined.cost.submissions
    );

    let mut clocks: Vec<(Policy, f64)> = Vec::new();
    for policy in [Policy::Batch, Policy::Async] {
        let started = std::time::Instant::now();
        for _ in 0..REPEATS {
            let stepped = prepared
                .step_with(policy, arguments())
                .unwrap_or_else(|refusal| panic!("a {policy:?} step runs: {refusal}"));
            std::hint::black_box(&stepped);
        }
        clocks.push((
            policy,
            started.elapsed().as_secs_f64() * 1.0e3 / REPEATS as f64,
        ));
    }
    let batched = clocks[0].1;
    let asynchronously = clocks[1].1;
    println!(
        "step clock over {REPEATS} steps of {NODES} dispatches: Batch 1 submission {batched:.4} ms, \
         Async {NODES} submissions / {} fences {asynchronously:.4} ms, so batching saved \
         {:.4} ms per step ({:.1}%)",
        pipelined.cost.fence_waits,
        asynchronously - batched,
        (asynchronously - batched) / asynchronously * 100.0
    );
}

/// A batched step: one row of three lanes per sequence, and the sequence's own value after the data.
const BATCHED: &str = r#"
ShiftIn = struct{.d (compute.Buf Float)}
ShiftPar = struct{.n Int, .in ShiftIn, .out Out}
shift_raw = (k : ShiftPar) => {
  i = compute.range k.n
  seq = i / 3
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) * 2.0) + (compute.read ((compute.Read _)(.from k.in.d, .at (k.n + seq)))))))
}
shift = compute.parallel shift_raw "gpu"
GArg = struct{.n Int, .in ShiftIn}
step = (s : GArg) => {
  a = (compute.plrun shift ((compute.A ShiftIn)(.n s.n, .I ShiftIn(.d s.in.d))) : Out)
  compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d a.e)))
}
built = compute.graph step
built
"#;

/// One prepared step serves any batch size, and the step's own cost is paid once for the batch.
///
/// # Invariant
/// The batch size is the extent **argument**, not a graph constant, so one `Prepared` serves any
/// batch size. The buffer holds the data first and the per-sequence values after it at `n`, and the
/// step is `shift` then `double`: four times the data plus twice the sequence's value, so the first
/// sequence's row is the same at every batch size and the last one's is not.
#[test]
fn one_prepared_step_serves_any_batch_size() {
    const REPEATS: usize = 200;
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED one_prepared_step_serves_any_batch_size: no device");
        return;
    };
    let prepared = session
        .prepare(&format!("{KERNEL}{BATCHED}"))
        .unwrap_or_else(|refusal| panic!("the batched step prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is two different dispatches: a repeat of one would be one node"
    );

    let arguments = |sequences: usize| {
        let mut buffer = Vec::with_capacity(sequences * 4);
        for _ in 0..sequences {
            buffer.extend([1.0f32, 2.0, 3.0]);
        }
        for ordinal in 0..sequences {
            buffer.push(10.0 * (ordinal as f32 + 1.0));
        }
        vec![
            RunArgument::Count((sequences * 3) as i64),
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: packed(&buffer),
            },
        ]
    };

    let mut clocks: Vec<(usize, f64)> = Vec::new();
    for sequences in [1usize, 4, 16] {
        let stepped = prepared
            .step(arguments(sequences))
            .unwrap_or_else(|refusal| panic!("a {sequences}-sequence step runs: {refusal}"));
        assert_eq!(
            (stepped.cost.submissions, stepped.cost.fence_waits),
            (1, 1),
            "a {sequences}-sequence step is one submission and one fence, whatever the batch"
        );
        let RunResult::Resident(resident) = &stepped.results[0] else {
            panic!("a device wrote this value, so it stays a resident id");
        };
        let ScalarData::Float(values) = session
            .fetch(*resident)
            .unwrap_or_else(|refusal| panic!("the batch is readable: {refusal}"))
        else {
            panic!("the step declared a Float buffer");
        };
        assert_eq!(values.len(), sequences * 3, "three lanes per sequence");
        assert_eq!(
            &values[..3],
            &[24.0, 28.0, 32.0],
            "the first sequence answers the same at {sequences} sequences in the batch: four times \
             1, 2 and 3 plus twice its own 10"
        );
        let last = 10.0 * (sequences as f32);
        assert_eq!(
            &values[values.len() - 3..],
            &[4.0 + 2.0 * last, 8.0 + 2.0 * last, 12.0 + 2.0 * last],
            "and the last sequence read its own value, not the first one's"
        );
        let started = std::time::Instant::now();
        for _ in 0..REPEATS {
            let stepped = prepared
                .step(arguments(sequences))
                .unwrap_or_else(|refusal| panic!("a {sequences}-sequence step runs: {refusal}"));
            std::hint::black_box(&stepped);
        }
        clocks.push((
            sequences,
            started.elapsed().as_secs_f64() * 1.0e3 / REPEATS as f64,
        ));
    }
    let alone = clocks[0].1;
    for (sequences, batched) in &clocks {
        let separate = alone * *sequences as f64;
        println!(
            "{sequences} sequence(s): one batched step {batched:.4} ms against {separate:.4} ms as \
             {sequences} separate step(s), so batching saved {:.4} ms ({:.0}%)",
            separate - batched,
            (separate - batched) / separate * 100.0
        );
    }
}
