//! Shifting the spans of a statement the splice cloned instead of re-parsing.
//!
//! The session's window splice re-parses only the statements an edit touched and
//! **clones** the untouched ones around it (see `session::splice_program`).  A
//! clone's bytes are unchanged, but its *position* is not: an edit that adds or
//! removes a line moves every statement after it, and a `Span` is a `(line, col)`
//! pair, not a byte offset.  So a cloned statement after the edit carries a stale
//! span — a diagnostic in it renders on the wrong line — and this is where the
//! shift is applied.
//!
//! The shift is exact rather than approximate: `offset_of_span` locates the span's
//! byte in the **old** source, the edit's byte delta moves it, and `line_col`
//! renders it back into the **new** one.  A `\r\n` file needs no special case —
//! both conversions use the same line model.
//!
//! Only a cloned statement needs this.  A *prefix* statement sits before the edit,
//! so its bytes and its position are both unchanged; a *window* statement was
//! re-parsed from the new token stream, so its spans are already the new ones.

use lichen_language_lex::{Span, line_col, offset_of_span};
use stacksafe::stacksafe;

use crate::ast::{Expr, RecordField, Stmt};

/// Shift every span in `expr` by the edit's byte `delta`.
///
/// `old_starts`/`new_starts` are the line starts of the source the statement was
/// parsed from and of the source it now sits in.
pub(crate) fn shift_expr(
    expr: &mut Expr,
    old_starts: &[usize],
    new_starts: &[usize],
    delta: isize,
) {
    match expr {
        Expr::Int(_, span)
        | Expr::Float(_, span)
        | Expr::Str(_, span)
        | Expr::TypeConst(_, span)
        | Expr::Name(_, span, _)
        | Expr::Placeholder(span) => shift(span, old_starts, new_starts, delta),
        // A recovered-error region: the mask is a **byte** range (what a diff
        // excludes) and the start is a span, so both move.
        Expr::Err { range, start } => {
            range.0 = moved(range.0, delta);
            range.1 = moved(range.1, delta);
            shift(start, old_starts, new_starts, delta);
        }
        Expr::Lambda {
            parameter_span,
            parameter_type,
            parameter_perspective,
            r#return,
            span,
            ..
        } => {
            shift(parameter_span, old_starts, new_starts, delta);
            shift(span, old_starts, new_starts, delta);
            for optional in [parameter_type, parameter_perspective] {
                if let Some(inner) = optional {
                    shift_expr(inner, old_starts, new_starts, delta);
                }
            }
            shift_expr(r#return, old_starts, new_starts, delta);
        }
        Expr::Apply {
            function,
            argument,
            span,
        } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(function, old_starts, new_starts, delta);
            shift_expr(argument, old_starts, new_starts, delta);
        }
        Expr::BinOp {
            left, right, span, ..
        } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(left, old_starts, new_starts, delta);
            shift_expr(right, old_starts, new_starts, delta);
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
            span,
        } => {
            shift(span, old_starts, new_starts, delta);
            for branch in [condition, then_branch, else_branch] {
                shift_expr(branch, old_starts, new_starts, delta);
            }
        }
        Expr::Assert { value, span } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(value, old_starts, new_starts, delta);
        }
        Expr::Convert { value, span, .. } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(value, old_starts, new_starts, delta);
        }
        Expr::NativeCall { args, span, .. } => {
            shift(span, old_starts, new_starts, delta);
            for arg in args {
                shift_expr(arg, old_starts, new_starts, delta);
            }
        }
        Expr::Index { array, index, span }
        | Expr::RawIndex {
            container: array,
            index,
            span,
        }
        | Expr::FieldRead {
            container: array,
            key: index,
            span,
        }
        | Expr::TableFind {
            container: array,
            key: index,
            span,
        } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(array, old_starts, new_starts, delta);
            shift_expr(index, old_starts, new_starts, delta);
        }
        Expr::NamedFieldRead {
            container, span, ..
        }
        | Expr::RawNamedField {
            container, span, ..
        } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(container, old_starts, new_starts, delta);
        }
        Expr::Annotation {
            value,
            r#type,
            perspective,
            doc,
            refinement,
            span,
        } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(value, old_starts, new_starts, delta);
            for attribute in [r#type, perspective, doc, refinement].into_iter().flatten() {
                shift_expr(attribute, old_starts, new_starts, delta);
            }
        }
        Expr::Arrow {
            parameter,
            r#return,
            span,
        } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(parameter, old_starts, new_starts, delta);
            shift_expr(r#return, old_starts, new_starts, delta);
        }
        Expr::Tuple(elements, span)
        | Expr::TypeTuple(elements, span)
        | Expr::Array(elements, span) => {
            shift(span, old_starts, new_starts, delta);
            for element in elements {
                shift_expr(element, old_starts, new_starts, delta);
            }
        }
        Expr::StructType(fields, span) => {
            shift(span, old_starts, new_starts, delta);
            for field in fields {
                shift_expr(&mut field.ty, old_starts, new_starts, delta);
            }
        }
        Expr::StructInst {
            callee,
            fields,
            span,
        } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(callee, old_starts, new_starts, delta);
            for field in fields {
                shift_expr(&mut field.value, old_starts, new_starts, delta);
            }
        }
        Expr::Table(entries, span) => {
            shift(span, old_starts, new_starts, delta);
            for (key, value) in entries {
                shift_expr(key, old_starts, new_starts, delta);
                shift_expr(value, old_starts, new_starts, delta);
            }
        }
        Expr::Shallow(inner, _, span) => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(inner, old_starts, new_starts, delta);
        }
        Expr::TypeArray {
            element_type,
            length,
            span,
        } => {
            shift(span, old_starts, new_starts, delta);
            shift_expr(element_type, old_starts, new_starts, delta);
            shift_expr(length, old_starts, new_starts, delta);
        }
        Expr::Block {
            statements,
            expr: body,
            span,
        } => {
            shift(span, old_starts, new_starts, delta);
            for statement in statements {
                shift_stmt(statement, old_starts, new_starts, delta);
            }
            shift_expr(body, old_starts, new_starts, delta);
        }
        Expr::RecordBlock { fields, span } => {
            shift(span, old_starts, new_starts, delta);
            for field in fields {
                shift_field(field, old_starts, new_starts, delta);
            }
        }
    }
}

/// [`expr`] for one statement.
pub(crate) fn shift_stmt(
    statement: &mut Stmt,
    old_starts: &[usize],
    new_starts: &[usize],
    delta: isize,
) {
    match statement {
        Stmt::Binding(binding) => {
            shift(&mut binding.span, old_starts, new_starts, delta);
            shift_expr(&mut binding.value, old_starts, new_starts, delta);
        }
        Stmt::Expr(inner) => shift_expr(inner, old_starts, new_starts, delta),
    }
}

/// [`expr`] for one record field.
pub(crate) fn shift_field(
    field: &mut RecordField,
    old_starts: &[usize],
    new_starts: &[usize],
    delta: isize,
) {
    shift(&mut field.span, old_starts, new_starts, delta);
    shift_expr(&mut field.value, old_starts, new_starts, delta);
}

/// Move one span.
#[stacksafe]
fn shift(span: &mut Span, old_starts: &[usize], new_starts: &[usize], delta: isize) {
    let byte = offset_of_span(old_starts, *span) as isize + delta;
    *span = line_col(new_starts, moved_from(byte));
}

/// Move one byte offset.
fn moved(byte: u32, delta: isize) -> u32 {
    moved_from(byte as isize + delta)
}

/// Move a byte offset through an edit that replaced `[a, b_old)` of the old
/// source with new text, shifting by `delta = new.len() - old.len()`.
///
/// `None` when the offset is **inside** the replaced region: those bytes are
/// gone, so no position in the new source is the one it named, and a caller that
/// needs a position there (a rendered diagnostic pointing into the text an edit
/// rewrote) must re-derive it rather than be handed a plausible-looking one.
/// This is what makes a *reuse* of a retained build safe when an edit moved text
/// without changing the resolved structure (`session::Cache`).
pub(crate) fn moved_offset(byte: u32, a: u32, b_old: u32, delta: isize) -> Option<u32> {
    if byte < a {
        Some(byte)
    } else if byte >= b_old {
        Some(moved_from(byte as isize + delta))
    } else {
        None
    }
}

/// A byte offset clamped to a position the source has (an edit never moves a
/// cloned statement before the start of the file, so the clamp is a floor for a
/// degenerate `delta`, not a policy).
fn moved_from(byte: isize) -> u32 {
    byte.max(0) as u32
}
