//! The parsed AST: one node per syntactic form, each carrying the source
//! position where it starts.  Terms and types share one `Expr` type — types
//! are expressions (see `docs/language.md` §2).

use lichen_highlevel::ir::Span;

/// The two type constants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeConst {
    Int,
    Type,
}

/// A binary operator.  `+`/`-` are arithmetic; `<=`/`==` compare and yield
/// `0` or `1` (there is no `Bool` — the comparison result drives an `if`
/// branch).  All operate on `Int`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Leq,
    Eq,
}

#[derive(Clone, Debug)]
pub enum Expr {
    /// An integer literal.
    Int(usize, Span),
    /// One of the type constants `Int` / `Type`.
    TypeConst(TypeConst, Span),
    /// A use of a name — resolved by [`crate::compile`] to the binder's id.
    Name(String, Span),
    /// `_` in type position — an inference placeholder (the checker infers
    /// the type from context).  In term position `_` parses as a
    /// [`Expr::Name`] and stays an ordinary (possibly discard) name.
    Placeholder(Span),
    /// `x => e`, or `x : T => e` — a lambda whose parameter is annotated.
    /// The annotation is desugared by [`crate::compile`] to
    /// `(x => e) : (T -> _)`.
    Lambda {
        parameter: String,
        parameter_span: Span,
        parameter_type: Option<Box<Expr>>,
        r#return: Box<Expr>,
        span: Span,
    },
    /// `f x` — application.
    Apply {
        function: Box<Expr>,
        argument: Box<Expr>,
        span: Span,
    },
    /// `a op b` — a binary integer operation.
    BinOp {
        operator: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    /// `if cond then e1 else e2` — a conditional.  Desugared by
    /// [`crate::compile`] to the lazy `Index` branch `[e2, e1][cond]` — the
    /// condition (`0`/`1`) selects the branch, and the untaken branch is
    /// never evaluated.
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
        span: Span,
    },
    /// `e[i]` — an index into an array or tuple.
    Index {
        array: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    /// `e : T` — an annotation.
    Annotation {
        value: Box<Expr>,
        r#type: Box<Expr>,
        span: Span,
    },
    /// `T1 -> T2` — a function type.
    Arrow {
        parameter: Box<Expr>,
        r#return: Box<Expr>,
        span: Span,
    },
    /// `(e1, ..., en)` — a tuple value.
    Tuple(Vec<Expr>, Span),
    /// `(T1, ..., Tn)` in type position — a tuple type.
    TypeTuple(Vec<Expr>, Span),
    /// `struct<T1, ..., Tn>` — a nominal struct type, positional fields.
    StructType(Vec<Expr>, Span),
    /// `[e1, ..., en]` — an array literal.
    Array(Vec<Expr>, Span),
    /// `T<e>` — an array type.
    TypeArray {
        element_type: Box<Expr>,
        length: Box<Expr>,
        span: Span,
    },
    /// `{ stmt; …; expr }` — a block: scoped statements followed by the
    /// block's value.  The statements are graph-shared like a program's; the
    /// block compiles to its final expression's own IR node (see
    /// [`crate::compile`]).
    Block {
        statements: Vec<Stmt>,
        expr: Box<Expr>,
        span: Span,
    },
}

/// One statement: a binding or a bare expression (the program's non-final
/// statements; the last statement is the final expression, kept separately in
/// [`Program::expr`] / [`Expr::Block`]).
#[derive(Clone, Debug)]
pub enum Stmt {
    /// `name = value` — a graph-sharing binding.
    Binding(Binding),
    /// A bare expression — evaluated for its type checks, its value
    /// discarded.
    Expr(Expr),
}

/// One statement binding: `name = value`, or `rec name = value` for a
/// recursive binding.
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    /// The name's span — diagnostics for the binding point here.
    pub span: Span,
    pub value: Expr,
    /// `rec` — the name is in scope inside its own value (the value must be
    /// a lambda), so the value may apply itself.
    pub recursive: bool,
}

/// A program: `name = expr; …` statements followed by the final expression.
///
/// The statements are *graph sharing*, not sugar for application: each
/// binding's value compiles once into the IR arena and every use of its name
/// is that same node id, so the IR stays a plain expression graph (no `let`,
/// no desugared lambda).  A bare expression statement is compiled too — the
/// frontend wires every statement into the root so each is checked and
/// evaluated (the runtime *is* the typechecker) — and the final expression is
/// the program's value and its root.
#[derive(Clone, Debug)]
pub struct Program {
    /// The non-final statements, in source order.
    pub statements: Vec<Stmt>,
    pub expr: Expr,
}

impl Expr {
    /// The expression's start position — its leftmost token.
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s) => *s,
            Expr::TypeConst(_, s) => *s,
            Expr::Name(_, s) => *s,
            Expr::Placeholder(s) => *s,
            Expr::Lambda { span, .. } => *span,
            Expr::Apply { span, .. } => *span,
            Expr::BinOp { span, .. } => *span,
            Expr::If { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::Annotation { span, .. } => *span,
            Expr::Arrow { span, .. } => *span,
            Expr::Tuple(_, s) => *s,
            Expr::TypeTuple(_, s) => *s,
            Expr::StructType(_, s) => *s,
            Expr::Array(_, s) => *s,
            Expr::TypeArray { span, .. } => *span,
            Expr::Block { span, .. } => *span,
        }
    }
}
