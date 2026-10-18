//! A `combine` names the whole datum, and a tiled fold computes that datum and not the buffer.

use super::common;

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
/// body would have put `1² + 2² + … + 192²` there instead.
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
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Float) * (compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Float))) ~ "combine" ~ "accumulator"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (i + 1, 0.0 ~ "identity")))
}
kernel = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" Out
step = (s : Par) => {
  compute.plrun kernel ((compute.A In)(.n s.n, .I In(.b s.in.b))) : Out
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

/// The sum of squares over `1 … COUNT`, the one number both schedules must agree on.
///
/// # Invariant
/// Every partial sum of `j²` for `j ≤ COUNT` is an integer below `2^24`, so the closed form is one
/// exact `f32` however it is spelled and no rounding enters the comparison.
fn sum_of_squares() -> f32 {
    let count = COUNT as f64;
    (count * (count + 1.0) * (2.0 * count + 1.0) / 6.0) as f32
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
    let Some((_context, _device)) =
        device("a_sum_of_squares_tiles_with_the_datum_its_claim_names_and_answers_the_squares")
    else {
        return;
    };
    let source = squares();

    let (module, value, _root) = common::run(&source);
    let per_lane = common::float_array(&module, &value);
    assert_eq!(
        per_lane[0], PER_LANE,
        "with no pass installed the program runs its own per-lane body, whose lane zero folds the \
         one element at position zero"
    );
    assert_eq!(
        per_lane[COUNT - 1],
        sum_of_squares(),
        "and its last lane folds the whole launch, so the program's own answer to `Σ x²` is \
         already on the device before any pass runs"
    );

    install_tiling(&module);
    let (passed_module, passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
    let tiled = common::float_array(&passed_module, &passed_value);
    clear_graph_pass();
    println!("the tiling pass reported:\n  {}", report.join("\n  "));

    assert!(
        report
            .iter()
            .any(|line| line.contains("tiled") && line.contains("`+`")),
        "the pass tiles the node and names the operator its own marks declared: {report:?}"
    );
    assert_eq!(
        tiled[0],
        sum_of_squares(),
        "the tiled fold applies the claim's own datum to each element, so `out[0]` is `Σ x²` and \
         not the `Σ x` the old fold would have read out of the raw buffer: {report:?}"
    );
    assert_eq!(
        tiled[0],
        per_lane[COUNT - 1],
        "and it is element for element the number the un-passed program computed for the same \
         launch, which is the whole claim of this slice: the tiled fold computes what the body \
         computes"
    );
    assert_ne!(
        tiled[0],
        ((COUNT * (COUNT + 1) / 2) as f32),
        "and it is not `Σ x`, the wrong answer this claim used to be scheduled into"
    );
}
