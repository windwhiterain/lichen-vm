//! Rendering: the program's output and the diagnostics share one printer.
//!
//! The printer core — [`TypePrinter`], [`ValuePrinter`], and the free
//! [`print_value`] / [`print_type`] / [`render_attributes`] /
//! [`render_struct_fields_named`] — is program-generic and lives in
//! [`lichen_render`]; this module re-exports it and layers the host-specific
//! shells on top:
//!
//! - [`checker_message`] re-renders the highlevel's raw facts in the CLI's
//!   vocabulary (the wording per [`DiagKind`]) with the shared [`TypePrinter`],
//!   and
//! - the caret shell [`render`]/[`render_all`] wraps either message with its
//!   source line and a caret.
//!
//! ```text
//! error: unresolved name 'y'
//!   --> 1:6
//!    |
//!  1 | x => y
//!    |      ^
//! ```
//!

use lichen_highlevel::diagnostic::{Diag as CheckerDiag, DiagKind};
use lichen_lowlevel::LowValue;

use crate::diag::Diag;
use crate::program::{LangProgram, LangValue};

pub use lichen_render::{
    TypePrinter, ValuePrinter, print_type, print_value, render_attributes,
    render_struct_fields_named,
};

// --- the caret shell ---------------------------------------------------------

/// Render a diagnostic with its source line and a caret.
///
/// ```text
/// error: unresolved name 'y'
///   --> 1:6
///    |
///  1 | x => y
///    |      ^
/// ```
pub fn render(source: &str, diag: &Diag) -> String {
    let mut out = format!("error: {}\n", diag.message);
    if let Some((line, col)) = diag.span {
        out.push_str(&format!("  --> {line}:{col}\n"));
        out.push_str("   |\n");
        if let Some(text) = source.lines().nth((line as usize).saturating_sub(1)) {
            let caret = format!("{}^", " ".repeat((col as usize).saturating_sub(1)));
            out.push_str(&format!(" {line} | {text}\n"));
            out.push_str(&format!("   | {caret}\n"));
        }
    }
    out
}

/// Render a whole diagnostic list back to back, exactly as the CLI prints
/// them: one caret block per diagnostic, no separator.
pub fn render_all(source: &str, diags: &[Diag]) -> String {
    diags.iter().map(|d| render(source, d)).collect()
}

// --- the pretty checker message ----------------------------------------------
// Re-renders the highlevel's raw facts in the CLI's vocabulary: the wording
// per kind.  One TypePrinter drives the whole message (and a whole report),
// so a class keeps a single `?a` name; it must carry the checker's arrow
// registry.  The `?a` journey is gone — every expression's type is queryable,
// so the user inspects an expr's type instead of reading a source trace.

/// Re-render a checker diagnostic's message with the shared pretty printer,
/// from the highlevel's structured facts, in the language's own type syntax.
/// `printer` is shared across a whole report, so a class keeps a single `?a`
/// name across diagnostics.
pub fn checker_message(
    printer: &mut TypePrinter<'_, LangProgram>,
    d: &CheckerDiag<LangProgram>,
) -> String {
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
        DiagKind::NamedField => format!(
            "no field with this name in the struct type {}",
            printer.node(d.a)
        ),
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
        DiagKind::BinOp => format!("expected Int, found {}", printer.node(d.a)),
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
        DiagKind::TableKeyUnbound => {
            "table key is not concrete (it depends on an unbound value) — the entry is dropped"
                .to_string()
        }
        DiagKind::Assert => {
            let value = match d.assert_value {
                Some(LangValue::LowValue(LowValue::USize(n))) => n.to_string(),
                Some(LangValue::LowValue(LowValue::None)) => "none".to_string(),
                Some(other) => format!("{other:?}"),
                None => "—".to_string(),
            };
            format!("assertion failed: expected 1, found {value}")
        }
        DiagKind::NonTerminating => {
            "this binding never terminates (non-terminating recursion)".to_string()
        }
    }
}

#[cfg(test)]
#[path = "tests/render_tests.rs"]
mod tests;
