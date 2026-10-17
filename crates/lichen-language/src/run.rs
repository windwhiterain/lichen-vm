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
use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_lowlevel::Module;

/// Whether a run says what the passes it dispatched did.
///
/// # Invariant
/// **The caller names the pass channel**: an invocation that names none
/// collects nothing and prints the run it printed before passes existed,
/// while one that names the categories its passes write under reports those
/// lines verbatim. What a run reports is decided by the invocation alone,
/// never by what the program turned out to be, and by no vocabulary it was
/// not composed over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PassReport<'a> {
    /// The invocation named no pass: the program's own output is the whole of what the run says.
    #[default]
    Silent,
    /// The invocation named its pass channel: those stage lines are reported beside that output.
    Reported(&'a [&'a str]),
}

/// A run that produced a value: the program's output line, and the stage lines reported with it.
///
/// # Invariant
/// The report is **carried, not written**: a host decides which stream carries it, so the value and
/// what the passes did with it can never reach one stream by accident.
pub struct RunOutput {
    /// The rendered output value, `value<attributes>: type`.
    pub output: String,
    /// The passes' stage lines, verbatim and in the order they were recorded; empty unless the run
    /// reported.
    pub pass_lines: Vec<String>,
    /// Why an undecided root has no value (`category: message` lines); empty when it answered.
    pub refusals: Vec<String>,
}

/// The stage lines a run's passes recorded, verbatim, under the named categories.
///
/// # Invariant
/// **Only the named categories are read**: a refusal is not a stage line, and
/// [`render_build`] reports one against the run rather than beside the value. A line
/// is self-describing (`region N: …`, `node N: …`, `pack "…" fired`), so it is
/// carried as the pass wrote it.
fn pass_lines<P: lichen_lowlevel::Program>(module: &Module<P>, categories: &[&str]) -> Vec<String> {
    module
        .extension_diagnostics
        .iter()
        .filter(|entry| categories.contains(&entry.category))
        .map(|entry| entry.message.clone())
        .collect()
}

/// Run a checked build to its output text: `value<attributes>: type`, and the
/// stage lines when the run reports.
///
/// # Invariant
///
/// Every runtime refusal is reported, whatever the program's answer: a value that
/// arrived does not make a recorded refusal go away. See
/// docs/notes/language-toolchain.md.
///
/// A stage line is **not a refusal**: the run reports it beside the value, so
/// naming a pack still says whether it fired.
fn render_build<P>(build: Build<P>, pass_report: PassReport<'_>) -> Result<RunOutput, Vec<Diag<P>>>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    let _ = module.evaluate_node_deep(build.root_ty, None);
    // An undecided root's reasons, so `none` is never the whole report.
    let refusals = if value.is_none() {
        module
            .extension_diagnostics
            .iter()
            .filter(|entry| lichen_highlevel::program::REFUSAL_CATEGORIES.contains(&entry.category))
            .map(|entry| format!("{}: {}", entry.category, entry.message))
            .collect()
    } else {
        Vec::new()
    };
    if module.has_extension_failures() {
        // The refusing layer's own text; no span. See docs/notes/compiler-plugin.md.
        return Err(module
            .extension_diagnostics
            .iter()
            .filter(|entry| entry.fails_build)
            .map(|entry| {
                Diag::unattributed(
                    crate::diag::Stage::Check,
                    format!("{}: {}", entry.category, entry.message),
                )
            })
            .collect());
    }
    // Read after the root is evaluated, because the consultation happens in there.
    let pass_lines = match pass_report {
        PassReport::Silent => Vec::new(),
        PassReport::Reported(categories) => pass_lines(&module, categories),
    };
    // Render only the attributes the root expression actually carries.
    let attr_ext = lang_attr_ext::<P>();
    let tail = &build.root_schema_tail;
    // A value one of whose attributes names it reads as that name.
    // See docs/notes/operator-polymorphism.md §8.1.
    let output = if let Some(label) = value_label::<P>(&module, build.root_term, tail, &*attr_ext) {
        format!("{label}: {}", print_type_lang::<P>(&module, build.root_ty))
    } else {
        // An undecided root renders as the printer's no-value reading.
        let value = value.unwrap_or_else(|| P::Value::from(lichen_lowlevel::LowValue::None));
        format!(
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
        )
    };
    Ok(RunOutput {
        output,
        pass_lines,
        refusals,
    })
}

/// Compile, check, and run `source`; the rendered output value and its type.
///
/// # Invariant
///
/// A non-terminating program exhausts a VM budget, which is recorded as a
/// [`BudgetExhausted`] failure and turned into a `NonTerminating` diagnostic
/// rather than an abort.
pub fn evaluate(source: &str) -> Result<String, Vec<Diag<LangProgram>>> {
    evaluate_reporting(source).map(|run| run.output)
}

/// [`evaluate`], with the run's report carried beside its value.
pub fn evaluate_reporting(source: &str) -> Result<RunOutput, Vec<Diag<LangProgram>>> {
    let report = compile(source);
    if !report.ok() {
        return Err(report.diagnostics);
    }
    render_build(report.build.unwrap(), PassReport::Silent)
}

/// Compile, check, and run a preprocessed source file; the store is the caller's.
pub fn evaluate_raw<P>(
    source: &str,
    base: Option<&Path>,
    store: &mut PackageStore<P>,
) -> Result<String, Vec<Diag<P>>>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    evaluate_raw_reporting(source, base, store, PassReport::Silent).map(|run| run.output)
}

/// [`Self::evaluate_raw`], with the run's pass report carried beside its value.
///
/// # Invariant
/// One run, one implementation: this is [`Self::evaluate_raw`]'s body, and `evaluate_raw` is this
/// call with the report dropped, so the two cannot say a run did different work.
pub fn evaluate_raw_reporting<P>(
    source: &str,
    base: Option<&Path>,
    store: &mut PackageStore<P>,
    pass_report: PassReport<'_>,
) -> Result<RunOutput, Vec<Diag<P>>>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
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
    render_build(report.build.unwrap(), pass_report)
}
