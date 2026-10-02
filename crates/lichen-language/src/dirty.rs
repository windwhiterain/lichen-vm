//! Dirty propagation: which retained cells an edit invalidates.
//!
//! The design is `docs/notes/incremental-update.md` §4.  A cell's identity is
//! its occurrence path, and nothing decides its reuse but dirtiness: an edit
//! dirties the positions it re-parsed and, transitively, every position that
//! **reads** a dirtied one.  No content key takes part in this path — the graph
//! is the resolver's own `BinderId` edges, so the propagation is exact and
//! needs no recorded read set.
//!
//! The graph's node is the **top-level statement**: a statement declares the
//! binders its subtree binds and reads the binders its `Name` uses resolve to.
//! A nested binder (a lambda parameter, a block local) is visible only inside
//! its own statement, so it can never carry an edge *between* two statements —
//! which is why the statement is the right node, and why a cell anywhere inside
//! a statement is dirtied exactly when its statement is.
//!
//! Two propagations run and their union is the answer, because an edit is the
//! only thing that can move a resolution and the two programs disagree about
//! what a name reads:
//!
//! - the **previous** program, seeded by the statements the edit re-parsed in
//!   it — this catches a read that *disappeared* (a name the edit deleted, or
//!   moved out of scope), which the current program no longer records as a read;
//! - the **current** program, seeded by the same statements in its own index
//!   space — this catches a read that *appeared* (a name the edit brought into
//!   scope), which the previous program did not record as a read.
//!
//! Either propagation alone is unsound, and neither is a superset of the other.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use lichen_language_parser::path::Path;
use stacksafe::stacksafe;

use crate::ast::{BinderId, Expr, Program, Stmt};
use crate::compile::cached_bindings;

/// The marked bindings an edit invalidates, by occurrence path.
///
/// `previous` and `current` are both **resolved** programs (the previous
/// build's and the one about to be lowered); `previous_window` and
/// `current_window` are the top-level statement indices the edit re-parsed in
/// each — [`crate::session`] derives them from the splice it already performed.
/// A window past the end of a program simply contributes nothing.
pub(crate) fn dirty_marked_paths(
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
                if let Some(path) = marked.get(binder) {
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

/// The dependency contribution of each logical statement — one per
/// [`Program::statements`] entry, plus one for the tail expression when there
/// is one (the same index space as [`Program::stmt_ranges`]).
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

/// The statements the edit reaches: those it re-parsed, plus every statement
/// that reads a binder a reached statement declares (transitively).
fn dirty_statements(deps: &[Deps], window: Range<usize>) -> Vec<usize> {
    let mut dirty = vec![false; deps.len()];
    let mut reached: Vec<usize> = Vec::new();
    for statement in window {
        if let Some(slot) = dirty.get_mut(statement) {
            *slot = true;
            reached.push(statement);
        }
    }
    // Binder → the statements that read it, so the fixpoint walks edges instead
    // of rescanning every statement for every newly dirty one.
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
/// Exhaustive over [`Expr`] on purpose: this walk decides whether a retained
/// cell is still valid, so a new expression form must be classified here rather
/// than silently contribute no edges.
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
        Expr::NativeCall { args, .. } => {
            for arg in args {
                walk(arg, deps);
            }
        }
        Expr::Index { array, index, .. } => {
            walk(array, deps);
            walk(index, deps);
        }
        Expr::RawIndex {
            container, index, ..
        } => {
            walk(container, deps);
            walk(index, deps);
        }
        Expr::FieldRead { container, key, .. } => {
            walk(container, deps);
            walk(key, deps);
        }
        Expr::NamedFieldRead { container, .. } | Expr::RawNamedField { container, .. } => {
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
            ..
        } => {
            walk(value, deps);
            for attribute in [r#type, perspective, doc].into_iter().flatten() {
                walk(attribute, deps);
            }
        }
        Expr::Arrow {
            parameter,
            r#return,
            ..
        } => {
            walk(parameter, deps);
            walk(r#return, deps);
        }
        Expr::Tuple(elements, _) | Expr::TypeTuple(elements, _) | Expr::Array(elements, _) => {
            for element in elements {
                walk(element, deps);
            }
        }
        Expr::StructType(fields, _) => {
            for field in fields {
                walk(&field.ty, deps);
            }
        }
        Expr::StructInst { callee, fields, .. } => {
            walk(callee, deps);
            for field in fields {
                walk(&field.value, deps);
            }
        }
        Expr::Table(entries, _) => {
            for (key, value) in entries {
                walk(key, deps);
                walk(value, deps);
            }
        }
        Expr::Shallow(inner, _, _) => walk(inner, deps),
        Expr::TypeArray {
            element_type,
            length,
            ..
        } => {
            walk(element_type, deps);
            walk(length, deps);
        }
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
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::TypeConst(..)
        | Expr::TypeOf(..)
        | Expr::Placeholder(..)
        | Expr::Err { .. } => {}
    }
}
