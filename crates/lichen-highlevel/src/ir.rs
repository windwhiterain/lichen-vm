//! The highlevel expression IR: a dense, id-referenced tree. See
//! docs/language-spec.md.
//!
//! # Invariant
//! The frontend builds it and the checker only reads it: the IR never changes
//! structurally, never runs and is never GC'd, so a plain `Vec` of [`ExprId`]
//! indices suffices. A `Checker` holds it as an `Arc<IR>`, which makes that
//! read-only-ness compiler-checked.

use std::collections::HashSet;

use crate::attr::{AttrSpec, NoAttr};
use crate::program::{HighProgramLiteral, TypeOperator};

/// The static schema of an expression: which attributes ride on its runtime
/// pair. See `docs/notes/attributes.md`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Schema<A> {
    pub tail: Vec<A>,
}

impl<A> Default for Schema<A> {
    fn default() -> Self {
        Schema { tail: Vec::new() }
    }
}

/// An interned index into [`IR::schema_table`]; `0` is the default schema, so
/// a fresh [`IR::alloc`] needs no write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SchemaId(pub u32);

/// A binary operation. Comparisons and `In` yield `USize(0/1)`, which drives
    /// an `if`. See docs/language-spec.md.
///
/// # Invariant
/// Everything but `Eq`/`Neq`/`In` is `Int`-only and unsigned: `Eq`/`Neq` are the
/// generalized equality, and `In` is the structural membership predicate
/// `value @in set`.
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

/// The [`TypeOperator`] each [`BinOp`] runs; `Fresh` mints a nominal struct id
/// and has no [`BinOp`] spelling.
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

/// A prefix **class conversion**: `int2float e` and `float2int e`. See
    /// `docs/notes/floating-point.md` §4.2.
///
/// # Invariant
/// These are the only place `Int` and `Float` meet: every other operator
/// computes within one class. `float2int` has no answer for a `NaN` or a
/// magnitude past the machine integer ([`crate::program::OUT_OF_RANGE`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConvOp {
    Int2Float,
    Float2Int,
}

/// The [`TypeOperator`] each [`ConvOp`] runs, as [`BinOp`] does.
impl From<ConvOp> for TypeOperator {
    fn from(operator: ConvOp) -> Self {
        match operator {
            ConvOp::Int2Float => TypeOperator::Int2Float,
            ConvOp::Float2Int => TypeOperator::Float2Int,
        }
    }
}

/// A dense index into [`IR::expr`].
    ///
/// # Invariant
/// References are pre-resolved: a use of a parameter *is* the
/// [`ExprKind::Parameter`]'s own `ExprId`, so the IR carries no name strings.
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
    /// occupy.
///
/// # Invariant
/// Every variadic arena write goes through here, so a range and the push it
/// describes cannot disagree.
fn extend_range<T>(arena: &mut Vec<T>, values: impl IntoIterator<Item = T>) -> ChildRange {
    let start = arena.len() as u32;
    arena.extend(values);
    ChildRange {
        start,
        end: arena.len() as u32,
    }
}

/// A source-blind diagnostic location: an IR expression and a **recursive**
    /// path through its `[value, type, …]` spine.
///
/// # Invariant
/// The highlevel never sees a source span, so a location is expressed purely
/// in terms of the expression's structure. There is no distinct "kind":
/// `LocStep::Type` is the type's type and may repeat arbitrarily.
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

/// The highlevel program: a pure expression tree, generic over the attribute
/// type `A` and the literal vocabulary `L`.
#[derive(Clone, Debug)]
pub struct IR<A = NoAttr, L = HighProgramLiteral> {
    pub expr: Vec<Expr<L>>,
    /// One dense arena for all variadic children lists.
    pub children: Vec<ExprId>,
    /// One dense arena for struct **field names**; `None` for a positional entry.
    ///
    /// # Invariant
    /// It is index-aligned with both [`ExprKind::TypeStruct`]'s `fields` range and
    /// [`ExprKind::Instantiate`]'s value tuple elements.
    pub struct_names: Vec<Option<&'static str>>,
    /// One dense arena for the `~` depths of [`ExprKind::ShallowArray`].
    ///
    /// # Invariant
    /// One `usize` per element: 0 unmarked, `usize::MAX` the bare `~`, n the value
    /// slot at the first n levels of the element's type spine.
    pub depths: Vec<usize>,
    pub root: ExprId,
    /// The block-wide binding placeholder ids — the only `ExprId`s whose subtree
    /// can reference themselves.
    ///
    /// # Invariant
    /// The checker pre-registers a cycle-cut skeleton only for these: an inline
    /// compound term can never cycle, and its skeleton's extra cells would
    /// otherwise poison the apply-time unify.
    pub block_roots: HashSet<ExprId>,
    /// The top-level statement expression ids, in source order, NOT including the
    /// final expression.
    ///
    /// # Invariant
    /// The build's cascade deep pass already computed each one's value/type, so a
    /// reader takes them by id and never re-evaluates — a lazy or recursive
    /// binding stays undecided rather than forced.
    pub stmt_roots: Vec<ExprId>,
    /// The per-expression static schema, index-aligned with [`IR::expr`] — one
    /// [`SchemaId`] each, default-stamped by `alloc`.
    pub schemas: Vec<SchemaId>,
    /// The interned schema table (see [`Schema`]); [`SchemaId`]s index it.
    pub schema_table: Vec<Schema<A>>,
}

#[derive(Clone, Copy, Debug)]
pub struct Expr<L> {
    pub kind: ExprKind<L>,
}

/// The expression kinds. Generic over the literal vocabulary `L`; the
/// built-in leaf literal wraps a raw value token.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ExprKind<L> {
    /// A literal leaf — a `value : type` pair declared together.
    ///
    /// # Invariant
    /// The literal builds its value and type nodes through the curated
    /// [`Ctx`](crate::program::Ctx), never raw lowlevel nodes.
    Literal(L),
    /// A function parameter, or a `let`-bound name.
    Parameter,
    /// `{ parameter, return }` — the parameter is a [`ExprKind::Parameter`].
    ///
    /// # Invariant
    /// `parent` is the enclosing function's IR node — it keeps sibling template
    /// scopes disjoint while absorbing nested closures into the parent's, and
    /// rides here because the node is allocated after its body. `None` for a
    /// top-level function.
    Function {
        parameter: ExprId,
        /// The annotated parameter's type (`x : T => e`), compiled *in body scope*.
        ///
        /// # Invariant
        /// The parameter's type slot still performs the argument check at each apply,
        /// so compiling in body scope changes what the body sees, not what is checked.
        parameter_type: Option<ExprId>,
        /// The annotated parameter's attribute (`x # n => e`), compiled in body
        /// scope like `parameter_type`.
        ///
        /// # Invariant
        /// This is the optimization for the `x # n => e` → `x => { x # n; e }` desugar:
        /// an unannotated body statement would otherwise materialize a block.
        parameter_attribute: Option<ExprId>,
        r#return: ExprId,
        parent: Option<ExprId>,
        /// The `@loop` marker, carried from the source binding onto the `Function` node
        /// its value compiled to.
        ///
        /// # Invariant
        /// The binding's node **is** its value's node, so the mark and the function
        /// are the same fact. The marker is **permission, not a command**: recursion
        /// *may* become a loop and the default is still the unroll whenever the trip
        /// count is decidable (`docs/notes/loop-conversion.md` §1.1).
        looping: bool,
    },
    /// `{ function, argument }`.
    Apply { function: ExprId, argument: ExprId },
    /// `{ operator, left, right }` — a binary integer operation.
    ///
    /// # Invariant
    /// The operand types are checked against `Int` and the result's type is `Int`:
    /// a comparison's `0/1` drives an `if` branch.
    BinOp {
        operator: BinOp,
        left: ExprId,
        right: ExprId,
    },
    /// `{ type_expr, value, names }` — struct instantiation `s(1, 2)`.
    ///
    /// # Invariant
    /// `names` is index-aligned with `value`'s tuple elements, so the checker can
    /// reorder them to the definition's order against the struct type's name table.
    /// The frontend recognises the glued comma-disciplined paren (`C()`, `C(,)`)
    /// syntactically; the checker still validates that the callee is a struct kind.
    Instantiate {
        type_expr: ExprId,
        value: ExprId,
        names: ChildRange,
    },
    /// `{ value, names }` — a struct-returning block (`RecordBlock`).
    ///
    /// # Invariant
    /// The checker builds an anonymous struct type from the value's element types,
    /// so the block's value is a struct instance reading positionally or by name.
    Record { value: ExprId, names: ChildRange },
    /// `assert(condition)` — a constraint, not a unify; the expression compiles to
    /// the condition itself.
    ///
    /// # Invariant
    /// The checker registers the condition as an assert point, deep-evaluates it
    /// after the definition pass and requires `USize(1)`. A condition that stays
    /// undecided is pending, not failed, and the apply clone re-checks the
    /// instantiated condition per call.
    Assert { condition: ExprId },
    /// `int2float e` / `float2int e` — a **class conversion**.
    ///
    /// # Invariant
    /// The checker pins the operand to the direction's source class and gives the
    /// expression the target class, so `int2float x` on a float `x` is a check
    /// error rather than a no-op.
    Convert { operator: ConvOp, value: ExprId },
    /// `{ array, index }` — an element read `a[i]`, the container *pinned* to an
    /// array type.
    ///
    /// # Invariant
    /// The read registers an `i < length` bounds assert and never kind-dispatches.
    Index { array: ExprId, index: ExprId },
    /// `{ container, index }` — a *raw* positional read `X<e>`, read without type
    /// validation.
    ///
    /// # Invariant
    /// No array-type pinning, no guard and no bounds assert: the container is read
    /// by value whatever its type, and the result is the element's own pair.
    RawIndex { container: ExprId, index: ExprId },
    /// `{ container, key }` — a positional slot read `a(k)` over a tuple element
    /// or struct field.
    ///
    /// # Invariant
    /// The type is `Index(shape, k)` evaluated lazily, so an untyped parameter
    /// resolves at the call. The frontend emits it for the *adjacent* paren form
    /// `a(1)`, distinct from instantiation (`a(1,)`) and application (`a (1)`).
    Field { container: ExprId, key: ExprId },
    /// `{ container, name }` — a *named* field read `a.name`.
    ///
    /// # Invariant
    /// `name` is an interned `&'static str`; the checker resolves it against the
    /// container type's struct name table, then reads like [`Self::Field`].
    NamedField {
        container: ExprId,
        name: &'static str,
    },
    /// `{ container, name }` — a *raw* named component read `X::a`, yielding the
    /// field's *type* as a value.
    ///
    /// # Invariant
    /// Unlike [`Self::NamedField`], this reads the name table straight from the
    /// container's **type**, which must be a TypeStruct (`container_ty[0][1]`) —
    /// a check-time unify, so it is not raw in the sense of [`Self::RawIndex`].
    RawNamedField {
        container: ExprId,
        name: &'static str,
    },
    /// `{ container, key }` — a table lookup `t{k}`.
    ///
    /// # Invariant
    /// The checker compiles the read to the dedicated lowlevel `TableGet`, never a
    /// kind dispatch.
    Find { container: ExprId, key: ExprId },
    /// `{ value, type?, attributes }` — a type and/or attribute annotation.
    ///
    /// # Invariant
    /// `attributes` is **aligned by position** with the node's schema tail, and it
    /// also stamps that schema: the slots come into existence by being annotated,
    /// which is the one asymmetry with `:`.
    Annotation {
        value: ExprId,
        r#type: Option<ExprId>,
        attributes: ChildRange,
    },
    /// `{ parameter, return }` — a function type, compiled to the kinded
    /// arrow `[[in, out], [FunctionType, Type]]`.
    TypeFunction { parameter: ExprId, r#return: ExprId },
    /// A tuple instance `[v1, ..., vn]`.
    ///
    /// # Invariant
    /// One type slot per element, so the elements may be heterogeneous.
    Tuple(ChildRange),
    /// A tuple type expression `[T1, ..., Tn]`, kinded `[[T1, …, Tn],
    /// [TupleType, Type]]`.
    TypeTuple(ChildRange),
    /// A struct type expression; `names` is the parallel range into
    /// [`IR::struct_names`].
    ///
    /// # Invariant
    /// The *fresh nominal* id and the optional name table sit in the kind's marker
    /// payload, never in the shape: `[[id, names, names_in_order], TypeStruct]`.
    TypeStruct {
        fields: ChildRange,
        names: ChildRange,
    },
    /// An array instance `[v1, ..., vn]`.
    ///
    /// # Invariant
    /// Every element shares one type, unlike a [`Self::Tuple`]'s per-element slots.
    Array(ChildRange),
    /// A set `set{a, b, …}`, homogeneous like an [`Self::Array`].
    ///
    /// # Invariant
    /// The instance is typed by the set kind, not by `array<T, n>`: the *value* is
    /// the members and the set's identity is its type, so no value-level tag is
    /// needed.
    Set(ChildRange),
    /// A constant table `table { k1 ==> v1, … }`, interleaved in [`IR::children`].
    ///
    /// # Invariant
    /// Every key shares one key type and every value one value type. The checker
    /// builds the table eagerly — keys must be deep-evaluated to hash them — and
    /// drops an entry whose key is not concrete.
    Table(ChildRange),
    /// An array instance with `~`-marked positions — `[v1, ~ v2, ~2 v3]`.
    ///
    /// # Invariant
    /// Typed like a tuple, not an [`Self::Array`]: a homogeneous element type
    /// would reject `[x, ~ f(x+1)]` with an `Int` head and a `Stream` tail.
    ShallowArray {
        range: ChildRange,
        depths: ChildRange,
    },
    /// `_` — an inference placeholder hole, usable in any position.
    ///
    /// # Invariant
    /// It compiles to a fresh undecided cell that binds to whatever the context
    /// unifies it with, so a hole at the top level is one cell for the whole
    /// program.
    Placeholder,
    /// A recovered-error region, masked at the frontend as an opaque leaf.
    ///
    /// # Invariant
    /// The checker skips it (no cells, no unification, no cascade) and compiles it
    /// to a pair of fresh, never-unified cells, so it cannot emit a *type*-level
    /// "expected X, found Y" from inside itself.
    ErrorBlock,
    /// The real array type `{ element_type, length }`, kinded
    /// `[[element_type, length], [ArrayType, Type]]`.
    TypeArray {
        element_type: ExprId,
        length: ExprId,
    },
    /// A value imported from a package in the shared registry; the IR carries only
    /// the exported pair node ref.
    ///
    /// # Invariant
    /// The checker materializes the ref and extracts the value/type leaves; the
    /// payload itself stays in the package's static arena.
    Static {
        export: lichen_lowlevel::StaticNodeId,
    },
    /// `$name(args…)` — a native operator call registered by the compiling module's
    /// plugin.
    ///
    /// # Invariant
    /// `op` is a *private* name resolved only against that module's registry, so
    /// two plugins each registering `$jit` never collide. The checker compiles each
    /// argument and adopts the pair the plugin's builder returns.
    NativeCall { op: &'static str, args: ChildRange },
}

impl<L> ExprKind<L> {
    /// Every expression id this kind holds, in declaration order.
    ///
    /// # Invariant
    /// A [`Self::Function`]'s `parent` is **not** an operand — it names the
    /// enclosing function's node — so the list is exactly the subtree the checker
    /// compiles. The non-variadic kinds are named rather than caught by a
    /// wildcard, or a new kind could compile and then be silently skipped.
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

    /// Replace every `from` reference in the kind's own fields with `to` — one half
    /// of [`IR::repoint`].
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

    /// Allocate an [`ExprKind::Annotation`] whose attribute expressions are
    /// `attrs`.
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

    /// Stamp an already-allocated node with its real kind.
    ///
    /// # Invariant
    /// A node's id must exist before its own subtree compiles (a self or mutual
    /// reference, a nested closure's parent link), so the kind is stamped after
    /// the subtree, not allocated with it.
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

    /// Allocate a set literal: the members are variadic children, and the kind
    /// alone decides the type.
    pub fn alloc_set(&mut self, members: &[ExprId]) -> ExprId {
        self.alloc_variadic(members, ExprKind::Set)
    }

    /// Allocate a constant table literal, interleaved entry by entry.
    pub fn alloc_table(&mut self, entries: &[(ExprId, ExprId)]) -> ExprId {
        let range = extend_range(
            &mut self.children,
            entries.iter().flat_map(|&(key, value)| [key, value]),
        );
        self.alloc(ExprKind::Table(range))
    }

    /// Allocate a shallow-marked array, each element carrying its `~` depth.
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

    /// The children of a **variadic** expression.
    ///
    /// # Invariant
    /// A kind storing its children in named fields reads as no children rather
    /// than an `unreachable!`, so a non-variadic kind reaching here is a caller's
    /// mistake that `debug_assert` names. This is the *closure* end of the
    /// encoding: a new [`ChildRange`] kind must be added to the range arm.
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

    /// The children of `e`, every kind. See [`ExprKind::children`].
    pub fn children(&self, e: ExprId) -> Vec<ExprId> {
        self.expr[e.0 as usize].kind.children(self)
    }

    pub fn set_root(&mut self, root: ExprId) {
        self.root = root;
    }

    /// Record the top-level statement expression ids, in source order.
    pub fn set_stmt_roots(&mut self, stmt_roots: Vec<ExprId>) {
        self.stmt_roots = stmt_roots;
    }

    /// Re-point every stored occurrence of `from` to `to`.
    ///
    /// # Invariant
    /// The frontend's alias fixup: a block-wide binding aliased to a bare name
    /// re-points the uses that captured its reserved placeholder, which then
    /// leaves the block-root set. No-op when `from == to`, so the self-alias
    /// `a = a` keeps its placeholder referenced.
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
