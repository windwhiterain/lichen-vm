//! Running a source program to its output value.
//!
//! [`evaluate`] compiles and checks a program, runs it (the deep evaluation
//! of its root value), and renders the result as text — the program's
//! output.  Diagnostics are returned unrendered so the caller (the CLI, the
//! example tests) can render them with carets.

use lichen_highlevel::program::{HighProgram, HighValue};
use lichen_lowlevel::{Module, Value};

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
/// source spellings `Int` / `Type`.
pub fn print_value(module: &Module<HighProgram>, value: Value<HighProgram>) -> String {
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
            let inner = elements
                .iter()
                .map(|&id| print_value(module, module.nodes[id].value.unwrap_or(Value::None)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
        Value::Function(_) => "Function".to_string(),
        Value::None => "none".to_string(),
        Value::Parameterized => "parameterized".to_string(),
    }
}
