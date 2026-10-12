//! A `combine` that is not the fold is refused by name, so no weaker algebra answers.
//! See docs/notes/graph-fusion.md §5.

mod common;

use common::device;

use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::clear_graph_pass;
use lichen_compute::language::LangProgram;
use lichen_compute_gpu::LOCAL_SIZE_X;
use lichen_lowlevel::Module;

/// The launch the tile divides: three workgroups cover `[0, 192)`.
const COUNT: usize = 3 * LOCAL_SIZE_X as usize;

/// The per-lane answer at `out[0]`: the body folds the lane's own one element there, and that
/// element is `1.0²`.
///
/// # Invariant
/// The same schedule `automatic_tiling.rs`'s `Σ x` case pins: the fold's bound is the lane's own
/// index plus one, so lane zero folds `b[0]` — squared here, which is still `1.0` — and the tiled
/// body would have put `1 + 2 + … + 192` there instead.
const PER_LANE: f32 = 1.0;

/// A `Σ x²` program: the one of `automatic_tiling.rs` with the datum squared inside the combine.
///
/// # Invariant
/// One expression differs and the algebra with it: the marked combine canonicalizes to
/// `(+ t (* d d))`, so its **top-level operator** is still `+` while its datum is not the datum.
fn squares() -> String {
    let mut source = String::from(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct{.b (compute.Buf Float)}
Out = struct{.z (compute.Buf Float)}
Par = compute.P (compute.KT Float)(.I In, .O Out)
reduction = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) * compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)))) ~ "combine" ~ "accumulator"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (i + 1, 0.0 ~ "identity")))
}
kernel = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu"
step = (s : Par) => {
  compute.plrun kernel ((compute.A In)(.n s.n, .I In(.b s.in.b)))
}
built = compute.graph step
data = (compute.buffer ([ELEMENTS] : array{Float, COUNT}) : (compute.Buf _))
out = compute.collect (compute.graphrun built (COUNT, data))
out
"#,
    );
    let elements = (1..=COUNT)
        .map(|position| format!("{position}.0"))
        .collect::<Vec<String>>()
        .join(", ");
    source = source.replace("ELEMENTS", &elements);
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

/// The refusal names its check and the program's own body is what answers.
///
/// # Invariant
/// The pass rebuilds the fold from the combine's **top-level operator** over the raw buffer, so a
/// `Σ x²` claim used to install `Σ x`: same program, same device, same data, and `out[0]` moved
/// from [`PER_LANE`] to `18528`. Nothing landed under the gate, so the weaker number cannot be
/// produced at all — and the line is what tells an operator author their claim is at fault.
#[test]
fn a_sum_of_squares_is_refused_by_name_and_the_weaker_sum_never_answers() {
    let Some((_context, _device)) =
        device("a_sum_of_squares_is_refused_by_name_and_the_weaker_sum_never_answers")
    else {
        return;
    };
    let source = squares();

    let (module, _value, _root) = common::run(&source);
    install_tiling(&module);
    let (passed_module, passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
    clear_graph_pass();

    assert!(
        report
            .iter()
            .any(|line| line.starts_with("node 0: refused:")
                && line.contains("CombineIsNotAFold")
                && line.contains("describe the whole datum")),
        "the pass refuses the claim by the check's own name, and says what a claim must describe: \
         {report:?}"
    );
    assert_eq!(
        common::float_array(&passed_module, &passed_value)[0],
        PER_LANE,
        "and the program's own per-lane body answered, so the weaker `Σ x` schedule produced no \
         number at all: {report:?}"
    );
}
