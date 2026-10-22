//! Probe: the `if` in a fold's step, and the body the loop conversion lowers.
//!
//! # Invariant
//!
//! Print-only. Each variant differs from the others in one spelling of the `if`, and
//! the reading is two counts: the `Gt` nodes the graph holds, and the `Gt`
//! instructions the body carries. A body count higher than the graph's is the
//! emitter doubling a comparison; equal counts move the question into the graph.

use super::common;

/// The program, with the `if`'s condition and arms spelled by the caller.
fn program(condition: &str, then: &str, otherwise: &str) -> String {
    format!(
        r#"
--- compute = import "compute.lichen" ---
TIn  = struct{{.x (compute.Buf Float)}}
TOut = struct{{.y (compute.Buf Float)}}
TPar = struct{{.n Int, .taps Int, .in TIn, .out TOut}}
TArg = struct{{.n Int, .taps Int, .I TIn}}
tiny_raw = (c : TPar) => {{
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (if {condition} then {then} else {otherwise})) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}}
tiny = compute.parallel (tiny_raw ~ "claim:Reduction(combine,identity,element)") "cpu" TOut
x = (compute.buffer ([1.0, 1.0] : array{{Float, 2}}) : (compute.Buf Float))
out = (compute.plrun tiny ((TArg)(.n 2, .taps 3, .I TIn(.x x))) : TOut)
compute.collect out.y : array{{Float, 2}}
"#
    )
}

/// How many `Gt` nodes the graph holds, and how many `Gt` (or `Select`) instructions
/// the lowered body carries.
fn counts(source: &str) -> String {
    use lichen_highlevel::program::TypeOperator;
    use lichen_kernel_ir::KernelInstr;
    use lichen_utils::extend::AsEnum;

    let report = common::check(source);
    if !report.ok() {
        return format!("refused: {:?}", report.diagnostics);
    }
    let build = report.build.expect("a checked program builds");
    let mut graph_gts = 0;
    let mut graph_selects = 0;
    for (id, _node) in build.module.nodes.iter() {
        let Some(operation) = build.module.node_operation(id) else {
            continue;
        };
        match AsEnum::<TypeOperator>::as_enum(&operation.operator) {
            Some(TypeOperator::Gt) => graph_gts += 1,
            _ => {}
        }
        if matches!(
            build.module.selection_of(id),
            Some(lichen_lowlevel::Selection::Computed)
        ) {
            graph_selects += 1;
        }
    }
    let body = common::sole_fragment(&build.module).body;
    let mut body_gts = 0; // comparisons of any kind
    let mut body_selects = 0;
    for block in &body.blocks {
        for &instr in &block.instrs {
            let Some(lichen_kernel_ir::ValueDef::Instr { op, .. }) =
                body.values.get(instr.0 as usize)
            else {
                continue;
            };
            match op {
                KernelInstr::Bin(_, op)
                    if !matches!(
                        op,
                        lichen_kernel_ir::KernelBin::Add
                            | lichen_kernel_ir::KernelBin::Sub
                            | lichen_kernel_ir::KernelBin::Mul
                            | lichen_kernel_ir::KernelBin::Div
                    ) =>
                {
                    body_gts += 1
                }
                KernelInstr::Select => body_selects += 1,
                _ => {}
            }
        }
    }
    format!(
        "graph: {graph_gts} Gt node(s), {graph_selects} selection(s); body: {body_gts} comparison(s), {body_selects} Select"
    )
}

#[test]
fn probe_the_body_an_if_in_a_fold_lowers_to() {
    let _slot = common::slot();
    for (label, condition, then, otherwise) in [
        ("state_gt_1", "s(0) > 1", "1.0", "0.0"),
        ("state_gt_leaf", "s(0) > c.taps", "1.0", "0.0"),
        ("leaf_gt_state", "c.taps > s(0)", "1.0", "0.0"),
        ("two_state_gt", "s(0) > s(0)", "1.0", "0.0"),
        ("leaf_gt_1", "c.taps > 1", "1.0", "0.0"),
        ("one_lt_state", "1 < s(0)", "1.0", "0.0"),
        ("state_gt_1_equal_arms", "s(0) > 1", "1.0", "1.0"),
        ("state_eq_2", "s(0) == 2", "1.0", "0.0"),
    ] {
        println!("[{label}] {}", counts(&program(condition, then, otherwise)));
    }
}
