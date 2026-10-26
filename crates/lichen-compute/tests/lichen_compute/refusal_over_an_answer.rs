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
In  := _x => {.k (compute.Buf (compute.BufParam{.type Float, .length 1}))}
Out := _x => {.e (compute.Buf (compute.BufParam{.type Float, .length 1}))}
Par := _x => {.n @any : Int, .in In _}
Arg := _x => {.n @any : Int, .I In _}
Fill := _x => {.n @any : Int}
fill := (c <: Fill) => {
  i := compute.range c.n
  d := compute.buf (compute.BufParam{.type Float, .length 1})
  ((In){.k (compute.write (compute.Write{.to d, .at i, .value 1.0}))})
}
filler := compute.parallel fill "cpu" (In _)
raw := (lane <: Par) => {
  i := compute.range lane.n
  ((Out){.e (compute.write (compute.Write{.to lane.in.k, .at i, .value (99.0 : Float)}))})
}
k := compute.parallel raw "cpu" (Out _)
kbuf := (compute.plrun filler ((Fill){.n 1}) : Struct)
out := (compute.plrun k ((Arg){.n 1, .I In{.k kbuf.k}}) : Struct)
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
