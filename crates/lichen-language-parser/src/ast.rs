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
    /// `Tuple` — the builtin tuple template.
    Tuple,
    /// `Array` — the builtin array template.
    Array,
    /// `Struct` — the builtin struct template.
    Struct,
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

/// A prefix keyword operator: its `arity` operands follow it by juxtaposition.
/// See `docs/notes/operators.md` §2, §11.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixOp {
    /// `int2float e` — the `Int`'s value as the nearest `f32`.
    Int2Float,
    /// `float2int e` — the `Float` truncated toward zero.
    Float2Int,
    /// `max a b` — the larger of two values, in the operands' class.
    Max,
    /// `min a b` — the smaller of two values, in the operands' class.
    Min,
    /// `exp x` — the exponential of one `Float` (`docs/notes/floating-point.md` §6).
    Exp,
    /// `sqrt x` — the square root of one `Float` (`docs/notes/floating-point.md` §7).
    Sqrt,
    /// `floor x` — the largest integral `Float` not above one `Float`.
    Floor,
    /// `sin x` — the sine of one `Float` (`crates/lichen-kernel-ir/src/unary.rs`).
    Sin,
    /// `cos x` — the cosine of one `Float` (`crates/lichen-kernel-ir/src/unary.rs`).
    Cos,
    /// `ln x` — the natural logarithm of one `Float`
    /// (`crates/lichen-kernel-ir/src/unary.rs`).
    Ln,
}

impl PrefixOp {
    /// How many operands the operator takes by juxtaposition.
    ///
    /// # Invariant
    /// The parser consumes exactly this many and the checker reads exactly this many, so
    /// one spelling cannot come to mean two arities.
    pub fn arity(self) -> usize {
        match self {
            PrefixOp::Int2Float
            | PrefixOp::Float2Int
            | PrefixOp::Exp
            | PrefixOp::Sqrt
            | PrefixOp::Floor
            | PrefixOp::Sin
            | PrefixOp::Cos
            | PrefixOp::Ln => 1,
            PrefixOp::Max | PrefixOp::Min => 2,
        }
    }
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
    /// `@any` — the wildcard literal: it unifies with everything and constrains
    /// nothing (`compound-element-slots.md`).
    Any(Span),
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
    /// `f x` — application; a glued `(` is a read, not an argument (see
    /// [`Expr::FieldRead`]).
    Apply {
        function: Box<Expr>,
        argument: Box<Expr>,
        span: Span,
    },
    /// `a = b` — a unify of two expressions: the actual value and the expected
    /// one it is unified with.
    Unify {
        actual: Box<Expr>,
        expected: Box<Expr>,
        span: Span,
    },
    /// `actual <: template` — apply `template` to `_`, then unify with its result.
    TemplateUnify {
        actual: Box<Expr>,
        template: Box<Expr>,
        span: Span,
    },
    /// `a op b` — a binary integer operation.
    BinOp {
        operator: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    /// `if cond then e1 else e2`; desugared to the lazy application
    /// `(e2, e1)(cond)`.
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
        span: Span,
    },
    /// `@assert e` — a prefix assert, the highlevel `assert(e)` form: a side
    /// constraint, not a unify.
    Assert { value: Box<Expr>, span: Span },
    /// `max a b`, `int2float e` — a prefix keyword operator over its operands
    /// (`docs/notes/operators.md` §2, §11).
    Prefix {
        operator: PrefixOp,
        operands: Vec<Expr>,
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
    /// `a(k)` — a positional slot read; the `(` is adjacent and holds one
    /// expression.
    FieldRead {
        container: Box<Expr>,
        key: Box<Expr>,
        span: Span,
    },
    /// `a.name` — a named field read through the struct value's name table.
    NamedFieldRead {
        container: Box<Expr>,
        name: String,
        span: Span,
    },
    /// `t <== k` — a table lookup: the entry whose stored key is deep-equal to
    /// `k`.
    TableFind {
        container: Box<Expr>,
        key: Box<Expr>,
        span: Span,
    },
    /// `e : T`, `e # p`, `e ! r`, `e ? d`, `e ~ "m"` — an annotation, and the
    /// marks `e` carries.
    ///
    /// # Invariant
    /// A **mark** is the one annotation that is not an attribute: its payloads are text,
    /// never expressions, so no reader evaluates one and none reaches the expr layout.
    Annotation {
        value: Box<Expr>,
        r#type: Option<Box<Expr>>,
        perspective: Option<Box<Expr>>,
        refinement: Option<Box<Expr>>,
        doc: Option<Box<Expr>>,
        marks: Vec<String>,
        span: Span,
    },
    /// `(e1, ..., en)` — a tuple value (always; no type/value mode).
    Tuple(Vec<Expr>, Span),
    /// `set{E}` — a set type, whose one element is the element **slot**.
    TypeSet(Box<Expr>, Span),
    /// `table{K, V}` — a table type, whose two elements are the key and value
    /// **slots**.
    TypeTable {
        key: Box<Expr>,
        value: Box<Expr>,
        span: Span,
    },
    /// `S{…}` — fill a struct value or template with field values.
    StructInst {
        callee: Box<Expr>,
        fields: Vec<StructFieldValue>,
        span: Span,
    },
    /// `{.a 1, .b 2}` — a struct value: the fields carry their own names.
    StructValue {
        fields: Vec<StructFieldValue>,
        span: Span,
    },
    /// `[template, length]` — the array template: the element template and the
    /// array's length.
    Array {
        template: Box<Expr>,
        length: Box<Expr>,
        span: Span,
    },
    /// `template[e1, ..., en]` — an array instance: the template and the
    /// element values it is filled with.
    ArrayInst {
        template: Box<Expr>,
        elements: Vec<Expr>,
        span: Span,
    },
    /// `table(k ==> v)` — a constant table literal; a key that is not
    /// concrete is dropped with an error.
    Table(Vec<(Expr, Expr)>, Span),
    /// `set(a, b)` — a set of ordinary values
    /// (`docs/notes/operator-polymorphism.md` §3).
    Set(Vec<Expr>, Span),
    /// `@shallow e` — a shallow-marked position; `usize::MAX` is the bare form.
    Shallow(Box<Expr>, usize, Span),
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
    /// `@gc e` — `e` in a scope of its own, released once it decides
    /// (`docs/notes/gc-block.md`). A block is an expression.
    Gc(Box<Expr>, Span),
    /// A syntactic error the parser recovered — an opaque error block; `range`
    /// is the byte span it masks.
    Err { range: (u32, u32), start: Span },
}

/// One statement: a binding or a bare expression (the final expression is kept
/// separately).
#[derive(Clone, Debug)]
pub enum Stmt {
    /// `name := value` — a graph-sharing binding.
    Binding(Binding),
    /// A bare expression — evaluated for its type checks, its value
    /// discarded.
    Expr(Expr),
}

/// A binding: `name := value`, or restrictive `let name := value`.
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    /// The name's span — diagnostics for the binding point here.
    pub span: Span,
    /// The binding's own `BinderId`; unset by the parser.
    pub binder: Option<BinderId>,
    pub value: Expr,
    /// `let` — visible only to later statements (`let a := a` uses outer `a`).
    pub restrictive: bool,
    /// `@cache` — a retained cell, whose identity is the binding's occurrence
    /// path (`docs/notes/incremental-update.md` §3).
    pub cached: bool,
    /// `@loop` — the recursion **may become a loop**; its absence is the unroll
    /// (`docs/notes/loop-conversion.md` §1.1).
    pub looping: bool,
}

/// One field of a struct value or a struct instance: a `.name` and the
/// field's value.
#[derive(Clone, Debug)]
pub struct StructFieldValue {
    pub name: String,
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
    /// `@cache` — a retained cell, exactly as [`Binding::cached`].
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
            Expr::Any(s) => *s,
            Expr::Lambda { span, .. } => *span,
            Expr::Apply { span, .. } => *span,
            Expr::Unify { span, .. } => *span,
            Expr::TemplateUnify { span, .. } => *span,
            Expr::BinOp { span, .. } => *span,
            Expr::If { span, .. } => *span,
            Expr::Assert { span, .. } => *span,
            Expr::Prefix { span, .. } => *span,
            Expr::NativeCall { span, .. } => *span,
            Expr::Index { span, .. } => *span,
            Expr::FieldRead { span, .. } => *span,
            Expr::NamedFieldRead { span, .. } => *span,
            Expr::TableFind { span, .. } => *span,
            Expr::Annotation { span, .. } => *span,
            Expr::Tuple(_, s) => *s,
            Expr::TypeSet(_, s) => *s,
            Expr::TypeTable { span, .. } => *span,
            Expr::StructInst { span, .. } => *span,
            Expr::StructValue { span, .. } => *span,
            Expr::Array { span, .. } => *span,
            Expr::ArrayInst { span, .. } => *span,
            Expr::Table(_, s) => *s,
            Expr::Set(_, s) => *s,
            Expr::Shallow(_, _, s) => *s,
            Expr::Block { span, .. } => *span,
            Expr::RecordBlock { span, .. } => *span,
            Expr::Gc(_, s) => *s,
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
