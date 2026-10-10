//! The parsed AST: one node per syntactic form, carrying its start position;
//! terms and types share one `Expr`.

use lichen_language_lex::Span;

/// A binding's identity in one resolution pass: a dense index the resolver
/// assigns to every binder.
pub type BinderId = usize;

/// The type constants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeConst {
    Int,
    Float,
    String,
    Type,
}

/// A binary operator: unsigned `Int` arithmetic, or a comparison yielding
/// `0`/`1` (`docs/language-spec.md` §2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Lt,
    Gt,
    Leq,
    Geq,
    Eq,
    Neq,
    BitAnd,
    BitOr,
    BitXor,
    /// `value @in set` — membership in a set.
    In,
}

/// The two class conversions, `int2float` and `float2int`
/// (`docs/notes/floating-point.md` §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvOp {
    /// `int2float e` — the `Int`'s value as the nearest `f32`.
    Int2Float,
    /// `float2int e` — the `Float` truncated toward zero.
    Float2Int,
}

#[derive(Clone, Debug)]
pub enum Expr {
    /// An integer literal.
    Int(usize, Span),
    /// A float literal — the same `f32` as `LowValue::Float`
    /// (`docs/notes/floating-point.md` §3.4).
    Float(f32, Span),
    /// A string literal — the immutable builtin `string` value.
    Str(String, Span),
    /// One of the type constants `Int` / `Float` / `string` / `Type`.
    TypeConst(TypeConst, Span),
    /// A use of a name; the third field is the `BinderId` it resolves to, or
    /// `None` when unresolved.
    Name(String, Span, Option<BinderId>),
    /// `_` — an inference placeholder hole in any position; never a name.
    Placeholder(Span),
    /// `x => e`, optionally annotated (`x : T => e`, `x # n => e`).
    Lambda {
        parameter: String,
        parameter_span: Span,
        /// The parameter's own `BinderId`; unset by the parser.
        parameter_binder: Option<BinderId>,
        parameter_type: Option<Box<Expr>>,
        parameter_perspective: Option<Box<Expr>>,
        r#return: Box<Expr>,
        span: Span,
    },
    /// `f x` — application; only a spaced `(` is an argument (see
    /// [`Expr::StructInst`]).
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
    /// `if cond then e1 else e2`; desugared to the lazy index `[e2, e1][cond]`.
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
        span: Span,
    },
    /// `@assert e` — a prefix assert, the highlevel `assert(e)` form: a side
    /// constraint, not a unify.
    Assert { value: Box<Expr>, span: Span },
    /// `int2float e` / `float2int e` — a prefix class conversion
    /// (`docs/notes/floating-point.md` §4.2).
    Convert {
        operator: ConvOp,
        value: Box<Expr>,
        span: Span,
    },
    /// `$name(args…)` — a call to a native operator of the compiling module.
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
    /// `X<e>` — a raw positional read of `X`'s value; the container must be a
    /// tuple type (`docs/notes/raw-index.md`).
    RawIndex {
        container: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    /// `a(k)` — a positional slot read; the `(` is adjacent and holds one
    /// expression (`a(1,)` is an instantiation).
    FieldRead {
        container: Box<Expr>,
        key: Box<Expr>,
        span: Span,
    },
    /// `a.name` — a named field read; the name resolves through the struct
    /// type's name table to a positional index.
    NamedFieldRead {
        container: Box<Expr>,
        name: String,
        span: Span,
    },
    /// `X::a` — a raw named read: field `a` of a **TypeStruct value**
    /// (`docs/notes/raw-field.md`).
    RawNamedField {
        container: Box<Expr>,
        name: String,
        span: Span,
    },
    /// `t{k}` — a table lookup: the entry whose stored key is deep-equal to `k`.
    TableFind {
        container: Box<Expr>,
        key: Box<Expr>,
        span: Span,
    },
    /// `e : T`, `e # p`, `e ! r`, and/or `e ? d` — a type, perspective,
    /// refinement, and/or doc annotation.
    Annotation {
        value: Box<Expr>,
        r#type: Option<Box<Expr>>,
        perspective: Option<Box<Expr>>,
        refinement: Option<Box<Expr>>,
        doc: Option<Box<Expr>>,
        span: Span,
    },
    /// `T1 -> T2` — a function type.
    Arrow {
        parameter: Box<Expr>,
        r#return: Box<Expr>,
        span: Span,
    },
    /// `(e1, ..., en)` — a tuple value (always; no type/value mode).
    Tuple(Vec<Expr>, Span),
    /// `<T1, ..., Tn>` — a tuple type (angle brackets always produce one).
    TypeTuple(Vec<Expr>, Span),
    /// `struct<.a Int, .b string>` — a nominal struct type; the field list is
    /// optional (`struct<>`).
    StructType(Vec<StructField>, Span),
    /// `C(.x 1, .y Int)` — struct instantiation; the `(` is adjacent to the
    /// callee (`C(e)` alone is a slot read).
    StructInst {
        callee: Box<Expr>,
        fields: Vec<StructInstArg>,
        span: Span,
    },
    /// `[e1, ..., en]` — an array literal.
    Array(Vec<Expr>, Span),
    /// `table { k ==> v }` — a constant table literal; a key that is not
    /// concrete is dropped with an error.
    Table(Vec<(Expr, Expr)>, Span),
    /// `set{a, b}` — a set of ordinary values
    /// (`docs/notes/operator-polymorphism.md` §3).
    Set(Vec<Expr>, Span),
    /// `~n e` — a shallow-marked array position; `usize::MAX` is the bare `~`.
    Shallow(Box<Expr>, usize, Span),
    /// `array<T, n>` — an array type: the element type `T` and the length `n`.
    TypeArray {
        element_type: Box<Expr>,
        length: Box<Expr>,
        span: Span,
    },
    /// `{ stmt; …; expr }` — a scoped block whose value is its final expression.
    Block {
        statements: Vec<Stmt>,
        expr: Box<Expr>,
        span: Span,
    },
    /// `{ stmt; … }` with no trailing expression — an anonymous struct instance
    /// (`docs/notes/record-program.md`).
    RecordBlock {
        fields: Vec<RecordField>,
        span: Span,
    },
    /// A syntactic error the parser recovered — an opaque error block; `range`
    /// is the byte span it masks.
    Err { range: (u32, u32), start: Span },
}

/// One statement: a binding or a bare expression (the final expression is kept
/// separately).
#[derive(Clone, Debug)]
pub enum Stmt {
    /// `name = value` — a graph-sharing binding.
    Binding(Binding),
    /// A bare expression — evaluated for its type checks, its value
    /// discarded.
    Expr(Expr),
}

/// One statement binding: `name = value` (block-wide visible), or
/// `let name = value` (visible only to later statements).
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    /// The name's span — diagnostics for the binding point here.
    pub span: Span,
    /// The binding's own `BinderId`; unset by the parser.
    pub binder: Option<BinderId>,
    pub value: Expr,
    /// `let` — the name is visible only to later statements (so `let a = a`
    /// resolves the outer `a`).
    pub restrictive: bool,
    /// `cache` — a retained cell, whose identity is the binding's occurrence
    /// path (`docs/notes/incremental-update.md` §3).
    pub cached: bool,
    /// `@loop` — the recursion **may become a loop**; its absence is the unroll
    /// (`docs/notes/loop-conversion.md` §1.1).
    pub looping: bool,
}

/// One field of a `struct<…>` type: an optional name plus the field's type.
#[derive(Clone, Debug)]
pub struct StructField {
    pub name: Option<String>,
    pub ty: Expr,
}

/// One field argument of a struct instantiation: an optional `.name` plus the
/// value.
#[derive(Clone, Debug)]
pub struct StructInstArg {
    pub name: Option<String>,
    pub value: Expr,
}

/// One field of a [`Expr::RecordBlock`]: an optional name, the value, and
/// whether it is `pub` and a field at all.
#[derive(Clone, Debug)]
pub struct RecordField {
    pub name: Option<String>,
    /// The named field's own `BinderId`; unset by the parser.
    pub binder: Option<BinderId>,
    pub value: Expr,
    pub public: bool,
    /// `false` for a `let` binding (a block-local, never a struct field).
    pub field: bool,
    /// `cache` — a retained cell, exactly as [`Binding::cached`].
    pub cached: bool,
    /// `@loop` — exactly as [`Binding::looping`].
    pub looping: bool,
    pub span: Span,
}

/// One statement inside a `{ … }` block, plus whether it is `pub`-marked.
#[derive(Clone, Debug)]
pub struct BlockStmt {
    pub stmt: Stmt,
    pub public: bool,
}

/// A recovered-error region the parser masked: its byte `range` and the `start`
/// where the broken construct began.
#[derive(Clone, Copy, Debug)]
pub struct ErrorBlock {
    pub range: (u32, u32),
    pub start: Span,
}

/// A program: a block body ended by the input.  With a tail it is ordinary;
/// without one, it is a record program.
#[derive(Clone, Debug)]
pub struct Program {
    /// The top-level statements in source order: the non-final ones with a tail,
    /// the module's fields without one.
    pub statements: Vec<BlockStmt>,
    /// The tail expression; `None` for a record program (a module,
    /// `docs/notes/record-program.md`).
    pub expr: Option<Expr>,
    /// The recovered error blocks: the byte-range masks this program's
    /// [`Expr::Err`] nodes describe.
    pub error_blocks: Vec<ErrorBlock>,
    /// The **token-index** range each statement covers — one per statement, plus
    /// one for the tail.
    pub stmt_ranges: Vec<(usize, usize)>,
}

impl Expr {
    /// The expression's start position — its leftmost token.
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s) => *s,
            Expr::Float(_, s) => *s,
            Expr::Str(_, s) => *s,
            Expr::TypeConst(_, s) => *s,
            Expr::Name(_, s, _) => *s,
            Expr::Placeholder(s) => *s,
            Expr::Lambda { span, .. } => *span,
            Expr::Apply { span, .. } => *span,
            Expr::BinOp { span, .. } => *span,
            Expr::If { span, .. } => *span,
            Expr::Assert { span, .. } => *span,
            Expr::Convert { span, .. } => *span,
            Expr::NativeCall { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::RawIndex { span, .. } => *span,
            Expr::FieldRead { span, .. } => *span,
            Expr::NamedFieldRead { span, .. } => *span,
            Expr::RawNamedField { span, .. } => *span,
            Expr::TableFind { span, .. } => *span,
            Expr::Annotation { span, .. } => *span,
            Expr::Arrow { span, .. } => *span,
            Expr::Tuple(_, s) => *s,
            Expr::TypeTuple(_, s) => *s,
            Expr::StructType(_, s) => *s,
            Expr::StructInst { span, .. } => *span,
            Expr::Array(_, s) => *s,
            Expr::Table(_, s) => *s,
            Expr::Set(_, s) => *s,
            Expr::Shallow(_, _, s) => *s,
            Expr::TypeArray { span, .. } => *span,
            Expr::Block { span, .. } => *span,
            Expr::RecordBlock { span, .. } => *span,
            Expr::Err { start, .. } => *start,
        }
    }
}

impl Stmt {
    /// The statement's start position — its leftmost token (a binding's name
    /// span, or the expression's).
    pub fn span(&self) -> Span {
        match self {
            Stmt::Binding(binding) => binding.span,
            Stmt::Expr(e) => e.span(),
        }
    }
}
