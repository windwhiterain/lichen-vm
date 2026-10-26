//! Dirty propagation: which retained cells an edit invalidates.
//!
//! # Invariant
//!
//! A statement is dirty iff the edit re-parsed it or it reads a binder a dirty
//! statement declares, propagated over both the previous and the current
//! program. See docs/notes/incremental-update.md §4.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use lichen_language_parser::path::Path;
use stacksafe::stacksafe;

use crate::ast::{BinderId, Expr, Program, Stmt};
use crate::compile::cached_bindings;

/// The marked bindings an edit invalidates, by occurrence path.
///
/// # Invariant
///
/// This is the shipped judge over one edit: the key in [`crate::key`] must agree with it
/// before it may decide reuse itself (`docs/notes/heavy-value-cache.md` §2, §5).
pub fn dirty_marked_paths(
    previous: &Program,
    current: &Program,
    previous_window: Range<usize>,
    current_window: Range<usize>,
) -> HashSet<Path> {
    let mut dirty = HashSet::new();
    for (program, window) in [(previous, previous_window), (current, current_window)] {
        let marked = cached_bindings(program);
        let deps = dependencies(program);
        for statement in dirty_statements(&deps, window) {
            for binder in &deps[statement].declares {
                if let Some((path, _)) = marked.get(binder) {
                    dirty.insert(path.clone());
                }
            }
        }
    }
    dirty
}

/// One statement's dependency contribution.
#[derive(Default)]
struct Deps {
    /// Every binder its subtree declares (at any depth).
    declares: Vec<BinderId>,
    /// Every binder a `Name` use in its subtree resolves to.
    reads: Vec<BinderId>,
}

/// One entry per [`Program::statements`], plus the tail when there is one.
fn dependencies(program: &Program) -> Vec<Deps> {
    let mut out =
        Vec::with_capacity(program.statements.len() + usize::from(program.expr.is_some()));
    for statement in &program.statements {
        let mut deps = Deps::default();
        collect(&statement.stmt, &mut deps);
        out.push(deps);
    }
    if let Some(tail) = &program.expr {
        let mut deps = Deps::default();
        walk(tail, &mut deps);
        out.push(deps);
    }
    out
}

/// The statement graph: each top-level statement's declared and read binders.
///
/// # Invariant
///
/// One entry per statement plus the tail, in source order. A `declares` set holds binders
/// declared *at any depth* in the statement, so a nested binder is named by the statement
/// that contains it and not by a statement of its own.
pub(crate) fn statement_dependencies(program: &Program) -> Vec<(Vec<BinderId>, Vec<BinderId>)> {
    dependencies(program)
        .into_iter()
        .map(|deps| (deps.declares, deps.reads))
        .collect()
}

/// The statements the edit reaches: re-parsed, plus transitive readers.
fn dirty_statements(deps: &[Deps], window: Range<usize>) -> Vec<usize> {
    let mut dirty = vec![false; deps.len()];
    let mut reached: Vec<usize> = Vec::new();
    for statement in window {
        if let Some(slot) = dirty.get_mut(statement) {
            *slot = true;
            reached.push(statement);
        }
    }
    // Binder → readers, so the fixpoint walks edges instead of rescanning.
    let mut readers: HashMap<BinderId, Vec<usize>> = HashMap::new();
    for (statement, deps) in deps.iter().enumerate() {
        for binder in &deps.reads {
            readers.entry(*binder).or_default().push(statement);
        }
    }
    while let Some(statement) = reached.pop() {
        for binder in &deps[statement].declares {
            for &reader in readers.get(binder).into_iter().flatten() {
                if !dirty[reader] {
                    dirty[reader] = true;
                    reached.push(reader);
                }
            }
        }
    }
    (0..deps.len())
        .filter(|&statement| dirty[statement])
        .collect()
}

/// Accumulate one statement's declared and read binders.
fn collect(statement: &Stmt, deps: &mut Deps) {
    match statement {
        Stmt::Binding(binding) => {
            if let Some(binder) = binding.binder {
                deps.declares.push(binder);
            }
            walk(&binding.value, deps);
        }
        Stmt::Expr(expr) => walk(expr, deps),
    }
}

/// Accumulate the declared and read binders of `expr`'s subtree.
///
/// # Invariant
///
/// Exhaustive over [`Expr`] on purpose: a new form must be classified here
/// rather than silently contribute no edges.
#[stacksafe]
fn walk(expr: &Expr, deps: &mut Deps) {
    match expr {
        Expr::Name(_, _, binder) => {
            if let Some(binder) = binder {
                deps.reads.push(*binder);
            }
        }
        Expr::Lambda {
            parameter_binder,
            parameter_type,
            parameter_perspective,
            r#return,
            ..
        } => {
            if let Some(binder) = parameter_binder {
                deps.declares.push(*binder);
            }
            if let Some(ty) = parameter_type {
                walk(ty, deps);
            }
            if let Some(perspective) = parameter_perspective {
                walk(perspective, deps);
            }
            walk(r#return, deps);
        }
        Expr::Apply {
            function, argument, ..
        } => {
            walk(function, deps);
            walk(argument, deps);
        }
        Expr::BinOp { left, right, .. } => {
            walk(left, deps);
            walk(right, deps);
        }
        // `a = b`: the actual value and the expected one.
        Expr::Unify {
            actual, expected, ..
        } => {
            walk(actual, deps);
            walk(expected, deps);
        }
        // `actual <: template`: the actual and the template it is matched against.
        Expr::TemplateUnify {
            actual, template, ..
        } => {
            walk(actual, deps);
            walk(template, deps);
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            walk(condition, deps);
            walk(then_branch, deps);
            walk(else_branch, deps);
        }
        Expr::Assert { value, .. } => walk(value, deps),
        Expr::Prefix { operands, .. } => {
            for operand in operands {
                walk(operand, deps);
            }
        }
        Expr::NativeCall { args, .. } => {
            for arg in args {
                walk(arg, deps);
            }
        }
        Expr::Index { array, index, .. } => {
            walk(array, deps);
            walk(index, deps);
        }
        Expr::FieldRead { container, key, .. } => {
            walk(container, deps);
            walk(key, deps);
        }
        Expr::NamedFieldRead { container, .. } => {
            walk(container, deps);
        }
        Expr::TableFind { container, key, .. } => {
            walk(container, deps);
            walk(key, deps);
        }
        Expr::Annotation {
            value,
            r#type,
            perspective,
            doc,
            refinement,
            ..
        } => {
            walk(value, deps);
            for attribute in [r#type, perspective, doc, refinement].into_iter().flatten() {
                walk(attribute, deps);
            }
        }
        Expr::Tuple(elements, _) | Expr::Set(elements, _) => {
            for element in elements {
                walk(element, deps);
            }
        }
        // `{.a e1, …}`: the struct template's fields.
        Expr::StructValue { fields, .. } => {
            for field in fields {
                walk(&field.value, deps);
            }
        }
        Expr::StructInst { callee, fields, .. } => {
            walk(callee, deps);
            for field in fields {
                walk(&field.value, deps);
            }
        }
        // `[template, length]`: the array template and its length.
        Expr::Array {
            template, length, ..
        } => {
            walk(template, deps);
            walk(length, deps);
        }
        // `template[e1, …]`: the array instance's template and its elements.
        Expr::ArrayInst {
            template, elements, ..
        } => {
            walk(template, deps);
            for element in elements {
                walk(element, deps);
            }
        }
        Expr::Table(entries, _) => {
            for (key, value) in entries {
                walk(key, deps);
                walk(value, deps);
            }
        }
        Expr::TypeSet(element, _) => walk(element, deps),
        Expr::TypeTable { key, value, .. } => {
            walk(key, deps);
            walk(value, deps);
        }
        Expr::Shallow(inner, _, _) => walk(inner, deps),
        Expr::Block {
            statements, expr, ..
        } => {
            for statement in statements {
                collect(statement, deps);
            }
            walk(expr, deps);
        }
        Expr::RecordBlock { fields, .. } => {
            for field in fields {
                if let Some(binder) = field.binder {
                    deps.declares.push(binder);
                }
                walk(&field.value, deps);
            }
        }
        // `@gc` declares nothing of its own; its expression does.
        Expr::Gc(inner, _) => walk(inner, deps),
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::TypeConst(..)
        | Expr::Placeholder(..)
        | Expr::Any(..)
        | Expr::Err { .. } => {}
    }
}
