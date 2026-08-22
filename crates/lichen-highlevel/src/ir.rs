//! The highlevel expression IR: a dense, id-referenced tree built once by
//! the language frontend and walked by the checker.
//!
//! Not slotmap-shaped: the IR never changes structurally, never runs, and is
//! never GC'd, so a plain [`Vec`] with [`ExprId`] indices suffices.  The
//! checker only reads it (its products — pairs, type cells — are lowlevel
//! nodes, so the table does not even grow).  One exception: a recursive
//! binding's id is reserved *before* its lambda compiles (the body references
//! the id being defined), so its [`Expr`] kind is filled in afterwards — the
//! frontend records the id in [`IR::recursive`] and the checker reads that.

/// A constant leaf value: an int literal or one of the type constants.  This
/// is the frontend's closed vocabulary of constants — the subset of the
/// lowlevel value type a program may embed directly.  The other values
/// (`Array`, `Function`, `None`, `Parameterized`) are built by other
/// expression kinds, never constants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Constant {
    /// An int literal.
    USize(usize),
    /// The `int` type constant.
    TypeInt,
    /// The `Type` constant — the canonical universe node itself (`Type : Type`).
    TypeType,
    /// The kind marker of function type expressions.
    TypeFunction,
    /// The kind marker of tuple type expressions.
    TypeTuple,
    /// The kind marker of array type expressions.
    TypeArray,
}

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

/// The highlevel program: a pure expression tree.
#[derive(Clone, Debug)]
pub struct IR {
    pub expr: Vec<Expr>,
    /// One dense arena for all variadic children lists ([`ExprKind::Tuple`],
    /// [`ExprKind::TypeTuple`], [`ExprKind::Array`], [`ExprKind::TypeStruct`]).
    pub children: Vec<ExprId>,
    pub root: ExprId,
    /// The ids of recursive bindings' lambdas (`rec fib = …`): a function
    /// whose own name is in scope in its body, so its `ExprId` appears in
    /// its own subtree — the IR is a cycle there, which the checker cuts by
    /// registering the function's pair before the body compiles.
    pub recursive: std::collections::HashSet<ExprId>,
}

#[derive(Clone, Copy, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Option<Span>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExprKind {
    /// A constant leaf value — see [`Constant`].
    Constant(Constant),
    /// A function parameter (or, `let` desugared by the frontend, a let-bound
    /// name).  Uses of the parameter in the return expression are the
    /// parameter's own `ExprId`.
    Parameter,
    /// `{ parameter, return }` — the parameter is a [`ExprKind::Parameter`].
    Function {
        parameter: ExprId,
        r#return: ExprId,
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
    /// `{ array, index }` — element selection; `array` must be a tuple or
    /// array value, `index` a `USize`.
    Index { array: ExprId, index: ExprId },
    /// `{ value, type }` — the value's type must unify with the type expression.
    Annotation {
        value: ExprId,
        r#type: ExprId,
    },
    /// `{ parameter, return }` — a function type, compiled to the kinded
    /// arrow `[[in, out], [FunctionType, Type]]`.
    TypeFunction {
        parameter: ExprId,
        r#return: ExprId,
    },
    /// A tuple instance `[v1, ..., vn]` — one type slot per element, so the
    /// elements may be heterogeneous.  Elements stored in
    /// [`ExprTable::children`].
    Tuple(ChildRange),
    /// A tuple type expression `[T1, ..., Tn]` — the element types, kinded
    /// `[[T1, ..., Tn], [TupleType, Type]]`.  Elements stored in
    /// [`ExprTable::children`].
    TypeTuple(ChildRange),
    /// A struct type expression `[T1, ..., Tn]` — the field types
    /// (positional, no names in v1), kinded with a *fresh nominal* id:
    /// `[[T1, ..., Tn], [TypeId(n), Type]]`.  Each occurrence's `Fresh`
    /// call allocates a new id, so two occurrences never unify; a struct
    /// type is reused by binding it once through a parameter.  Elements
    /// stored in [`ExprTable::children`].
    TypeStruct(ChildRange),
    /// An array instance `[v1, ..., vn]` — every element shares one type
    /// (unlike a [`Self::Tuple`]'s per-element slots).  Elements stored in
    /// [`ExprTable::children`].
    Array(ChildRange),
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

impl IR {
    pub fn new() -> Self {
        IR {
            expr: Vec::new(),
            children: Vec::new(),
            root: ExprId(0),
            recursive: std::collections::HashSet::new(),
        }
    }

    pub fn alloc(&mut self, kind: ExprKind, span: Option<Span>) -> ExprId {
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

    fn alloc_variadic(
        &mut self,
        elements: &[ExprId],
        make: fn(ChildRange) -> ExprKind,
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

impl Default for IR {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Index<ExprId> for IR {
    type Output = Expr;
    fn index(&self, id: ExprId) -> &Expr {
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
