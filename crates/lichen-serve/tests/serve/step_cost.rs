//! A recording reports a refused program in the recorder's own words.

use lichen_serve::{ServeRefusal, record};

/// One kernel, so that only the schedule can differ between two runs of the same work.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
Inc = struct{.d (compute.Buf Float)}
Out = struct{.e (compute.Buf Float)}
Par = struct{.n Int, .in Inc, .out Out}
double_raw = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) * 2.0)))
}
double = compute.parallel double_raw "gpu" Out
data = (compute.buffer ([1.0, 2.0, 3.0] : array{Float, 3}) : (compute.Buf Float))
"#;

/// A dispatch whose count is neither a literal nor an argument, which the recorder refuses.
const REFUSED: &str = r#"
a = (compute.plrun double ((compute.A Inc)(.n 3, .I Inc(.d data))) : Out)
b = (compute.plrun double ((compute.A Inc)(.n ((compute.read ((compute.Read _)(.from a.e, .at 0)) : Float)), .I Inc(.d a.e))) : Out)
compute.collect b.e : array{Float, 3}
"#;

/// A refused program comes back with the recording's own lines, not with a count or a guess.
///
/// # Invariant
/// The refusal is asked of a **recording** and not of a run: the program is compiled and recorded
/// and nothing is dispatched, so the lines it answers with are the recorder's and not a device's,
/// and no backend has to be open for the test to ask.
#[test]
fn a_refused_program_answers_with_its_diagnostics() {
    match record(&format!("{KERNEL}{REFUSED}"), &[]) {
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
        Ok(_) => panic!(
            "a count that is neither a literal nor an argument does not record, and this program \
             was recorded"
        ),
    }
}
