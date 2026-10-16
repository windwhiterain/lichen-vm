//! A refusal for an answer nothing arrived for is reported whatever shape that answer has.

use lichen_compute::language::new_store;

/// A parallel kernel whose two writes name each other's output, and is refused by name.
///
/// # Invariant
///
/// The body is fixed and only the program's **tail** varies, so the two tests below differ in
/// the shape of the answer alone and not in the refusal they are about.
const MISPLACED_WRITE: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct{.a Int}
Out = struct{.c (compute.Buf _), .s (compute.Buf _)}
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.s, .at i, .value i + 100)), compute.write ((compute.Write _)(.to k.out.c, .at i, .value i)))
}
k = compute.parallel f "cpu"
outs = (compute.plrun k ((compute.A In)(.n 3, .I In(.a 0))) : Out)
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
    let messages = attempt("(compute.collect outs.c, compute.collect outs.s)")
        .expect_err("a refusal must not be lost because the answer is a tuple");
    assert_eq!(
        messages.len(),
        1,
        "one misplaced write is one diagnostic: {messages:?}"
    );
    assert!(
        messages[0].contains("compute.parallel") && messages[0].contains("to output 1"),
        "the refusal must name the output the write points at: {messages:?}"
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
        messages[0].contains("compute.parallel") && messages[0].contains("to output 1"),
        "the refusal must name the output the write points at: {messages:?}"
    );
}
