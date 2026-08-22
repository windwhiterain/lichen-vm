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
    /// `x => e`.
    Lambda {
        parameter: String,
        parameter_span: Span,
        r#return: Box<Expr>,
        span: Span,
    },
    /// `f x` — application.
    Apply {
        function: Box<Expr>,
        argument: Box<Expr>,
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
    /// `{ name = expr; …; expr }` — a block: scoped bindings followed by the
    /// block's value.  The bindings are graph-shared like a program's; the
    /// block compiles to its final expression's own IR node (see
    /// [`crate::compile`]).
    Block {
        bindings: Vec<Binding>,
        expr: Box<Expr>,
        span: Span,
    },
}

/// One statement binding: `name = value`.
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    /// The name's span — diagnostics for the binding point here.
    pub span: Span,
    pub value: Expr,
}

/// A program: `name = expr; …` bindings followed by the final expression.
///
/// The bindings are *graph sharing*, not sugar for application: each value
/// compiles once into the IR arena and every use of its name is that same
/// node id, so the IR stays a plain expression graph (no `let`, no desugared
/// lambda).  The final expression is the program's value and its root.
#[derive(Clone, Debug)]
pub struct Program {
    pub bindings: Vec<Binding>,
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
