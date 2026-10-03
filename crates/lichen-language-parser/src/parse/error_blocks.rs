//! The recovered-error walk over a parsed program: every [`Expr::Err`] node's
//! byte range, in source order.

use super::*;
use crate::ast::ErrorBlock;
use stacksafe::stacksafe;
/// Collect the byte-range masks of every recovered-error node in the AST, in
/// source order.  [`Program::error_blocks`] carries these so the frontend can
/// exclude the error regions from a content signature / diff.
pub fn collect_error_blocks(program: &Program) -> Vec<ErrorBlock> {
    /// The walk's recursion: one frame per nested expression (through
    /// `walk_stmt` for a block, and back here), so a deep program overflows the
    /// stack it runs on.  The in-parser call site is inside the parser's 16 MiB
    /// worker, but `lichen_language`'s session splice calls this walk on the
    /// caller's thread.  `#[stacksafe]`: the recursion grows the stack instead
    /// of overflowing the process — the arrangement `docs/notes/code-audit.md`
    /// (P1-22) gave the frontend's walks.
    #[stacksafe]
    fn walk_expr(e: &Expr, out: &mut Vec<ErrorBlock>) {
        match e {
            Expr::Err { range, start } => out.push(ErrorBlock {
                range: *range,
                start: *start,
            }),
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::TypeConst(..)
            | Expr::Name(..)
            | Expr::Placeholder(..) => {}
            Expr::Lambda {
                parameter_type,
                parameter_perspective,
                r#return,
                ..
            } => {
                if let Some(t) = parameter_type {
                    walk_expr(t, out);
                }
                if let Some(p) = parameter_perspective {
                    walk_expr(p, out);
                }
                walk_expr(r#return, out);
            }
            Expr::Apply {
                function, argument, ..
            } => {
                walk_expr(function, out);
                walk_expr(argument, out);
            }
            Expr::BinOp { left, right, .. } => {
                walk_expr(left, out);
                walk_expr(right, out);
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                walk_expr(condition, out);
                walk_expr(then_branch, out);
                walk_expr(else_branch, out);
            }
            Expr::Assert { value, .. } => walk_expr(value, out),
            Expr::Convert { value, .. } => walk_expr(value, out),
            Expr::NativeCall { args, .. } => {
                for a in args {
                    walk_expr(a, out);
                }
            }
            Expr::Index { array, index, .. } => {
                walk_expr(array, out);
                walk_expr(index, out);
            }
            Expr::RawIndex {
                container, index, ..
            } => {
                walk_expr(container, out);
                walk_expr(index, out);
            }
            Expr::FieldRead { container, key, .. } => {
                walk_expr(container, out);
                walk_expr(key, out);
            }
            Expr::NamedFieldRead { container, .. } => {
                walk_expr(container, out);
            }
            Expr::RawNamedField { container, .. } => {
                walk_expr(container, out);
            }
            Expr::TableFind { container, key, .. } => {
                walk_expr(container, out);
                walk_expr(key, out);
            }
            Expr::Annotation {
                value,
                r#type,
                perspective,
                ..
            } => {
                walk_expr(value, out);
                if let Some(t) = r#type {
                    walk_expr(t, out);
                }
                if let Some(p) = perspective {
                    walk_expr(p, out);
                }
            }
            Expr::Arrow {
                parameter,
                r#return,
                ..
            } => {
                walk_expr(parameter, out);
                walk_expr(r#return, out);
            }
            Expr::Tuple(elems, _) | Expr::TypeTuple(elems, _) | Expr::Array(elems, _) => {
                for el in elems {
                    walk_expr(el, out);
                }
            }
            Expr::StructType(fields, _) => {
                for field in fields {
                    walk_expr(&field.ty, out);
                }
            }
            Expr::StructInst { callee, fields, .. } => {
                walk_expr(callee, out);
                for f in fields {
                    walk_expr(&f.value, out);
                }
            }
            Expr::Table(entries, _) => {
                for (k, v) in entries {
                    walk_expr(k, out);
                    walk_expr(v, out);
                }
            }
            Expr::Shallow(inner, _, _) => walk_expr(inner, out),
            Expr::TypeArray {
                element_type,
                length,
                ..
            } => {
                walk_expr(element_type, out);
                walk_expr(length, out);
            }
            Expr::Block {
                statements, expr, ..
            } => {
                for s in statements {
                    walk_stmt(s, out);
                }
                walk_expr(expr, out);
            }
            Expr::RecordBlock { fields, .. } => {
                for f in fields {
                    walk_expr(&f.value, out);
                }
            }
        }
    }
    fn walk_stmt(s: &Stmt, out: &mut Vec<ErrorBlock>) {
        match s {
            Stmt::Binding(binding) => walk_expr(&binding.value, out),
            Stmt::Expr(e) => walk_expr(e, out),
        }
    }
    let mut out = Vec::new();
    for bs in &program.statements {
        walk_stmt(&bs.stmt, &mut out);
    }
    if let Some(e) = &program.expr {
        walk_expr(e, &mut out);
    }
    out
}
