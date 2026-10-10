//! The highlevel's concrete lowlevel program. See
//! `docs/notes/compiler-plugin.md`.
//!
//! # Invariant
//! The composed vocabularies [`HighProgramValue`] and [`HighProgramOperator`]
//! are flat unions — one `enum_ext!` invocation carrying each extension whole as
//! one sibling variant — so the checker builds and inspects every value without
//! an `Ext` wrapper.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use lichen_lowlevel::codec::{OperatorCodec, Reader, ValueCodec, Writer};
use lichen_lowlevel::{
    AnyNodeId, BlockId, GlobalExt, LowOperator, LowShape, LowValue, Module, ModuleKey, NodeId,
    OperatorExt, Program, StaticModule, ValueExt,
};
use lichen_utils::compose::AsField;
use lichen_utils::extend::AsEnum;

use crate::attr::{AttrSet, NoAttr};
use crate::diagnostic::DiagKind;
use crate::ir::Loc;
use crate::shape::for_each_kind_marker;

/// The fresh-nominal-type-id state — one extension component of
/// [`HighGlobalExt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HighGlobal {
    /// The next nominal type id; `Fresh` reads and increments it, so each call
    /// yields a distinct id.
    pub type_id_counter: usize,
}

impl HighGlobal {
    /// Consume the next nominal type id: read the counter, increment it, and
    /// return the previous value.
    pub fn next_type_id(&mut self) -> usize {
        let id = self.type_id_counter;
        self.type_id_counter += 1;
        id
    }
}

lichen_utils::compose_ext! {
    /// The highlevel's global extension state, in `Module`'s `global_ext` slot.
    ///
    /// # Invariant
    /// Read and mutated through `AsField`.
    ///
    /// ```
    /// use lichen_highlevel::program::{HighGlobal, HighGlobalExt};
    /// use lichen_utils::compose::AsField;
    ///
    /// let mut ext = HighGlobalExt::default();
    /// for expected in [0, 1] {
    ///     let id = AsField::<HighGlobal>::get_mut(&mut ext).next_type_id();
    ///     assert_eq!(id, expected);
    /// }
    /// assert_eq!(AsField::<HighGlobal>::get(&ext).type_id_counter, 2);
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct HighGlobalExt(
        HighGlobal,
    );
}
impl GlobalExt for HighGlobalExt {}

/// Per-package metadata for the highlevel `Program`: the exported pair ref and
/// the direct bindings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HighPackageMeta {
    /// The package's exported final-expression pair; `None` until a higher
    /// layer records it.
    pub export: Option<lichen_lowlevel::StaticNodeId>,
    /// The package's directly-exposed `(name, export)` bindings.
    ///
    /// # Invariant
    /// They are recorded so a *later* store over the same shared registry can
    /// rebuild the handle instead of recompiling the module, which the registry
    /// refuses — a key names one artifact.
    pub direct: Vec<(String, lichen_lowlevel::StaticNodeId)>,
    /// The package's **own source**, kept only for a built-in. See
    /// `docs/notes/core-prelude.md`.
    ///
    /// # Invariant
    /// A runtime failure names a frozen node by `StaticNodeId`; this is what
    /// turns that ref into a position in the file the user can open.
    pub source: Option<PackageSource>,
}

/// A built-in package's source, as the package layer records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSource {
    /// The file the source is exposed at — the copy the store materialized under
    /// the device's cache root.
    pub path: std::path::PathBuf,
    /// The source text, so a renderer can print the line a position is on
    /// without a second read of the file.
    pub code: std::sync::Arc<str>,
    /// `(frozen node index, line, column)`, sorted by index so a lookup is a
    /// binary search.
    pub spans: Vec<(usize, (u32, u32))>,
}

impl PackageSource {
    /// Where the frozen node `index` came from, when the build recorded it.
    pub fn span_of(&self, index: lichen_lowlevel::LocalNodeId) -> Option<(u32, u32)> {
        self.spans
            .binary_search_by_key(&index.index, |(node, _)| *node)
            .ok()
            .map(|at| self.spans[at].1)
    }
}

/// The result of building a literal: the compiled `[value, type]` pair plus its
/// value and type nodes.
///
/// # Invariant
/// `Type : Type` is the one case where the pair *is* the value's universe node
/// (self-referential), not `[value, type]`, so a literal returns all three
/// rather than assuming `pair = [value, ty]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiteralBuild {
    pub pair: NodeId,
    pub value: NodeId,
    pub ty: NodeId,
}

// The marker-node accessors are generated from the registry — one
// `fn …_marker_node(&self) -> NodeId` per marker.
macro_rules! define_ctx_marker_accessors {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $(
            #[doc = concat!("The installed `", $display, "` marker node.")]
            fn $node_fn(&self) -> NodeId;
        )*
    };
}

/// The curated safe context an extension point sees: the program-generic subset
/// of the checker's encoding surface.
///
/// # Invariant
/// Every encoder builds a well-formed structure by construction — a
/// [`Self::pair`] is always `[value, type]`, a [`Self::kind_expr`] always
/// `[marker, Type]` — and the active block is managed internally, so an
/// extension can never build into the wrong block. Driven by a literal's
/// `LiteralExt::build`, a native operator's `NativeOp::build`, and an
/// attribute's `AttrExt`.
pub trait Ctx<P: Program> {
    /// The value node for a raw value; a built-in marker reuses the installed
    /// shared node.
    fn value_node(&mut self, value: P::Value) -> NodeId;
    /// An array node of the given element nodes — a structural shape.
    fn array_node(&mut self, ids: &[NodeId]) -> NodeId;
    /// An operation node with the given operator and optional operand.
    fn op_node(&mut self, op: P::Operator, operand: Option<NodeId>) -> NodeId;
    /// A `[value, type]` pair node — the encoding of an expression's term.
    fn pair(&mut self, value: NodeId, ty: NodeId) -> NodeId;
    /// A kind expression `[marker, Type]`.
    fn kind_expr(&mut self, marker: NodeId) -> NodeId;
    /// A function type expression `[[domain, codomain], [FunctionType, Type]]`.
    ///
    /// # Invariant
    /// It does **not** register the result in the checker's `arrows` set: that
    /// set drives the type *printer*, so only a source-level arrow belongs in
    /// it. An arrow built as a unification *pattern* stays out.
    fn arrow(&mut self, domain: NodeId, codomain: NodeId) -> NodeId;
    /// A fresh undecided type cell.
    fn fresh(&mut self) -> NodeId;
    /// The canonical universe node `[Type, ↺]`, referenced rather than
    /// rebuilt: cloning it breaks unification.
    fn universe(&self) -> NodeId;
    /// The canonical, shared `[int, Type]` type expression: the type of every
    /// int value and of the `Int` type constant.
    ///
    /// # Invariant
    /// Referenced, never rebuilt — a composite type with a single semantic
    /// identity is shared, because diagnostics are attributed by the lowlevel
    /// unify trace and the checker's edges, never by span-on-node.
    fn int_type(&self) -> NodeId;
    /// The canonical, shared `[float, Type]` type expression, shared on the
    /// same terms as [`Self::int_type`].
    fn float_type(&self) -> NodeId;
    /// The canonical, shared `[string, Type]` type expression, shared like
    /// [`Self::int_type`].
    fn string_type(&self) -> NodeId;
    // The 9 marker-node accessors are registry-derived — one per kind marker.
    for_each_kind_marker!(define_ctx_marker_accessors);
    /// A checker-issued unification, executed through the highlevel's own
    /// discipline (diary-attributed).
    fn check_unify(&mut self, a: NodeId, b: NodeId, loc: Loc, kind: DiagKind);
    /// A unification that may be relaxed by an attribute's optional subtype
    /// relation. See [`Checker::check_unify_relaxed`].
    ///
    /// # Invariant
    /// The `is_subtype` callback receives the curated context, so relation reads
    /// go through [`Self::class_value`], never raw node inspection.
    fn check_unify_relaxed(
        &mut self,
        a: NodeId,
        b: NodeId,
        loc: Loc,
        kind: DiagKind,
        is_subtype: &dyn Fn(&dyn Ctx<P>, NodeId, NodeId) -> bool,
    );
    /// The value currently held by `node`'s equality class — read-only, for
    /// an attribute's subtype relation.
    fn class_value(&self, node: NodeId) -> Option<P::Value>;
}

/// A literal — the operator-like value-extension point. See
/// `docs/notes/compiler-plugin.md`.
///
/// # Invariant
/// `build` is the single creation function: it decides the value node and the
/// type node, referencing the prebuilt singleton exprs the context exposes.
pub trait LiteralExt<P>: Clone + Copy + PartialEq + std::fmt::Debug {
    /// Build this literal's value and type nodes (and their pair).
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild;
}

/// The built-in int literal: stores just the value, and references the shared
/// `[int, Type]` type expression.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct IntLit(pub usize);

impl<P> LiteralExt<P> for IntLit
where
    P: Program,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        let value_node = ctx.value_node(P::Value::from(LowValue::USize(self.0)));
        let ty = ctx.int_type();
        let pair = ctx.pair(value_node, ty);
        LiteralBuild {
            pair,
            value: value_node,
            ty,
        }
    }
}

/// The built-in float literal: stores the `f32` the lexer round-tripped.
/// See `docs/notes/floating-point.md` §3.3.
///
/// # Invariant
/// The pair is `[Float(1.5), [float, Type]]`, never an `Int`-shaped pair: §4.2
/// makes that a *different type*, not a lossy rendering of this one.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FloatLit(pub f32);

impl<P> LiteralExt<P> for FloatLit
where
    P: Program,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        let value_node = ctx.value_node(P::Value::from(LowValue::Float(self.0)));
        let ty = ctx.float_type();
        let pair = ctx.pair(value_node, ty);
        LiteralBuild {
            pair,
            value: value_node,
            ty,
        }
    }
}

/// The built-in string literal: a `&'static str` leaked once from the source,
/// plus the shared `[string, Type]`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StrLit(pub &'static str);

impl<P> LiteralExt<P> for StrLit
where
    P: Program,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        let value_node = ctx.value_node(P::Value::from(LowValue::Str(self.0)));
        let ty = ctx.string_type();
        let pair = ctx.pair(value_node, ty);
        LiteralBuild {
            pair,
            value: value_node,
            ty,
        }
    }
}

/// The built-in `Int` type constant — `Int : Type`. A unit literal whose pair
/// is the shared `[int, Type]`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct IntTypeLit;

impl<P> LiteralExt<P> for IntTypeLit
where
    P: Program,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        let value_node = ctx.int_marker_node();
        let ty = ctx.universe();
        let pair = ctx.int_type();
        LiteralBuild {
            pair,
            value: value_node,
            ty,
        }
    }
}

/// The built-in `Float` type constant — `Float : Type`. A unit literal whose
/// pair is the shared `[float, Type]`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FloatTypeLit;

impl<P> LiteralExt<P> for FloatTypeLit
where
    P: Program,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        let value_node = ctx.float_marker_node();
        let ty = ctx.universe();
        let pair = ctx.float_type();
        LiteralBuild {
            pair,
            value: value_node,
            ty,
        }
    }
}

/// The built-in `string` type constant — `string : Type`. A unit literal whose
/// pair is the shared `[string, Type]`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StringTypeLit;

impl<P> LiteralExt<P> for StringTypeLit
where
    P: Program,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        let value_node = ctx.string_marker_node();
        let ty = ctx.universe();
        let pair = ctx.string_type();
        LiteralBuild {
            pair,
            value: value_node,
            ty,
        }
    }
}

/// The built-in `Type` type constant — `Type : Type`. A unit literal whose pair
/// is the self-referential universe node.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TypeTypeLit;

impl<P> LiteralExt<P> for TypeTypeLit
where
    P: Program,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        LiteralBuild {
            pair: ctx.universe(),
            value: ctx.type_marker_node(),
            ty: ctx.universe(),
        }
    }
}

// The highlevel program's literal vocabulary: the built-in literals as sibling
// carry variants.
lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum HighProgramLiteral {
    }
    + IntLit as Int;
    + FloatLit as Float;
    + StrLit as Str;
    + IntTypeLit as IntType;
    + FloatTypeLit as FloatType;
    + StringTypeLit as StringType;
    + TypeTypeLit as TypeType;
}

impl<P> LiteralExt<P> for HighProgramLiteral
where
    P: Program,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        match self {
            HighProgramLiteral::Int(lit) => lit.build(ctx),
            HighProgramLiteral::Float(lit) => lit.build(ctx),
            HighProgramLiteral::Str(lit) => lit.build(ctx),
            HighProgramLiteral::IntType(lit) => lit.build(ctx),
            HighProgramLiteral::FloatType(lit) => lit.build(ctx),
            HighProgramLiteral::StringType(lit) => lit.build(ctx),
            HighProgramLiteral::TypeType(lit) => lit.build(ctx),
        }
    }
}

// The 9 kind-marker variants are generated from the registry.
// `TypeId` is not a kind marker and is spelled out below.
macro_rules! define_type_value {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        /// The highlevel's own value extension: a plain enum of the type
        /// constants.
        ///
        /// # Invariant
        /// Every variant is a *type constant* whose own type is the canonical
        /// universe (`Type : Type`), which makes the composed vocabulary's
        /// literal build a one-arm answer for this branch.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum TypeValue {
            $(
                $(#[$doc])*
                $variant,
            )*
            /// A nominal type id: a struct type's identity marker, at `shape[0]`
            /// of a `TypeStruct`-kinded pair.
            ///
            /// # Invariant
            /// Equal ids unify, different ids do not, and an id never unifies with
            /// the structural markers above.
            TypeId(usize),
        }
    };
}
for_each_kind_marker!(define_type_value);

impl TypeValue {
    /// The nominal type id carried by a `TypeId` value, if this is one.
    ///
    /// # Invariant
    /// A composed vocabulary's `ValueType::type_id` delegates here; the leaf
    /// keeps the one place the id lives.
    pub fn as_type_id(&self) -> Option<usize> {
        match self {
            TypeValue::TypeId(n) => Some(*n),
            _ => None,
        }
    }
}

// The highlevel program's value vocabulary: a flat union of the lowlevel
// structural values and the type values.
lichen_utils::enum_ext! {
    /// The highlevel program's value vocabulary: the lowlevel structural
    /// values and the type values, as sibling variants.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum HighProgramValue {
    }
    + LowValue as LowValue;
    + TypeValue as TypeValue;
}

impl ValueExt for HighProgramValue {
    fn is_handle(&self) -> bool {
        false
    }
}

// The marker methods are generated from the registry with default bodies.
macro_rules! define_value_type_marker_methods {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $(
            $(#[$doc])*
            fn $marker_fn() -> Self {
                Self::from(TypeValue::$variant)
            }
        )*
        /// Whether this value is one of the registry's kind markers — the honest
        /// tag test, not a structural guess.
        fn is_kind_marker(&self) -> bool {
            $( if *self == Self::$marker_fn() { return true; } )*
            false
        }
    };
}

/// An extension value leaf's contribution to the **open** kind-marker set: the
/// leaf's own type-constant atoms.
///
/// # Invariant
/// The highlevel's 9 kind markers are closed, but the marker *concept* is open:
/// a plugin composing its own value leaf into a language vocabulary may add type
/// constants, and the composed `ValueType::is_kind_marker` consults this trait.
pub trait LeafKindMarkers {
    /// Whether this value is one of the leaf's kind markers.
    fn is_kind_marker(&self) -> bool;
}

/// The value→type contract a vocabulary must satisfy to flow through the
/// checker. See `docs/notes/compiler-plugin.md`.
///
/// # Invariant
/// Every value union implements it and the checker is generic over it. The 9
/// kind-marker methods are registry-derived with default bodies over
/// `From<TypeValue>`; an implementation spells only [`Self::type_id`] and
/// [`Self::type_id_value`].
pub trait ValueType:
    ValueExt + From<LowValue> + AsEnum<LowValue> + From<TypeValue> + Clone
{
    for_each_kind_marker!(define_value_type_marker_methods);
    /// The nominal id of a struct type value, if this is one.
    fn type_id(&self) -> Option<usize>;
    /// A nominal type id value — what the checker's `Fresh` operator yields.
    fn type_id_value(n: usize) -> Self;
}

impl ValueType for HighProgramValue {
    fn type_id(&self) -> Option<usize> {
        match self {
            Self::TypeValue(TypeValue::TypeId(n)) => Some(*n),
            _ => None,
        }
    }
    fn type_id_value(n: usize) -> Self {
        Self::TypeValue(TypeValue::TypeId(n))
    }
}

// The highlevel's own operator extension: the type-level computations with no
// structural operator form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TypeOperator {
    /// A fresh nominal type id: each call reads and increments
    /// [`HighGlobal::next_type_id`].
    ///
    /// # Invariant
    /// Nullary — the checker emits it with no operand, so it fires once per
    /// source occurrence and the cached value is reused wherever the struct type
    /// it tags is referenced.
    Fresh,
    /// Binary operators over `[left, right]`. See
    /// `docs/notes/floating-point.md` §4.2.
    ///
    /// # Invariant
    /// Arithmetic and the order comparisons compute over one scalar class and
    /// never over a mixture: the checker pins both operands to the class a
    /// float operand selects, or to `Int`. `Rem` and the bitwise trio are
    /// `Int`-only. A comparison yields `USize(0/1)`, the arithmetic the
    /// operands' class, and `Div`/`Rem` by zero is the integer side's one
    /// run-time refusal ([`DIVIDE_BY_ZERO`]).
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
    /// The two **class conversions** — unary, over `[value]`. See
    /// `docs/notes/floating-point.md` §4.2.
    ///
    /// # Invariant
    /// `int2float` maps every `Int` up to `2^24` to one `f32` and above that
    /// keeps the magnitude while dropping low bits — a value fact, not a
    /// refusal. `float2int` truncates toward zero and is the one partial
    /// operator here: a `NaN`, an infinity, a negative or an out-of-range
    /// magnitude has no answer, so [`OUT_OF_RANGE`] records it and leaves the
    /// result lazy.
    Int2Float,
    Float2Int,
    /// Whether a value's class is a member of a **class domain**:
    /// `[class value, domain value]`, answering `USize(0/1)`.
    ///
    /// # Invariant
    /// It exists as an operator rather than as `==`: [`TypeOperator::Eq`]
    /// compares through [`ValueExt::value_eq`], which for an array is *handle*
    /// identity, so a structurally identical class node out of another module
    /// would compare unequal. This operator decodes structurally. An undecided
    /// side leaves the operator lazy, which keeps a refinement on an open
    /// parameter pending.
    InDomain,
    /// Whether a type value is a **struct type**, answering `USize(0/1)`.
    ///
    /// # Invariant
    /// The operand is `[type value, universe]`: the value to judge and the
    /// checker's canonical universe, needed to recognise the kind's universe
    /// slot. The decode is `shape::is_struct_type_any`, the reader the checker
    /// judges a decided container with. An undecided operand stays lazy.
    IsStructType,
}

/// The category an **integer** `Div`/`Rem` by zero is recorded under.
///
/// # Invariant
/// A run-time refusal, not a check error: `Module::eval_errors` is a closed
/// enum of *structural* value facts, and a zero divisor is not one, so the
/// lowlevel's general extension channel carries it. The answer is the lazy
/// marker. `Int`-only: a float `Div` has the IEEE answer, so no float divisor
/// is recorded. Only the interpreter refuses — a JIT'd kernel has left this
/// crate.
pub const DIVIDE_BY_ZERO: &str = "operator.divide_by_zero";

/// A `Float2Int` whose operand has no `Int` to truncate toward.
///
/// # Invariant
/// The four shapes are named rather than lumped together because the fix differs
/// for each. In range is a fact about a runtime value, not a checkable type, so
/// this is the [`DIVIDE_BY_ZERO`] answer.
pub const OUT_OF_RANGE: &str = "operator.out_of_range";

// --- the highlevel leaves' per-leaf artifact codec --------------------------

// `TypeValue` and `TypeOperator` have no arena payload, so their codecs are the
// smallest leaf-codec implementations.

// Both sides are generated from the kind-marker registry: each entry's tag is
// the persisted artifact tag.

// `TypeId` keeps tag 8, spelled here: a registry entry claiming 8 would shadow
// this read arm.
macro_rules! define_type_value_codec {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        impl TypeValue {
            /// Every kind-marker variant, in registry order — the same list the
            /// enum and codec are generated from.
            pub const KIND_MARKERS: &[TypeValue] = &[$(TypeValue::$variant),*];
        }

        impl ValueCodec for TypeValue {
            fn write_value<P: Program>(
                w: &mut Writer,
                value: Self,
                _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
            ) -> Result<(), String> {
                match value {
                    $(TypeValue::$variant => w.u8($tag),)*
                    TypeValue::TypeId(n) => {
                        w.u8(8);
                        w.u64(n as u64);
                    }
                }
                Ok(())
            }

            fn read_value<P: Program>(
                r: &mut Reader<'_>,
                _self_key: ModuleKey,
                _self_arena: &[u8],
                _self_base: *const u8,
                _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
            ) -> Result<Self, String> {
                Ok(match r.u8()? {
                    $($tag => TypeValue::$variant,)*
                    8 => TypeValue::TypeId(r.u64()? as usize),
                    tag => return Err(format!("unknown type-value tag {tag}")),
                })
            }
        }
    };
}
for_each_kind_marker!(define_type_value_codec);

// The one list of the type-level operators: the codec's two sides and
// [`TypeOperator::ALL`] are generated from it.

// Each tag is the persisted artifact tag — the same contract as the kind-marker
// tags above.

// The write side is an exhaustive match, so a variant missing from this list
// fails to compile.
macro_rules! define_type_operator_codec {
    ($( $variant:ident = $tag:literal; )*) => {
        impl TypeOperator {
            /// Every variant, generated from the same list as the codec.
            pub const ALL: &[TypeOperator] = &[$(TypeOperator::$variant),*];
        }

        impl OperatorCodec for TypeOperator {
            fn write_operator(w: &mut Writer, op: Self) -> Result<(), String> {
                match op {
                    $(TypeOperator::$variant => w.u8($tag),)*
                }
                Ok(())
            }

            fn read_operator(r: &mut Reader<'_>) -> Result<Self, String> {
                Ok(match r.u8()? {
                    $($tag => TypeOperator::$variant,)*
                    tag => return Err(format!("unknown type-operator tag {tag}")),
                })
            }
        }
    };
}
define_type_operator_codec! {
    Fresh = 0;
    Add = 1;
    Sub = 2;
    Leq = 3;
    Eq = 4;
    Mul = 5;
    Div = 6;
    Rem = 7;
    Lt = 8;
    Gt = 9;
    Geq = 10;
    Neq = 11;
    BitAnd = 12;
    BitOr = 13;
    BitXor = 14;
    Int2Float = 15;
    Float2Int = 16;
    InDomain = 17;
    IsStructType = 18;
}

// The highlevel program's operator vocabulary: a flat union of the structural
// and type-level operators.
lichen_utils::enum_ext! {
    /// The highlevel program's operator vocabulary: the structural and
    /// type-level operators, as sibling carry variants.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum HighProgramOperator {
    }
    + LowOperator as LowOperator;
    + TypeOperator as TypeOperator;
}

impl<V, A, L, G> OperatorExt<ProgramImpl<V, HighProgramOperator, A, L, G>> for HighProgramOperator
where
    V: ValueType,
    A: AttrSet,
    L: std::fmt::Debug + Copy + PartialEq,
    G: GlobalExt + AsField<HighGlobal>,
{
    fn run(
        &self,
        operand: V,
        _block: BlockId,
        module: &mut Module<ProgramImpl<V, HighProgramOperator, A, L, G>>,
    ) -> Option<V> {
        match self {
            // The structural operators never reach `run`: the VM dispatches
            // them through `AsEnum`.
            HighProgramOperator::LowOperator(_) => {
                unreachable!("structural operators are dispatched by the VM")
            }
            // The type-level operators are delegated to [`OperatorExt`] for
            // [`TypeOperator`].
            HighProgramOperator::TypeOperator(op) => op.run(operand, _block, module),
        }
    }

    /// The union's low-type transfer: each leaf states its own computation's
    /// low shape.
    fn low_type(&self, arguments: &[Option<LowShape>]) -> Option<LowShape> {
        match self {
            HighProgramOperator::LowOperator(_) => None,
            // Qualified: a transfer mentions no `P`-typed argument, so the
            // impl could not be inferred.
            HighProgramOperator::TypeOperator(op) => <TypeOperator as OperatorExt<
                ProgramImpl<V, HighProgramOperator, A, L, G>,
            >>::low_type(op, arguments),
        }
    }
}

/// The highlevel's own type-level operators. See
/// `docs/notes/operator-polymorphism.md`.
///
/// # Invariant
/// Dispatched by any program whose value vocabulary implements [`ValueType`] and
/// whose global state carries [`HighGlobal`]; every such program shares this one
/// impl and differs only in which union wraps the operator.
impl<P> OperatorExt<P> for TypeOperator
where
    P: Program,
    P::Value: ValueType,
    P::GlobalExt: AsField<HighGlobal>,
{
    fn run(&self, operand: P::Value, _block: BlockId, module: &mut Module<P>) -> Option<P::Value> {
        // An undecided answer is `None` (see `OperatorExt::run`); the closure
        // gives every early `return` one return type.
        (|| match self {
            TypeOperator::Fresh => {
                let id = AsField::<HighGlobal>::get_mut(&mut module.global_ext).next_type_id();
                Some(P::Value::type_id_value(id))
            }
            // The two class conversions: one operand, in a one-element array.
            TypeOperator::Int2Float | TypeOperator::Float2Int => {
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("a conversion expects a one-element operand array")
                };
                // SAFETY: `operands` is the array the VM just evaluated for this
                // node; its home block is alive here.
                let items = unsafe { operands.items() };
                // An undecided operand keeps the operator lazy.
                let Some(value) = module.node_value(items[0].node) else {
                    return None;
                };
                match (self, value.as_enum()) {
                    // `Int` into `f32`: exact up to `2^24`, nearest above it.
                    (TypeOperator::Int2Float, Some(LowValue::USize(n))) => {
                        Some(P::Value::from(LowValue::Float(n as f32)))
                    }
                    // Truncation toward zero, unnamed shapes staying lazy.
                    (TypeOperator::Float2Int, Some(LowValue::Float(f))) => {
                        let truncated = f.trunc();
                        // The exclusive ceiling of the unsigned machine integer
                        // on either target width.
                        if !f.is_finite()
                            || truncated < 0.0
                            || (truncated as f64) >= (usize::MAX as f64)
                        {
                            return out_of_range(module, f);
                        }
                        Some(P::Value::from(LowValue::USize(truncated as usize)))
                    }
                    _ => None,
                }
            }
            TypeOperator::Add
            | TypeOperator::Sub
            | TypeOperator::Mul
            | TypeOperator::Div
            | TypeOperator::Rem
            | TypeOperator::Lt
            | TypeOperator::Gt
            | TypeOperator::Leq
            | TypeOperator::Geq
            | TypeOperator::Eq
            | TypeOperator::Neq
            | TypeOperator::BitAnd
            | TypeOperator::BitOr
            | TypeOperator::BitXor
            | TypeOperator::InDomain
            | TypeOperator::IsStructType => {
                // An undecided operand never reaches this operator.
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("binary operators expect an operand array of [left, right]")
                };
                // SAFETY: `operands` is the array the VM just evaluated for this
                // node; its home block is alive here.
                let operands = unsafe { operands.items() };
                // An undecided side (an empty slot) keeps the operator lazy.
                let left = module.node_value(operands[0].node);
                let right = module.node_value(operands[1].node);
                let (Some(left), Some(right)) = (left, right) else {
                    return None;
                };
                match self {
                    // A non-`Int` operand is a reported type error: stay lazy
                    // rather than panic.

                    // A zero divisor is recorded rather than panicked.
                    TypeOperator::Rem
                    | TypeOperator::BitAnd
                    | TypeOperator::BitOr
                    | TypeOperator::BitXor => {
                        let to_usize = |value: &P::Value| match value.as_enum() {
                            Some(LowValue::USize(n)) => Some(n),
                            _ => None,
                        };
                        let (Some(left), Some(right)) = (to_usize(&left), to_usize(&right)) else {
                            return None;
                        };
                        let value = match self {
                            TypeOperator::Rem => match left.checked_rem(right) {
                                Some(n) => n,
                                None => return divide_by_zero(module, true),
                            },
                            TypeOperator::BitAnd => left & right,
                            TypeOperator::BitOr => left | right,
                            TypeOperator::BitXor => left ^ right,
                            _ => unreachable!("the Int-only operators are handled above"),
                        };
                        Some(P::Value::from(LowValue::USize(value)))
                    }
                    // A cross-class operand is already refused, so the mixed case
                    // stays lazy rather than coercing.
                    TypeOperator::Add
                    | TypeOperator::Sub
                    | TypeOperator::Mul
                    | TypeOperator::Div
                    | TypeOperator::Lt
                    | TypeOperator::Gt
                    | TypeOperator::Leq
                    | TypeOperator::Geq => match (left.as_enum(), right.as_enum()) {
                        // `Int` is unsigned, so the four order comparisons are
                        // the unsigned ones and the arithmetic wraps.
                        (Some(LowValue::USize(left)), Some(LowValue::USize(right))) => {
                            let value = match self {
                                TypeOperator::Add => left.wrapping_add(right),
                                TypeOperator::Sub => left.wrapping_sub(right),
                                TypeOperator::Mul => left.wrapping_mul(right),
                                TypeOperator::Div => match left.checked_div(right) {
                                    Some(n) => n,
                                    None => return divide_by_zero(module, false),
                                },
                                TypeOperator::Lt => (left < right) as usize,
                                TypeOperator::Gt => (left > right) as usize,
                                TypeOperator::Leq => (left <= right) as usize,
                                TypeOperator::Geq => (left >= right) as usize,
                                _ => {
                                    unreachable!("the float operators are handled beside this arm")
                                }
                            };
                            Some(P::Value::from(LowValue::USize(value)))
                        }
                        // A float's arithmetic is IEEE: a zero divisor is an
                        // ordinary infinity or `NaN`.
                        (Some(LowValue::Float(left)), Some(LowValue::Float(right))) => match self {
                            TypeOperator::Add => {
                                Some(P::Value::from(LowValue::Float(left + right)))
                            }
                            TypeOperator::Sub => {
                                Some(P::Value::from(LowValue::Float(left - right)))
                            }
                            TypeOperator::Mul => {
                                Some(P::Value::from(LowValue::Float(left * right)))
                            }
                            TypeOperator::Div => {
                                Some(P::Value::from(LowValue::Float(left / right)))
                            }
                            TypeOperator::Lt => {
                                Some(P::Value::from(LowValue::USize((left < right) as usize)))
                            }
                            TypeOperator::Gt => {
                                Some(P::Value::from(LowValue::USize((left > right) as usize)))
                            }
                            TypeOperator::Leq => {
                                Some(P::Value::from(LowValue::USize((left <= right) as usize)))
                            }
                            TypeOperator::Geq => {
                                Some(P::Value::from(LowValue::USize((left >= right) as usize)))
                            }
                            _ => unreachable!("the Int operators are handled beside this arm"),
                        },
                        _ => None,
                    },
                    // `==` routes through [`ValueExt::value_eq`], so the two
                    // agree by construction.
                    TypeOperator::Eq => Some(P::Value::from(LowValue::USize(
                        left.value_eq(&right) as usize,
                    ))),
                    TypeOperator::Neq => Some(P::Value::from(LowValue::USize(
                        (!left.value_eq(&right)) as usize,
                    ))),
                    TypeOperator::Fresh => unreachable!("Fresh is handled above"),
                    TypeOperator::Int2Float | TypeOperator::Float2Int => {
                        unreachable!("the conversions are unary, and handled above")
                    }
                    // The refinement's membership test; the outer arm already
                    // gated on an undecided side.
                    TypeOperator::InDomain => {
                        // Operand 1 is the domain, operand 0 the class tested;
                        // the decode is structural.
                        let member =
                            crate::set::contains(module, operands[1].node, operands[0].node);
                        Some(P::Value::from(LowValue::USize(member as usize)))
                    }
                    // The named read's container requirement, decoded as the
                    // checker decodes a decided container.
                    TypeOperator::IsStructType => {
                        let AnyNodeId::Dynamic(universe) = operands[1].node else {
                            return None;
                        };
                        let is_struct =
                            crate::shape::is_struct_type_any(module, universe, operands[0].node);
                        Some(P::Value::from(LowValue::USize(is_struct as usize)))
                    }
                }
            }
        })()
    }

    /// The low-type transfer of the type-level operators.
    ///
    /// # Invariant
    /// Every comparison, the `Int`-only operators and the equality produce a
    /// machine scalar, so their transfer is `USize`. The arithmetic
    /// operators produce their operands' own class: `Float` when either operand's
    /// low type is a float, keeping a float-valued expression out of a
    /// backend, else `USize`. `Fresh` is a nominal type id, which the
    /// vocabulary has no shape for, so it declines.
    fn low_type(&self, arguments: &[Option<LowShape>]) -> Option<LowShape> {
        match self {
            TypeOperator::Add | TypeOperator::Sub | TypeOperator::Mul | TypeOperator::Div => {
                if arguments
                    .iter()
                    .any(|argument| matches!(argument, Some(LowShape::Float)))
                {
                    Some(LowShape::Float)
                } else {
                    Some(LowShape::USize)
                }
            }
            TypeOperator::Rem
            | TypeOperator::Lt
            | TypeOperator::Gt
            | TypeOperator::Leq
            | TypeOperator::Geq
            | TypeOperator::Eq
            | TypeOperator::Neq
            | TypeOperator::BitAnd
            | TypeOperator::BitOr
            | TypeOperator::BitXor => Some(LowShape::USize),
            TypeOperator::Int2Float => Some(LowShape::Float),
            TypeOperator::Float2Int => Some(LowShape::USize),
            // A membership test answers `0`/`1`, so its own class is the machine
            // scalar whatever its operands are.
            TypeOperator::InDomain => Some(LowShape::USize),
            // The same `0`/`1`: a read's container-kind requirement is asked of
            // type values and answered by the assert channel.
            TypeOperator::IsStructType => Some(LowShape::USize),
            TypeOperator::Fresh => None,
        }
    }
}

/// A `Div`/`Rem` whose divisor evaluated to zero: record why and stay lazy.
///
/// # Invariant
/// `remainder` picks the wording, because saying which operator it was is the
/// difference between a message a reader can act on and one they must guess at.
fn divide_by_zero<P>(module: &mut Module<P>, remainder: bool) -> Option<P::Value>
where
    P: Program,
    P::Value: ValueType,
    P::GlobalExt: AsField<HighGlobal>,
{
    let operation = if remainder { "remainder" } else { "division" };
    module.record_extension_diagnostic(
        DIVIDE_BY_ZERO,
        None,
        format!("the divisor of this {operation} evaluated to 0, and there is no value for a {operation} by zero"),
    );
    None
}

/// A `Float2Int` whose operand has no `Int` to truncate toward: record which
/// shape it was and stay lazy.
///
/// # Invariant
/// The four shapes are named rather than lumped together because the fix differs
/// for each. In range is a fact about a runtime value, not a checkable type.
fn out_of_range<P>(module: &mut Module<P>, value: f32) -> Option<P::Value>
where
    P: Program,
    P::Value: ValueType,
    P::GlobalExt: AsField<HighGlobal>,
{
    let reason = if value.is_nan() {
        "a NaN, which is a number nowhere"
    } else if value.is_infinite() {
        "an infinity, wider than any machine integer"
    } else if value < 0.0 {
        "negative, and this language's Int is unsigned"
    } else {
        "at or past the widest machine integer"
    };
    module.record_extension_diagnostic(
        OUT_OF_RANGE,
        None,
        format!("this float2int operand evaluated to {value}, which is {reason}, and there is no Int for it to truncate toward"),
    );
    None
}

/// The highlevel's associated-type collector: what the checker is generic over.
///
/// # Invariant
/// It extends the lowlevel [`Program`] with the attribute type an expression's
/// schema carries; the checker never names a concrete attribute, only
/// `Self::Attr`.
pub trait HighProgram: Program {
    /// The compile-time attribute type an expression's schema may carry: a
    /// composed attribute *set* ([`AttrSet`]).
    type Attr: AttrSet;
    /// The literal vocabulary: a downstream's composed union, or the built-in
    /// [`HighProgramLiteral`].
    type Literal: LiteralExt<Self>;
}

/// The highlevel's concrete lowlevel program: a marker generic over its four
/// vocabularies.
///
/// # Invariant
/// A downstream needing more operators composes its own enum carrying
/// [`LowOperator`] and [`TypeOperator`] as siblings; the
/// runtime/static-module/registry machinery is then reusable unchanged.
pub struct ProgramImpl<
    V: ValueType = HighProgramValue,
    O: std::fmt::Debug + Copy + PartialEq = HighProgramOperator,
    A: AttrSet = NoAttr,
    L = HighProgramLiteral,
    G: GlobalExt = HighGlobalExt,
>(#[doc(hidden)] pub PhantomData<(V, O, A, L, G)>);

// The marker's `Debug`/`Clone`/`Copy`/`PartialEq` are structural: `PhantomData`
// is all four for *any* type argument.
impl<V, O, A, L, G> std::fmt::Debug for ProgramImpl<V, O, A, L, G>
where
    V: ValueType,
    O: std::fmt::Debug + Copy + PartialEq,
    A: AttrSet,
    G: GlobalExt,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramImpl").finish()
    }
}
impl<V, O, A, L, G> Clone for ProgramImpl<V, O, A, L, G>
where
    V: ValueType,
    O: std::fmt::Debug + Copy + PartialEq,
    A: AttrSet,
    G: GlobalExt,
{
    fn clone(&self) -> Self {
        *self
    }
}
impl<V, O, A, L, G> Copy for ProgramImpl<V, O, A, L, G>
where
    V: ValueType,
    O: std::fmt::Debug + Copy + PartialEq,
    A: AttrSet,
    G: GlobalExt,
{
}
impl<V, O, A, L, G> PartialEq for ProgramImpl<V, O, A, L, G>
where
    V: ValueType,
    O: std::fmt::Debug + Copy + PartialEq,
    A: AttrSet,
    G: GlobalExt,
{
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<V, O, A, L, G> Program for ProgramImpl<V, O, A, L, G>
where
    V: ValueType,
    A: AttrSet,
    L: std::fmt::Debug + Copy + PartialEq,
    G: GlobalExt,
    O: lichen_lowlevel::OperatorExt<ProgramImpl<V, O, A, L, G>>
        + From<LowOperator>
        + lichen_utils::extend::AsEnum<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq,
{
    type Value = V;
    type Operator = O;
    type GlobalExt = G;
    type PackageMeta = HighPackageMeta;
}

impl<V, O, A, L, G> HighProgram for ProgramImpl<V, O, A, L, G>
where
    V: ValueType,
    A: AttrSet,
    L: LiteralExt<ProgramImpl<V, O, A, L, G>>,
    G: GlobalExt,
    O: lichen_lowlevel::OperatorExt<ProgramImpl<V, O, A, L, G>>
        + From<LowOperator>
        + lichen_utils::extend::AsEnum<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq,
{
    type Attr = A;
    type Literal = L;
}
