//! Running a source program to its output value.
//!
//! [`evaluate`] compiles and checks a program, runs it (the deep evaluation
//! of its root value), and renders the result as text — the program's
//! output, with its type: `5: Int`, `[1, 2, 3]: array<Int, 3>`.  The value renders
//! *against its type chain* (a struct type value prints `struct<.f Int, .g Type>`,
//! a tuple `(1, Int)`) — see [`crate::render`].  Diagnostics are returned
//! unrendered so the caller (the CLI, the example tests) can render them
//! with carets.  The output rendering itself lives in [`crate::render`] —
//! the same pretty printer also drives the checker diagnostics' messages.

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

/// Run a checked build to its output text: deep-evaluate the root value and
/// type, then render `value<attributes>: type`.  The one place the output line
/// is formed, so [`evaluate`] and [`evaluate_raw`] — which differ only in how
/// they obtain the build — cannot drift.
///
/// **A refusal is an explanation, so it is reported only when there is
/// something to explain.**  The deep evaluation is where an operator actually
/// executes, so it is where a plugin's *runtime* refusal lands (a `plrun` whose
/// element count is past the limit, say — see `P1-30`), and the value that comes
/// back is the lazy marker: returning that as the output would print
/// `parameterized: Int` and say nothing about why.  But the same channel also
/// carries a **provisional** refusal — the checker evaluates speculatively, so a
/// `$jit` whose parameter domain is not decided *yet* records one, and a later
/// attempt with the domain known compiles the very same kernel.  That program
/// works and has a value.  The line between them is the outcome, not the
/// channel: a refusal explains a value that never arrived, and a program that
/// produced one has nothing to explain.
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
        // The refusing layer's own text, rendered by the host that owns the
        // message channel — see `docs/notes/compiler-plugin.md`.  No span: the
        // entry names a lowlevel node, not an IR expression.
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
    // A value one of whose attributes **names** it reads as that name: the
    // override is the general one (`docs/notes/operator-polymorphism.md` §8.1),
    // not a rule about a particular attribute, and it is what lets a value with
    // no spelling of its own — a refinement's predicate, a function — be shown
    // at all.
    if let Some(label) = value_label::<P>(&module, build.root_term, tail, &*attr_ext) {
        return Ok(format!(
            "{label}: {}",
            print_type_lang::<P>(&module, build.root_ty)
        ));
    }
    // An **undecided** root — an empty slot — has no value of its own, so it
    // renders as the printer's no-value reading, exactly as a value-less
    // element already does.
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
/// On failure the diagnostics (frontend and checker) are returned.  A
/// terminating program evaluates to its value.  A non-terminating one (a
/// recursive function whose recursion never reaches a base case) exhausts a
/// VM budget, which is a recorded failure rather than an abort: the guard
/// latches [`BudgetExhausted`] instead of unwinding, and the checker turns it
/// into a `NonTerminating` diagnostic naming the budget and its limit.
pub fn evaluate(source: &str) -> Result<String, Vec<Diag<LangProgram>>> {
    let report = compile(source);
    if !report.ok() {
        return Err(report.diagnostics);
    }
    render_build(report.build.unwrap())
}

/// Compile, check, and run a raw source file after preprocessing imports.
/// The package store is caller-owned so multiple files can share a registry.
/// Generic over a single program type `P` (the associated-type collector),
/// so a plugin-built compiler runs through the same path.
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
