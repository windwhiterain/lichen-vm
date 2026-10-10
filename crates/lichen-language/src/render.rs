//! Rendering: the output value and the diagnostics share one pretty printer.
//! See docs/notes/raw-rendering-mark.md.
use lichen_highlevel::diagnostic::{AssertSpelling, Diag as CheckerDiag, DiagKind};
use lichen_highlevel::program::{HighProgram, ValueType};
use lichen_lowlevel::{BudgetExhausted, LowValue, Module, NodeId};

use lichen_compute::ComputeValue;
use lichen_utils::extend::AsEnum;

use crate::diag::Diag;

pub use lichen_render::{
    TypePrinter, ValuePrinter, print_type, print_value, render_attributes,
    render_struct_fields_named, struct_type_named_fields, value_label,
};

// --- the extension-vocabulary render hooks ---------------------------------

// The shared printer cannot know the compute values, so these
// hooks spell them by name instead of `?`.

/// The compute plugin's value-variant spelling: `Kernel`/`ParKernel`/`Buffer`.
fn lang_value_render<P>(value: &P::Value) -> Option<String>
where
    P: HighProgram,
    P::Value: AsEnum<ComputeValue>,
{
    match value.as_enum() {
        Some(ComputeValue::Kernel(_)) => Some("Kernel".to_string()),
        // A value's name is its kind, not the backend it is dispatched to.
        Some(ComputeValue::ParKernel(..)) => Some("ParKernel".to_string()),
        Some(ComputeValue::Buffer(..)) => Some("Buffer".to_string()),
        _ => None,
    }
}

/// The `&'static` hook injected into the shared printers for this vocabulary.
pub fn lang_render_ext<P>() -> &'static dyn Fn(&P::Value) -> Option<String>
where
    P: HighProgram + 'static,
    P::Value: AsEnum<ComputeValue> + 'static,
{
    &lang_value_render::<P>
}

/// [`print_type`] with the language's extension vocabulary.
pub fn print_type_lang<P>(module: &Module<P>, root: NodeId) -> String
where
    P: HighProgram + 'static,
    P::Value: ValueType + AsEnum<ComputeValue> + 'static,
{
    TypePrinter::new_with_ext(module, Some(lang_render_ext::<P>())).node(root)
}

/// [`print_value`] with the language's extension vocabulary.
pub fn print_value_lang<P>(module: &Module<P>, value: P::Value, ty: NodeId) -> String
where
    P: HighProgram + 'static,
    P::Value: ValueType + AsEnum<ComputeValue> + 'static,
{
    ValuePrinter::new_with_ext(module, Some(lang_render_ext::<P>())).print(value, ty)
}

// --- the caret shell ---------------------------------------------------------

/// Render a diagnostic with its source line and a caret.
///
/// # Invariant
///
/// A diagnostic with no span prints its message alone, with no position and no
/// caret.
pub fn render<P: lichen_lowlevel::Program>(source: &str, diag: &Diag<P>) -> String {
    let starts = crate::lex::line_starts(source);
    render_with_line_starts(source, &starts, diag)
}

/// [`render`] against an already-computed line model, so a whole report costs
/// one scan of the source.
fn render_with_line_starts<P: lichen_lowlevel::Program>(
    source: &str,
    starts: &[usize],
    diag: &Diag<P>,
) -> String {
    let mut out = format!("error: {}\n", diag.message);
    let Some((line, col)) = diag.span else {
        return out;
    };
    // A foreign position carries that file's path and its own line model, so
    // text and `(line, col)` agree.
    let foreign_starts = diag
        .file
        .as_ref()
        .map(|package| crate::lex::line_starts(&package.code));
    let (text, starts, arrow) = match (diag.file.as_ref(), foreign_starts.as_deref()) {
        (Some(package), Some(starts)) => (
            package.code.as_ref(),
            starts,
            format!("{}:{line}:{col}", package.path.display()),
        ),
        _ => (source, starts, format!("{line}:{col}")),
    };
    out.push_str(&format!("  --> {arrow}\n"));
    out.push_str("   |\n");
    if let Some(text) = crate::lex::line_text(text, starts, line) {
        let caret = format!("{}^", " ".repeat((col as usize).saturating_sub(1)));
        out.push_str(&format!(" {line} | {text}\n"));
        out.push_str(&format!("   | {caret}\n"));
    }
    out
}

/// A **class domain** — a set's value, its member type values — spelled
/// `{Int, Float}`.
///
/// # Invariant
///
/// The domain is read as the member list it is; a node that is not a member
/// list falls back to the type printer.
fn class_domain<P>(printer: &mut TypePrinter<'_, P>, domain: NodeId) -> String
where
    P: HighProgram,
    P::Value: ValueType,
{
    let members = lichen_highlevel::set::members(
        printer.module(),
        lichen_lowlevel::AnyNodeId::Dynamic(domain),
    );
    match members {
        Some(members) => format!(
            "{{{}}}",
            members
                .into_iter()
                .map(|member| printer.any_node(member))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => printer.node(domain),
    }
}

/// Render a whole diagnostic list back to back: one caret block each.
pub fn render_all<P: lichen_lowlevel::Program>(source: &str, diags: &[Diag<P>]) -> String {
    let starts = crate::lex::line_starts(source);
    diags
        .iter()
        .map(|d| render_with_line_starts(source, &starts, d))
        .collect()
}

// --- the pretty checker message ----------------------------------------------

// Re-render the highlevel's facts in the CLI's vocabulary, through one shared
// printer so a class keeps one `?a` name.

/// Re-render a checker diagnostic's message from the highlevel's facts.
///
/// # Invariant
///
/// `printer` is shared across a whole report, so a class keeps one `?a` name.
pub fn checker_message<P>(printer: &mut TypePrinter<'_, P>, d: &CheckerDiag<P>) -> String
where
    P: HighProgram,
    P::Value: ValueType,
{
    match d.kind {
        DiagKind::Annotation
        | DiagKind::Attribute
        | DiagKind::ArrayElement
        | DiagKind::TableKey
        | DiagKind::TableValue
        | DiagKind::Guard => {
            format!(
                "expected {}, found {}",
                printer.node(d.b),
                printer.node(d.a)
            )
        }
        DiagKind::IndexTarget => {
            format!(
                "expected a tuple, array, or struct type, found {}",
                printer.node(d.a)
            )
        }
        DiagKind::RuntimeIndexTarget => {
            "this value is not a container — it has no element to read".to_string()
        }
        DiagKind::RuntimeRawElement => {
            "this raw read found an element that is not a value/type pair — \
             `X<e>` reads the element's own pair, so the container holds pairs \
             (a type value); a runtime array's element is read with `e[i]`"
                .to_string()
        }
        DiagKind::RuntimeIndexSubscript => {
            "this value is not an index — an element can only be read by position".to_string()
        }
        DiagKind::RuntimeApplyTarget => {
            "this value is not a function — it cannot be applied".to_string()
        }
        DiagKind::ImportExport => {
            "this package's export is not a value — a package must end in a value".to_string()
        }
        DiagKind::NamedField => {
            let base = format!(
                "no field with this name in the struct type {}",
                printer.node(d.a)
            );
            // Append a did-you-mean clause naming the struct's actual fields.
            let Some(name) = d.field.as_deref() else {
                return base;
            };
            let Some(fields) = struct_type_named_fields(printer.module(), d.a) else {
                return base;
            };
            match crate::suggest::did_you_mean(name, fields.into_iter().flatten()) {
                Some(clause) => format!("{base}{clause}"),
                None => base,
            }
        }
        DiagKind::StructUnknownField => match &d.field {
            Some(name) => format!(
                "no field named {name} in the struct type {}",
                printer.node(d.a)
            ),
            None => format!("no such field in the struct type {}", printer.node(d.a)),
        },
        DiagKind::StructDuplicateField => match &d.field {
            Some(name) => format!("duplicate field {name} in struct instantiation"),
            None => "duplicate field in struct instantiation".to_string(),
        },
        DiagKind::StructMissingField => match &d.field {
            Some(name) => format!("missing field {name} in struct instantiation"),
            None => "missing a field in struct instantiation".to_string(),
        },
        DiagKind::StructExcessField => "too many fields in struct instantiation".to_string(),
        DiagKind::StructAnonymousField => {
            "cannot name a field — the struct has no named fields".to_string()
        }
        DiagKind::StructFieldName => {
            "a struct field must be named — write `struct<.name T>`".to_string()
        }
        DiagKind::InstantiateCallee => format!(
            "the callee of an instantiation must be a struct type, found {}",
            printer.node(d.a)
        ),
        // The expected side is the class the operation computes over, read from
        // the report rather than spelled here.
        DiagKind::BinOp | DiagKind::Conv => format!(
            "expected {}, found {}",
            printer.node(d.b),
            printer.node(d.a)
        ),
        // A runtime apply-time failure: the parameter is the expected side
        // (a), the argument the found side (b).
        DiagKind::Runtime => format!(
            "expected {}, found {}",
            printer.node(d.a),
            printer.node(d.b)
        ),
        DiagKind::IndexOutOfBounds => {
            let (Some(index), Some(length)) = (d.index, d.length) else {
                return "index out of bounds".to_string();
            };
            format!("index {index} out of bounds (array length {length})")
        }
        DiagKind::TableMiss => "table lookup missed — no entry for this key".to_string(),
        // The report invariant's last resort: the build failed with nothing to
        // attribute it to.
        DiagKind::UnattributedFailure => {
            "the build failed, but the failing check could not be attributed to an expression in this source".to_string()
        }
        DiagKind::TableKeyUndecided => {
            "table key is not concrete (it is undecided or a failed read) — the entry is dropped"
                .to_string()
        }
        DiagKind::NativeOpUnresolved => match d.field.as_deref() {
            Some(name) => format!(
                "unresolved native operator '{name}' — this module composes no plugin registering it"
            ),
            None => "unresolved native operator — this module composes no plugin registering it"
                .to_string(),
        },
        DiagKind::NativeOpContract => match d.field.as_deref() {
            Some(name) => format!(
                "native operator '{name}' returned a malformed term — a native operator must return the [value, type] pair it built in the current block"
            ),
            None => "a native operator returned a malformed term — a native operator must return the [value, type] pair it built in the current block"
                .to_string(),
        },
        DiagKind::NoAttributeExtension => {
            "this expression carries an attribute, but this build has no attribute extension to lower it"
                .to_string()
        }
        DiagKind::Assert => match d.assert_spelling {
            // A refinement: the value is outside the admitted class set.
            // See docs/notes/operator-polymorphism.md §8.5.
            Some(AssertSpelling::Refinement { domain }) => {
                format!("does not satisfy {}", class_domain(printer, domain))
            }
            // The deferred half of the container-kind requirement, in the read's
            // own vocabulary.
            Some(AssertSpelling::StructKind { container }) => format!(
                "expected a struct type, found {}",
                printer.node(container)
            ),
            // An explicit `@assert e`: the condition's own failed value.
            _ => {
                let value = match d.assert_value.as_ref().and_then(|v| v.as_enum()) {
                    Some(LowValue::USize(n)) => n.to_string(),
                    Some(LowValue::None | LowValue::Error) => "none".to_string(),
                    Some(other) => format!("{other:?}"),
                    None => "—".to_string(),
                };
                format!("assertion failed: expected 1, found {value}")
            }
        },
        DiagKind::NonTerminating => match d.budget {
            // The recorded guard and its limit name the runaway shape exactly.
            Some(BudgetExhausted::ApplyDepth { limit }) => format!(
                "this binding never terminates — nested applications exceeded {limit} levels (non-terminating recursion)"
            ),
            Some(BudgetExhausted::ApplyTotal { limit }) => format!(
                "this binding never terminates — it applied a function more than {limit} times (non-terminating recursion)"
            ),
            Some(BudgetExhausted::EvaluateDepth { limit }) => format!(
                "this binding never terminates — its value grows deeper than {limit} levels (non-terminating evaluation)"
            ),
            None => "this binding never terminates (non-terminating recursion)".to_string(),
        },
        DiagKind::LoopNotRecorded => {
            // The wording names what the program said and what is missing, so a
            // refusal is not read as a miscompile.
            match &d.field {
                Some(rule) => format!(
                    "this call is a recursion of a `@loop`-marked binding whose trip \
                     count is not decided before the body is lowered, and whose shape \
                     does not convert to a loop — it is {rule} — so the call is refused \
                     rather than expanded"
                ),
                None => "this call is a recursion of a `@loop`-marked binding whose trip \
                         count is not decided before the body is lowered — no loop has \
                         been recorded for it, so the call is refused rather than expanded"
                    .to_string(),
            }
        }
        DiagKind::LoopNotEmitted => {
            // The shape is a loop but no backend consumes it yet: the compiler
            // cannot do this yet, not the program being wrong.
            "this call is a recursion of a `@loop`-marked binding whose trip count is \
             not decided before the body is lowered; its shape converts to a loop, but \
             no backend emits one yet, so the call is refused rather than expanded"
                .to_string()
        }
    }
}

#[cfg(test)]
#[path = "tests/render_tests.rs"]
mod tests;
