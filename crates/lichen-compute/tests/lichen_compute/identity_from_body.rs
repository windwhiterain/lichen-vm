//! A fold's seed is the one its own body marked, so a tile answers what the un-tiled body answers.

use super::common;

use super::common::LANES;
use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, take_graph_pass_lines};
use lichen_compute::fusion_claim::{REDUCTION, claim_of, fragment_of, kernel_id};
use lichen_compute::language::LangProgram;
use lichen_graph_ir::{Count, Graph, KernelNode, Node};
use lichen_lowlevel::Module;

/// The launch the tile divides: three workgroups cover `[0, 192)`.
const COUNT: usize = 3 * LANES as usize;

/// The datum a fill kernel writes at lane `i` for the row a `max` folds.
///
/// # Invariant
/// `(0.0 - 2000000.0) - int2float i` is the row `−2_000_000 … −2_000_191` descending, so the
/// row's maximum is its first element, and **every** element is below the
/// `(0.0 - 1000000.0)` the defect was seeded with. That is the whole defect: the old seed was
/// never checked against the data, and the data is what the pass never sees.
const BELOW_THE_OLD_SEED: &str = "(0.0 - 2000000.0) - int2float i";

/// The row a `min` folds: `0.0 … 191.0`, the row the defect was measured over.
///
/// # Invariant
/// Ascending from zero, so the row's minimum is its first element and the old `(0.0 - 1000000.0)`
/// seed answered a value no element of the row holds.
fn a_row_of_positives() -> Vec<f32> {
    (0..COUNT).map(|value| value as f32).collect()
}

/// The datum a fill kernel writes at lane `i` for [`a_row_of_positives`]'s row.
///
/// # Invariant
/// `int2float i` is `a_row_of_positives()[i]` lane for lane, so the row the device folds and the
/// row this test derives its own minimum from are one list.
const A_ROW_OF_POSITIVES: &str = "int2float i";

/// A `Reduction` program folding `combine` over the row `data_at_i` writes, from `seed`.
///
/// # Invariant
/// The fold's bound is `i + 1`, the lane's own index plus one, so the program's own body is the
/// per-lane schedule the tiling pass replaces and the two answers are read at `out[COUNT - 1]`
/// and `out[0]` — which is what makes the comparison a statement about **tile ≡ body** rather
/// than about a number chosen here. `Par` declares no `.out` group: the body declares `z` with
/// `compute.slice` and the write is the codomain field's value, and the input data is a fill
/// kernel's own output, because a host builds its data by running a kernel
/// (`docs/notes/kernel-buffer-declarations.md`).
fn fold_over(combine: &str, seed: &str, data_at_i: &str) -> String {
    let read = "(compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Float)";
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct{{.b (compute.Buf Float)}}
Out = struct{{.z (compute.Buf Float)}}
Par = struct{{.n Int, .in In}}
reduction = (k : Par) => {{
  i = compute.range k.n
  z = compute.slice ((compute.Slice _)(.count k.n, .element Float))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, {combine} ~ "combine")
  ((Out)(.z (compute.write ((compute.Write _)(.to z, .at i, .value fold (i + 1, {seed} ~ "identity"))))))
}}
kernel = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" Out
step = (s : Par) => {{
  compute.plrun kernel ((compute.A In)(.n s.n, .I In(.b s.in.b))) : Out
}}
built = compute.graph step
Fill = struct{{.n Int}}
fill_raw = (c : Fill) => {{
  i = compute.range c.n
  y = compute.slice ((compute.Slice _)(.count c.n, .element Float))
  ((Out)(.z (compute.write ((compute.Write _)(.to y, .at i, .value ({data_at_i} ~ "element"))))))
}}
filler = compute.parallel (fill_raw ~ "claim:Elementwise(element)") "cpu" Out
data = (compute.plrun filler ((Fill)(.n {count})) : Out)
out = compute.collect (compute.graphrun built ({count}, data.z))
out
"#,
        combine = combine.replace("READ", read),
        count = COUNT,
    )
}

/// The one-node graph the recording's own fragment makes.
///
/// # Invariant
/// The fragment is the kernel whose claim names [`REDUCTION`], chosen by the claim and not by
/// the order the module compiled in: the program compiles the fill kernel as well as the fold, and
/// the pass is handed the fold's body and not the fill's. The node is built from the registry's
/// own fragment and not from a run's, so the pass is handed exactly the body the program
/// compiled: a refusal is then the derivation's answer to that body, and the consultation
/// dispatches nothing.
fn recorded_graph(module: &Module<LangProgram>) -> Graph {
    let kernel = module
        .nodes
        .iter()
        .find_map(|(id, _node)| kernel_id(module, id).ok())
        .filter(|id| claim_of(*id).is_some_and(|claim| claim.interface == REDUCTION))
        .expect("the program compiled one parallel kernel that folds");
    let fragment = fragment_of(kernel).expect("the registry holds the fragment it compiled");
    let mut graph = Graph::with_inputs(1);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment,
                inputs: vec![0],
                count: Count::Constant(COUNT),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    graph
}

/// The lines the tiling pass reports for `source`'s recorded graph.
///
/// # Invariant
/// The pass is consulted over the recording and never through a run, so its refusals are read
/// without a dispatch: both refusals this test reads are the derivation's, and neither needs a
/// backend to be raised.
fn consulted(source: &str) -> Vec<String> {
    let build = common::check(source).build.expect("the program checks");
    install_tiling(&build.module);
    let graph = recorded_graph(&build.module);
    consult_graph_pass(&graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    lines
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
/// disagree with. The row the device folds is the one [`BELOW_THE_OLD_SEED`] writes, whose
/// maximum is its first element and every one of which is below the old seed.
#[test]
fn a_maximum_over_data_below_the_old_seed_answers_what_the_untiled_body_answers() {
    let (_stub, _guard) = common::stub();
    let source = fold_over(
        "(max s(1) (READ))",
        "((0.0 - 1.0) / 0.0)",
        BELOW_THE_OLD_SEED,
    );

    // The floor: the program runs its own body, and a stub computes nothing.
    let (module, _value, _root) = common::run(&source);

    install_tiling(&module);
    let (passed_module, _passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
    clear_graph_pass();
    println!("the tiling pass reported: {}", report.join(" "));
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
    let source = fold_over("(min s(1) (READ))", "(1.0 / 0.0)", A_ROW_OF_POSITIVES);

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
/// Both refusals come out of the **derivation**, and both used to reach the report through an arm
/// that printed only the sentence, so a report could name the answer but not the variant. These
/// two measure the fix: a `NaN` seed, which the restated `Div` keeps in a tree rather than
/// folding to an infinity, and a claim asking for an `identity` the body never marked.
#[test]
fn a_seed_that_is_no_literal_and_a_missing_role_are_refused_by_name() {
    // `0.0 / 0.0` is a `NaN` and not an infinity, so it keeps its tree and is no literal.
    let not_a_literal = fold_over("(max s(1) (READ))", "(0.0 / 0.0)", A_ROW_OF_POSITIVES);
    let report = consulted(&not_a_literal);
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
        A_ROW_OF_POSITIVES,
    )
    .replace(
        "claim:Reduction(combine,identity)",
        "claim:Reduction(combine)",
    );
    let report = consulted(&missing);
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
