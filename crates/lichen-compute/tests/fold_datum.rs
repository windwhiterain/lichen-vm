//! What a fold can be performed over: the operators, and the datum it applies.

mod common;

use common::device;

use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::clear_graph_pass;
use lichen_compute::language::LangProgram;
use lichen_compute_gpu::LOCAL_SIZE_X;
use lichen_fusion::{BinaryOp, Expr, Reduction, Variable};
use lichen_fusion_bridge::{Element, Folding};
use lichen_kernel_ir::ScalarClass;
use lichen_lowlevel::Module;

/// The launch the tile divides: three workgroups cover `[0, 192)`.
const COUNT: usize = 3 * LOCAL_SIZE_X as usize;

/// One `Reduction` program whose combine is `combine`, over the buffer read as `READ`.
///
/// # Invariant
/// The fold bound is `i + 1`, the lane's own index plus one, so the program's own body is the
/// per-lane schedule the tiling pass replaces, and `combine` is the only thing that varies
/// between the cases here — so every difference in what answers is the datum's.
fn fold_over(combine: &str, identity: &str) -> String {
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
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, COMBINE ~ "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (i + 1, IDENTITY ~ "identity")))
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
    let read = "compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1))";
    let elements = (1..=COUNT)
        .map(|position| format!("{position}.0"))
        .collect::<Vec<String>>()
        .join(", ");
    source = source.replace("COMBINE", combine.replace("READ", read).as_str());
    source = source.replace("IDENTITY", identity);
    source = source.replace("ELEMENTS", &elements);
    source.replace("COUNT", &COUNT.to_string())
}

/// A `max` fold over the raw datum, seeded the way `examples/softmax.lichen` seeds its own.
///
/// # Invariant
/// A maximum's own identity is `-inf` and the language cannot write one, so the seed is
/// `softmax.lichen`'s own `(0.0 - 1000000.0)`: below every element here, so the fold's answer is
/// the row maximum either way. **The pass carries the seed the claim declared and invents no
/// sentinel**, which is what makes the tile right for this data and not for any data.
fn maximum() -> String {
    fold_over("(max s(1) (READ))", "(0.0 - 1000000.0)")
}

/// A fold whose datum is a comparison, which no kernel body computes.
///
/// # Invariant
/// `(+ t (> d 0.0))` canonicalizes to a sum over a comparison, and a comparison narrows to a
/// condition rather than a value: `KernelBin::Gt` yields `0`/`1` and the symbolic operator has no
/// lowered form to apply per element, so no body computes this datum and a fold that read the
/// raw buffer instead would answer `Σ x`.
fn unreadable_datum() -> String {
    fold_over("((s(1) + (READ > 0.0)))", "0.0")
}

/// A fold whose combine subtracts, which the canonicalizer rewrites into a sum over a negation.
///
/// # Invariant
/// `Sub` is not associative, so it is not a fold at all — but `normalize` never emits it, it
/// writes `(t - d)` as `(+ t (- d))`, and so what the gate refuses here is the **negation**: a
/// unary operator the kernel IR carries no instruction for. The line names which of the two
/// questions the claim failed.
fn difference() -> String {
    fold_over("((s(1) - READ))", "0.0")
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

/// A claim whose datum is the bare slot builds the body the raw buffer read built.
///
/// # Invariant
/// **The `Σ x` case, unchanged.** A datum of `d` alone compiles to the one read the raw element
/// already emitted, so the pass's `Element::Function` and the `Element::Buffer` it replaced agree
/// on every value, block and barrier — which is what keeps an already-tiled sum tiling byte for
/// byte rather than to within a tolerance.
#[test]
fn a_datum_that_is_the_bare_slot_builds_the_raw_buffers_body() {
    let reduction = Reduction {
        combine: BinaryOp::Add,
        identity: 0.0,
    };
    let raw = Folding::new(
        &reduction,
        Element::Buffer { position: 0 },
        ScalarClass::Float,
    )
    .expect("a sum over a buffer is a fold");
    let carried = Folding::new(
        &reduction,
        Element::Function {
            position: 0,
            function: Box::new(Expr::Variable(Variable::DATUM)),
        },
        ScalarClass::Float,
    )
    .expect("a sum over the datum itself is the same fold");
    assert_eq!(
        carried
            .tiled(LOCAL_SIZE_X, 0)
            .map(|tiled| tiled.into_body()),
        raw.tiled(LOCAL_SIZE_X, 0).map(|tiled| tiled.into_body()),
        "the two elements build the same tiled body, value for value"
    );
    assert_eq!(
        carried.per_lane(),
        raw.per_lane(),
        "and the same per-lane body, which is what the un-passed program is differenced against"
    );
}

/// A `max` reduction tiles under the same proof, and answers what the un-tiled body answers.
///
/// # Invariant
/// The operator is the gate's only question: `max` is associative and has a lowered instruction,
/// so it is a fold now, where `CombineIsNotAFold` refused it by name before. The two schedules
/// place the same maximum at two addresses, and `max` answers the same element for element under
/// re-association, where `+` does not.
#[test]
fn a_maximum_tiles_under_the_same_proof_and_answers_the_untiled_body() {
    let Some((_context, _device)) =
        device("a_maximum_tiles_under_the_same_proof_and_answers_the_untiled_body")
    else {
        return;
    };
    let source = maximum();
    let largest = COUNT as f32;

    let (module, value, _root) = common::run(&source);
    let per_lane = common::float_array(&module, &value);
    assert_eq!(
        per_lane[COUNT - 1],
        largest,
        "with no pass installed the program's own body folds the whole launch, so the maximum is \
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
            .any(|line| line.contains("tiled") && line.contains("`max`")),
        "the pass tiles the node and names the operator its own marks declared: {report:?}"
    );
    assert!(
        report.iter().all(|line| !line.contains("refused")),
        "and nothing was refused, so `max` reached the same schedule `+` does: {report:?}"
    );
    assert_eq!(
        tiled[0], largest,
        "the tiled fold reads the maximum into `out[0]`"
    );
    assert_eq!(
        tiled[0],
        per_lane[COUNT - 1],
        "and it is the number the un-passed body computed for the same launch, element for \
         element: `max` is re-associated exactly, so re-association costs this fold nothing"
    );
}

/// A datum no kernel body computes is refused by name, and the program's own body answers.
///
/// # Invariant
/// The refusal is the gate's and it is a **stage line**, so a runner reads which question the
/// claim failed; nothing lands, so the answer is the un-passed program's element for element and
/// the `Σ x` a raw-buffer fold would have produced is not reachable.
#[test]
fn a_datum_no_body_computes_is_refused_by_name_and_the_body_answers() {
    let Some((_context, _device)) =
        device("a_datum_no_body_computes_is_refused_by_name_and_the_body_answers")
    else {
        return;
    };
    let source = unreadable_datum();

    let (module, value, _root) = common::run(&source);
    let per_lane = common::float_array(&module, &value);
    assert_eq!(
        per_lane[0], 1.0,
        "the un-passed body folds one element, and the element is a comparison that is true"
    );

    install_tiling(&module);
    let (passed_module, passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
    let passed = common::float_array(&passed_module, &passed_value);
    clear_graph_pass();
    println!("the tiling pass reported:\n  {}", report.join("\n  "));

    assert!(
        report
            .iter()
            .any(|line| line.starts_with("node 0: refused:")
                && line.contains("DatumIsNotAReadableFunction")
                && line.contains("no lowered instruction carries")),
        "the pass refuses the datum by the check's own name and says why: {report:?}"
    );
    assert_eq!(
        passed, per_lane,
        "nothing landed, so the node keeps the program's own body, element for element"
    );
    assert_ne!(
        passed[COUNT - 1],
        ((COUNT * (COUNT + 1) / 2) as f32),
        "and the weaker `Σ x` a raw-buffer fold would have read is not the answer here either"
    );

    // **A second program**: the pass is installed from the module that dispatched it.
    let source = difference();
    let (difference_module, difference_value, _root) = common::run(&source);
    let per_difference = common::float_array(&difference_module, &difference_value);
    install_tiling(&difference_module);
    let (passed_module, passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
    let difference = common::float_array(&passed_module, &passed_value);
    clear_graph_pass();
    assert!(
        report.iter().any(|line| line.contains("refused:")
            && line.contains("DatumIsNotAReadableFunction")
            && line.contains("-d")),
        "a subtraction is refused too, and by the datum arm rather than the operator's: \
         `normalize` never emits `Sub`, it writes the sum over a negation, and the negation is \
         what no body computes: {report:?}"
    );
    assert_eq!(
        difference, per_difference,
        "and its node kept the program's own body, so a fold `Folding` cannot perform is named and \
         left alone rather than scheduled as something else"
    );
    assert_eq!(
        difference[COUNT - 1],
        -((COUNT * (COUNT + 1) / 2) as f32),
        "which for this program is the negated sum its own per-lane body folds, and not the `Σ x` \
         a raw-buffer fold would have read"
    );
}
