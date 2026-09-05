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
use lichen_lowlevel::{LowValue, Module, NodeId};

use lichen_compute::ComputeValue;

use crate::diag::Diag;
use crate::program::{LangProgram, LangValue};

pub use lichen_render::{
    TypePrinter, ValuePrinter, print_type, print_value, render_attributes,
    render_struct_fields_named,
};

// --- the extension-vocabulary render hooks ---------------------------------
//
// The shared `lichen_render` printer is generic over the value vocabulary, so
// it cannot know the compute plugin's own value variants.  The base renderer
// spells an unknown extension value `?`; these hooks spell the compute leaves a
// kernel carries so the editor/CLI renders them by name.

/// The compute plugin's value-variant spelling: a `Kernel` value and the
/// `TypeKernel` kind marker both read as `Kernel` — the marker's are internal
/// (the kernel's *signature* is what the type's arrow carries), and the value
/// is an opaque compiled artifact, so one name suffices.  The parallel-kernel
/// and buffer leaves spell by name too (`ParKernel`, `Buffer`).
fn lang_value_render(value: &LangValue) -> Option<String> {
    match value {
        LangValue::ComputeValue(ComputeValue::Kernel(_))
        | LangValue::ComputeValue(ComputeValue::TypeKernel) => Some("Kernel".to_string()),
        LangValue::ComputeValue(ComputeValue::ParKernel(_))
        | LangValue::ComputeValue(ComputeValue::TypeParKernel) => Some("ParKernel".to_string()),
        LangValue::ComputeValue(ComputeValue::Buffer(_))
        | LangValue::ComputeValue(ComputeValue::TypeBuffer) => Some("Buffer".to_string()),
        _ => None,
    }
}

/// The `&'static` extension-vocabulary render hook, injected into the shared
/// printers so a kernel's value/type spell correctly instead of degrading to `?`.
pub fn lang_render_ext() -> &'static dyn Fn(&LangValue) -> Option<String> {
    &lang_value_render
}

/// [`print_type`] with the language's extension vocabulary: a kernel type
/// (`[sig, [TypeKernel, K]]`, which mirrors a function type) renders as
/// `in -> out` and its kind marker as `Kernel`.
pub fn print_type_lang(module: &Module<LangProgram>, root: NodeId) -> String {
    TypePrinter::new_with_ext(module, Some(lang_render_ext())).node(root)
}

/// [`print_value`] with the language's extension vocabulary: a `Kernel` value
/// renders as `Kernel` instead of the raw-layout `?`.
pub fn print_value_lang(module: &Module<LangProgram>, value: LangValue, ty: NodeId) -> String {
    ValuePrinter::new_with_ext(module, Some(lang_render_ext())).print(value, ty)
}

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
