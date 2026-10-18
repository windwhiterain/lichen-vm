//! Probe: a `compute.plrun` inside a plain-applied function, and where its forcing goes.
//!
//! # Invariant
//!
//! A plain host run is one pass, and a run stamp is written once: an operator asked before
//! its operand is decided never dispatches. Only a recording re-drives, through
//! `reopen_stalled`. See docs/notes/graph-recording-rounds.md, which already calls the
//! matching test's premise a test premise.

use std::panic::{AssertUnwindSafe, catch_unwind, set_hook, take_hook};

use lichen_compute::ComputeOperator;
use lichen_compute::language::{LangProgram, LangValue, new_store};
use lichen_language::render::print_value_lang;
use lichen_lowlevel::{AnyNodeId, EvaluatedDeep, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

use super::common;

/// The failing test's kernels, with every class left a fresh cell.
const KERNELS_OPEN: &str = r#"
In1  = struct{.a Int}
Out1 = struct{.z (compute.Buf _)}
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "gpu" Out1
In2  = struct{.b (compute.Buf _)}
Out2 = struct{.w (compute.Buf _)}
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : Int)
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}
k2 = compute.parallel f2 "gpu" Out2
"#;

/// The same kernels with every class decided, which the graph path needs anyway.
const KERNELS_INT: &str = r#"
In1  = struct{.a Int}
Out1 = struct{.z (compute.Buf Int)}
Par1 = compute.P (compute.KT Int)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "gpu" Out1
In2  = struct{.b (compute.Buf Int)}
Out2 = struct{.w (compute.Buf Int)}
Par2 = compute.P (compute.KT Int)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : Int)
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}
k2 = compute.parallel f2 "gpu" Out2
"#;

/// The argument the step is applied to.
const ARGUMENT: &str = "((GArg)(.n 3, .in InS(.a 0)))";

/// The step of the failing test: two dispatches the body reads, and one nothing reads.
const STEP_OPEN: &str = r#"
InS  = struct{.a Int}
GArg = struct{.n Int, .in InS}
step = (s : GArg) => {
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  dead = (compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z))) : Out2)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z)))
}
"#;

/// The same step with its last `plrun` named, which halves the graph leg's trace.
const STEP_ANNOTATED: &str = r#"
InS  = struct{.a Int}
GArg = struct{.n Int, .in InS}
step = (s : GArg) => {
  first = (compute.plrun k1 ((compute.A In1)(.n s.n, .I In1(.a 0))) : Out1)
  compute.plrun k2 ((compute.A In2)(.n s.n, .I In2(.b first.z))) : Out2
}
"#;

/// The two dispatches with no apply at all: the control the graph leg is compared against.
const DIRECT: &str = r#"
first = (compute.plrun k1 ((compute.A In1)(.n 3, .I In1(.a 0))) : Out1)
out = (compute.plrun k2 ((compute.A In2)(.n 3, .I In2(.b first.z))) : Out2)
compute.collect out.w
"#;

/// One program of the probe: its kernels, its body, and the expression it answers with.
fn program(kernels: &str, body: &str, tail: &str) -> String {
    format!(
        r#"---
  compute = import "compute.lichen"
---
{kernels}
{body}
{tail}
"#
    )
}

/// The verdict a node carries, as the one word a reader needs.
fn verdict(module: &Module<LangProgram>, node: NodeId) -> String {
    match module.node_evaluated_deep(node) {
        None => "never-walked".to_string(),
        Some(EvaluatedDeep { undecided: true }) => "undecided".to_string(),
        Some(EvaluatedDeep { undecided: false }) => "concrete".to_string(),
    }
}

/// What a node holds, in the words the lowlevel printer uses.
fn value_of(module: &Module<LangProgram>, node: NodeId) -> String {
    match module.node_value(AnyNodeId::Dynamic(node)) {
        None => "no-value".to_string(),
        Some(value) => match AsEnum::<LowValue>::as_enum(&value) {
            Some(LowValue::USize(n)) => format!("usize({n})"),
            Some(LowValue::Float(f)) => format!("float({f})"),
            Some(LowValue::Str(s)) => format!("str({s})"),
            Some(LowValue::Array(_)) => "array".to_string(),
            Some(LowValue::Table(_)) => "table".to_string(),
            Some(LowValue::Function(_)) => "function".to_string(),
            _ => format!("{value:?}"),
        },
    }
}

/// One node as a line: what it holds, whether it still owes an answer, and how deep the
/// pass that reached it read it.
fn node_line(module: &Module<LangProgram>, node: NodeId) -> String {
    format!(
        "{node:?} depth={} owed={} value={} verdict={}",
        module.node_depth(node),
        module.has_no_result_yet(node),
        value_of(module, node),
        verdict(module, node),
    )
}

/// The elements of a node's operand array, with the shallow marker the deep pass skips on.
fn operand_items(module: &Module<LangProgram>, node: NodeId) -> String {
    let Some(operation) = module.node_operation(node) else {
        return "not-an-operation".to_string();
    };
    let Some(operand) = operation.operand else {
        return "no-operand".to_string();
    };
    let Some(LowValue::Array(array)) = module
        .node_value(AnyNodeId::Dynamic(operand))
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
    else {
        return format!("operand {operand:?} has no array value");
    };
    // SAFETY: `array` is the live operand value read from a node of a live module.
    let items = unsafe { array.items() };
    let parts: Vec<String> = items
        .iter()
        .map(|item| match item.node {
            AnyNodeId::Dynamic(node) => format!(
                "{node:?}{}{}",
                if item.shallow { " (shallow)" } else { "" },
                node_line(module, node)
            ),
            AnyNodeId::Static(sref) => format!("static({sref:?})"),
        })
        .collect();
    parts.join(" | ")
}

/// Every compute node of the module, so a reader sees which operator ran and which did not.
fn compute_nodes(module: &Module<LangProgram>) -> Vec<(ComputeOperator, NodeId)> {
    let mut found = Vec::new();
    for (id, _node) in module.nodes.iter() {
        let Some(operation) = module.node_operation(id) else {
            continue;
        };
        let Some(operator) = AsEnum::<ComputeOperator>::as_enum(&operation.operator) else {
            continue;
        };
        found.push((operator, id));
    }
    found
}

/// The forcing path from `node`: its operand elements and its value elements, one level on.
fn tree(module: &Module<LangProgram>, node: NodeId, indent: &str, left: usize) {
    println!("{indent}{}", node_line(module, node));
    if left == 0 {
        return;
    }
    let Some(operation) = module.node_operation(node) else {
        return;
    };
    let Some(operand) = operation.operand else {
        return;
    };
    let Some(LowValue::Array(array)) = module
        .node_value(AnyNodeId::Dynamic(operand))
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
    else {
        return;
    };
    // SAFETY: `array` is the live operand value read from a node of a live module.
    for entry in unsafe { array.items() } {
        let AnyNodeId::Dynamic(element) = entry.node else {
            println!("{indent}  static element");
            continue;
        };
        println!(
            "{indent}  element{} {}",
            if entry.shallow { " (shallow)" } else { "" },
            node_line(module, element)
        );
        tree(module, element, &format!("{indent}    "), left - 1);
    }
    // A value that is itself an array is walked the same way the deep pass walks it.
    if let Some(LowValue::Array(array)) = module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
    {
        // SAFETY: as above.
        for entry in unsafe { array.items() } {
            let AnyNodeId::Dynamic(element) = entry.node else {
                continue;
            };
            println!(
                "{indent}  value-element{} {}",
                if entry.shallow { " (shallow)" } else { "" },
                node_line(module, element)
            );
            tree(module, element, &format!("{indent}    "), left - 1);
        }
    }
}

/// `common::run`'s own steps, without its two assertions: an undecided root is a reading.
fn build_and_run(source: &str) -> (Module<LangProgram>, Option<LangValue>, NodeId, NodeId) {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    assert!(diags.is_empty(), "expected imports to resolve: {diags:?}");
    let line_starts = lichen_language::lex::line_starts(source);
    let report = lichen_language::compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    assert!(
        report.ok(),
        "expected a clean check: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    let (root_val, root_ty) = (build.root_val, build.root_ty);
    let mut module = build.module;
    let value = module.evaluate_node_deep(root_val, None);
    (module, value, root_val, root_ty)
}

/// Run one program and print every reading about it, one line per fact.
fn report(name: &str, source: &str) {
    println!("=== [{name}] ===");
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw::<LangProgram>(source, None, &mut store) {
        Ok(output) => println!("  rendered: {output}"),
        Err(diagnostics) => {
            println!("  refused: {} diagnostic(s)", diagnostics.len());
            for diagnostic in &diagnostics {
                println!("    {diagnostic:?}");
            }
        }
    }
    // What `common::run` — the harness the failing test uses — does with this program.
    let hook = take_hook();
    set_hook(Box::new(|_| {}));
    let harness = catch_unwind(AssertUnwindSafe(|| common::run(source)));
    set_hook(hook);
    match harness {
        Ok((module, value, root_ty)) => println!(
            "  harness: ran, root renders as {}",
            print_value_lang(&module, value, root_ty)
        ),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_else(|| "<a non-string panic>".to_string());
            println!("  harness: panicked: {message}");
        }
    }
    let (module, value, root_val, root_ty) = build_and_run(source);
    println!("  root value decided: {}", value.is_some());
    if let Some(value) = &value {
        println!(
            "  root renders as: {}",
            print_value_lang(&module, *value, root_ty)
        );
    }
    println!("  root node: {}", node_line(&module, root_val));
    println!("  root operand items: {}", operand_items(&module, root_val));
    println!("  forcing path from the root:");
    tree(&module, root_val, "    ", 3);
    println!(
        "  extension diagnostics: {} recorded, has_failures={}",
        module.extension_diagnostics.len(),
        module.has_extension_failures()
    );
    for diagnostic in &module.extension_diagnostics {
        println!("    {diagnostic:?}");
    }
    for (operator, node) in compute_nodes(&module) {
        println!("  {operator:?}: {}", node_line(&module, node));
    }
}

#[test]
fn probe_the_plain_leg() {
    let Some((_context, _guard)) = common::device("zz_plain_leg_probe") else {
        return;
    };
    report(
        "plain_unread",
        &program(KERNELS_OPEN, STEP_OPEN, &format!("out = (step {ARGUMENT} : Out2)")),
    );
    report(
        "plain_collected",
        &program(
            KERNELS_OPEN,
            STEP_OPEN,
            &format!("out = (step {ARGUMENT} : Out2)\ncompute.collect out.w"),
        ),
    );
    report(
        "plain_collected_decided_classes",
        &program(
            KERNELS_INT,
            STEP_OPEN,
            &format!("out = (step {ARGUMENT} : Out2)\ncompute.collect out.w"),
        ),
    );
    report(
        "plain_field_collected",
        &program(
            KERNELS_OPEN,
            STEP_OPEN,
            &format!("compute.collect ((step {ARGUMENT}) : Out2).w"),
        ),
    );
    report(
        "plain_annotated_collected",
        &program(
            KERNELS_OPEN,
            STEP_ANNOTATED,
            &format!("out = (step {ARGUMENT} : Out2)\ncompute.collect out.w"),
        ),
    );
    report("direct_no_apply", &program(KERNELS_OPEN, DIRECT, "0"));
}
