//! Probe: the `if` in a fold's step, and the body the loop conversion lowers.
//!
//! # Invariant
//!
//! Print-only. Each variant differs from the others in one spelling of the `if`, and the
//! reading is four things: the refusal the check recorded, the `Gt` nodes the graph holds,
//! the operand shape of each of those nodes, and the count of comparison instructions the
//! lowered body carries. A body count higher than the source's is the emitter doubling a
//! comparison; equal counts move the question into the graph.

use super::common;
use lichen_highlevel::program::TypeOperator;
use lichen_utils::extend::AsEnum;

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

/// The one word naming an operator, so a dump reads in the program's own spelling.
///
/// # Invariant
///
/// The name comes off the vocabulary's own `AsEnum` conversion — the same one the
/// checker's classification uses — so a probe cannot drift from the class it reads.
fn operator_name(operator: &TypeOperator) -> String {
    format!("{operator:?}")
}

/// Whether the name is one of the four order comparisons the probe counts.
fn is_order_comparison(name: &str) -> bool {
    matches!(name, "Gt" | "Lt" | "Geq" | "Leq")
}

/// How many `Gt` nodes the graph holds, and how many comparison instructions the lowered
/// body carries — through a refusal as much as through a clean check.
///
/// # Invariant
///
/// A refused check still hands back the build it refused (`Report::build` is `Some` on
/// every path), so the graph is readable whether or not the body lowered; only the body
/// count is missing when the refusal came from the body walk itself.
fn counts(source: &str) -> String {
    use lichen_kernel_ir::KernelInstr;

    let report = common::check(source);
    let clean = report.ok();
    let refusal = if clean {
        "clean".to_string()
    } else {
        format!(
            "refused: {}",
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<String>>()
                .join(" | ")
        )
    };
    let Some(build) = report.build else {
        return format!("{refusal}; no build");
    };
    let mut graph_gts = 0;
    for (id, _node) in build.module.nodes.iter() {
        let Some(operation) = build.module.node_operation(id) else {
            continue;
        };
        if is_order_comparison(&operator_name(
            &AsEnum::<TypeOperator>::as_enum(&operation.operator)
                .unwrap_or(TypeOperator::Add),
        )) {
            graph_gts += 1;
        }
    }
    // The body is only lowered when the check reached it; a refusal from the body walk
    // leaves no fragment to count, so the count is the clean check's alone.
    if !clean {
        return format!(
            "{refusal}; graph: {graph_gts} order-comparison node(s); body: not lowered"
        );
    }
    let body = common::sole_fragment(&build.module).body;
    let mut body_gts = 0;
    for block in &body.blocks {
        for &instr in &block.instrs {
            let Some(lichen_kernel_ir::ValueDef::Instr { op, .. }) = body.values.get(instr.0 as usize)
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
                _ => {}
            }
        }
    }
    format!("{refusal}; graph: {graph_gts} order-comparison node(s); body: {body_gts} comparison(s)")
}

/// Every order-comparison node of the graph, with the operands it actually holds.
///
/// # Invariant
///
/// A node is printed against the class root its operands resolve to, because a doubled
/// comparison is a node whose operand reads back as the node itself — a fact about the
/// equality classes, not about the operand array.
fn dump_comparisons(source: &str) -> String {
    let report = common::check(source);
    let Some(build) = report.build else {
        return "no build".to_string();
    };
    let module = &build.module;
    let mut lines = Vec::new();
    for (id, _node) in module.nodes.iter() {
        let Some(operation) = module.node_operation(id) else {
            continue;
        };
        let Some(operator) = AsEnum::<TypeOperator>::as_enum(&operation.operator) else {
            continue;
        };
        let name = operator_name(&operator);
        if !is_order_comparison(&name) && name != "Eq" {
            continue;
        }
        let items = operation
            .operand
            .and_then(|operand| module.operand_items(operand).ok())
            .map(|items| {
                items
                    .iter()
                    .map(|item| match item.node {
                        lichen_lowlevel::AnyNodeId::Static(reference) => {
                            format!("static({reference:?})")
                        }
                        lichen_lowlevel::AnyNodeId::Dynamic(node) => format!(
                            "{node:?}{}{}",
                            if module.class_root(node) == module.class_root(id) {
                                " [SAME CLASS AS THE COMPARISON]"
                            } else {
                                ""
                            },
                            match module.node_origin(node) {
                                Some(origin) => format!(" <- {origin:?}"),
                                None => String::new(),
                            }
                        ),
                    })
                    .collect::<Vec<String>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "<no operand array>".to_string());
        let selection = match module.selection_of(id) {
            Some(lichen_lowlevel::Selection::Computed) => " computed-selection",
            Some(lichen_lowlevel::Selection::Views(_)) => " view",
            None => "",
        };
        lines.push(format!(
            "{name} {id:?}{selection}: [{items}]",
        ));
    }
    lines.join("\n    ")
}

/// What the program answers, or what it refused with.
fn run_answer(source: &str) -> String {
    let report = common::check(source);
    if !report.ok() {
        return format!(
            "refused: {}",
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<String>>()
                .join(" | ")
        );
    }
    let build = report.build.expect("a checked program builds");
    let (module, value, _root) = lichen_language::testing::finish(build);
    format!("answered {:?}", common::float_array(&module, &value))
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
        let source = program(condition, then, otherwise);
        println!("[{label}] {}", counts(&source));
        println!("    {}", dump_comparisons(&source));
    }
    println!("[run state_gt_1] {}", run_answer(&program("s(0) > 1", "1.0", "0.0")));
    println!(
        "[run leaf_gt_1] {}",
        run_answer(&program("c.taps > 1", "1.0", "0.0"))
    );
    println!(
        "[run state_eq_2] {}",
        run_answer(&program("s(0) == 2", "1.0", "0.0"))
    );
}