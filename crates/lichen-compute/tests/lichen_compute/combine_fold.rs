//! A `combine` names the whole datum, and a tiled fold computes that datum and not the buffer.

use super::common;

use super::common::LANES;
use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::clear_graph_pass;
use lichen_compute::language::LangProgram;
use lichen_lowlevel::Module;

/// The launch the tile divides: three workgroups cover `[0, 192)`.
const COUNT: usize = 3 * LANES as usize;
/// A `Σ x²` program: the one of `automatic_tiling.rs` with the datum squared inside the combine.
///
/// # Invariant
/// One expression differs and the algebra with it: the marked combine canonicalizes to
/// `(+ t (* d d))`, so its **top-level operator** is still `+` while its datum is not the datum.
/// The fold's parameter carries no `.out` group and the data is a fill kernel's own output,
/// because a kernel declares its buffers and a host builds its data by running a kernel
/// (`docs/notes/kernel-buffer-declarations.md`).
fn squares() -> String {
    let mut source = String::from(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct{.b (compute.Buf Float)}
Out = struct{.z (compute.Buf Float)}
Par = struct{.n Int, .in In}
reduction = (k : Par) => {
  i = compute.range k.n
  z = compute.slice ((compute.Slice _)(.count k.n, .element Float))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Float) * (compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Float))) ~ "combine" ~ "accumulator"))
  ((Out)(.z (compute.write ((compute.Write _)(.to z, .at i, .value fold (i + 1, 0.0 ~ "identity"))))))
}
kernel = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" Out
step = (s : Par) => {
  compute.plrun kernel ((compute.A In)(.n s.n, .I In(.b s.in.b))) : Out
}
built = compute.graph step
Fill = struct{.n Int}
fill_raw = (c : Fill) => {
  i = compute.range c.n
  y = compute.slice ((compute.Slice _)(.count c.n, .element Float))
  ((Out)(.z (compute.write ((compute.Write _)(.to y, .at i, .value ((int2float i + 1.0) ~ "element"))))))
}
filler = compute.parallel (fill_raw ~ "claim:Elementwise(element)") "cpu" Out
data = (compute.plrun filler ((Fill)(.n COUNT)) : Out)
out = compute.collect (compute.graphrun built (COUNT, data.z))
out
"#,
    );
    source.replace("COUNT", &COUNT.to_string())
}

/// The lines the installed pass reported for a module, through the run report's own channel.
fn pass_report(module: &Module<LangProgram>) -> Vec<String> {
    module
        .extension_diagnostics
        .iter()
        .filter(|entry| entry.category.contains("pass"))
        .map(|entry| entry.message.clone())
        .collect()
}

/// The carryable datum makes `Σ x²` tile, and the tiled fold answers the squares.
///
/// # Invariant
/// **This is the case the gate used to refuse.** It pinned that `(+ t (* d d))` was refused
/// `CombineIsNotAFold` and the weaker `Σ x` never answered; the capability moves it. The check is
/// the one number the same program computes under two schedules: the un-passed per-lane body
/// writes the whole `Σ x²` at `out[COUNT - 1]` and the tiled body writes it at `out[0]`.
#[test]
fn a_sum_of_squares_tiles_with_the_datum_its_claim_names_and_answers_the_squares() {
    let (_stub, _guard) = common::stub();
    let source = squares();

    // The floor: the program runs its own body, and a stub computes nothing.
    let (module, _value, _root) = common::run(&source);

    install_tiling(&module);
    let (passed_module, _passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
    clear_graph_pass();
    println!("the tiling pass reported: {report:?}");

    assert!(
        report
            .iter()
            .any(|line| line.contains("tiled") && line.contains("`+`")),
        "the pass tiles the node and names the operator its own marks declared: {report:?}"
    );
    assert!(
        report.iter().all(|line| !line.contains("refused")),
        "and nothing was refused, so the carryable datum tiles: {report:?}"
    );
}
