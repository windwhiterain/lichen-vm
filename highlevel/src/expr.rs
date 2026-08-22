//! The highlevel expression IR: a dense, id-referenced tree built once by
//! the language frontend and walked by the checker.
//!
//! Not slotmap-shaped: the IR never changes structurally, never runs, and is
//! never GC'd, so a plain [`Vec`] with [`ExprId`] indices suffices.  The
//! checker only reads it (its products — pairs, type cells — are lowlevel
//! nodes, so the table does not even grow).

use crate::program::HighValue;

/// A dense index into [`ExprTable::expr`].  References are pre-resolved: a
/// [`ExprKind::Var`] points at the [`ExprKind::Binder`] it refers to, so the
/// IR carries no name strings.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExprId(pub u32);

/// A half-open range into [`ExprTable::children`] (as plain fields, since
/// `std::ops::Range` is not `Copy`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChildRange {
    pub start: u32,
    pub end: u32,
}

/// A source span, supplied by the language frontend: `(line, column)`.
pub type Span = (u32, u32);

/// The highlevel program: a pure expression tree.
#[derive(Clone, Debug)]
pub struct ExprTable {
    pub expr: Vec<Expr>,
    /// One dense arena for all variadic [`ExprKind::Array`] children lists.
    pub children: Vec<ExprId>,
    pub root: ExprId,
}

#[derive(Clone, Copy, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Option<Span>,
}

#[derive(Clone, Copy, Debug)]
pub enum ExprKind {
    /// Integer literal.
    Int(u64),
    /// The `Type` constant (`Type : Type`).
    Type,
    /// A program-defined constant value (e.g. the `int` type constant).
    Const(HighValue),
    /// A binding cell — the reference target of [`ExprKind::Var`].
    Binder,
    /// A pre-resolved reference to a [`ExprKind::Binder`].
    Var(ExprId),
    /// `(binder, body)`.
    Lam(ExprId, ExprId),
    /// `(binder, value, body)`.
    Let(ExprId, ExprId, ExprId),
    /// `(function, argument)`.
    App(ExprId, ExprId),
    /// `(expression, type-expression)`.
    Ann(ExprId, ExprId),
    /// `(input-type, output-type)` — a function type, compiled to the
    /// kinded arrow `[[in, out], [FunctionType, Type]]`.
    Arrow(ExprId, ExprId),
    /// Elements stored in [`ExprTable::children`].
    Array(ChildRange),
}

impl ExprTable {
    pub fn new() -> Self {
        ExprTable {
            expr: Vec::new(),
            children: Vec::new(),
            root: ExprId(0),
        }
    }

    pub fn alloc(&mut self, kind: ExprKind, span: Option<Span>) -> ExprId {
        let id = ExprId(self.expr.len() as u32);
        self.expr.push(Expr { kind, span });
        id
    }

    pub fn alloc_array(&mut self, elements: &[ExprId], span: Option<Span>) -> ExprId {
        let start = self.children.len() as u32;
        self.children.extend_from_slice(elements);
        let range = ChildRange {
            start,
            end: self.children.len() as u32,
        };
        self.alloc(ExprKind::Array(range), span)
    }

    pub fn set_root(&mut self, root: ExprId) {
        self.root = root;
    }
}

impl Default for ExprTable {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Index<ExprId> for ExprTable {
    type Output = Expr;
    fn index(&self, id: ExprId) -> &Expr {
        &self.expr[id.0 as usize]
    }
}

impl std::ops::Index<ExprId> for Vec<Option<lichen_vm::lowlevel::NodeId>> {
    type Output = Option<lichen_vm::lowlevel::NodeId>;
    fn index(&self, id: ExprId) -> &Option<lichen_vm::lowlevel::NodeId> {
        &self[id.0 as usize]
    }
}

impl std::ops::IndexMut<ExprId> for Vec<Option<lichen_vm::lowlevel::NodeId>> {
    fn index_mut(&mut self, id: ExprId) -> &mut Option<lichen_vm::lowlevel::NodeId> {
        &mut self[id.0 as usize]
    }
}
