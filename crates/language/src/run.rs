//! Running a source program to its output value.
//!
//! [`evaluate`] compiles and checks a program, runs it (the deep evaluation
//! of its root value), and renders the result as text — the program's
//! output.  Diagnostics are returned unrendered so the caller (the CLI, the
//! example tests) can render them with carets.

use lichen_highlevel::program::{HighProgram, HighValue};
use lichen_lowlevel::{Module, NodeId, Value};

use crate::compile;
use crate::diag::Diag;

/// Compile, check, and run `source`; the rendered output value.
///
/// On failure the diagnostics (frontend and checker) are returned.  Every
/// v1 program terminates, so the deep evaluation cannot hit the VM's
/// recursion guard.
pub fn evaluate(source: &str) -> Result<String, Vec<Diag>> {
    let report = compile(source);
    if !report.diagnostics.is_empty() {
        return Err(report.diagnostics);
    }
    let build = report.build.unwrap();
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    Ok(print_value(&module, value))
}

/// Render a runtime value as the program's output.
///
/// The runtime value is all the printer sees: arrays print with `[ ]`
/// whether the source wrote a tuple or an array (the same value at
/// runtime), functions print as `Function`, and the type constants by their
/// source spellings `Int` / `Type`.  A type pair `[head, [Type, ↺]]` renders
/// as its head (`[TypeInt, K]` → `Int`), and the self-looping universe as
/// `Type` — otherwise the recursive pair encoding would loop forever.
pub fn print_value(module: &Module<HighProgram>, value: Value<HighProgram>) -> String {
    print_inner(module, value, &mut Vec::new())
}

fn print_inner(
    module: &Module<HighProgram>,
    value: Value<HighProgram>,
    path: &mut Vec<NodeId>,
) -> String {
    match value {
        Value::USize(n) => n.to_string(),
        Value::Ext(HighValue::TypeInt) => "Int".to_string(),
        Value::Ext(HighValue::TypeType) => "Type".to_string(),
        Value::Ext(HighValue::TypeFunction) => "TypeFunction".to_string(),
        Value::Ext(HighValue::TypeTuple) => "TypeTuple".to_string(),
        Value::Ext(HighValue::TypeArray) => "TypeArray".to_string(),
        Value::Ext(HighValue::TypeId(n)) => format!("TypeId({n})"),
        Value::Array(ptr) => {
            let elements = unsafe { &*ptr };
            // A type pair `[head, K]`: the kind slot is the self-looping
            // universe, so render just the head (and cut the cycle).
            if elements.len() == 2 && is_universe(module, elements[1]) {
                return print_inner(
                    module,
                    module.nodes[elements[0]].value.unwrap_or(Value::None),
                    path,
                );
            }
            let mut out = Vec::new();
            for &id in elements {
                if path.contains(&id) {
                    out.push("…".to_string());
                } else {
                    path.push(id);
                    let element = module.nodes[id].value.unwrap_or(Value::None);
                    let text = print_inner(module, element, path);
                    path.pop();
                    out.push(text);
                }
            }
            format!("[{}]", out.join(", "))
        }
        Value::Function(_) => "Function".to_string(),
        Value::None => "none".to_string(),
        Value::Parameterized => "parameterized".to_string(),
    }
}

/// The canonical universe `K = [Type, ↺]` — a node whose value is an array
/// that contains the node itself.
fn is_universe(module: &Module<HighProgram>, node: NodeId) -> bool {
    matches!(module.nodes[node].value, Some(Value::Array(ptr)) if unsafe { &*ptr }.contains(&node))
}
