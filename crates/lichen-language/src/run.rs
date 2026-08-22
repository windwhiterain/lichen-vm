//! Running a source program to its output value.
//!
//! [`evaluate`] compiles and checks a program, runs it (the deep evaluation
//! of its root value), and renders the result as text — the program's
//! output, with its type: `5: Int`, `[1, 2, 3]: Int<3>`.  Diagnostics are
//! returned unrendered so the caller (the CLI, the example tests) can render
//! them with carets.  The output rendering itself lives in [`crate::render`]
//! — the same pretty printer also drives the checker diagnostics' messages.

use crate::compile;
use crate::diag::Diag;
pub use crate::render::print_type;
pub use crate::render::print_value;

/// Compile, check, and run `source`; the rendered output value and its type.
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
    module.evaluate_node_deep(build.root_ty, None);
    Ok(format!(
        "{}: {}",
        print_value(&module, value),
        print_type(&module, build.root_ty)
    ))
}
