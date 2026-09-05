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
use crate::persist::ArtifactCodec;
use crate::preprocess::preprocess;
use crate::program::{GcdOp, LangProgram};
pub use crate::render::print_type;
pub use crate::render::print_value;
use crate::render::{print_type_lang, print_value_lang, render_attributes};
use crate::{CompiledProgram, lang_attr_ext};

use lichen_highlevel::program::ValueType;
use lichen_lowlevel::OperatorExt;
use lichen_utils::extend::AsEnum;

/// Compile, check, and run `source`; the rendered output value and its type.
///
/// On failure the diagnostics (frontend and checker) are returned.  A
/// terminating program evaluates to its value; a non-terminating one (a
/// recursive function whose recursion never reaches a base case) panics at
/// the VM's recursion-depth guard — that is the designed behavior of the
/// core (an upper limit on nested applications), not a diagnostic.
pub fn evaluate(source: &str) -> Result<String, Vec<Diag<LangProgram>>> {
    let report = compile(source);
    if !report.diagnostics.is_empty() {
        return Err(report.diagnostics);
    }
    let build = report.build.unwrap();
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    module.evaluate_node_deep(build.root_ty, None);
    Ok(format!(
        "{}{}: {}",
        print_value_lang::<LangProgram>(&module, value, build.root_ty),
        {
            // Render only the attributes the root expression actually carries.
            let attr_ext = lang_attr_ext::<LangProgram>();
            let tail = &build.ir.schema(build.ir.root).tail;
            let attrs = render_attributes(&module, build.root_term, tail, &*attr_ext);
            if attrs.is_empty() {
                String::new()
            } else {
                format!(" {attrs}")
            }
        },
        print_type_lang::<LangProgram>(&module, build.root_ty)
    ))
}

/// Compile, check, and run a raw source file after preprocessing imports.
/// The package store is caller-owned so multiple files can share a registry.
/// Generic over the value/operator vocabularies `V`/`O` and the artifact codec
/// `C`, so a plugin-built compiler runs through the same path.
pub fn evaluate_raw<V, O, C>(
    source: &str,
    base: Option<&Path>,
    store: &mut PackageStore<V, O, C>,
) -> Result<String, Vec<Diag<CompiledProgram<V, O>>>>
where
    V: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<lichen_lowlevel::LowOperator>
        + From<lichen_lowlevel::LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq
        + From<GcdOp>
        + From<lichen_highlevel::program::TypeOperator>
        + From<lichen_compute::ComputeOperator>
        + 'static,
    C: ArtifactCodec<CompiledProgram<V, O>> + Default,
{
    let (preprocessed, diags) = preprocess(source, base, store);
    if !diags.is_empty() {
        return Err(diags);
    }
    let line_starts = crate::lex::line_starts(source);
    let report = crate::compile_with_imports_at::<V, O>(
        &preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    if !report.diagnostics.is_empty() {
        return Err(report.diagnostics);
    }
    let build = report.build.unwrap();
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    module.evaluate_node_deep(build.root_ty, None);
    Ok(format!(
        "{}{}: {}",
        print_value_lang::<CompiledProgram<V, O>>(&module, value, build.root_ty),
        {
            // Render only the attributes the root expression actually carries.
            let attr_ext = lang_attr_ext::<CompiledProgram<V, O>>();
            let tail = &build.ir.schema(build.ir.root).tail;
            let attrs = render_attributes(&module, build.root_term, tail, &*attr_ext);
            if attrs.is_empty() {
                String::new()
            } else {
                format!(" {attrs}")
            }
        },
        print_type_lang::<CompiledProgram<V, O>>(&module, build.root_ty)
    ))
}
