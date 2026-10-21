//! A fold's seed is the one its own body marked, so a tile answers what the un-tiled body answers.

use super::common;

use super::common::LANES;
use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::clear_graph_pass;
use lichen_compute::language::LangProgram;
use lichen_lowlevel::Module;

/// The launch the tile divides: three workgroups cover `[0, 192)`.
const COUNT: usize = 3 * LANES as usize;

/// The row a `max` folds: every element **below** the `(0.0 - 1000000.0)` the defect was seeded with.
///
/// # Invariant
/// `−2_000_000 … −2_000_191` descending, so the row's maximum is its first element. The old seed
/// is above every one of them, which is the whole defect: it was never checked against the data,
/// and the data is what the pass never sees.
fn below_the_old_seed() -> Vec<f32> {
    (0..COUNT)
        .map(|offset| -2_000_000.0 - offset as f32)
        .collect()
}

/// The row a `min` folds: `0.0 … 191.0`, the row the defect was measured over.
///
/// # Invariant
/// Ascending from zero, so the row's minimum is its first element and the old `(0.0 - 1000000.0)`
/// seed answered a value no element of it holds.
fn a_row_of_positives() -> Vec<f32> {
    (0..COUNT).map(|value| value as f32).collect()
}

/// One element as the language spells it: it has no unary minus, so a negative is a subtraction.
///
/// # Invariant
/// Every element goes through this one spelling, so the array the device folds is the row this
/// test derived its own maximum and minimum from and the two cannot drift apart.
fn literal(value: f32) -> String {
    if value < 0.0 {
        let magnitude = -(value as i64);
        format!("(0.0 - {magnitude}.0)")
    } else {
        format!("{value}.0")
    }
}

/// A `Reduction` program folding `combine` over `data` from `seed`.
///
/// # Invariant
/// The fold's bound is `i + 1`, the lane's own index plus one, so the program's own body is the
/// per-lane schedule the tiling pass replaces and the two answers are read at `out[COUNT - 1]`
/// and `out[0]` — which is what makes the comparison a statement about **tile ≡ body** rather
/// than about a number chosen here.
fn fold_over(combine: &str, seed: &str, data: &[f32]) -> String {
    let elements = data
        .iter()
        .copied()
        .map(literal)
        .collect::<Vec<String>>()
        .join(", ");
    let read = "(compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Float)";
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct{{.b (compute.Buf Float)}}
Out = struct{{.z (compute.Buf Float)}}
Par = compute.P (compute.KT Float)(.I In, .O Out)
reduction = (k : Par) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, {combine} ~ "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (i + 1, {seed} ~ "identity")))
}}
kernel = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" Out
step = (s : Par) => {{
  compute.plrun kernel ((compute.A In)(.n s.n, .I In(.b s.in.b))) : Out
}}
built = compute.graph step
data = (compute.buffer ([{elements}] : array{{Float, {COUNT}}}) : (compute.Buf _))
out = compute.collect (compute.graphrun built ({COUNT}, data))
out
"#,
        combine = combine.replace("READ", read),
    )
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

/// A `max` over data **below** the old seed answers what the un-tiled body answers.
///
/// # Invariant
/// The seed is the unit of `max` — `((0.0 - 1.0) / 0.0)`, which folds to `−∞` — and it is **the
/// body's own mark**: a claim spells `Reduction(combine,identity)` and carries no value to
/// disagree with. The two answers are one number off two schedules, and it is the row's maximum
/// as derived here from the literals the device folded.
#[test]
fn a_maximum_over_data_below_the_old_seed_answers_what_the_untiled_body_answers() {
    let (_stub, _guard) = common::stub();
    let data = below_the_old_seed();
    let source = fold_over("(max s(1) (READ))", "((0.0 - 1.0) / 0.0)", &data);

    let (module, value, _root) = common::run(&source);
    let body = common::float_array(&module, &value);

    install_tiling(&module);
    let (passed_module, passed_value, _root) = common::run(&source);
    let tiled = common::float_array(&passed_module, &passed_value);
    let report = pass_report(&passed_module);
    clear_graph_pass();
    println!("the tiling pass reported:\n  {}", report.join("\n  "));

    assert_eq!(
        body[COUNT - 1],
        data.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        "the un-tiled body folds the whole launch from the seed its own mark named, and that \
         seed is `−∞`, so its answer is the row's maximum and not the old `(0.0 - 1000000.0)`"
    );
    assert_eq!(
        tiled[0],
        body[COUNT - 1],
        "the tile folds the same launch from the same seed, so the two schedules agree element \
         for element: `max` is re-associated exactly, so re-association costs this fold nothing"
    );
    assert!(
        report
            .iter()
            .any(|line| line.contains("tiled") && line.contains("seeding the fold with `-inf`")),
        "and the pass's own line names the identity it seeded with, so a wrong seed is visible \
         in the report and not only in the answer: {report:?}"
    );
}

/// A `min` over a row of non-negatives answers what the un-tiled body answers.
///
/// # Invariant
/// The seed is the unit of `min` — `1.0 / 0.0`, the same division with the other numerator — where
/// the old `(0.0 - 1000000.0)` answered a value no element of the row holds. `min` shows the
/// defect was no `max` peculiarity: it answers the seed on a row of positives, and `max` answers
/// it on a row of large negatives.
#[test]
fn a_minimum_over_a_row_of_positives_answers_what_the_untiled_body_answers() {
    let (_stub, _guard) = common::stub();
    let data = a_row_of_positives();
    let source = fold_over("(min s(1) (READ))", "(1.0 / 0.0)", &data);

    let (module, value, _root) = common::run(&source);
    let body = common::float_array(&module, &value);

    install_tiling(&module);
    let (passed_module, passed_value, _root) = common::run(&source);
    let tiled = common::float_array(&passed_module, &passed_value);
    let report = pass_report(&passed_module);
    clear_graph_pass();
    println!("the tiling pass reported:\n  {}", report.join("\n  "));

    assert_eq!(
        body[COUNT - 1],
        data.iter().copied().fold(f32::INFINITY, f32::min),
        "the un-tiled body folds from `+∞`, so its answer is the row's minimum and not a value \
         the row does not hold"
    );
    assert_eq!(
        tiled[0],
        body[COUNT - 1],
        "and the tile folds from the same `+∞`, so the two schedules agree element for element"
    );
    assert!(
        report
            .iter()
            .any(|line| line.contains("tiled") && line.contains("seeding the fold with `inf`")),
        "and the line names `+inf` as the identity the tile seeded with: {report:?}"
    );
}

/// A seed that is no literal, and a claim naming a role its body never marked, are refused by name.
///
/// # Invariant
/// Both refusals come out of the **derivation**, and both reached the run report through an arm
/// that printed only the sentence, so a report could name the answer but not the variant. These
/// two measure the fix: a `NaN` seed, which the restated `Div` keeps in a tree rather than
/// folding to an infinity, and a claim asking for an `identity` the body never marked.
#[test]
fn a_seed_that_is_no_literal_and_a_missing_role_are_refused_by_name() {
    let (_stub, _guard) = common::stub();

    // `0.0 / 0.0` is a `NaN` and not an infinity, so it keeps its tree and is no literal.
    let not_a_literal = fold_over("(max s(1) (READ))", "(0.0 / 0.0)", &a_row_of_positives());
    let (module, _value, _root) = common::run(&not_a_literal);
    install_tiling(&module);
    let (passed, _value, _root) = common::run(&not_a_literal);
    let report = pass_report(&passed);
    clear_graph_pass();
    println!("the tiling pass reported:\n  {}", report.join("\n  "));
    assert!(
        report.iter().any(|line| line.contains("refused:")
            && line.contains("NotAnIdentity:")
            && line.contains("which is not a literal")),
        "the seed that is no literal is refused with the check's own name as well as its \
         sentence: {report:?}"
    );

    // A claim asking for an `identity` the body never marked: no such projection to read.
    let missing = fold_over(
        "(max s(1) (READ))",
        "((0.0 - 1.0) / 0.0)",
        &a_row_of_positives(),
    )
    .replace(
        "claim:Reduction(combine,identity)",
        "claim:Reduction(combine)",
    );
    let (module, _value, _root) = common::run(&missing);
    install_tiling(&module);
    let (passed, _value, _root) = common::run(&missing);
    let report = pass_report(&passed);
    clear_graph_pass();
    println!("the tiling pass reported:\n  {}", report.join("\n  "));
    assert!(
        report.iter().any(|line| {
            line.contains("refused:")
                && line.contains("MissingRole:")
                && line.contains("has no `identity` projection")
        }),
        "and so is the claim whose roles the schema cannot satisfy, named the same way: {report:?}"
    );
}
