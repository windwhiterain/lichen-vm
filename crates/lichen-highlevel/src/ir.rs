//! The highlevel expression IR: a dense, id-referenced tree built once by
//! the language frontend and walked by the checker.
//!
//! Not slotmap-shaped: the IR never changes structurally, never runs, and is
//! never GC'd, so a plain [`Vec`] with [`ExprId`] indices suffices.  The
//! checker only reads it (its products — pairs, type cells — are lowlevel
//! nodes, so the table does not even grow).  The frontend records the
//! block-wide binding placeholder ids in [`IR::block_roots`]; the checker
//! uses that set to place its cycle-cut skeleton only where a cycle can
//! actually form.

use std::collections::HashSet;

use crate::attr::{AttrSpec, NoAttr};
use crate::program::{HighProgramLiteral, TypeOperator};

/// The static schema of an expression: which compile-time attributes ride on
/// its runtime pair and in which order.  `tail` is index-aligned with the
/// slots below the `[value, type]` head — an empty `tail` is the ordinary
/// 2-wide pair, a `[Perspective]` tail a 3-wide pair `[value, type, attr]`.
///
/// The ordinary "type" is a *runtime* value (the `[value, type]` pair); a
/// schema is lichen's first *static* thing — it describes the *shape* of an
/// expression's runtime pair (its arity and which attribute sits in which
/// slot) and is known at lowering.  It is never a runtime node, never unified,
/// never cloned: the checker consumes it to decide how to build the runtime
/// pair, then it is gone.  It is generic over the attribute type `A` (the
/// `HighProgram::Attr`), so a language plugs in its own attribute marker and
/// the highlevel stays attribute-agnostic.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Schema<A> {
    pub tail: Vec<A>,
}

impl<A> Default for Schema<A> {
    fn default() -> Self {
        Schema { tail: Vec::new() }
    }
}

/// An interned index into [`IR::schema_table`].  `0` is always the default
/// (empty-`tail`) schema, so a fresh [`IR::alloc`] needs no write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SchemaId(pub u32);

/// A binary operation.  The arithmetic ops (`Add`, `Sub`, `Mul`, `Div`, `Rem`)
/// and the bitwise ops (`BitAnd`, `BitOr`, `BitXor`) yield their result; the
/// comparisons (`Lt`, `Gt`, `Leq`, `Geq`, `Eq`, `Neq`) yield `USize(0/1)` so the
/// result can drive the lazy `Index` branch of an `if` — there is no `Bool`
/// value in the universe.
///
/// `Eq`/`Neq` are the **generalized** equality (see `docs/language-spec.md`):
/// they compare any two same-typed values whole, so they are the only two whose
/// operands the checker does not pin to `Int`.  Everything else is `Int`-only,
/// and unsigned — an `Int` is a machine-sized unsigned integer, so `Div`/`Rem`
/// are the unsigned division and remainder.
///
/// `In` is the membership predicate `value @in set`: it yields `0`/`1` like a
/// comparison, and its two operands are a value and a **set** of such values
/// (`docs/notes/operator-polymorphism.md` §3).  It is not arithmetic, so it is
/// the third member — after `Eq`/`Neq` — whose operands are not pinned to
/// `Int`; unlike them it pins nothing at all and is checked structurally.
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
    /// `value @in set` — membership in a set (`docs/notes/operator-polymorphism.md` §3).
    In,
}

/// Every [`BinOp`] names the [`TypeOperator`] the checker runs: the two enums
/// spell the same operators, so the checker converts once here instead
/// of repeating the mapping at each site.  [`TypeOperator::Fresh`] is the
/// other direction and has no [`BinOp`] spelling — it mints a nominal struct
/// id, which no source operator does.
impl From<BinOp> for TypeOperator {
    fn from(operator: BinOp) -> Self {
        match operator {
            BinOp::Add => TypeOperator::Add,
            BinOp::Sub => TypeOperator::Sub,
            BinOp::Mul => TypeOperator::Mul,
            BinOp::Div => TypeOperator::Div,
            BinOp::Rem => TypeOperator::Rem,
            BinOp::Lt => TypeOperator::Lt,
            BinOp::Gt => TypeOperator::Gt,
            BinOp::Leq => TypeOperator::Leq,
            BinOp::Geq => TypeOperator::Geq,
            BinOp::Eq => TypeOperator::Eq,
            BinOp::Neq => TypeOperator::Neq,
            BinOp::BitAnd => TypeOperator::BitAnd,
            BinOp::BitOr => TypeOperator::BitOr,
            BinOp::BitXor => TypeOperator::BitXor,
            BinOp::In => TypeOperator::InDomain,
        }
    }
}

/// A prefix **class conversion**: `int2float e` and `float2int e`, the two
/// directions between the language's two scalar classes.
///
/// These are the only place `Int` and `Float` meet at all — every other
/// operator computes *within* one class and the checker refuses a mixture
/// (`docs/notes/floating-point.md` §4.2).  Each direction is a total function
/// on values but not on types: `int2float` widens without loss, `float2int`
/// truncates toward zero and has no answer for a `NaN` or a magnitude past the
/// machine integer (see [`crate::program::OUT_OF_RANGE`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConvOp {
    Int2Float,
    Float2Int,
}

/// Every [`ConvOp`] names the [`TypeOperator`] it runs, exactly as
/// [`BinOp`](Self::BinOp) does: the two spell the same computation, so the
/// checker converts once here.
impl From<ConvOp> for TypeOperator {
    fn from(operator: ConvOp) -> Self {
        match operator {
            ConvOp::Int2Float => TypeOperator::Int2Float,
            ConvOp::Float2Int => TypeOperator::Float2Int,
        }
    }
}

/// A dense index into [`IR::expr`].  References are pre-resolved: a
/// use of a parameter *is* the [`ExprKind::Parameter`]'s own `ExprId` (the
/// checker's scope stack is keyed by it), so the IR carries no name strings.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExprId(pub u32);

/// A half-open range into [`IR::children`] (as plain fields, since
/// `std::ops::Range` is not `Copy`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChildRange {
    pub start: u32,
    pub end: u32,
}

impl ChildRange {
    /// An empty range — no children.
    pub const EMPTY: ChildRange = ChildRange { start: 0, end: 0 };
}

/// Push `id` when an `Option` operand is present — the shape every optional
/// operand field of [`ExprKind::children`] has.
fn push_opt(id: &Option<ExprId>, push: &mut impl FnMut(ExprId)) {
    if let Some(id) = id {
        push(*id);
    }
}

/// Append `values` to an arena and return the half-open [`ChildRange`] they
/// occupy.  Every variadic arena write goes through here, so a range and the
/// push it describes cannot disagree.
fn extend_range<T>(arena: &mut Vec<T>, values: impl IntoIterator<Item = T>) -> ChildRange {
    let start = arena.len() as u32;
    arena.extend(values);
    ChildRange {
        start,
        end: arena.len() as u32,
    }
}

/// A source-blind diagnostic location: the IR expression a check is about,
/// plus a single **recursive** descent path through its `[value, type, …]`
/// spine.
///
/// The highlevel is deliberately source-blind — it never sees a source span —
/// so a location must
/// be expressible purely in terms of the expression's structure.  The highlevel
/// *does* parse each level of that structure, tagging it as either an
/// expression's `[value, type]` pair (a [`LocStep::Value`]/[`LocStep::Type`]/
/// [`LocStep::Attr`] slot) or a tuple/array/struct shape (a [`LocStep::Elem`]),
/// so the language layer can build a precise diagnostic without re-deriving
/// the type grammar.
///
/// There is no distinct "kind" in lichen: `kind` is just the type's type, one
/// more `[value, type]` pairing, and that chain is unbounded (`Type : Type`).
/// So a [`LocStep::Type`] may repeat arbitrarily; a [`LocStep::Type`] followed
/// by [`LocStep::Type`] is the type's type, and so on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Loc {
    /// The IR expression the diagnostic is about.
    pub expr: ExprId,
    /// The recursive descent of [`LocStep`]s (see [`Self`]).
    pub path: Vec<LocStep>,
}

/// One step of the recursive location descent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocStep {
    /// The value slot of a `[value, type]` pair (element 0).  For an atomic
    /// type expression this is that type's marker.
    Value,
    /// The type slot of a `[value, type]` pair (element 1).  Repeating reaches
    /// the type's type, ad infinitum.
    Type,
    /// An attribute-tail slot of an expression pair (element 2 + index).
    Attr(usize),
    /// Entering a tuple/array/struct structure — a compound type's element 0,
    /// the list of its fields/elements.
    Shape,
    /// Element `i` of a tuple/array/struct shape.
    Elem(usize),
}

/// The highlevel program: a pure expression tree, generic over the
/// compile-time attribute type `A` (an expression schema's tail) and the
/// literal vocabulary `L` (the [`LiteralExt`](crate::program::LiteralExt)
/// value a literal node carries, defaulting to the built-in
/// [`HighProgramLiteral`]).
#[derive(Clone, Debug)]
pub struct IR<A = NoAttr, L = HighProgramLiteral> {
    pub expr: Vec<Expr<L>>,
    /// One dense arena for all variadic children lists ([`ExprKind::Tuple`],
    /// [`ExprKind::TypeTuple`], [`ExprKind::Array`], [`ExprKind::TypeStruct`],
    /// [`ExprKind::ShallowArray`], [`ExprKind::Table`]).
    pub children: Vec<ExprId>,
    /// One dense arena for struct **field names** — index-aligned with the
    /// [`ExprKind::TypeStruct`] `fields` range **and** the
    /// [`ExprKind::Instantiate`] `value` tuple's elements: `None` for an
    /// unnamed (positional) field/argument, `Some(name)` for a `.name Ty`
    /// field or a `.name bool` argument.
    pub struct_names: Vec<Option<&'static str>>,
    /// One dense arena for the shallow depths of [`ExprKind::ShallowArray`]
    /// — one `usize` per element: 0 = unmarked, `usize::MAX` = the bare `~`
    /// (the whole subtree shallow), n = the value slot at each of the first
    /// n levels of the element's type spine shallow.
    pub depths: Vec<usize>,
    pub root: ExprId,
    /// The block-wide binding placeholder ids — the only `ExprId`s whose
    /// subtree can reference themselves (a self/mutual cycle).  The checker
    /// pre-registers a cycle-cut skeleton only for these: an inline compound
    /// term can never cycle, and its skeleton's extra cells would otherwise
    /// poison the apply-time unify (a placeholder reached through an
    /// index-typed apply would stay an undecided `?a`).
    pub block_roots: HashSet<ExprId>,
    /// The top-level (outer-block) statement expression ids, in source order —
    /// bindings and bare-expression statements, NOT including the final
    /// expression.  The build's cascade deep pass already computed each one's
    /// value/type; a reader (the language server) reads them by id rather than
    /// re-deriving them from the root, and never re-evaluates (lazy/recursive
    /// bindings stay undecided and are not forced).
    pub stmt_roots: Vec<ExprId>,
    /// The per-expression static schema, index-aligned with [`IR::expr`] — one
    /// [`SchemaId`] each.  `alloc` stamps the default (empty-`tail`) schema.
    pub schemas: Vec<SchemaId>,
    /// The interned schema table (see [`Schema`]); [`SchemaId`]s index it.
    pub schema_table: Vec<Schema<A>>,
}

#[derive(Clone, Copy, Debug)]
pub struct Expr<L> {
    pub kind: ExprKind<L>,
}

/// The expression kinds.  Generic over the literal vocabulary `L` — a literal
/// node carries an extensible [`LiteralExt`](crate::program::LiteralExt)
/// value (any struct that builds a `value : type` pair); the built-in leaf
/// literal wraps a raw value token.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ExprKind<L> {
    /// A literal leaf — a `value : type` pair declared together.  The literal
    /// is any struct implementing [`LiteralExt`](crate::program::LiteralExt):
    /// it builds the value and type nodes through the curated
    /// [`Ctx`](crate::program::Ctx) (a leaf literal is the built-in case — an
    /// int literal or a type constant whose type is derived via
    /// [`ValueType::type_of`]).
    Literal(L),
    /// A function parameter (or, `let` desugared by the frontend, a let-bound
    /// name).  Uses of the parameter in the return expression are the
    /// parameter's own `ExprId`.
    Parameter,
    /// `{ parameter, return }` — the parameter is a [`ExprKind::Parameter`].
    /// `parent` is the enclosing function's IR node — the explicit link the
    /// checker hands to `begin_function` as this function's
    /// [`Function::parent`](lichen_lowlevel::Function::parent).  It keeps
    /// sibling functions' template scopes disjoint while absorbing
    /// truly-nested closures into their parent's template; it is carried in
    /// the IR because a `Function` node is allocated *after* its body, so the
    /// enclosing function's id is not known to the checker at that point.
    /// `None` for a top-level function (and for the mutual-recursion sibling
    /// case — see the frontend's `fn_parents` invariant).
    Function {
        parameter: ExprId,
        /// The annotated parameter's type (`x : T => e`): compiled *in
        /// body scope* — so in-body readers of the parameter see the
        /// annotated kind (an array annotation's length, a function
        /// annotation's arrow) — while the parameter's type slot still
        /// performs the argument check at each apply.  `None` for an
        /// unannotated parameter.
        parameter_type: Option<ExprId>,
        /// The annotated parameter's attribute (`x # n => e`), compiled in
        /// body scope like `parameter_type`.  `None` for an unannotated
        /// parameter.  This is the optimization for the `x # n => e` →
        /// `x => { x # n; e }` desugar (an unannotated body statement would
        /// otherwise materialize a block; the field rides the `Function`
        /// and the checker compiles it in body scope).
        parameter_attribute: Option<ExprId>,
        r#return: ExprId,
        parent: Option<ExprId>,
        /// The `@loop` marker, carried from the source binding onto the
        /// `Function` node its value compiled to — a marked binding's node
        /// **is** its value's node (the frontend transplants one into the
        /// other), so the binding's mark and the function's are the same fact.
        ///
        /// The marker is **permission, not a command**: it says this
        /// function's recursion *may* become a loop, and the default is still
        /// the unroll whenever the trip count is decidable
        /// (`docs/notes/loop-conversion.md` §1.1).  It is the only thing the
        /// evaluator has to go on, so it rides here rather than being
        /// re-derived from the source.
        ///
        /// `false` for every function the source did not mark, which is the
        /// overwhelming majority and must behave exactly as it always did.
        looping: bool,
    },
    /// `{ function, argument }`.
    Apply { function: ExprId, argument: ExprId },
    /// `{ operator, left, right }` — a binary integer operation: `+`, `-`,
    /// `<=`, `==`.  The operand types are checked against `Int`, and the
    /// result's type is `Int` (a comparison's `0/1` drives an `if` branch).
    BinOp {
        operator: BinOp,
        left: ExprId,
        right: ExprId,
    },
    /// `{ type_expr, value, names }` — struct instantiation: `s(1, 2)` wraps
    /// the positional tuple `value` in the struct type `type_expr`.  `names`
    /// is a range into [`IR::struct_names`], index-aligned with `value`'s
    /// tuple elements, one optional field name per argument (a `.x 1`
    /// argument is `Some("x")`, a positional `1` is `None`); the checker
    /// reorders the value's elements to the definition's positional order
    /// against the struct type's name table.  The value's element types are
    /// checked against the struct's field list, and the expression's type is
    /// the struct type itself.  Emitted by the frontend for the glued
    /// comma-disciplined paren (`C()`, `C(,)`, `C(e,)`, `C(e1, …, en)`) —
    /// recognition is *syntactic*; the checker validates that the callee's
    /// type is a struct kind (a non-struct callee is a
    /// [`DiagKind::InstantiateCallee`](crate::DiagKind) diagnostic).
    Instantiate {
        type_expr: ExprId,
        value: ExprId,
        names: ChildRange,
    },
    /// `{ value, names }` — a struct-returning block (`RecordBlock`): `value`
    /// is a positional tuple of the emitted field values, and `names` is a
    /// range into [`IR::struct_names`] holding each field's optional name
    /// (parallel to the tuple elements).  The checker builds an anonymous
    /// struct type from the value's element types and wraps the value in it,
    /// so the block's value is a struct instance whose fields read
    /// positionally or by name.  A `let` field does not make it here — it is
    /// a block-local.
    Record { value: ExprId, names: ChildRange },
    /// `assert(condition)` — an explicit constraint, not a unify: the
    /// condition's value node is registered as an assert point.  The
    /// checker deep-evaluates every assert after the definition pass
    /// and requires `USize(1)`; a condition that stays
    /// undecided (an undecided parameter, a lazy marker) is not triggered, and the apply clone
    /// re-checks the instantiated condition per call.  The expression
    /// compiles to the condition itself — the assert is a side constraint.
    Assert { condition: ExprId },
    /// `int2float e` / `float2int e` — a **class conversion**, the one
    /// construct that crosses between `Int` and `Float`.  The checker pins the
    /// operand to the direction's source class and gives the expression the
    /// target class, so `int2float x` on a float `x` is a check error rather
    /// than a no-op; the value is the single operand (a one-element operand
    /// array, since [`TypeOperator`](crate::program::TypeOperator)'s unary
    /// forms read `operands[0]`).
    Convert { operator: ConvOp, value: ExprId },
    /// `{ array, index }` — an element read `a[i]`; the container is *pinned*
    /// to an array type (its element type is the pinned shape's element cell
    /// and the read registers an `i < length` bounds assert), so this form
    /// never kind-dispatches.
    Index { array: ExprId, index: ExprId },
    /// `{ container, index }` — a *raw* positional read `X<e>` (the glued `<`
    /// postfix): element `index` of the container's **value**, read
    /// structurally through the lowlevel `Index` **without type validation**.
    /// There is no array-type pinning, no [`IndexTarget`](crate::DiagKind)
    /// guard, and no bounds assert — the container is read by value whatever
    /// its type, so it reads a component of a type-as-value
    /// (`<Int, string><0>`, `struct<.f Int, .g string><1>`) or of any expression's
    /// value.  The result is the element's own pair: its value is element 0
    /// of the read, its type element 1, both lazily (an undecided container
    /// resolves at the apply).  This form is what the `T<e>` array-type
    /// postfix used to be; the array type is now [`Self::TypeArray`] spelled
    /// `array<T, n>`.
    RawIndex { container: ExprId, index: ExprId },
    /// `{ container, key }` — a positional slot read `a(k)` over a tuple
    /// element or struct field (both shapes are positional type lists; the
    /// nominal struct id lives in the kind, so the extraction is the same
    /// for both).  The value is the structural `Index` over the container's
    /// value; the type is `Index(shape, k)` over the container type's shape,
    /// evaluated lazily so an untyped parameter resolves at the call.  The
    /// frontend emits it for the *adjacent* single-expression paren form —
    /// `a(1)` — the syntactic distinction from struct instantiation
    /// (`a(1,)`, `a(1,1)`, `a()`, `a(,)`) and from function application
    /// (a spaced paren).
    Field { container: ExprId, key: ExprId },
    /// `{ container, name }` — a *named* field read `a.name`.  The checker
    /// resolves `name` against the container type's struct name table to the
    /// positional index, then reads like [`Self::Field`].  `name` is an
    /// interned `&'static str` (the source's leaked field name).
    NamedField {
        container: ExprId,
        name: &'static str,
    },
    /// `{ container, name }` — a *raw* named component read `X::a`.  Unlike
    /// [`Self::NamedField`] (whose name table sits in the container type's
    /// **kind**, `container_ty[1][0][1]`), this reads the table directly from
    /// the container's **type**, which must be a TypeStruct (`container_ty[0][1]`),
    /// and yields the field's *type* as a value (`S::a` on
    /// `struct<.a Int, .b string>` is `Int : Type`).  `name` is an interned
    /// `&'static str`.  The requirement is a check-time unify (a concretely
    /// non-TypeStruct container is a diagnostic), so it is *not* raw in the
    /// no-validation sense of [`Self::RawIndex`].
    RawNamedField {
        container: ExprId,
        name: &'static str,
    },
    /// `{ container, key }` — a table lookup `t{k}`: the entry whose stored
    /// key is deep-content-equal to `k`.  The frontend emits it for the
    /// *adjacent* brace form — the syntactic distinction from positional
    /// [`Self::Index`] — so the checker compiles the read to the dedicated
    /// lowlevel `TableGet` directly, never a kind dispatch.
    Find { container: ExprId, key: ExprId },
    /// `{ value, type?, attributes }` — a type and/or attribute annotation.
    /// `: T` fills `r#type`.  `attributes` is a contiguous range into
    /// [`IR::children`] holding one value expression per schema-tail entry,
    /// **aligned by position** with the node's schema tail (so a `# p ? doc`
    /// tail `[Perspective, Doc]` pairs `attributes[0]` with the perspective
    /// and `attributes[1]` with the doc).  The attributes also stamp the
    /// annotated node's schema — the one asymmetry with `:` (the slots come
    /// into existence by being annotated).
    Annotation {
        value: ExprId,
        r#type: Option<ExprId>,
        attributes: ChildRange,
    },
    /// `{ parameter, return }` — a function type, compiled to the kinded
    /// arrow `[[in, out], [FunctionType, Type]]`.
    TypeFunction { parameter: ExprId, r#return: ExprId },
    /// A tuple instance `[v1, ..., vn]` — one type slot per element, so the
    /// elements may be heterogeneous.  Elements stored in
    /// [`IR::children`].
    Tuple(ChildRange),
    /// A tuple type expression `[T1, ..., Tn]` — the element types, kinded
    /// `[[T1, ..., Tn], [TupleType, Type]]`.  Elements stored in
    /// [`IR::children`].
    TypeTuple(ChildRange),
    /// A struct type expression.  `fields` is the field-type list; the
    /// corresponding `names` range (into [`IR::struct_names`] holds each
    /// field's optional name.  The checker builds it as the usual
    /// `[shape, kind]` pair — shape `[T1, …, Tn]`, kind
    /// `[[id, names, names_in_order], TypeStruct]` — so the *fresh nominal* id
    /// and the
    /// optional name table sit in the kind's marker payload, never in the shape
    /// (see the
    /// checker's struct-type construction).  A struct type is reused by
    /// binding it once through a parameter.
    TypeStruct {
        fields: ChildRange,
        names: ChildRange,
    },
    /// An array instance `[v1, ..., vn]` — every element shares one type
    /// (unlike a [`Self::Tuple`]'s per-element slots).  Elements stored in
    /// [`IR::children`].
    Array(ChildRange),
    /// A set `set{a, b, …}` — every member shares one type (a set is
    /// homogeneous, like an [`Self::Array`]), but the instance is typed by the
    /// set kind, not by `array<T, n>`: the *value* is the members themselves
    /// and the set's identity is its type.  That is what keeps a set out of
    /// array indexing, and it is why no value-level tag is needed
    /// (`docs/notes/operator-polymorphism.md` §3).  Members in
    /// [`IR::children`].
    Set(ChildRange),
    /// A constant table instance `table { k1 ==> v1, k2 ==> v2, … }` — every
    /// key shares one key type and every value one value type (checked
    /// against two shared cells, like an array's single element cell).  The
    /// entries are stored interleaved in [`IR::children`]:
    /// `[k1, v1, k2, v2, …]`.  The checker builds the lowlevel table value
    /// eagerly — keys must be deep-evaluated to hash them — and drops an
    /// entry whose key is not concrete (recording the error), per the table
    /// contract.
    Table(ChildRange),
    /// An array instance with `~`-marked positions — `[v1, ~ v2, ~2 v3]`.
    /// Typed like a tuple (per-element type slots — a homogeneous
    /// [`Self::Array`] type would reject `[x, ~ f(x+1)]` with an `Int` head
    /// and a `Stream` tail).  Elements stored in [`IR::children`],
    /// the per-element depths in [`IR::depths`] (0 = unmarked, `usize::MAX`
    /// = the bare `~`, n = the value slot shallow at the first n levels of
    /// the element's type spine).
    ShallowArray {
        range: ChildRange,
        depths: ChildRange,
    },
    /// `_` — an inference placeholder hole, usable in any position (type or
    /// value).  Compiles to a fresh undecided cell that binds to whatever the
    /// context unifies it with: `x : _`, `x : Int -> _`, `x : array<Int, _>`,
    /// `x : <Int, _>`, `struct<.f Int, .g _>`, and the value holes `_ : Int`,
    /// `f _`, `(1, _)`.
    Placeholder,
    /// A recovered-error region, masked at the frontend: an opaque leaf the
    /// checker *skips* (no cells, no unification, no cascade).  Distinct from
    /// [`ExprKind::Placeholder`] (a real `_` inference hole the context
    /// fills) so the frontend can identify an error region and exclude it
    /// from a content signature / diff.  The checker compiles it to a pair of
    /// fresh, never-unified cells, so it cannot emit a *type*-level
    /// "expected X, found Y" from inside itself; the parser's own *syntactic*
    /// diagnostic for the region still fires at the parse layer.
    ErrorBlock,
    /// The real array type `{ element_type, length }`.  Its type instance
    /// is the 2-element shape `[element_type, length]` — element 0 is the
    /// type shared by all elements, element 1 the length — kinded
    /// `[[element_type, length], [ArrayType, Type]]`.
    TypeArray {
        element_type: ExprId,
        length: ExprId,
    },
    /// A value imported from a package in the shared registry.  The IR only
    /// carries the exported pair node ref; the checker materializes it and
    /// extracts the value/type leaves (the payload itself stays in the
    /// package's static arena).
    Static {
        export: lichen_lowlevel::StaticNodeId,
    },
    /// `$name(args…)` — a call to a native operator registered by the compiling
    /// module's plugin.  `op` is a *private* name (an interned `&'static str`),
    /// resolved only against that module's registry
    /// ([`Checker::native_ops`](crate::checker::Checker)) — so two plugins each
    /// registering `$jit` never collide.  The checker compiles each argument
    /// (stored in [`IR::children`], like a tuple), delegates to the plugin's
    /// [`NativeOp`] builder, and adopts the `[value, type]` pair it returns; it
    /// has no knowledge of what the operator does.
    NativeCall { op: &'static str, args: ChildRange },
}

impl<L> ExprKind<L> {
    /// Every expression id this kind holds, in declaration order: the named
    /// fields *and* the variadic kinds' arena range, resolved to ids.
    ///
    /// The complete child list, which is what a walk over the IR needs —
    /// [`IR::range_children`] alone is the variadic arena only, so a descent
    /// built on it would skip every named operand (`Apply`, `BinOp`, `Index`,
    /// …) and miss most of the graph.  The non-variadic kinds are named rather
    /// than caught by a wildcard, for the same reason
    /// [`Self::repoint`] names them: a new kind that forgets to list its
    /// children here is a walk that silently skips it.
    ///
    /// A [`Self::Function`]'s `parent` is **not** an operand — it names the
    /// enclosing function's node, not an expression of this body — so it is
    /// left out, and the list is exactly the subtree the checker compiles.
    pub fn children(&self, ir: &IR<impl crate::attr::AttrSpec, L>) -> Vec<ExprId> {
        let mut out = Vec::new();
        let mut push = |id: ExprId| out.push(id);
        match self {
            ExprKind::Literal(_)
            | ExprKind::Parameter
            | ExprKind::Placeholder
            | ExprKind::ErrorBlock
            | ExprKind::Static { .. } => {}
            ExprKind::Function {
                parameter,
                parameter_type,
                parameter_attribute,
                r#return,
                ..
            } => {
                push(*parameter);
                push_opt(parameter_type, &mut push);
                push_opt(parameter_attribute, &mut push);
                push(*r#return);
            }
            ExprKind::Apply { function, argument } => {
                push(*function);
                push(*argument);
            }
            ExprKind::BinOp { left, right, .. } => {
                push(*left);
                push(*right);
            }
            ExprKind::Instantiate {
                type_expr, value, ..
            } => {
                push(*type_expr);
                push(*value);
            }
            ExprKind::Record { value, .. } => push(*value),
            ExprKind::Convert { value, .. } => push(*value),
            ExprKind::Assert { condition } => push(*condition),
            ExprKind::Index { array, index } => {
                push(*array);
                push(*index);
            }
            ExprKind::RawIndex { container, index } => {
                push(*container);
                push(*index);
            }
            ExprKind::Field { container, key } => {
                push(*container);
                push(*key);
            }
            ExprKind::NamedField { container, .. } | ExprKind::RawNamedField { container, .. } => {
                push(*container);
            }
            ExprKind::Find { container, key } => {
                push(*container);
                push(*key);
            }
            ExprKind::Annotation { value, r#type, .. } => {
                push(*value);
                push_opt(r#type, &mut push);
            }
            ExprKind::TypeFunction {
                parameter,
                r#return,
            } => {
                push(*parameter);
                push(*r#return);
            }
            ExprKind::TypeArray {
                element_type,
                length,
            } => {
                push(*element_type);
                push(*length);
            }
            ExprKind::Tuple(range)
            | ExprKind::TypeTuple(range)
            | ExprKind::Array(range)
            | ExprKind::Set(range)
            | ExprKind::Table(range)
            | ExprKind::ShallowArray { range, .. }
            | ExprKind::TypeStruct { fields: range, .. }
            | ExprKind::NativeCall { args: range, .. } => {
                out.extend_from_slice(&ir.children[range.start as usize..range.end as usize]);
            }
        }
        out
    }

    /// Replace every `from` reference in the kind's own fields with `to` —
    /// one half of [`IR::repoint`].  The variadic kinds hold ranges into the
    /// children arena, not ids; the arena is [`IR::repoint`]'s other half.
    pub fn repoint(&mut self, from: ExprId, to: ExprId) {
        let fix = |id: &mut ExprId| {
            if *id == from {
                *id = to;
            }
        };
        match self {
            ExprKind::Literal(_)
            | ExprKind::Parameter
            | ExprKind::Placeholder
            | ExprKind::ErrorBlock
            | ExprKind::Static { .. }
            | ExprKind::Tuple(_)
            | ExprKind::TypeTuple(_)
            | ExprKind::TypeStruct { .. }
            | ExprKind::Array(_)
            | ExprKind::Set(_)
            | ExprKind::Table(_)
            | ExprKind::ShallowArray { .. }
            | ExprKind::NativeCall { .. } => {}
            ExprKind::Function {
                parameter,
                parameter_type,
                parameter_attribute,
                r#return,
                parent,
                ..
            } => {
                fix(parameter);
                if let Some(parameter_type) = parameter_type {
                    fix(parameter_type);
                }
                if let Some(parameter_attribute) = parameter_attribute {
                    fix(parameter_attribute);
                }
                if let Some(parent) = parent {
                    fix(parent);
                }
                fix(r#return);
            }
            ExprKind::Apply { function, argument } => {
                fix(function);
                fix(argument);
            }
            ExprKind::BinOp { left, right, .. } => {
                fix(left);
                fix(right);
            }
            ExprKind::Instantiate {
                type_expr, value, ..
            } => {
                fix(type_expr);
                fix(value);
            }
            ExprKind::Record { value, .. } => fix(value),
            ExprKind::Assert { condition } => fix(condition),
            ExprKind::Convert { value, .. } => fix(value),
            ExprKind::Index { array, index } => {
                fix(array);
                fix(index);
            }
            ExprKind::RawIndex { container, index } => {
                fix(container);
                fix(index);
            }
            ExprKind::Field { container, key } => {
                fix(container);
                fix(key);
            }
            ExprKind::NamedField { container, .. } | ExprKind::RawNamedField { container, .. } => {
                fix(container);
            }
            ExprKind::Find { container, key } => {
                fix(container);
                fix(key);
            }
            ExprKind::Annotation { value, r#type, .. } => {
                fix(value);
                if let Some(r#type) = r#type {
                    fix(r#type);
                }
            }
            ExprKind::TypeFunction {
                parameter,
                r#return,
            } => {
                fix(parameter);
                fix(r#return);
            }
            ExprKind::TypeArray {
                element_type,
                length,
            } => {
                fix(element_type);
                fix(length);
            }
        }
    }
}

impl<A: AttrSpec, L> IR<A, L> {
    pub fn new() -> Self {
        IR {
            expr: Vec::new(),
            children: Vec::new(),
            struct_names: Vec::new(),
            depths: Vec::new(),
            root: ExprId(0),
            block_roots: HashSet::new(),
            stmt_roots: Vec::new(),
            schemas: Vec::new(),
            // Slot 0 is always the default (empty-tail) schema, so a fresh
            // `alloc` (which stamps `SchemaId(0)`) needs no write.
            schema_table: vec![Schema::default()],
        }
    }

    pub fn alloc(&mut self, kind: ExprKind<L>) -> ExprId {
        let id = ExprId(self.expr.len() as u32);
        self.expr.push(Expr { kind });
        self.schemas.push(SchemaId(0));
        id
    }

    /// Allocate an [`ExprKind::Annotation`] whose attribute value expressions
    /// are `attrs` — stored contiguously in [`IR::children`] and referenced as
    /// the node's `attributes` range, so the checker pairs each one with its
    /// schema-tail entry by position.
    pub fn alloc_annotation(
        &mut self,
        value: ExprId,
        r#type: Option<ExprId>,
        attrs: &[ExprId],
    ) -> ExprId {
        let attributes = extend_range(&mut self.children, attrs.iter().copied());
        self.alloc(ExprKind::Annotation {
            value,
            r#type,
            attributes,
        })
    }

    /// The attribute value expressions of an [`ExprKind::Annotation`] — aligned,
    /// by position, with the node's schema tail.
    pub fn annotation_attrs(&self, e: ExprId) -> &[ExprId] {
        let range = match self.expr[e.0 as usize].kind {
            ExprKind::Annotation { attributes, .. } => attributes,
            _ => unreachable!("annotation_attrs on a non-annotation expression"),
        };
        &self.children[range.start as usize..range.end as usize]
    }

    /// Intern a schema into the table (deduped) and return its id.
    pub fn intern_schema(&mut self, schema: Schema<A>) -> SchemaId {
        if let Some(pos) = self.schema_table.iter().position(|s| *s == schema) {
            SchemaId(pos as u32)
        } else {
            let id = self.schema_table.len();
            self.schema_table.push(schema);
            SchemaId(id as u32)
        }
    }

    /// Stamp an already-allocated node with a schema (interned).
    pub fn set_schema(&mut self, e: ExprId, schema: Schema<A>) {
        let id = self.intern_schema(schema);
        self.schemas[e.0 as usize] = id;
    }

    /// The schema of an expression.
    pub fn schema(&self, e: ExprId) -> &Schema<A> {
        &self.schema_table[self.schemas[e.0 as usize].0 as usize]
    }

    /// Stamp an already-allocated node with its real kind — the write half of
    /// the reserve-then-fill discipline a block-wide binding and a `Function`
    /// both use, where a node's id must exist before its own subtree compiles
    /// (a self/mutual reference, a nested closure's parent link) but its kind
    /// is only known after.
    pub fn set_kind(&mut self, e: ExprId, kind: ExprKind<L>) {
        self.expr[e.0 as usize].kind = kind;
    }

    pub fn alloc_tuple(&mut self, elements: &[ExprId]) -> ExprId {
        self.alloc_variadic(elements, ExprKind::Tuple)
    }

    pub fn alloc_type_tuple(&mut self, elements: &[ExprId]) -> ExprId {
        self.alloc_variadic(elements, ExprKind::TypeTuple)
    }

    pub fn alloc_type_struct(&mut self, fields: &[(ExprId, Option<&'static str>)]) -> ExprId {
        let field_range = extend_range(
            &mut self.children,
            fields.iter().map(|&(element, _)| element),
        );
        let name_range = extend_range(&mut self.struct_names, fields.iter().map(|&(_, name)| name));
        self.alloc(ExprKind::TypeStruct {
            fields: field_range,
            names: name_range,
        })
    }

    pub fn alloc_instantiate(
        &mut self,
        type_expr: ExprId,
        value: ExprId,
        names: &[Option<&'static str>],
    ) -> ExprId {
        let name_range = extend_range(&mut self.struct_names, names.iter().copied());
        self.alloc(ExprKind::Instantiate {
            type_expr,
            value,
            names: name_range,
        })
    }

    pub fn alloc_record(&mut self, value: ExprId, names: &[Option<&'static str>]) -> ExprId {
        let name_range = extend_range(&mut self.struct_names, names.iter().copied());
        self.alloc(ExprKind::Record {
            value,
            names: name_range,
        })
    }

    pub fn alloc_array(&mut self, elements: &[ExprId]) -> ExprId {
        self.alloc_variadic(elements, ExprKind::Array)
    }

    /// Allocate a set literal: the members are variadic children like an
    /// array's elements; the kind alone decides the type the checker gives it.
    pub fn alloc_set(&mut self, members: &[ExprId]) -> ExprId {
        self.alloc_variadic(members, ExprKind::Set)
    }

    /// Allocate a constant table literal: each `(key, value)` pair is
    /// flattened into the children arena (interleaved, entry by entry), and
    /// the expression is an [`ExprKind::Table`] over that range.
    pub fn alloc_table(&mut self, entries: &[(ExprId, ExprId)]) -> ExprId {
        let range = extend_range(
            &mut self.children,
            entries.iter().flat_map(|&(key, value)| [key, value]),
        );
        self.alloc(ExprKind::Table(range))
    }

    /// Allocate a shallow-marked array: each `(element, depth)` pair carries
    /// the element's `~` depth (0 = unmarked, `usize::MAX` = bare `~`).
    pub fn alloc_shallow_array(&mut self, elements: &[(ExprId, usize)]) -> ExprId {
        let range = extend_range(
            &mut self.children,
            elements.iter().map(|&(element, _)| element),
        );
        let depths = extend_range(&mut self.depths, elements.iter().map(|&(_, depth)| depth));
        self.alloc(ExprKind::ShallowArray { range, depths })
    }

    fn alloc_variadic(
        &mut self,
        elements: &[ExprId],
        make: fn(ChildRange) -> ExprKind<L>,
    ) -> ExprId {
        let range = extend_range(&mut self.children, elements.iter().copied());
        self.alloc(make(range))
    }

    /// The children of a **variadic** expression (`Tuple`, `TypeTuple`,
    /// `Array`, `TypeStruct`, `ShallowArray`, `Table`, `NativeCall`).
    ///
    /// The kinds that store their children in named fields rather than an
    /// arena range are named rather than caught by a wildcard: this is the
    /// *open* end of the encoding — a new kind that stores its children as a
    /// [`ChildRange`] must be added to the range arm, and a wildcard would let
    /// it compile and then panic at run time.  One reads as no children (not
    /// an `unreachable!`) so a caller that only ever passes a variadic kind
    /// needs no knowledge of which kinds exist; a non-variadic kind reaching
    /// here is a caller's mistake, and `debug_assert` says so.
    ///
    /// **The checker's own `range_children` is the authority, not this.** It
    /// already spelled this list out — including `Set`, which the first draft
    /// of this function left out, which is the drift a second list invites.
    /// This is kept as the *closure* end: a caller's mistake reads as a
    /// `debug_assert` naming itself rather than a wrong child list.
    pub fn range_children(&self, e: ExprId) -> Vec<ExprId> {
        let range = match self.expr[e.0 as usize].kind {
            ExprKind::Tuple(range)
            | ExprKind::TypeTuple(range)
            | ExprKind::Array(range)
            | ExprKind::Set(range)
            | ExprKind::ShallowArray { range, .. }
            | ExprKind::Table(range) => range,
            ExprKind::TypeStruct { fields, .. } => fields,
            ExprKind::NativeCall { args, .. } => args,
            _ => {
                debug_assert!(
                    false,
                    "range_children on a kind whose children are named fields, not an arena range"
                );
                return Vec::new();
            }
        };
        self.children[range.start as usize..range.end as usize].to_vec()
    }

    /// The children of `e` — every kind, named fields and arena alike.  See
    /// [`ExprKind::children`], which is the authority this defers to.
    pub fn children(&self, e: ExprId) -> Vec<ExprId> {
        self.expr[e.0 as usize].kind.children(self)
    }

    pub fn set_root(&mut self, root: ExprId) {
        self.root = root;
    }

    /// Record the top-level statement expression ids, in source order.  The
    /// build's cascade deep pass computes each one's value/type; a reader
    /// (the language server) reads them by id instead of re-deriving them from
    /// the root, and never re-evaluates.
    pub fn set_stmt_roots(&mut self, stmt_roots: Vec<ExprId>) {
        self.stmt_roots = stmt_roots;
    }

    /// Re-point every stored occurrence of `from` to `to`: the kind fields of
    /// every expression, the variadic children arena, the root, and the
    /// block-root set.  The frontend's alias fixup: a block-wide binding whose
    /// value is a bare name (`c = b`) aliases its target, so the uses that
    /// captured the binding's reserved placeholder (compiled before the alias
    /// statement) are re-pointed to the aliased node — the placeholder keeps
    /// no references and leaves the block-root set.  The alias target is a
    /// block root already (a forward alias is another binding's placeholder)
    /// or a `let`/statement value no new cycle can form through.
    ///
    /// No-op when `from == to` (the degenerate self-alias `a = a`): the
    /// placeholder must stay referenced and block-rooted.
    pub fn repoint(&mut self, from: ExprId, to: ExprId) {
        if from == to {
            return;
        }
        for expr in &mut self.expr {
            expr.kind.repoint(from, to);
        }
        for child in &mut self.children {
            if *child == from {
                *child = to;
            }
        }
        if self.root == from {
            self.root = to;
        }
        self.block_roots.remove(&from);
    }
}

impl<A: AttrSpec, L> Default for IR<A, L> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A, L> std::ops::Index<ExprId> for IR<A, L> {
    type Output = Expr<L>;
    fn index(&self, id: ExprId) -> &Expr<L> {
        &self.expr[id.0 as usize]
    }
}
