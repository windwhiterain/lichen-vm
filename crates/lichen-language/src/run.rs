//! Running a source program to its rendered output value.
//!
//! # Invariant
//!
//! The output line is formed in one place, so [`evaluate`] and [`evaluate_raw`]
//! cannot drift; diagnostics are returned unrendered.

use std::path::Path;

use crate::LangProgramShape;
use crate::compile;
use crate::diag::Diag;
use crate::lang_attr_ext;
use crate::package::PackageStore;
use crate::preprocess::preprocess;
use crate::program::{GcdOp, LangProgram};
pub use crate::render::print_type;
pub use crate::render::print_value;
use crate::render::{print_type_lang, print_value_lang, render_attributes, value_label};

use lichen_highlevel::checker::Build;
use lichen_highlevel::program::ValueType;
use lichen_utils::extend::AsEnum;

/// Run a checked build to its output text: `value<attributes>: type`.
///
/// # Invariant
///
/// A runtime refusal explains a value that never arrived, so it is reported
/// only when evaluation produced nothing; a provisional refusal on a program
/// that produced a value is not reported. See docs/notes/language-toolchain.md.
fn render_build<P>(build: Build<P>) -> Result<String, Vec<Diag<P>>>
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp>
        + From<lichen_highlevel::program::TypeOperator>
        + From<lichen_compute::ComputeOperator>
        + 'static,
{
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    let _ = module.evaluate_node_deep(build.root_ty, None);
    // An undecided root is `None` — the empty slot — and a budget-refused one
    // is the computed-nothing value.
    let produced_nothing = match value {
        None => true,
        Some(value) => matches!(
            AsEnum::<lichen_lowlevel::LowValue>::as_enum(&value),
            Some(lichen_lowlevel::LowValue::Error)
        ),
    };
    if produced_nothing && !module.extension_diagnostics.is_empty() {
        // The refusing layer's own text; no span. See docs/notes/compiler-plugin.md.
        return Err(module
            .extension_diagnostics
            .iter()
            .map(|entry| {
                Diag::unattributed(
                    crate::diag::Stage::Check,
                    format!("{}: {}", entry.category, entry.message),
                )
            })
            .collect());
    }
    // Render only the attributes the root expression actually carries.
    let attr_ext = lang_attr_ext::<P>();
    let tail = &build.root_schema_tail;
    // A value one of whose attributes names it reads as that name.
    // See docs/notes/operator-polymorphism.md §8.1.
    if let Some(label) = value_label::<P>(&module, build.root_term, tail, &*attr_ext) {
        return Ok(format!(
            "{label}: {}",
            print_type_lang::<P>(&module, build.root_ty)
        ));
    }
    // An undecided root renders as the printer's no-value reading.
    let value = value.unwrap_or_else(|| P::Value::from(lichen_lowlevel::LowValue::None));
    Ok(format!(
        "{}{}: {}",
        print_value_lang::<P>(&module, value, build.root_ty),
        {
            let attrs = render_attributes(&module, build.root_term, tail, &*attr_ext);
            if attrs.is_empty() {
                String::new()
            } else {
                format!(" {attrs}")
            }
        },
        print_type_lang::<P>(&module, build.root_ty)
    ))
}

/// Compile, check, and run `source`; the rendered output value and its type.
///
/// # Invariant
///
/// A non-terminating program exhausts a VM budget, which is recorded as a
/// [`BudgetExhausted`] failure and turned into a `NonTerminating` diagnostic
/// rather than an abort.
pub fn evaluate(source: &str) -> Result<String, Vec<Diag<LangProgram>>> {
    let report = compile(source);
    if !report.ok() {
        return Err(report.diagnostics);
    }
    render_build(report.build.unwrap())
}

/// Compile, check, and run a preprocessed source file; the store is the caller's.
pub fn evaluate_raw<P>(
    source: &str,
    base: Option<&Path>,
    store: &mut PackageStore<P>,
) -> Result<String, Vec<Diag<P>>>
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp>
        + From<lichen_highlevel::program::TypeOperator>
        + From<lichen_compute::ComputeOperator>
        + 'static,
{
    let (preprocessed, diags) = preprocess(source, base, store);
    if !diags.is_empty() {
        return Err(diags);
    }
    let line_starts = crate::lex::line_starts(source);
    let report = crate::compile_with_imports_at::<P>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    if !report.ok() {
        return Err(report.diagnostics);
    }
    render_build(report.build.unwrap())
}
