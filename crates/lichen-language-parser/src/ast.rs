//! The parsed AST: one node per syntactic form, each carrying the source
//! position where it starts.  Terms and types share one `Expr` type — types
//! are expressions (see `docs/language-spec.md` §2).

use lichen_language_lex::Span;

/// A binding's identity within one resolution pass — a dense, program-wide
/// index assigned by [`crate::resolve::Resolver`] to every binder (a block-wide
/// or `let` binding, a lambda parameter, a record field).  A [`Name`](Expr::Name)
/// use carries the id of the binder it resolves to.  The compiler maps an id to
/// its `ExprId`; the session compares resolved forms by these ids instead of
/// hashing name resolution.  Chosen as a plain `usize` because it is also used
/// as a dense `Vec` index (the ids are `0..n` over one pass).
pub type BinderId = usize;

/// The type constants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeConst {
    Int,
    Float,
    String,
    Type,
}

/// A binary operator.  `+ - * / %` and `& | ^` are arithmetic on `Int`; the
/// comparisons (`< > <= >= == !=`) compare and yield `0` or `1` (there is no
/// `Bool` — the comparison result drives an `if` branch).  `==`/`!=` are the
/// generalized equality over any two same-typed values; every other operator
/// is `Int`-only, and an `Int` is **unsigned**, so `/` and `%` are the unsigned
/// division and remainder.
///
/// `@in` is the one member that is not arithmetic: it is the **membership
/// test** `value @in set`, which asks whether a value *is one of* a set's
/// members and answers the same `0`/`1` scalar the comparisons do.  It is
/// spelled as a keyword operator rather than as a symbol because it is a
/// predicate over a set, and it is `@`-led for the same reason `@assert` and
/// `@loop` are: the sigil is reserved for keywords
/// (`docs/notes/operator-polymorphism.md` §3).  It sits at the comparison
/// level, so `x @in S == 1` reads as `(x @in S) == 1`.
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

/// The two class conversions, `int2float` and `float2int`.
///
/// They are spelled as prefix keywords rather than as an infix `as` because
/// there is nothing to choose: a conversion has one operand and one direction,
/// and the direction is the name (`docs/notes/operators.md` §1's set, and §4.2
/// of `docs/notes/floating-point.md` — the classes never convert implicitly, so
/// these two words are the only way one class becomes the other).
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
    /// A float literal — the payload is the same `f32` the lowlevel
    /// `LowValue::Float` and the highlevel `FloatLit` carry
    /// (`docs/notes/floating-point.md` §3.4), so the three are one width by
    /// construction.  The form carries no sign: the lexer emits `Minus` then
    /// `Float` (§3.3) and the parser has no prefix minus, so `-1.5` is a parse
    /// error at the `-` exactly as `-1` is.
    Float(f32, Span),
    /// A string literal — the immutable builtin `string` value.
    Str(String, Span),
    /// One of the type constants `Int` / `Float` / `string` / `Type`.
    TypeConst(TypeConst, Span),
    /// A use of a name — resolved by [`crate::resolve::Resolver`] to the
    /// binder's id.  The third field is the `BinderId` the use resolves to
    /// (`Some`) or `None` when the name does not resolve (an unresolved name
    /// lowers to the same inert `ErrorBlock` and emits a `Resolve` diagnostic).
    /// The parser leaves it unset; resolution fills it.
    Name(String, Span, Option<BinderId>),
    /// `_` — an inference placeholder hole, usable in *any* position (type or
    /// value): the checker infers the type from context.  It is its own token
    /// and never a name, so it cannot be bound or used as a lambda parameter.
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
        /// The parameter's own `BinderId`, assigned by the resolver so the
        /// body's uses of the parameter resolve to it.  Unset by the parser.
        parameter_binder: Option<BinderId>,
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
    /// `@assert 1 == 1` parses as `(@assert 1) == 1`; assert a comparison by parenthesizing
    /// it: `@assert (1 == 1)`.
    Assert { value: Box<Expr>, span: Span },
    /// `int2float e` / `float2int e` — a prefix class conversion.  The only
    /// place the language's two scalar classes meet: `Int` and `Float` do not
    /// convert implicitly (`docs/notes/floating-point.md` §4.2), so the
    /// direction is in the word and there is nothing to infer.  It binds at the
    /// prefix level beside `!`, so `int2float a + 1` converts `a` and
    /// `int2float f x` converts `f x`.
    Convert {
        operator: ConvOp,
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
    /// `X<e>` — a *raw* positional read.  The glued `<` (was the array-type
    /// postfix; the array type is now spelled `array<T, n>`, see
    /// [`Expr::TypeArray`]) reads element `e` of `X`'s **value** directly
    /// through the lowlevel `Index`, with **no type validation** — no
    /// array-type pinning, no `IndexTarget` guard, no bounds assert.  It reads
    /// a component of a type-as-value (`<Int, string><0>`,
    /// `struct<Int, string><1>`), and stays lazy on an unbound container
    /// (a parameter, a call result) so it resolves at the apply.
    RawIndex {
        container: Box<Expr>,
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
    /// `a.name` — a *named* field read over a struct field.  The field name
    /// is resolved against the struct type's name table (the
    /// `struct<.a Int, …>` names) to the field's positional index, then
    /// read like [`Expr::FieldRead`].  The name is a plain identifier,
    /// distinct from `a(k)` (a positional index expression).
    NamedFieldRead {
        container: Box<Expr>,
        name: String,
        span: Span,
    },
    /// `X::a` — a *raw* named component read.  The glued `::` reads the
    /// field named `a` from a **TypeStruct value** — the container's *type*
    /// must be a TypeStruct (the name→index table lies directly there, at
    /// `container_ty[0][1]`), unlike [`Expr::NamedFieldRead`]'s `.a`, which
    /// requires the container's *kind* to be TypeStruct (its table at
    /// `container_ty[1][0][1]`).  Both are check-time requirements; the
    /// difference is where the requirement and the name table sit.  `X::a`
    /// reads the field's type as a value (`struct<.a Int, .b string>::a`
    /// is `Int : Type`), and is lazy on an unbound container.
    RawNamedField {
        container: Box<Expr>,
        name: String,
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
    /// `e : T`, `e # p`, `e ! r`, and/or `e ? d` — a type, perspective,
    /// refinement, and/or doc (label) annotation.  `: T` fills `r#type`,
    /// `# p` fills `perspective`, `! r` fills `refinement`, `? d` fills
    /// `doc`.  Any may be absent (`e # p`, `e : T`, `e ? d`); at most one of
    /// each.  A refinement is the one annotation that is not metadata: `r` is
    /// a predicate on the annotated value required to evaluate to `1`
    /// (`docs/notes/operator-polymorphism.md` §3).
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
    /// `struct<T1, ..., Tn>` — a nominal struct type.  Each field may carry
    /// an optional name (`.name T`), so the syntax is `struct<.a Int, .b
    /// string>` where the name is the syntactic prefix `.name`.
    StructType(Vec<StructField>, Span),
    /// `C(e1, ..., en)` — struct instantiation: a callee (a struct type or a
    /// generic struct constructor) applied to a field list.  Each field may be
    /// positional (`C(1, 2)`) or *named* (`C(.x 1, .y Int)`) — the `.name`
    /// prefix is the same discriminator a `struct<.x T, ...>` definition uses,
    /// and the checker reorders the values to the definition's positional
    /// order.  The callee and the `(` are *adjacent* — no space between them —
    /// which is the syntactic distinction from function application (see
    /// [`Expr::Apply`]).  The instantiation forms mirror the tuple grammar's
    /// comma discipline: `C()` and the empty-tuple spelling `C(,)` carry no
    /// fields, `C(e,)` one, `C(e1, ..., en)` n.  The bare single-expression
    /// form `C(e)` is *reserved for the positional slot read*
    /// ([`Expr::FieldRead`]) — it is never an instantiation.
    StructInst {
        callee: Box<Expr>,
        fields: Vec<StructInstArg>,
        span: Span,
    },
    /// `[e1, ..., en]` — an array literal.
    Array(Vec<Expr>, Span),
    /// `table { k1 ==> v1, k2 ==> v2, … }` — a constant table literal.  Each
    /// entry is a `key ==> value` pair (any expressions; the parser recognizes
    /// the table arrow as the pair separator).  The keys are force-evaluated
    /// and deep-content-hashed when the table is built; a key that is not
    /// concrete (it depends on an unbound value) is dropped with an error.
    Table(Vec<(Expr, Expr)>, Span),
    /// `set{a, b, …}` — a set of ordinary values.  The members are full
    /// expressions and share one element type (a set is homogeneous, exactly
    /// like an array literal); the *value* is the members themselves, and what
    /// makes it a set rather than an array is its **type**
    /// (`docs/notes/operator-polymorphism.md` §3): a set is not array-indexable
    /// and is not an `array<T, n>`.
    Set(Vec<Expr>, Span),
    /// `~n e` — a shallow-marked array position (parsed only inside array
    /// literals).  `n` is the marker depth: `usize::MAX` = the bare `~` (the
    /// whole subtree shallow), `0` = unmarked (a no-op), `n` = the value
    /// slot at each of the first `n` levels of the element's type spine
    /// shallow.
    Shallow(Box<Expr>, usize, Span),
    /// `array<T, n>` — an array type: the element type `T` and the length `n`.
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
    /// `{ stmt; …; stmt }` with no trailing expression — a struct-returning
    /// block.  Its value is an anonymous struct instance; each statement is
    /// a field (a `name = value` binding is a named field, a bare expression
    /// a positional one).  A `pub`-marked statement becomes a field, and when
    /// any statement is `pub`, only the `pub` statements are fields.
    RecordBlock {
        fields: Vec<RecordField>,
        span: Span,
    },
    /// A syntactic error recovered by the parser — an opaque error block,
    /// carried by the frontend only and never consumed by the lower layers as
    /// real code (it lowers to a distinct [`ExprKind::ErrorBlock`], not an
    /// inference placeholder).  `range` is the byte span the broken
    /// construct's fallback covers (the mask — what a content signature /
    /// diff excludes); `start` the position where the broken construct began.
    Err { range: (u32, u32), start: Span },
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
    /// The binding's own `BinderId`, assigned by the resolver so uses of the
    /// name (including a block-wide binding's self/mutual references) resolve
    /// to it.  Unset by the parser.
    pub binder: Option<BinderId>,
    pub value: Expr,
    /// `let` — the name is visible only to *later* statements (the name is
    /// not in scope in its own value, so `let a = a` resolves `a` to the
    /// outer binding).  `false` (the default) — the name is visible
    /// throughout the block, so it may reference and recurse with itself and
    /// with any other binding in the block.
    pub restrictive: bool,
    /// `cache` — the binding is a **retained cell**: its value survives a
    /// rebuild, and its identity is the binding's occurrence path from the
    /// program root.  Orthogonal to `restrictive` (the two marks may appear
    /// together).  The mechanism is
    /// `lichen_language::compile_with_cells`, which lowers a clean cell to a
    /// static read; the design is `docs/notes/incremental-update.md`.
    pub cached: bool,
    /// `@loop` — the binding's recursion **may become a loop**.
    ///
    /// Orthogonal to both marks above. It does **not** mean "compile this to a
    /// loop": its absence is the unroll, which stays the default and still wins
    /// whenever it terminates. It says this recursion is *allowed to need* one,
    /// and evaluation takes that option where it would otherwise have expanded —
    /// `docs/notes/loop-conversion.md` §1.1.
    pub looping: bool,
}

/// One field of a `struct<…>` type: an optional name plus the field's type
/// expression.  `name` is `Some` for a `.name Ty` field and `None` for a
/// positional (`Ty`) field.
#[derive(Clone, Debug)]
pub struct StructField {
    pub name: Option<String>,
    pub ty: Expr,
}

/// One field argument of a struct instantiation: an optional `.name` prefix
/// plus the value expression.  `name` is `Some` for a `.x 1` argument and
/// `None` for a positional (`1`) argument.
#[derive(Clone, Debug)]
pub struct StructInstArg {
    pub name: Option<String>,
    pub value: Expr,
}

/// One field of a [`Expr::RecordBlock`]: an optional field name (from a
/// `name = value` binding), the field's value expression, whether it is
/// `pub`-marked, and whether it is a field at all — a `let` (restrictive)
/// statement is a block-local and is `field: false`, never a struct field.
#[derive(Clone, Debug)]
pub struct RecordField {
    pub name: Option<String>,
    /// The field's own `BinderId` (for a named field), assigned by the
    /// resolver.  Unset by the parser; `None` for a positional field.
    pub binder: Option<BinderId>,
    pub value: Expr,
    pub public: bool,
    /// `false` for a `let` binding (a block-local, never a struct field).
    pub field: bool,
    /// `cache` — the field is a retained cell, exactly as
    /// [`Binding::cached`] is for a statement binding (a record block's fields
    /// *are* the block's statements).  See `docs/notes/incremental-update.md`.
    pub cached: bool,
    /// `@loop` — the field's binding's recursion may become a loop, exactly as
    /// [`Binding::looping`] is for a statement binding.
    pub looping: bool,
    pub span: Span,
}

/// One statement inside a `{ … }` block: the statement plus whether it is
/// `pub`-prefixed (a `pub` statement becomes a struct field when the block is
/// a [`Expr::RecordBlock`]).
#[derive(Clone, Debug)]
pub struct BlockStmt {
    pub stmt: Stmt,
    pub public: bool,
}

/// A recovered-error region the parser masked: `range` is the byte span it
/// covers in the source, `start` the position where the broken construct began.
/// The frontend surfaces these so a content signature / diff can exclude the
/// error regions (see [`Program::error_blocks`]).
#[derive(Clone, Copy, Debug)]
pub struct ErrorBlock {
    pub range: (u32, u32),
    pub start: Span,
}

/// A program — the top level is always a block, terminated by the end of the
/// input (EOF acts as an implicit separating `Separator`).
///
/// The top level is exactly a `{ … }` block body: a list of (possibly
/// `pub`-marked) statements, optionally followed by a tail expression.  With a
/// tail, the program is an ordinary program whose value is that final
/// expression (its root).  Without a tail, it is a **record program** — a
/// module — whose value is an anonymous struct built from the statements (a
/// `name = value` binding is a named field, a bare expression a positional
/// one, a `let` binding is a block-local and never a field, and when any
/// statement is `pub` only the `pub` statements are fields).
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
    /// The top-level statements, in source order — the same (publically
    /// markable) block statements a `{ … }` body holds.  With a tail these are
    /// the non-final statements; without one, they are the module's fields.
    pub statements: Vec<BlockStmt>,
    /// The program's tail expression.  `Some` → an ordinary program whose
    /// value is this final expression; `None` → a record program (a module).
    pub expr: Option<Expr>,
    /// The error blocks the parser recovered, in source order.  These are the
    /// byte-range masks this program's `Expr::Err` nodes describe — the
    /// frontend excludes them from a content signature so an edit that only
    /// grows an error block reuses the established AST/IR/check.
    pub error_blocks: Vec<ErrorBlock>,
    /// The **token-index** range each logical statement covers, in source order.
    /// There is one entry per statement in [`Program::statements`], plus one
    /// for the program's tail [`Program::expr`] when there is one.  Tokens own
    /// byte ranges; the AST owns token ranges, so the session can map a changed
    /// byte region to the statements it touches (via the token stream) for
    /// incremental re-parsing.
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
