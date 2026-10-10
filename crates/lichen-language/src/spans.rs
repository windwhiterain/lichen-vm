//! Shifting the spans of a statement the splice cloned instead of re-parsing.
//!
//! # Invariant
//!
//! A clone's bytes are unchanged but its `(line, col)` position may not be, so
//! a cloned statement after the edit has its spans shifted. Only a clone needs
//! it: a prefix is before the edit, a window statement was re-parsed.

use lichen_language_lex::{Span, line_col, offset_of_span};
use stacksafe::stacksafe;

use crate::ast::{Expr, RecordField, Stmt};

/// Shift every span in `expr` by the edit's byte `delta`.
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
        | Expr::Array(elements, span)
        | Expr::Set(elements, span) => {
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

/// Move a byte offset through an edit that replaced `[a, b_old)` by `delta`.
///
/// # Invariant
///
/// `None` when the offset is inside the replaced region: no position in the new
/// source names it, so a caller that needs one must re-derive it.
pub(crate) fn moved_offset(byte: u32, a: u32, b_old: u32, delta: isize) -> Option<u32> {
    if byte < a {
        Some(byte)
    } else if byte >= b_old {
        Some(moved_from(byte as isize + delta))
    } else {
        None
    }
}

/// A byte offset clamped to a floor of zero for a degenerate `delta`.
fn moved_from(byte: isize) -> u32 {
    byte.max(0) as u32
}
