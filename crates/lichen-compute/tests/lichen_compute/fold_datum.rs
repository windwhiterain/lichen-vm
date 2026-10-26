//! The operators a fold is performed over, and the datum it applies.
//! See docs/notes/kernel-buffer-declarations.md.

use super::common;

use super::common::LANES;
use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::clear_graph_pass;
use lichen_compute::language::LangProgram;
use lichen_fusion::{BinaryOp, Expr, Reduction, Variable};
use lichen_fusion_bridge::{Element, Folding};
use lichen_kernel_ir::ScalarClass;
use lichen_lowlevel::Module;

/// The launch the tile divides: three workgroups cover `[0, 192)`.
const COUNT: usize = 3 * LANES as usize;

/// One `Reduction` program whose combine is `combine`, over the buffer read as `READ`.
///
/// # Invariant
/// The fold bound is `i + 1`, the lane's own index plus one, so the program's own body is
/// the per-lane schedule the tiling pass replaces, and `combine` alone varies between the
/// cases, so every difference in the answer is the datum's.
///
/// No parameter carries an `.out` group: the write is the codomain's value, and the data is
/// a fill kernel's output: a host builds data by running one.
fn fold_over(combine: &str, identity: &str) -> String {
    let mut source = String::from(
        r#"
---
  compute = import "compute.lichen"
---
In  := _x => {.b (compute.Buf (compute.BufParam{.type Float, .length COUNT}))}
Out := _x => {.z (compute.Buf (compute.BufParam{.type Float, .length COUNT}))}
Par := _x => {.n @any : Int, .in In _}
reduction := (k <: Par) => {
  i := compute.range k.n
  z := compute.buf (compute.BufParam{.type Float, .length k.n})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, COMBINE ~ "combine")
  ((Out){.z (compute.write (compute.Write{.to z, .at i, .value fold (i + 1, IDENTITY ~ "identity")}))})
}
kernel := compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" (Out _)
step := (s <: Par) => {
  compute.plrun kernel (compute.A{.n s.n, .I In{.b s.in.b}}) : Struct
}
built := compute.graph step
Fill := _x => {.n @any : Int}
fill := (c <: Fill) => {
  i := compute.range c.n
  y := compute.buf (compute.BufParam{.type Float, .length COUNT})
  ((Out){.z (compute.write (compute.Write{.to y, .at i, .value (int2float i + 1.0)}))})
}
filler := compute.parallel fill "cpu" (Out _)
data := (compute.plrun filler ((Fill){.n COUNT}) : Struct)
out := compute.collect (compute.graphrun built (COUNT, data.z))
out
"#,
    );
    let read = "(compute.read (compute.Read{.from k.in.b, .at s(0) - 1}) : Float)";
    source = source.replace("COMBINE", combine.replace("READ", read).as_str());
    source = source.replace("IDENTITY", identity);
    source.replace("COUNT", &COUNT.to_string())
}

/// A `max` fold over the raw datum, seeded with the unit of `max` itself.
///
/// # Invariant
/// A maximum's own identity is `−∞` and `examples/softmax.lichen` now **writes it**, as the
/// literal `(0.0 - 1.0)` over the literal `0.0`. The earlier `(0.0 - 1000000.0)` moved with the
/// example: it was a stand-in that happened to sit below every element here, and
/// `identity_from_body.rs` is where the data below it is the case.
fn maximum() -> String {
    fold_over("(max s(1) (READ))", "((0.0 - 1.0) / 0.0)")
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
        carried.tiled(LANES, 0).map(|tiled| tiled.into_body()),
        raw.tiled(LANES, 0).map(|tiled| tiled.into_body()),
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
    let (_stub, _guard) = common::stub();
    let source = maximum();

    // The floor: the program runs its own body, and a stub computes nothing.
    let (module, _value, _root) = common::run(&source);

    install_tiling(&module);
    let (passed_module, _passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
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
}

/// A datum no kernel body computes is refused by name, and the program's own body answers.
///
/// # Invariant
/// The refusal is the gate's and it is a **stage line**, so a runner reads which question the
/// claim failed; nothing lands, so the answer is the un-passed program's element for element and
/// the `Σ x` a raw-buffer fold would have produced is not reachable.
#[test]
fn a_datum_no_body_computes_is_refused_by_name_and_the_body_answers() {
    let (_stub, _guard) = common::stub();
    let source = unreadable_datum();

    // The floor: the program runs its own body, and a stub computes nothing.
    let (module, _value, _root) = common::run(&source);

    install_tiling(&module);
    let (passed_module, _passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
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

    // **A second program**: the pass is installed from the module that dispatched it.
    let source = difference();
    let (difference_module, _difference_value, _root) = common::run(&source);
    install_tiling(&difference_module);
    let (passed_module, _passed_value, _root) = common::run(&source);
    let report = pass_report(&passed_module);
    clear_graph_pass();
    assert!(
        report.iter().any(|line| line.contains("refused:")
            && line.contains("DatumIsNotAReadableFunction")
            && line.contains("-d")),
        "a subtraction is refused too, and by the datum arm rather than the operator's:          `normalize` never emits `Sub`, it writes the sum over a negation, and the negation is          what no body computes: {report:?}"
    );
}
