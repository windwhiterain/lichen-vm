//! A session installs the backend, counts what its runs ask for, and reports refusals by name.

use lichen_serve::{ServeRefusal, Session};

/// One kernel, whose body reads nothing, so only the schedule can differ between runs.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
Out = struct{.e (compute.Buf Float)}
Par = struct{.n Int, .out Out}
Arg = struct{.n Int}
seven_raw = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value 7.0))
}
seven = compute.parallel seven_raw "gpu"
"#;

/// Three dispatches, which is one submission and one fence per node.
const CHAINED: &str = r#"
a = (compute.plrun seven ((Arg)(.n 3)) : Out)
b = (compute.plrun seven ((Arg)(.n 3)) : Out)
c = (compute.plrun seven ((Arg)(.n 3)) : Out)
compute.collect c.e : array{Float, 3}
"#;

/// One dispatch, to which the readback adds its own share.
const ONE: &str = r#"
a = (compute.plrun seven ((Arg)(.n 3)) : Out)
compute.collect a.e : array{Float, 3}
"#;

/// A dispatch whose count is neither a literal nor an argument, which the recorder refuses.
const REFUSED: &str = r#"
a = (compute.plrun seven ((Arg)(.n 3)) : Out)
b = (compute.plrun seven ((Arg)(.n (compute.read ((compute.Read _)(.from a.e, .at 0))))) : Out)
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
        three.answer, "[7.0, 7.0, 7.0]: array{Float, 3}",
        "the body writes the same number on every lane"
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
