//! Probe: a `@loop` whose step writes, and what the codomain makes of it.
//! `@B@` is the backend, `@BODY@` the body.
//!
//! # Invariant
//!
//! Print-only. The state carries the output buffer, so a step's write returns the
//! buffer it filled; the codomain's position is the loop's result, not a write node.
//! The reading is the check's refusal, or a run's answer (the last iteration's store
//! wins), and the lowered body with the store's own block named. The measured
//! reading is in `docs/notes/loop-conversion.md` §6.

use lichen_compute::ComputeOperator;
use lichen_compute::language::{LangProgram, new_store};
use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, FunctionId, LowOperator, LowValue, Module, NodeId,
};
use lichen_utils::extend::AsEnum;

use super::common;

/// The program, with the backend and the body substituted.
const TEMPLATE: &str = r#"
--- compute = import "compute.lichen" ---
In  := _x => {.unused (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Out := _x => {.y (compute.Buf (compute.BufParam{{.type Float, .length _}}))}
Par := _x => {.n @any : Int, .taps @any : Int, .in In _, .out Out}
Arg := _x => {.n @any : Int, .taps @any : Int, .I In _}
raw := (c <: Par) => {
  i := compute.range c.n
@BODY@
}
k := compute.parallel raw "@B@" Out
x := (compute.buffer ([@any : Float, 2][1.0, 1.0] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
out := (compute.plrun k ((Arg){.n 2, .taps 3, .I In{.unused x}}) : Struct)
compute.collect out.y : Array
"#;

/// One plain write: the control the loop is compared against.
const CONTROL: &str = r#"  compute.write (compute.Write{.to c.out.y, .at i, .value 2.0})"#;

/// A fold whose step writes the countdown into the buffer it carries.
const LOOP: &str = r#"  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (compute.write (compute.Write{.to s(1), .at i, .value (int2float s(0))})))
  fold (c.taps, c.out.y)"#;

/// What a run answered, or how it refused.
fn run(body: &str, backend: &str) -> String {
    let source = TEMPLATE.replace("@BODY@", body).replace("@B@", backend);
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(&source, None, &mut store) {
        Ok(text) => format!("answered {text}"),
        Err(diagnostics) => format!(
            "refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| format!("{:?}: {}", diagnostic.span, diagnostic.message))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_loop_that_writes() {
    let _slot = common::slot();
    println!("[control] {}", run(CONTROL, "cpu"));
    println!("[loop_cpu] {}", run(LOOP, "cpu"));
    println!("[loop_gpu] {}", run(LOOP, "gpu"));
    println!("[body]\n    {}", dump_body(LOOP));
}

/// The lowered body, block by block: the write's own block is the reading, since a
/// store in the step is the point.
fn dump_body(body: &str) -> String {
    use lichen_kernel_ir::{KernelInstr, ValueDef};

    let source = TEMPLATE.replace("@BODY@", body).replace("@B@", "cpu");
    let report = common::check(&source);
    if !report.ok() {
        return format!(
            "refused: {}",
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
                .join(" | ")
        );
    }
    let build = report.build.expect("a checked program builds");
    let body = common::sole_fragment(&build.module).body;
    let mut lines = Vec::new();
    for (index, block) in body.blocks.iter().enumerate() {
        let stores: Vec<String> = block
            .instrs
            .iter()
            .filter_map(|&instr| {
                let Some(ValueDef::Instr { op, .. }) = body.values.get(instr.0 as usize) else {
                    return None;
                };
                matches!(
                    op,
                    KernelInstr::BufferWriteCall(_) | KernelInstr::BufferAtomicAddCall(_)
                )
                .then(|| format!("v{}", instr.0))
            })
            .collect();
        lines.push(format!(
            "block {index} params {:?} terminator {:?} stores [{}]",
            block.params,
            block.terminator,
            stores.join(", ")
        ));
        for &instr in &block.instrs {
            let Some(def) = body.values.get(instr.0 as usize) else {
                continue;
            };
            match def {
                ValueDef::Instr { op, args, .. } => {
                    lines.push(format!("  v{} = {op:?}{args:?}", instr.0));
                }
                other => lines.push(format!("  v{} = {other:?}", instr.0)),
            }
        }
    }
    lines.join("\n    ")
}

/// The same fold with the state's second slot holding something else: the place the
/// walk stops at is the slot's contents.
#[test]
fn probe_the_state_slot_the_walk_stops_at() {
    let _slot = common::slot();
    for (label, body) in [
        (
            "buffer",
            r#"  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (compute.write (compute.Write{.to s(1), .at i, .value (int2float s(0))})))
  fold (c.taps, c.out.y)"#,
        ),
        (
            "literal",
            r#"  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (compute.write (compute.Write{.to s(1), .at i, .value (int2float s(0))})))
  fold (c.taps, 0.0)"#,
        ),
        (
            "scalar",
            r#"  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (compute.write (compute.Write{.to s(1), .at i, .value (int2float s(0))})))
  fold (c.taps, c.n)"#,
        ),
        (
            "write_outside",
            r#"  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, s(1))
  compute.write (compute.Write{.to c.out.y, .at i, .value (fold (c.taps, 0.0))})"#,
        ),
    ] {
        println!("[{label}] {}", run(body, "cpu"));
    }
}

/// The graph the refusal is about: the kernel body's return node, what it holds, and
/// the chain a walk follows from it.
#[test]
fn probe_the_graph_a_loop_body_leaves() {
    let _slot = common::slot();
    let source = TEMPLATE.replace("@BODY@", LOOP).replace("@B@", "cpu");
    let report = common::check(&source);
    for diagnostic in &report.diagnostics {
        println!("diag: {}", diagnostic.message);
    }
    let Some(build) = report.build else {
        println!("no build");
        return;
    };
    let module = &build.module;
    let Some(kernel) = kernel_function(module) else {
        println!("no kernel function");
        return;
    };
    let function = &module.functions[kernel];
    println!(
        "kernel {kernel:?} looping {:?} return {:?} parameter {:?}",
        function.looping, function.r#return, function.parameter
    );
    println!("  return: {}", describe(module, function.r#return));
    println!(
        "  return's expr value half: {:?}",
        module.expr_value_half(function.r#return)
    );
    println!(
        "  return's items: {:?}",
        array_items(module, function.r#return)
    );
    // The chain the return's value half is.
    if let Some(value) = module.expr_value_half(function.r#return) {
        println!("  value half chain:");
        print_chain(module, value, 1);
    }
    // The loop function itself.
    for (id, entry) in module.functions.iter() {
        if entry.looping.is_none() {
            continue;
        }
        println!("loop {id:?} return {:?}", entry.r#return);
        println!("  loop return: {}", describe(module, entry.r#return));
        let Ok(conversion) = module.loop_conversion(id) else {
            println!("  not convertible");
            continue;
        };
        println!("  state {:?}", conversion.state);
        for (index, exit) in conversion.exits.iter().enumerate() {
            println!(
                "  exit {index} value {:?} path {:?}",
                exit.value,
                module.parameter_value_path(id, exit.value)
            );
            println!("    {}", describe(module, exit.value));
        }
        for (index, step) in conversion.steps.iter().enumerate() {
            println!("  step {index} apply {:?}", step.apply);
            for next in &step.next {
                println!(
                    "    next {:?} path {:?}",
                    next,
                    module.parameter_value_path(id, *next)
                );
                println!("      {}", describe(module, *next));
            }
        }
        for (index, test) in conversion.tests.iter().enumerate() {
            println!("  test {index} condition {:?}", test.condition);
        }
    }
    // The apply of the loop function.
    for (id, _node) in module.nodes.iter() {
        let Some(operation) = module.node_operation(id) else {
            continue;
        };
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Apply)
        ) {
            continue;
        }
        let Some(callee) = module.callee_function(id) else {
            continue;
        };
        if !module.functions[callee].looping.is_some() {
            continue;
        }
        println!("apply {id:?} of loop {callee:?}");
        println!("  apply: {}", describe(module, id));
        println!("  apply's items: {:?}", array_items(module, id));
        println!(
            "  apply's expr value half: {:?}",
            module.expr_value_half(id)
        );
        if let Some(operands) = module.operands_of(id).ok() {
            for operand in operands {
                println!("  operand {operand:?}: {}", describe(module, operand));
            }
        }
    }
    // The nodes a walk cannot emit: no value and no operation.
    for (id, _node) in module.nodes.iter() {
        if module.node_operation(id).is_some()
            || module.node_value(AnyNodeId::Dynamic(id)).is_some()
        {
            continue;
        }
        let root = module.class_root(id);
        let members: Vec<String> = module
            .nodes
            .iter()
            .filter(|(member, _)| module.class_root(*member) == root)
            .map(|(member, _)| {
                let own = module.node_operation(member).map(|operation| {
                    format!(
                        "{:?}",
                        AsEnum::<LowOperator>::as_enum(&operation.operator)
                            .map(|operator| format!("{operator:?}"))
                            .unwrap_or_else(|| "structural".to_string())
                    )
                });
                format!("{member:?} {own:?}")
            })
            .collect();
        println!(
            "empty {id:?} owner {:?} origin {:?} class {root:?} members [{}]",
            module.node_function(id),
            module.node_origin(id),
            members.join(", ")
        );
    }
}

/// The function `compute.parallel` was handed.
fn kernel_function(module: &Module<LangProgram>) -> Option<FunctionId> {
    for (id, _node) in module.nodes.iter() {
        let Some(operation) = module.node_operation(id) else {
            continue;
        };
        if !matches!(
            AsEnum::<ComputeOperator>::as_enum(&operation.operator),
            Some(ComputeOperator::Parallel)
        ) {
            continue;
        }
        let items = operation
            .operand
            .and_then(|operand| module.operand_items(operand).ok())
            .unwrap_or_default();
        let Some(callee) = items.first().and_then(|item| item.node.dynamic()) else {
            continue;
        };
        if let Some(LowValue::Function(AnyFunctionId::Dynamic(fid))) = module
            .node_value(AnyNodeId::Dynamic(callee))
            .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
        {
            return Some(fid);
        }
    }
    None
}

/// A node's own reading: its operator, its value, and the items it holds.
fn describe(module: &Module<LangProgram>, node: NodeId) -> String {
    let operator = module.node_operation(node).map(|operation| {
        format!(
            "{:?}",
            AsEnum::<ComputeOperator>::as_enum(&operation.operator)
                .map(|operator| format!("{operator:?}"))
                .unwrap_or_else(|| {
                    format!(
                        "{:?}",
                        AsEnum::<LowOperator>::as_enum(&operation.operator)
                            .map(|operator| format!("{operator:?}"))
                            .unwrap_or_else(|| "structural".to_string())
                    )
                })
        )
    });
    let value = module
        .node_value(AnyNodeId::Dynamic(node))
        .map(|value| format!("{value:?}"));
    let origin = module.node_origin(node).map(|origin| format!("{origin:?}"));
    let operands = module
        .operands_of(node)
        .ok()
        .map(|operands| {
            format!(
                "[{}]",
                operands
                    .iter()
                    .map(|operand| format!("{operand:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .unwrap_or_default();
    format!(
        "{node:?} class {:?} op {operator:?} value {value:?} origin {origin:?} operands {operands} items {:?}",
        module.class_root(node),
        array_items(module, node)
    )
}

/// The items a node's value holds, when it holds an array.
fn array_items(module: &Module<LangProgram>, node: NodeId) -> Option<Vec<String>> {
    // SAFETY: `node` is a live node of `module`.
    let items = unsafe { module.array_items(node) }?;
    Some(
        items
            .iter()
            .map(|item| {
                let Some(node) = item.node.dynamic() else {
                    return format!("{:?}", item.node);
                };
                let operator = module.node_operation(node).map(|operation| {
                    format!(
                        "{:?}",
                        AsEnum::<LowOperator>::as_enum(&operation.operator)
                            .map(|operator| format!("{operator:?}"))
                            .unwrap_or_else(|| "structural".to_string())
                    )
                });
                format!("{node:?} {operator:?}")
            })
            .collect(),
    )
}

/// The chain of `Index` reads a node is, outermost first.
fn print_chain(module: &Module<LangProgram>, node: NodeId, depth: usize) {
    let indent = "  ".repeat(depth);
    println!("{indent}{}", describe(module, node));
    let Some(operation) = module.node_operation(node) else {
        return;
    };
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&operation.operator),
        Some(LowOperator::Index)
    ) {
        return;
    }
    let Ok(operands) = module.operands_of(node) else {
        return;
    };
    let Some(target) = operands.first().copied() else {
        return;
    };
    let index = operands
        .get(1)
        .copied()
        .and_then(|index| module.usize_value(index));
    println!("{indent}  index {index:?}");
    print_chain(module, target, depth + 1);
}
