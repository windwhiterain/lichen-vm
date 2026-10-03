//! Occurrence paths: a node's identity as a route from the program root.
//!
//! The design is `docs/notes/incremental-update.md` §2.  A retained unit's
//! identity is its **position** in the syntax tree, not a hash of its content:
//! a step is a **name** wherever the syntax gives the position one (a binding,
//! a named struct field, a named instantiation argument) and an **index** only
//! where it does not — so inserting a statement *before* a binding does not
//! move that binding's path, while a pure child-index scheme would move every
//! path after it.  A name that **repeats** in its own list is not a position
//! (two entries would share one path, and a path is an identity): every entry
//! carrying a repeated name falls back to its index, exactly as an unnamed one
//! does.  Only that list's own names are compared — the same name in two
//! different lists is two positions, which is what a path's steps already say.
//!
//! The tree is the **AST**, not the highlevel IR.  The IR is a graph (a
//! binding's node *is* its value's node, and it holds no binding names), while
//! the AST is the tree the resolver already walks by name.  A path therefore
//! names a *position*: a node reached by several positions has several paths,
//! and a retained cell is a position, not a node.
//!
//! Two contracts live here and nowhere else:
//!
//! - [`children`] is the **step vocabulary** — which positions a node has and
//!   what reaches each one.  Changing it renumbers every stored path, so it is
//!   a compatibility contract exactly as the attribute order is.
//! - Fixed-arity positions use **reserved role slots**, not compacted
//!   positions: a lambda's `parameter_type` is always `Index(0)` and its
//!   `return` always `Index(2)`, whether or not the optional annotations are
//!   present.  An absent role simply does not resolve.

use std::collections::HashSet;
use std::fmt;

use crate::ast::{Binding, Expr, Program, RecordField, Stmt};

/// One step of an occurrence path.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Step {
    /// A named position: a binding, a named struct field, a named
    /// instantiation argument.  Stable under an insertion before it.
    Name(String),
    /// A positional step: a reserved role slot (see the module doc) or an
    /// element of a list whose syntax gives no name.
    Index(u32),
    /// A body's tail expression — a role, not a name (`return e` and a
    /// trailing `e` are the same position).
    Tail,
}

/// A route from the program root to a position in the AST.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Path(pub Vec<Step>);

impl Path {
    /// The path of the root itself (the program).
    pub fn root() -> Self {
        Path(Vec::new())
    }

    /// This path extended by one step.
    pub fn child(&self, step: Step) -> Self {
        let mut steps = self.0.clone();
        steps.push(step);
        Path(steps)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, step) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            match step {
                Step::Name(name) => f.write_str(name)?,
                Step::Index(index) => write!(f, "{index}")?,
                Step::Tail => f.write_str("return")?,
            }
        }
        Ok(())
    }
}

/// What a path resolves to.
#[derive(Clone, Copy, Debug)]
pub enum Node<'a> {
    /// A binding statement's position.  Its `binder` is the id the compiler
    /// maps to the value's node.
    Binding(&'a Binding),
    /// A record-block field's position.
    Field(&'a RecordField),
    /// Any other expression position.
    Expr(&'a Expr),
}

impl<'a> Node<'a> {
    /// The expression this position holds — a binding's or a field's *value*
    /// (a binding's node is its value's node: the IR's graph-sharing
    /// invariant), or the expression itself.
    pub fn expr(self) -> &'a Expr {
        match self {
            Node::Binding(binding) => &binding.value,
            Node::Field(field) => &field.value,
            Node::Expr(expr) => expr,
        }
    }
}

/// The children of `expr`, each with the step that reaches it.
///
/// This function *is* the step vocabulary (see the module doc).
pub fn children(expr: &Expr) -> Vec<(Step, Node<'_>)> {
    let mut out = Vec::new();
    match expr {
        // Leaves: no position below them.
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Str(..)
        | Expr::TypeConst(..)
        | Expr::Name(..)
        | Expr::Placeholder(..)
        | Expr::Err { .. } => {}
        Expr::Lambda {
            parameter_type,
            parameter_perspective,
            r#return,
            ..
        } => {
            if let Some(ty) = parameter_type {
                push_index(&mut out, 0, ty);
            }
            if let Some(perspective) = parameter_perspective {
                push_index(&mut out, 1, perspective);
            }
            push_index(&mut out, 2, r#return);
        }
        Expr::Apply {
            function, argument, ..
        } => {
            push_index(&mut out, 0, function);
            push_index(&mut out, 1, argument);
        }
        Expr::BinOp { left, right, .. } => {
            push_index(&mut out, 0, left);
            push_index(&mut out, 1, right);
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            push_index(&mut out, 0, condition);
            push_index(&mut out, 1, then_branch);
            push_index(&mut out, 2, else_branch);
        }
        Expr::Assert { value, .. } => push_index(&mut out, 0, value),
        Expr::Convert { value, .. } => push_index(&mut out, 0, value),
        Expr::NativeCall { args, .. } => {
            for (i, arg) in args.iter().enumerate() {
                push_index(&mut out, i as u32, arg);
            }
        }
        Expr::Index {
            array, index: i, ..
        } => {
            push_index(&mut out, 0, array);
            push_index(&mut out, 1, i);
        }
        Expr::RawIndex {
            container,
            index: i,
            ..
        } => {
            push_index(&mut out, 0, container);
            push_index(&mut out, 1, i);
        }
        Expr::FieldRead { container, key, .. } => {
            push_index(&mut out, 0, container);
            push_index(&mut out, 1, key);
        }
        Expr::NamedFieldRead { container, .. } | Expr::RawNamedField { container, .. } => {
            push_index(&mut out, 0, container);
        }
        Expr::TableFind { container, key, .. } => {
            push_index(&mut out, 0, container);
            push_index(&mut out, 1, key);
        }
        Expr::Annotation {
            value,
            r#type,
            perspective,
            doc,
            refinement,
            ..
        } => {
            push_index(&mut out, 0, value);
            if let Some(ty) = r#type {
                push_index(&mut out, 1, ty);
            }
            if let Some(perspective) = perspective {
                push_index(&mut out, 2, perspective);
            }
            if let Some(doc) = doc {
                push_index(&mut out, 3, doc);
            }
            if let Some(refinement) = refinement {
                push_index(&mut out, 4, refinement);
            }
        }
        Expr::Arrow {
            parameter,
            r#return,
            ..
        } => {
            push_index(&mut out, 0, parameter);
            push_index(&mut out, 1, r#return);
        }
        Expr::Tuple(elements, _) | Expr::TypeTuple(elements, _) | Expr::Array(elements, _) => {
            for (i, element) in elements.iter().enumerate() {
                push_index(&mut out, i as u32, element);
            }
        }
        Expr::StructType(fields, _) => {
            let repeated = repeated_names(fields.iter().map(|field| field.name.as_deref()));
            for (i, field) in fields.iter().enumerate() {
                out.push((field_step(&field.name, i, &repeated), Node::Expr(&field.ty)));
            }
        }
        Expr::StructInst { callee, fields, .. } => {
            push_index(&mut out, 0, callee);
            let repeated = repeated_names(fields.iter().map(|field| field.name.as_deref()));
            for (i, field) in fields.iter().enumerate() {
                // The callee reserves slot 0, so the field list starts at 1.
                out.push((
                    field_step(&field.name, i + 1, &repeated),
                    Node::Expr(&field.value),
                ));
            }
        }
        Expr::Table(entries, _) => {
            for (i, (key, value)) in entries.iter().enumerate() {
                push_index(&mut out, 2 * i as u32, key);
                push_index(&mut out, 2 * i as u32 + 1, value);
            }
        }
        Expr::Shallow(inner, _, _) => push_index(&mut out, 0, inner),
        Expr::TypeArray {
            element_type,
            length,
            ..
        } => {
            push_index(&mut out, 0, element_type);
            push_index(&mut out, 1, length);
        }
        Expr::Block {
            statements, expr, ..
        } => {
            let repeated = repeated_names(statements.iter().map(statement_name));
            let mut positions: Vec<(Step, Node<'_>)> = statements
                .iter()
                .enumerate()
                .map(|(i, statement)| statement_position(statement, i, &repeated))
                .collect();
            positions.push((Step::Tail, Node::Expr(expr)));
            out.extend(positions);
        }
        Expr::RecordBlock { fields, .. } => {
            let repeated = repeated_names(fields.iter().map(|field| field.name.as_deref()));
            for (i, field) in fields.iter().enumerate() {
                out.push((field_step(&field.name, i, &repeated), Node::Field(field)));
            }
        }
    }
    out
}

/// The names that appear more than once among `names` — the entries whose step
/// must fall back to an index (see the module doc).
fn repeated_names<'a>(names: impl Iterator<Item = Option<&'a str>>) -> HashSet<&'a str> {
    let mut seen = HashSet::new();
    let mut repeated = HashSet::new();
    for name in names.into_iter().flatten() {
        if !seen.insert(name) {
            repeated.insert(name);
        }
    }
    repeated
}

/// The step reaching a named-or-positional list entry: the name where the
/// syntax gives one *and it is unique in this list*, the entry's index
/// otherwise.
fn field_step(name: &Option<String>, index: usize, repeated: &HashSet<&str>) -> Step {
    match name {
        Some(name) if !repeated.contains(name.as_str()) => Step::Name(name.clone()),
        _ => Step::Index(index as u32),
    }
}

/// Push a reserved role slot's position.
fn push_index<'a>(out: &mut Vec<(Step, Node<'a>)>, index: u32, expr: &'a Expr) {
    out.push((Step::Index(index), Node::Expr(expr)));
}

/// The step and node of a statement position: a binding is reached by its
/// **name** (unless that name repeats in the same statement list), a bare
/// expression statement by its index.
fn statement_position<'a>(
    statement: &'a Stmt,
    index: usize,
    repeated: &HashSet<&str>,
) -> (Step, Node<'a>) {
    match statement {
        Stmt::Binding(binding) => {
            let step = if repeated.contains(binding.name.as_str()) {
                Step::Index(index as u32)
            } else {
                Step::Name(binding.name.clone())
            };
            (step, Node::Binding(binding))
        }
        Stmt::Expr(expr) => (Step::Index(index as u32), Node::Expr(expr)),
    }
}

/// A statement's binding name, for [`repeated_names`].
fn statement_name(statement: &Stmt) -> Option<&str> {
    match statement {
        Stmt::Binding(binding) => Some(binding.name.as_str()),
        Stmt::Expr(_) => None,
    }
}

/// The children of the program root: its statements, then its tail (absent for
/// a record program — a module has no tail position).
pub fn root_children(program: &Program) -> Vec<(Step, Node<'_>)> {
    let repeated = repeated_names(program.statements.iter().map(|bs| statement_name(&bs.stmt)));
    let mut out: Vec<(Step, Node<'_>)> = program
        .statements
        .iter()
        .enumerate()
        .map(|(i, statement)| statement_position(&statement.stmt, i, &repeated))
        .collect();
    if let Some(tail) = &program.expr {
        out.push((Step::Tail, Node::Expr(tail)));
    }
    out
}

/// Resolve `path` against `program`, one step at a time — the dynamic lookup
/// the design asks for: nothing is registered when a node is built, and a path
/// that no longer resolves (a moved, renamed or deleted position) simply
/// answers `None`.
pub fn resolve<'a>(program: &'a Program, path: &Path) -> Option<Node<'a>> {
    let mut positions = root_children(program);
    let mut node = None;
    for step in &path.0 {
        let (_, next) = positions.into_iter().find(|(s, _)| s == step)?;
        node = Some(next);
        positions = children(next.expr());
    }
    node
}

/// Every position in `program`, depth-first, with its path.
pub fn for_each<'a>(program: &'a Program, visit: &mut impl FnMut(&Path, Node<'a>)) {
    let mut path = Path::root();
    descend(&mut path, root_children(program), visit);
}

fn descend<'a>(
    path: &mut Path,
    positions: Vec<(Step, Node<'a>)>,
    visit: &mut impl FnMut(&Path, Node<'a>),
) {
    for (step, node) in positions {
        path.0.push(step);
        visit(path, node);
        // A binding's or a field's position holds its value, so the value's
        // own children hang below it.
        descend(path, children(node.expr()), visit);
        path.0.pop();
    }
}
