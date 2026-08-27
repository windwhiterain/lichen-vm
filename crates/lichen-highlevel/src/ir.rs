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

use crate::program::HighProgramValue;

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

/// A dense index into [`ExprTable::expr`].  References are pre-resolved: a
/// use of a parameter *is* the [`ExprKind::Parameter`]'s own `ExprId` (the
/// checker's scope stack is keyed by it), so the IR carries no name strings.
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

/// The highlevel program: a pure expression tree, generic over the value
/// vocabulary it embeds (defaults to the highlevel's own [`HighProgramValue`]).
#[derive(Clone, Debug)]
pub struct IR<V = HighProgramValue> {
    pub expr: Vec<Expr<V>>,
    /// One dense arena for all variadic children lists ([`ExprKind::Tuple`],
    /// [`ExprKind::TypeTuple`], [`ExprKind::Array`], [`ExprKind::TypeStruct`],
    /// [`ExprKind::ShallowArray`]).
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
}

#[derive(Clone, Copy, Debug)]
pub struct Expr<V> {
    pub kind: ExprKind<V>,
    pub span: Option<Span>,
}

/// The expression kinds.  Generic over the constant vocabulary `V`: a
/// constant leaf is a value of the program's union directly (an int literal
/// or one of the type constants — the other values are built by other
/// expression kinds, never constants).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ExprKind<V> {
    /// A constant leaf value — an int literal or a type constant.
    Constant(V),
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
    /// `{ array, index }` — element selection; `array` must be a tuple or
    /// array value, `index` a `USize`.
    Index { array: ExprId, index: ExprId },
    /// `{ value, type }` — the value's type must unify with the type expression.
    Annotation { value: ExprId, r#type: ExprId },
    /// `{ parameter, return }` — a function type, compiled to the kinded
    /// arrow `[[in, out], [FunctionType, Type]]`.
    TypeFunction { parameter: ExprId, r#return: ExprId },
    /// A tuple instance `[v1, ..., vn]` — one type slot per element, so the
    /// elements may be heterogeneous.  Elements stored in
    /// [`ExprTable::children`].
    Tuple(ChildRange),
    /// A tuple type expression `[T1, ..., Tn]` — the element types, kinded
    /// `[[T1, ..., Tn], [TupleType, Type]]`.  Elements stored in
    /// [`ExprTable::children`].
    TypeTuple(ChildRange),
    /// A struct type expression `[T1, ..., Tn]` — the field types
    /// (positional, no names in v1), kinded with a fixed `TypeStruct` marker
    /// and shaped `[TypeId(n), [T1, ..., Tn]]`: a *fresh nominal* id bundled
    /// with the field-type list (mirroring an array type's `[element type,
    /// length]` shape).  Each occurrence's `Fresh` call allocates a new id,
    /// so two occurrences never unify at the value level; a struct type is
    /// reused by binding it once through a parameter.  Elements stored in
    /// [`ExprTable::children`].
    TypeStruct(ChildRange),
    /// An array instance `[v1, ..., vn]` — every element shares one type
    /// (unlike a [`Self::Tuple`]'s per-element slots).  Elements stored in
    /// [`ExprTable::children`].
    Array(ChildRange),
    /// An array instance with `~`-marked positions — `[v1, ~ v2, ~2 v3]`.
    /// Typed like a tuple (per-element type slots — a homogeneous
    /// [`Self::Array`] type would reject `[x, ~ f(x+1)]` with an `Int` head
    /// and a `Stream` tail).  Elements stored in [`ExprTable::children`],
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
    /// The real array type `{ element_type, length }`.  Its type instance
    /// is the 2-element shape `[element_type, length]` — element 0 is the
    /// type shared by all elements, element 1 the length — kinded
    /// `[[element_type, length], [ArrayType, Type]]`.
    TypeArray {
        element_type: ExprId,
        length: ExprId,
    },
}

impl<V> IR<V> {
    pub fn new() -> Self {
        IR {
            expr: Vec::new(),
            children: Vec::new(),
            depths: Vec::new(),
            root: ExprId(0),
            block_roots: HashSet::new(),
        }
    }

    pub fn alloc(&mut self, kind: ExprKind<V>, span: Option<Span>) -> ExprId {
        let id = ExprId(self.expr.len() as u32);
        self.expr.push(Expr { kind, span });
        id
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
        make: fn(ChildRange) -> ExprKind<V>,
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

impl<V> Default for IR<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> std::ops::Index<ExprId> for IR<V> {
    type Output = Expr<V>;
    fn index(&self, id: ExprId) -> &Expr<V> {
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
