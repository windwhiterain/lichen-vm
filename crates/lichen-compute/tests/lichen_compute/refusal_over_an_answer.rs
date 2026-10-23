//! A runtime refusal is reported even when the program's answer is a value that arrived.

use lichen_compute::language::new_store;

/// The committed reproduction of the withheld refusal, reduced to one element.
///
/// # Invariant
///
/// The kernel writes to the **input** buffer, which `compute.write` refuses by name, and
/// the root collects that same input buffer — a value that arrives whatever the kernel
/// did. The buffer is a fill kernel's own output, because a host builds its data with a
/// kernel (`docs/notes/kernel-buffer-declarations.md`). See
/// `crates/lichen-compute/tests/inplace_write_refusal.lichen`.
const IN_PLACE_WRITE: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 1)))}
Out = struct{.e (compute.Buf ((compute.BufParam _)(.type Float, .length 1)))}
Par = struct{.n Int, .in In}
Arg = struct{.n Int, .I In}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 1))
  ((In)(.k (compute.write ((compute.Write _)(.to d, .at i, .value 1.0)))))
}
filler = compute.parallel fill "cpu" In
raw = (lane : Par) => {
  i = compute.range lane.n
  ((Out)(.e (compute.write ((compute.Write _)(.to lane.in.k, .at i, .value (99.0 : Float))))))
}
k = compute.parallel raw "cpu" Out
kbuf = (compute.plrun filler ((Fill)(.n 1)) : In)
out = (compute.plrun k ((Arg)(.n 1, .I In(.k kbuf.k))) : Out)
compute.collect kbuf.k
"#;

/// The refusal wins over the answer: it is what the program did, whatever it collected.
#[test]
fn a_refusal_over_an_input_buffer_is_reported_beside_an_answer_that_arrived() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let diags = lichen_language::run::evaluate_raw(IN_PLACE_WRITE, None, &mut store)
        .expect_err("a collected input buffer has a value, and the refusal reports anyway");
    assert_eq!(
        diags.len(),
        1,
        "one misplaced write is one diagnostic: {diags:?}"
    );
    assert!(
        diags[0].message.contains("compute.parallel"),
        "the refusal names the operator that refused: {:?}",
        diags[0].message
    );
}
