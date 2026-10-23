//! A refusal for an answer nothing arrived for is reported whatever shape that answer has.

use lichen_compute::language::new_store;

/// A parallel kernel whose write names a buffer it did not declare, and is refused by name.
///
/// # Invariant
///
/// The body is fixed and only the program's **tail** varies, so the two tests below differ in
/// the shape of the answer alone and not in the refusal they are about. The parameter carries
/// no `.out` group: the body declares its output, and the write that names the parameter's
/// input buffer is the one the compiler refuses (`docs/notes/kernel-buffer-declarations.md`).
const MISPLACED_WRITE: &str = r#"
--- compute = import "compute.lichen" ---
Data = struct{.d (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Out = struct{.e (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 3))
  ((Data)(.d (compute.write ((compute.Write _)(.to d, .at i, .value 1.0)))))
}
filler = compute.parallel fill "cpu" Data
data = (compute.plrun filler ((Fill)(.n 3)) : Data)
In  = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Par = struct{.n Int, .in In}
Arg = struct{.n Int, .I In}
f = (k : Par) => {
  i = compute.range k.n
  ((Out)(.e (compute.write ((compute.Write _)(.to k.in.k, .at i, .value (99.0 : Float))))))
}
k = compute.parallel f "cpu" Out
out = (compute.plrun k ((Arg)(.n 3, .I In(.k data.d))) : Out)
"#;

/// Run `tail` after [`MISPLACED_WRITE`]: the output line, or the rendered diagnostics.
fn attempt(tail: &str) -> Result<String, Vec<String>> {
    let mut store = new_store(None).expect("the embedded compute package registers");
    lichen_language::run::evaluate_raw(&format!("{MISPLACED_WRITE}{tail}\n"), None, &mut store)
        .map_err(|diags| diags.into_iter().map(|diag| diag.message).collect())
}

/// The answer is a tuple of empty slots, which the root-only gate read as a value.
#[test]
fn a_refusal_reaches_an_answer_that_is_a_tuple() {
    let messages = attempt("(data.d, data.d)")
        .expect_err("a refusal must not be lost because the answer is a tuple");
    assert_eq!(
        messages.len(),
        1,
        "one misplaced write is one diagnostic: {messages:?}"
    );
    assert!(
        messages[0].contains("compute.parallel")
            && messages[0].contains("a kernel body's index cannot be placed"),
        "the refusal must name the write it could not place: {messages:?}"
    );
}

/// The other side: the same refusal is reported even though the answer itself arrived.
///
/// # Invariant
///
/// The tail is `(1, 2)` — a literal that arrived whatever the kernel did — so the refusal
/// here cannot be explained by a value that never came. The gate that once withheld it
/// was deleted; this test was `a_refusal_stays_quiet_when_the_answer_arrived`.
#[test]
fn a_refusal_is_reported_even_when_the_answer_arrived() {
    let messages = attempt("(1, 2)")
        .expect_err("an answer that arrived does not make a recorded refusal go away");
    assert_eq!(
        messages.len(),
        1,
        "one misplaced write is one diagnostic: {messages:?}"
    );
    assert!(
        messages[0].contains("compute.parallel")
            && messages[0].contains("a kernel body's index cannot be placed"),
        "the refusal must name the write it could not place: {messages:?}"
    );
}
