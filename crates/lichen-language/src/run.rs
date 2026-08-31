//! Running a source program to its output value.
//!
//! [`evaluate`] compiles and checks a program, runs it (the deep evaluation
//! of its root value), and renders the result as text — the program's
//! output, with its type: `5: Int`, `[1, 2, 3]: Int<3>`.  The value renders
//! *against its type chain* (a struct type value prints `struct<Int, Type>`,
//! a tuple `(1, Int)`) — see [`crate::render`].  Diagnostics are returned
//! unrendered so the caller (the CLI, the example tests) can render them
//! with carets.  The output rendering itself lives in [`crate::render`] —
//! the same pretty printer also drives the checker diagnostics' messages.

use std::path::Path;

use crate::compile;
use crate::diag::Diag;
use crate::package::PackageStore;
use crate::preprocess::preprocess;
pub use crate::render::print_type;
pub use crate::render::print_value;

/// Compile, check, and run `source`; the rendered output value and its type.
///
/// On failure the diagnostics (frontend and checker) are returned.  A
/// terminating program evaluates to its value; a non-terminating one (a
/// recursive function whose recursion never reaches a base case) panics at
/// the VM's recursion-depth guard — that is the designed behavior of the
/// core (an upper limit on nested applications), not a diagnostic.
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
        print_value(&module, value, build.root_ty),
        print_type(&module, build.root_ty)
    ))
}

/// Compile, check, and run a raw source file after preprocessing imports.
/// The package store is caller-owned so multiple files can share a registry.
pub fn evaluate_raw(
    source: &str,
    base: Option<&Path>,
    store: &mut PackageStore,
) -> Result<String, Vec<Diag>> {
    let (preprocessed, diags) = preprocess(source, base, store);
    if !diags.is_empty() {
        return Err(diags);
    }
    let report = crate::compile_with_imports_in(
        &preprocessed.source,
        &preprocessed.imports,
        Some(store.registry()),
    );
    if !report.diagnostics.is_empty() {
        return Err(report.diagnostics);
    }
    let build = report.build.unwrap();
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    module.evaluate_node_deep(build.root_ty, None);
    Ok(format!(
        "{}: {}",
        print_value(&module, value, build.root_ty),
        print_type(&module, build.root_ty)
    ))
}
