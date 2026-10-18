//! A session installs the backend, counts what its runs ask for, and reports refusals by name.

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
