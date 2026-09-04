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
use crate::program::HighProgramLiteral;

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

impl<A> Schema<A> {
    /// The runtime pair's arity: 2 (value, type) + one slot per attribute.
    pub fn arity(&self) -> usize {
        self.tail.len() + 2
    }
}

/// An interned index into [`IR::schema_table`].  `0` is always the default
/// (empty-`tail`) schema, so a fresh [`IR::alloc`] needs no write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SchemaId(pub u32);

/// A binary operation on integers.  The arithmetic ops (`Add`, `Sub`) yield
/// their result; the comparisons (`Leq`, `Eq`) yield `USize(0/1)` so the
/// result can drive the lazy `Index` branch of an `if` — there is no
/// `Bool` value in the universe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Leq,
    Eq,
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

/// A source span, supplied by the language frontend: `(line, column)`.
pub type Span = (u32, u32);

/// A source-blind diagnostic location: the IR expression a check is about,
/// plus a single **recursive** descent path through its `[value, type, …]`
/// spine.
///
/// The highlevel is deliberately source-blind — it never sees a [`Span`]
/// (the source↔IR mapping is the frontend's own record), so a location must
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

/// The coarse category that begins a [`Loc`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocKind {
    /// The expression's value slot (leading [`LocStep::Value`]).
    Value,
    /// The expression's type slot, or a further link of the type chain
    /// (leading [`LocStep::Type`]).
    Type,
    /// The expression's attribute-tail slot (leading [`LocStep::Attr`]).
    Attribute,
}

impl Loc {
    /// The coarse category the location denotes.
    pub fn kind(&self) -> LocKind {
        match self.path.first() {
            None | Some(LocStep::Value) => LocKind::Value,
            Some(LocStep::Type) => LocKind::Type,
            Some(LocStep::Attr(_)) => LocKind::Attribute,
            // A shape / element only appears deeper in a type's chain; the
            // leading slot is still the type of the surrounding pair.
            Some(LocStep::Shape) | Some(LocStep::Elem(_)) => LocKind::Type,
        }
    }

    /// How many `[value, type]` pairings the location descends: how many
    /// leading [`LocStep::Type`]s (the first is the type, the second is the
    /// type's type, and so on, unbounded).
    pub fn type_depth(&self) -> usize {
        self.path.iter().take_while(|s| **s == LocStep::Type).count()
    }
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
    /// index-typed apply would stay an unbound `?a`).
    pub block_roots: HashSet<ExprId>,
    /// The per-expression static schema, index-aligned with [`IR::expr`] — one
    /// [`SchemaId`] each.  `alloc` stamps the default (empty-`tail`) schema.
    pub schemas: Vec<SchemaId>,
    /// The interned schema table (see [`Schema`]); [`SchemaId`]s index it.
    pub schema_table: Vec<Schema<A>>,
}

#[derive(Clone, Copy, Debug)]
pub struct Expr<L> {
    pub kind: ExprKind<L>,
    pub span: Option<Span>,
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
    /// `depth` is the count of enclosing function scopes at declaration (0
    /// for a top-level function).  The checker uses it to keep sibling
    /// functions' template scopes disjoint while absorbing truly-nested
    /// closures into their parent's template.
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
        depth: u32,
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
    /// `{ type_expr, value }` — struct instantiation: `s(1, 2)` wraps the
    /// positional tuple `value` in the struct type `type_expr`.  The
    /// value's element types are checked against the struct's field list,
    /// and the expression's type is the struct type itself.  Emitted by the
    /// frontend when an application's callee is a struct type.
    Instantiate { type_expr: ExprId, value: ExprId },
    /// `assert(condition)` — an explicit constraint, not a unify: the
    /// condition's value node is registered as an assert point.  The
    /// checker force-evaluates every assert after the definition pass
    /// (ignoring laziness) and requires `USize(1)`; a condition that stays
    /// lazy (an unbound parameter) is not triggered, and the apply clone
    /// re-checks the instantiated condition per call.  The expression
    /// compiles to the condition itself — the assert is a side constraint.
    Assert { condition: ExprId },
    /// `{ array, index }` — an element read `a[i]`; the container is *pinned*
    /// to an array type (its element type is the pinned shape's element cell
    /// and the read registers an `i < length` bounds assert), so this form
    /// never kind-dispatches.
    Index { array: ExprId, index: ExprId },
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
    /// `{ container, key }` — a table lookup `t{k}`: the entry whose stored
    /// key is deep-content-equal to `k`.  The frontend emits it for the
    /// *adjacent* brace form — the syntactic distinction from positional
    /// [`Self::Index`] — so the checker compiles the read to the dedicated
    /// lowlevel `TableGet` directly, never a kind dispatch.
    Find { container: ExprId, key: ExprId },
    /// `{ value, type?, attribute? }` — a type and/or attribute annotation.
    /// `: T` fills `r#type` (existing), `# p` fills `attribute` (new).  Either
    /// may be absent (`e # p`, `e : T`); at most one each.  A `# p` on a term
    /// also stamps the annotated node's schema with the attribute tail — the
    /// one asymmetry with `:` (the slot comes into existence by being
    /// annotated).
    Annotation {
        value: ExprId,
        r#type: Option<ExprId>,
        attribute: Option<ExprId>,
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
    /// A struct type expression `[T1, ..., Tn]` — the field types
    /// (positional, no names in v1), kinded with a fixed `TypeStruct` marker
    /// and shaped `[TypeId(n), [T1, ..., Tn]]`: a *fresh nominal* id bundled
    /// with the field-type list (mirroring an array type's `[element type,
    /// length]` shape).  Each occurrence's `Fresh` call allocates a new id,
    /// so two occurrences never unify at the value level; a struct type is
    /// reused by binding it once through a parameter.  Elements stored in
    /// [`IR::children`].
    TypeStruct(ChildRange),
    /// An array instance `[v1, ..., vn]` — every element shares one type
    /// (unlike a [`Self::Tuple`]'s per-element slots).  Elements stored in
    /// [`IR::children`].
    Array(ChildRange),
    /// A constant table instance `table { k1 :: v1, k2 :: v2, … }` — every
    /// key shares one key type and every value one value type (checked
    /// against two shared cells, like an array's single element cell).  The
    /// entries are stored interleaved in [`IR::children`]:
    /// `[k1, v1, k2, v2, …]`.  The checker builds the lowlevel table value
    /// eagerly — keys must be force-evaluated to hash them — and drops an
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
    /// `_` — an inferrable type position.  Compiles to a fresh unbound cell
    /// that binds to whatever the context unifies it with: `x : _`,
    /// `x : Int -> _`, `x : Int<_>`, `x : <Int, _>`, `struct<Int, _>`.
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

impl<A: AttrSpec, L> IR<A, L> {
    pub fn new() -> Self {
        IR {
            expr: Vec::new(),
            children: Vec::new(),
            depths: Vec::new(),
            root: ExprId(0),
            block_roots: HashSet::new(),
            schemas: Vec::new(),
            // Slot 0 is always the default (empty-tail) schema, so a fresh
            // `alloc` (which stamps `SchemaId(0)`) needs no write.
            schema_table: vec![Schema::default()],
        }
    }

    pub fn alloc(&mut self, kind: ExprKind<L>, span: Option<Span>) -> ExprId {
        let id = ExprId(self.expr.len() as u32);
        self.expr.push(Expr { kind, span });
        self.schemas.push(SchemaId(0));
        id
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

    pub fn alloc_tuple(&mut self, elements: &[ExprId], span: Option<Span>) -> ExprId {
        self.alloc_variadic(elements, ExprKind::Tuple, span)
    }

    pub fn alloc_type_tuple(&mut self, elements: &[ExprId], span: Option<Span>) -> ExprId {
        self.alloc_variadic(elements, ExprKind::TypeTuple, span)
    }

    pub fn alloc_type_struct(&mut self, elements: &[ExprId], span: Option<Span>) -> ExprId {
        self.alloc_variadic(elements, ExprKind::TypeStruct, span)
    }

    pub fn alloc_instantiate(
        &mut self,
        type_expr: ExprId,
        value: ExprId,
        span: Option<Span>,
    ) -> ExprId {
        self.alloc(ExprKind::Instantiate { type_expr, value }, span)
    }

    pub fn alloc_array(&mut self, elements: &[ExprId], span: Option<Span>) -> ExprId {
        self.alloc_variadic(elements, ExprKind::Array, span)
    }

    /// Allocate a constant table literal: each `(key, value)` pair is
    /// flattened into the children arena (interleaved, entry by entry), and
    /// the expression is an [`ExprKind::Table`] over that range.
    pub fn alloc_table(&mut self, entries: &[(ExprId, ExprId)], span: Option<Span>) -> ExprId {
        let start = self.children.len() as u32;
        for &(key, value) in entries {
            self.children.push(key);
            self.children.push(value);
        }
        let range = ChildRange {
            start,
            end: self.children.len() as u32,
        };
        self.alloc(ExprKind::Table(range), span)
    }

    /// Allocate a shallow-marked array: each `(element, depth)` pair carries
    /// the element's `~` depth (0 = unmarked, `usize::MAX` = bare `~`).
    pub fn alloc_shallow_array(
        &mut self,
        elements: &[(ExprId, usize)],
        span: Option<Span>,
    ) -> ExprId {
        let start = self.children.len() as u32;
        let dstart = self.depths.len() as u32;
        for &(element, depth) in elements {
            self.children.push(element);
            self.depths.push(depth);
        }
        let range = ChildRange {
            start,
            end: self.children.len() as u32,
        };
        let depths = ChildRange {
            start: dstart,
            end: self.depths.len() as u32,
        };
        self.alloc(ExprKind::ShallowArray { range, depths }, span)
    }

    fn alloc_variadic(
        &mut self,
        elements: &[ExprId],
        make: fn(ChildRange) -> ExprKind<L>,
        span: Option<Span>,
    ) -> ExprId {
        let start = self.children.len() as u32;
        self.children.extend_from_slice(elements);
        let range = ChildRange {
            start,
            end: self.children.len() as u32,
        };
        self.alloc(make(range), span)
    }

    pub fn set_root(&mut self, root: ExprId) {
        self.root = root;
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

impl std::ops::Index<ExprId> for Vec<Option<lichen_lowlevel::NodeId>> {
    type Output = Option<lichen_lowlevel::NodeId>;
    fn index(&self, id: ExprId) -> &Option<lichen_lowlevel::NodeId> {
        &self[id.0 as usize]
    }
}

impl std::ops::IndexMut<ExprId> for Vec<Option<lichen_lowlevel::NodeId>> {
    fn index_mut(&mut self, id: ExprId) -> &mut Option<lichen_lowlevel::NodeId> {
        &mut self[id.0 as usize]
    }
}
