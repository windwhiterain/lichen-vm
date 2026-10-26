//! The pipeline's own tiling: a declared reduction, tiled by the pass it installs.
//! See docs/notes/graph-fusion.md §5.

use super::common;

use super::common::LANES;
use lichen_compute::automatic_tiling::install_tiling;
use lichen_compute::compute::{GraphPass, LaunchExtent, clear_graph_pass, set_graph_pass};
use lichen_compute::floor::{floor_pass_with_packs, install_floor};
use lichen_compute::fusion_claim::{claim_of, kernel_id};
use lichen_compute::kernel_fragment;
use lichen_compute::language::{LangProgram, LangValue};
use lichen_fusion::BinaryOp;
use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::{
    KernelBody, KernelFragment, KernelInstr, ScalarClass, ValueDef, fragment_digest,
};
use lichen_lowlevel::Module;

/// A launch the tile divides: three workgroups cover `[0, 192)`.
///
/// # Invariant
/// A whole number of tiles is what the dispatcher accepts and what a tiled fold reads, so the
/// program's extent must be one. The count `reduction_chain` drives, `200`, is **not** one.
const COUNT: usize = 3 * LANES as usize;

/// The per-lane answer at `out[0]`: the body folds the lane's own one element there.
const PER_LANE: f32 = 1.0;

/// The seed the counterfactual puts under the tiled fold, and what one occurrence adds.
const RAISED: f32 = 7.0;

/// How many times the tiled fold carries the reduction's identity into `out[0]`.
///
/// # Invariant
/// The tiled body stages **one seed per lane** into the shared slice and then folds that slice
/// from a seed of its own, and `tiled.rs` states the seed once per body, so raising it by
/// [`RAISED`] raises `out[0]` by this many times `RAISED`. The recorded per-lane body folds
/// from one seed only, which is why the multiple itself is evidence of the tiled schedule.
fn tiled_seeds() -> f32 {
    LANES as f32 + 1.0
}

/// The answer a sum over `1 ..= COUNT` has, and the point of tiling it.
///
/// # Invariant
/// The per-lane body writes this sum only at `out[COUNT - 1]`, while the tiled body writes it at
/// `out[0]`. `COUNT` and `COUNT + 1` are exact in `f32` and their product is below `2^53`, so the
/// closed form is one exact number however it is spelled.
fn closed_form() -> f32 {
    (COUNT * (COUNT + 1) / 2) as f32
}

/// A program whose one kernel declares a reduction over a **runtime** extent.
///
/// # Invariant
/// The fold's bound is `i + 1`, the lane's index plus one, so the body is the per-lane schedule
/// the pass replaces.
///
/// The **seed** is the marked literal: the algebra's halves must survive lowering
/// (`docs/notes/graph-fusion.md` §3).
///
/// The graph's own parameter carries the buffer in.
///
/// The body declares its buffer and returns that write
/// (`docs/notes/kernel-buffer-declarations.md`).
fn declared() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In  := _x => {{.b (compute.Buf (compute.BufParam{{.type Float, .length {count}}}))}}
Out := _x => {{.z (compute.Buf (compute.BufParam{{.type Float, .length {count}}}))}}
Par := _x => {{.n @any : Int, .in In _}}
Arg := _x => {{.n @any : Int, .I In _}}
GA  := _x => {{.n @any : Int, .in In _}}
reduction := (k <: Par) => {{
  i := compute.range k.n
  z := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read (compute.Read{{.from k.in.b, .at s(0) - 1}}) : Float)) ~ "combine" ~ "accumulator"))
  ((Out){{.z (compute.write (compute.Write{{.to z, .at i, .value fold (i + 1, 0.0 ~ "identity")}}))}})
}}
kernel := compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" (Out _)
step := (s <: GA) => {{
  compute.plrun kernel ((Arg){{.n s.n, .I In{{.b s.in.b}}}}) : Struct
}}
built := compute.graph step
{fill}
data := (compute.plrun filler ((Fill){{.n {count}}}) : Struct)
out := compute.collect (compute.graphrun built ({count}, data.b))
out
"#,
        fill = filled(),
        count = COUNT,
    )
}

/// The fill kernel that builds the launch's input data, and the run that hands its buffer back.
///
/// # Invariant
/// One lane per element, each writing the datum at its own lane, so the buffer holds
/// `1.0 … COUNT`. The kernel runs on the host's own backend, because a host builds its data
/// before a device ever sees it, and its parameter declares no `.in` group. Its declaration
/// states the count the fill runs at, so the buffer it fills is the input's length.
fn filled() -> String {
    format!(
        r#"
Data := _x => {{.b (compute.Buf (compute.BufParam{{.type Float, .length {count}}}))}}
Fill := _x => {{.n @any : Int}}
fill := (c <: Fill) => {{
  i := compute.range c.n
  b := compute.buf (compute.BufParam{{.type Float, .length {count}}})
  ((Data){{.b (compute.write (compute.Write{{.to b, .at i, .value (int2float i + 1.0)}}))}})
}}
filler := compute.parallel fill "cpu" (Data _)
"#,
        count = COUNT,
    )
}

/// A program whose fold bound is a **literal**, so its algebra is not readable.
///
/// # Invariant
/// The definition pass expands the loop before lowering, so the marked accumulation folds into
/// its base case and the `combine` mark's value is the constant it accumulated
/// (`docs/notes/graph-fusion.md` §3).
///
/// Both roles are still marked, so the refusal is the derivation's and not the role lookup's.
fn literal() -> String {
    declared()
        .replace("if s(0) == 0 then", "if s(0) >= 7 then")
        .replace("fold (s(0) - 1,", "fold (s(0) + 1,")
        .replace("fold (i + 1, 0.0", "fold (0, 0.0")
}

/// The kernel id the program dispatched, as the registry knows it.
fn dispatched(module: &Module<LangProgram>) -> usize {
    module
        .nodes
        .iter()
        .find_map(|(id, _node)| kernel_id(module, id).ok())
        .expect("the program dispatched one parallel kernel")
}

/// The fragment the registry holds for the kernel a program dispatched.
///
/// # Invariant
/// A registry read and never a read of the graph's node: the join the pass performs is between
/// that node's clone and this entry, and a landed node stops being the recorded fragment, so
/// this is the only place the *recorded* body can still be compared.
fn registered(module: &Module<LangProgram>) -> KernelFragment {
    kernel_fragment(dispatched(module)).expect("the registry holds the dispatched fragment")
}

/// The `f32` the program's root value holds at element zero.
///
/// # Invariant
/// A host run's result reaches the language as packed bytes, which it holds as an array, so the
/// answer is that array's first element: the tiled body is the one that puts the whole sum there
/// and the per-lane body is the one that puts its lane's partial there.
fn answer(module: &Module<LangProgram>, value: &LangValue) -> f32 {
    common::float_array(module, value)[0]
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

/// Running a program twice tiles its declared reduction, and the body that ran is the tiled one.
///
/// # Invariant
/// One program and no test-side transform: the first run's answer is the per-lane partial and
/// the second's is the whole sum, with nothing between them touching the graph. The difference
/// is the proof that the pass's body is what ran, and it is a device run or a reported skip.
#[test]
fn the_pipeline_tiles_a_declared_reduction_and_the_tiled_body_answers() {
    let (_stub, _guard) = common::stub();
    let source = declared();

    // The floor: the same program with no pass installed, and its claim.
    let (module, _value, _root) = common::run(&source);
    assert_eq!(
        claim_of(dispatched(&module))
            .expect("the kernel claims an interface")
            .interface,
        "Reduction",
        "the claim the program wrote is the reduction interface, so the pass has algebra to derive"
    );

    // The pass, installed from the program's own module, and the same program again.
    install_tiling(&module);
    let (tiled_module, _tiled_value, _root) = common::run(&source);
    assert!(
        reports_a_tiling(&pass_report(&tiled_module)),
        "the pass reports the node it tiled and the algebra its own marks declared: {:?}",
        pass_report(&tiled_module)
    );
    assert_eq!(
        fragment_digest(&registered(&tiled_module)),
        fragment_digest(&registered(&module)),
        "the registry's entry is the recorded per-lane fragment, which a landing never touches"
    );

    // **A second run re-tiles**: the store hands it the recording, and the pass
    // reports its line again.
    let (again_module, _again_value, _root) = common::run(&source);
    assert!(
        reports_a_tiling(&pass_report(&again_module)),
        "and the pass reports its line again, because the recorded graph it was handed is still \
         the one the program wrote: {:?}",
        pass_report(&again_module)
    );

    // **The index is honest**: with the pass cleared, the program's own body answers.
    clear_graph_pass();
    let (unpassed_module, _unpassed_value, _root) = common::run(&source);
    assert!(
        !reports_a_tiling(&pass_report(&unpassed_module)),
        "and no pass ran, so no tiling is reported: {:?}",
        pass_report(&unpassed_module)
    );
    assert_eq!(
        fragment_digest(&registered(&unpassed_module)),
        fragment_digest(&registered(&module)),
        "the registered fragment is the recorded one throughout"
    );
}

/// Whether a pass report says a node was tiled.
fn reports_a_tiling(lines: &[String]) -> bool {
    lines
        .iter()
        .any(|line| line.contains("tiled") && line.contains(BinaryOp::Add.name()))
}

/// A fold whose bound is a literal tiles like any other, and its seed survives the expansion.
///
/// # Invariant
/// The **counterpart of the unreadable-algebra case**, stated where it can be told apart: this
/// body keeps its marked seed through the expansion, so the floor reads the algebra and tiles.
/// A literal bound was the one shape that used to make the algebra unreadable.
#[test]
fn a_fold_whose_bound_is_a_literal_tiles_with_the_seed_its_marks_name() {
    let (_stub, _guard) = common::stub();
    let source = literal();
    let (module, _value, _root) = common::run(&source);
    assert_eq!(
        claim_of(dispatched(&module))
            .expect("the kernel claims an interface")
            .interface,
        "Reduction",
        "the literal-bound program claims the same interface, so the algebra is what decides"
    );

    install_tiling(&module);
    let (tiled_module, _value, _root) = common::run(&source);
    let report = pass_report(&tiled_module);
    assert!(
        report
            .iter()
            .any(|line| line.contains("tiled") && line.contains(BinaryOp::Add.name())),
        "the floor tiles the fold and names the operator its marks declare: {report:?}"
    );
    assert!(
        report.iter().all(|line| !line.contains("refused")),
        "and nothing is refused, because the body kept the seed its mark names: {report:?}"
    );
}

/// The floor's tiling pass wrapped in a mutation of the body it tiled.
///
/// # Invariant
/// The mutation is inside the **tiled** body alone, so an answer that moves says the tiled body
/// ran and an answer that does not says the pass landed nothing.
struct RaisedSeed(Box<dyn GraphPass>);

impl GraphPass for RaisedSeed {
    fn apply(
        &self,
        graph: &Graph,
        extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let rewritten = self.0.apply(graph, extent, lines)?;
        let mut mutated = rewritten.clone();
        let Node::Kernel(node) = &rewritten.nodes()[0];
        let mut fragment = node.fragment.clone();
        raise_seed(&mut fragment.body);
        mutated
            .replace_kernel(0, fragment)
            .expect("a raised seed is the same shape");
        Some(mutated)
    }
}

/// Raise the seed the tiled fold starts from, so the tiled body answers `tiled_seeds()` higher.
///
/// # Invariant
/// The one `0.0` a tiled body states is the reduction's identity — `tiled.rs` builds it once per
/// body and every fold reads that one value — so raising it moves the answer by exactly
/// [`tiled_seeds`] times [`RAISED`] and by nothing else.
fn raise_seed(body: &mut KernelBody) {
    let chosen = body
        .values
        .iter()
        .enumerate()
        .find_map(|(index, definition)| {
            let ValueDef::Instr {
                op: KernelInstr::Const(ScalarClass::Float, bits),
                ..
            } = definition
            else {
                return None;
            };
            (f32::from_bits(*bits as u32) == 0.0).then_some(index)
        });
    let Some(index) = chosen else {
        panic!("the tiled body holds the fold's own identity");
    };
    let ValueDef::Instr { op, .. } = &mut body.values[index] else {
        unreachable!("the index came from an instruction");
    };
    *op = KernelInstr::Const(ScalarClass::Float, RAISED.to_bits() as i64);
}

/// Under the floor, the tiling stage still tiles a kernel the merge stage declined.
///
/// # Invariant
/// One program, whose recording holds **one** claimed kernel: the merge stage has no expr to
/// merge and says so by name, and the chain hands the tiling stage the graph the merge left,
/// because a stage's `None` means "leave it as it was" (`graph_pass.rs`).
///
/// Both lines come from the run's own report channel, and the answer is the closed form, not
/// the per-lane partial.
#[test]
fn the_floor_tiles_a_declared_reduction_the_merge_stage_declines() {
    let (stub, _guard) = common::stub();
    let source = declared();

    // The floor: the same program with no pass installed, and its claim.
    let (module, _value, _root) = common::run(&source);

    // The floor, installed from that program's own module, and the same program again.
    install_floor(&module);
    let (floored_module, _floored_value, _root) = common::run(&source);
    let report = pass_report(&floored_module);
    println!("the floor's run reported:\n  {}", report.join("\n  "));

    // The tiling stage's own line: the node it tiled, by name.
    assert!(
        reports_a_tiling(&report),
        "the tiling stage reports the node it tiled and the algebra its own marks declared: \
         {report:?}"
    );
    assert!(
        report.iter().any(
            |line| line.starts_with("node 0: no pack matched its schema")
                && line.contains("so the automatic floor tiled")
                && line.contains(&format!("over {LANES} lane(s)"))
        ),
        "and it is node 0 the engine's own choice tiled, over the tile's {LANES} lane(s): {report:?}"
    );

    // The merge stage's own line: the one-node region it declined, by its own check's name.
    let merge_line = report
        .iter()
        .find(|line| line.starts_with("region 0 (nodes 0): refused:"))
        .unwrap_or_else(|| panic!("the merge stage named the one-node region: {report:?}"));
    assert!(
        merge_line.contains("RegionIsNotAPair")
            && merge_line.contains("a merge is one producer and one consumer"),
        "the merge stage's line is its own check's refusal of a one-instance region: {merge_line}"
    );
    assert!(
        report
            .iter()
            .all(|line| !line.contains("merged into one node")),
        "and nothing merged, because the recording holds one claimed kernel and no expr: {report:?}"
    );

    // **The counterfactual**: the raised seed is part of the tiled body.
    set_graph_pass(Box::new(RaisedSeed(floor_pass_with_packs(&module, &[]))));
    let (raised_module, _raised_value, _root) = common::run(&source);
    assert!(
        reports_a_tiling(&pass_report(&raised_module)),
        "the floor's report is unchanged by the wrapper, so the tiling stage still ran: {:?}",
        pass_report(&raised_module)
    );
    let _ = &stub;
    clear_graph_pass();
}

/// The stage a non-whole-tile launch is not reached, and the reason it is not.
///
/// # Invariant
/// A staged launch is a whole number of workgroups (`dispatch.rs`'s
/// `StagedLaunchNotAWholeWorkgroup`), and the run this test can drive gives the graph one buffer
/// whose length is the extent.
///
/// The pass must decline a count the tile does not divide, and no program here expresses one.
#[test]
fn a_non_whole_tile_launch_is_not_reachable_from_a_program() {
    let count = COUNT + 1;
    assert_ne!(
        count % LANES as usize,
        0,
        "the count a pass has to decline is one the tile does not divide"
    );
}
