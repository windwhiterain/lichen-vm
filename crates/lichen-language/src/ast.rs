//! The parsed AST: one node per syntactic form, each carrying the source
//! position where it starts.  Terms and types share one `Expr` type — types
//! are expressions (see `docs/language-spec.md` §2).

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
    /// `x => e`, `x : T => e`, `x # n => e`, or `x : T # n => e` — a
    /// lambda whose parameter is annotated.  The annotation(s) are desugared
    /// by [`crate::compile`] into the `Function`'s `parameter_type` /
    /// `parameter_perspective` fields, compiled in body scope (the §4.2
    /// optimization over the `x => { x : T; e }` / `x # n` body-statement
    /// desugar).
    Lambda {
        parameter: String,
        parameter_span: Span,
        parameter_type: Option<Box<Expr>>,
        parameter_perspective: Option<Box<Expr>>,
        r#return: Box<Expr>,
        span: Span,
    },
    /// `f x` — application.  A parenthesized argument requires a space
    /// before the `(` (`f (x)`); a `(` adjacent to the callee is struct
    /// instantiation (see [`Expr::StructInst`]), so function application
    /// never uses adjacent parens.
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
    /// `!e` — a prefix assert: the highlevel `assert(e)` form.  A side
    /// constraint, not a unify — the checker force-evaluates the condition
    /// and requires `USize(1)`, while the expression's own value stays the
    /// condition's (an assert checks its subject, it does not replace it).
    /// `! 1 == 1` parses as `(!1) == 1`; assert a comparison by parenthesizing
    /// it: `!(1 == 1)`.
    Assert {
        value: Box<Expr>,
        span: Span,
    },
    /// `$name(args…)` — a call to a native operator registered by the compiling
    /// module's plugin (a private, per-file naming contract).  `name` resolves
    /// only against the module's native registry; the args are ordinary
    /// expressions.
    NativeCall {
        op: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// `e[i]` — an index into an array.
    Index {
        array: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    /// `a(k)` — a positional slot read over a tuple element or a struct
    /// field.  The `(` is *adjacent* to the container and holds a single
    /// expression with no comma — the syntactic distinction from struct
    /// instantiation (`a(1,)`, `a(1,1)`, and the two zero-field spellings
    /// `a()` / `a(,)`, mirroring the tuple grammar's `()` unit vs `(,)`
    /// empty tuple) and from function application (a spaced paren).
    FieldRead {
        container: Box<Expr>,
        key: Box<Expr>,
        span: Span,
    },
    /// `t{k}` — a table lookup: the entry whose stored key is deep-content
    /// equal to `k`.  The `{` is *adjacent* to the container — no space
    /// between — which is the syntactic distinction from a block (a spaced
    /// or statement-position `{` is a block, and `t { … }` with a space
    /// applies `t` to one), mirroring `C(...)` vs `f (x)`.
    TableFind {
        container: Box<Expr>,
        key: Box<Expr>,
        span: Span,
    },
    /// `e : T` and/or `e # p` — a type and/or perspective annotation.
    /// `: T` fills `r#type`, `# p` fills `perspective`.  Either may be absent
    /// (`e # p`, `e : T`); at most one of each.
    Annotation {
        value: Box<Expr>,
        r#type: Option<Box<Expr>>,
        perspective: Option<Box<Expr>>,
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
    /// `C(e1, ..., en)` — struct instantiation: a callee (a struct type or a
    /// generic struct constructor) applied to a positional field list.  The
    /// callee and the `(` are *adjacent* — no space between them — which is
    /// the syntactic distinction from function application (see
    /// [`Expr::Apply`]).  The instantiation forms mirror the tuple grammar's
    /// comma discipline: `C()` and the empty-tuple spelling `C(,)` carry no
    /// fields, `C(e,)` one, `C(e1, ..., en)` n.  The bare single-expression
    /// form `C(e)` is *reserved for the positional slot read*
    /// ([`Expr::FieldRead`]) — it is never an instantiation.
    StructInst {
        callee: Box<Expr>,
        fields: Vec<Expr>,
        span: Span,
    },
    /// `[e1, ..., en]` — an array literal.
    Array(Vec<Expr>, Span),
    /// `table { k1 :: v1, k2 :: v2, … }` — a constant table literal.  Each
    /// entry is a `key :: value` pair (any expressions; the parser recognizes
    /// the double colon as the pair separator).  The keys are force-evaluated
    /// and deep-content-hashed when the table is built; a key that is not
    /// concrete (it depends on an unbound value) is dropped with an error.
    Table(Vec<(Expr, Expr)>, Span),
    /// `~n e` — a shallow-marked array position (parsed only inside array
    /// literals).  `n` is the marker depth: `usize::MAX` = the bare `~` (the
    /// whole subtree shallow), `0` = unmarked (a no-op), `n` = the value
    /// slot at each of the first `n` levels of the element's type spine
    /// shallow.
    Shallow(Box<Expr>, usize, Span),
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
    /// A syntactic error recovered by the parser — the position where the
    /// broken construct started.  A compile-time leaf: it lowers to an
    /// inference placeholder, so a partially parsed program still compiles
    /// and checks (the parse error is reported alongside).
    Err(Span),
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

/// One statement binding: `name = value` (block-wide visible — the name is
/// in scope in its own value and in every statement of the block), or
/// `let name = value` for a *restrictive* binding (the name is in scope only
/// in later statements).
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    /// The name's span — diagnostics for the binding point here.
    pub span: Span,
    pub value: Expr,
    /// `let` — the name is visible only to *later* statements (the name is
    /// not in scope in its own value, so `let a = a` resolves `a` to the
    /// outer binding).  `false` (the default) — the name is visible
    /// throughout the block, so it may reference and recurse with itself and
    /// with any other binding in the block.
    pub restrictive: bool,
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
            Expr::Assert { span, .. } => *span,
            Expr::NativeCall { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::FieldRead { span, .. } => *span,
            Expr::TableFind { span, .. } => *span,
            Expr::Annotation { span, .. } => *span,
            Expr::Arrow { span, .. } => *span,
            Expr::Tuple(_, s) => *s,
            Expr::TypeTuple(_, s) => *s,
            Expr::StructType(_, s) => *s,
            Expr::StructInst { span, .. } => *span,
            Expr::Array(_, s) => *s,
            Expr::Table(_, s) => *s,
            Expr::Shallow(_, _, s) => *s,
            Expr::TypeArray { span, .. } => *span,
            Expr::Block { span, .. } => *span,
            Expr::Err(s) => *s,
        }
    }
}
