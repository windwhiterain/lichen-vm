//! The highlevel's concrete lowlevel program.
//!
//! Each layer provides a plain enum of its own variants: the lowlevel's
//! [`LowValue`]/[`LowOperator`], the highlevel's type values
//! ([`TypeValue`]) and type-level operators ([`TypeOperator`]).  The
//! composed vocabularies [`HighProgramValue`] and [`HighProgramOperator`]
//! are flat unions — one `lichen_utils::enum_ext!` invocation carrying each
//! extension whole as one sibling variant — so the checker builds and
//! inspects every value and emits every operator without an `Ext` wrapper:
//! a structural value sits one carry variant down
//! (`HighProgramValue::LowValue(..)`), the highlevel's type values sit in
//! theirs, and nothing nests.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use lichen_lowlevel::codec::{OperatorCodec, Reader, ValueCodec, Writer};
use lichen_lowlevel::{
    BlockId, GlobalExt, LowOperator, LowValue, Module, ModuleKey, NodeId, OperatorExt, Program,
    StaticModule, ValueExt, is_unbound,
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
    /// The next nominal type id — [`HighProgramOperator::Fresh`] reads and
    /// increments it, so each call yields a distinct
    /// [`HighProgramValue::TypeId`].
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
    /// The highlevel's global extension state, injected into the lowlevel
    /// [`Module`]'s `global_ext` slot and threaded through the extension
    /// operators.
    ///
    /// It is a *tuple* host built by [`lichen_utils::compose_ext!`] over its
    /// extension components — a downstream composes more components by adding
    /// their types to this tuple and reads or mutates each one through
    /// [`lichen_utils::compose::AsField`] and the component's own methods (no
    /// per-component accessor trait).
    ///
    /// ```
    /// use lichen_highlevel::program::{HighGlobal, HighGlobalExt};
    /// use lichen_utils::compose::AsField;
    ///
    /// let mut ext = HighGlobalExt::default();
    /// assert_eq!(
    ///     AsField::<HighGlobal>::get_mut(&mut ext).next_type_id(),
    ///     0
    /// );
    /// assert_eq!(
    ///     AsField::<HighGlobal>::get_mut(&mut ext).next_type_id(),
    ///     1
    /// );
    /// assert_eq!(AsField::<HighGlobal>::get(&ext).type_id_counter, 2);
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct HighGlobalExt(
        HighGlobal,
    );
}
impl GlobalExt for HighGlobalExt {}

/// Per-package metadata for the highlevel `Program`.  The package layer
/// stores the single exported `[value, type]` pair ref here; the lowlevel's
/// [`Package`] only carries this as an opaque `Default` slot, keeping
/// highlevel concepts out of the lowlevel registry machinery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HighPackageMeta {
    /// The package's exported final-expression pair, filled by the language
    /// package store after freezing.  `None` until a higher layer records it.
    pub export: Option<lichen_lowlevel::StaticNodeId>,
}

/// The result of building a literal: the compiled `[value, type]` pair plus
/// its value and type nodes.  The checker records all three (the value and
/// type are read for the surrounding constructs), and the pair is the
/// expression's compiled term.  `Type : Type` is the one case where the pair
/// *is* the value's universe node (self-referential), not `[value, type]` —
/// so a literal returns all three rather than assuming `pair = [value, ty]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiteralBuild {
    pub pair: NodeId,
    pub value: NodeId,
    pub ty: NodeId,
}

// The marker-node accessors are generated from the registry — one
// `fn …_marker_node(&self) -> NodeId` per marker, each returning the
// checker's installed shared node for it.
macro_rules! define_ctx_marker_accessors {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $(
            #[doc = concat!("The installed `", $display, "` marker node.")]
            fn $node_fn(&self) -> NodeId;
        )*
    };
}

/// The curated safe context an extension point sees — the program-generic
/// subset of the checker's *encoding* surface.  This is the "who encode, who
/// parse" boundary: the highlevel owns the `[value, type]` grammar, and an
/// extension expresses *content* through these semantic encoders rather than
/// hand-assembling lowlevel nodes.  Each encoder builds a well-formed
/// structure by construction — a [`Self::pair`] is always `[value, type]`, a
/// [`Self::kind_expr`] always `[marker, Type]` — and the active block is
/// managed internally, so an extension can never build into the wrong block.
///
/// The checker implements this; a literal's [`LiteralExt::build`], a native
/// operator's [`crate::native::NativeExt::check_apply`], and an attribute's
/// [`crate::attr::AttrExt`] all drive it.  (The one deliberate exception: the
/// lowlevel-only constructs a native operator genuinely needs — a fresh cell,
/// an op node, an array shape — are the *shape* of the grammar, not a
/// bypass.)
pub trait Ctx<P: Program> {
    /// The value node for a raw value: a built-in type marker reuses the
    /// checker's installed shared marker node; anything else allocates a node.
    fn value_node(&mut self, value: P::Value) -> NodeId;
    /// An array node of the given element nodes — a structural shape.
    fn array_node(&mut self, ids: &[NodeId]) -> NodeId;
    /// An operation node with the given operator and optional operand.
    fn op_node(&mut self, op: P::Operator, operand: Option<NodeId>) -> NodeId;
    /// A `[value, type]` pair node — the encoding of an expression's term.
    fn pair(&mut self, value: NodeId, ty: NodeId) -> NodeId;
    /// A kind expression `[marker, Type]`.
    fn kind_expr(&mut self, marker: NodeId) -> NodeId;
    /// A fresh unbound type cell.
    fn fresh(&mut self) -> NodeId;
    /// The canonical universe node `[Type, ↺]` (`Type : Type`).  Referenced,
    /// not rebuilt — the prebuilt composite that must be shared, because
    /// cloning the self-referential universe breaks unification.
    fn universe(&self) -> NodeId;
    /// The canonical, shared `[int, Type]` type expression — the type of every
    /// int value and the pair of the `Int` type constant.  Referenced, not
    /// rebuilt: a composite type with a single semantic identity is shared
    /// across occurrences, since diagnostics are attributed by the lowlevel
    /// unify trace and the checker's edges (never by span-on-node).
    fn int_type(&self) -> NodeId;
    /// The canonical, shared `[string, Type]` type expression — the type of
    /// every `Str` value and the pair of the `string` type constant.  Shared
    /// across occurrences like [`Self::int_type`].
    fn string_type(&self) -> NodeId;
    // The 8 marker-node accessors (`int_marker_node`, `string_marker_node`,
    // `type_marker_node`, …) are registry-derived — one per kind marker.
    for_each_kind_marker!(define_ctx_marker_accessors);
    /// A checker-issued unification — an extension's type check, executed
    /// through the highlevel's own discipline (diary-attributed).
    fn check_unify(&mut self, a: NodeId, b: NodeId, loc: Loc, kind: DiagKind);
    /// A unification that may be relaxed by an attribute's optional subtype
    /// relation (see [`Checker::check_unify_relaxed`]).  The `is_subtype`
    /// callback receives the curated context, so relation reads go through
    /// [`Self::class_value`] again — never raw node inspection.
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

/// A literal — the operator-like value-extension point: any struct that
/// builds a `value : type` pair through the curated [`Ctx`].  A literal's
/// [`LiteralExt::build`] is the single creation function: it decides the
/// value node and the type node (referencing the prebuilt singleton exprs the
/// context exposes).
///
/// A downstream composes its literal vocabulary with
/// [`lichen_utils::enum_ext!`](`lichen_utils::extend`), carrying the
/// built-in literal structs and its own literal structs as sibling variants,
/// then implements this trait for the composed enum (delegating each variant
/// to its own build) and passes the enum as the program's `Literal` type.
pub trait LiteralExt<P>: Clone + Copy + PartialEq + std::fmt::Debug {
    /// Build this literal's value and type nodes (and their pair).
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild;
}

/// The built-in int literal: stores just the value.  `build` references the
/// canonical, shared `[int, Type]` type expression for the type, so the
/// composite is never duplicated per occurrence.
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

/// The built-in string literal: stores the (immutable) string content, a
/// `&'static str` leaked once from the source.  `build` references the
/// canonical, shared `[string, Type]` type expression for the type, exactly
/// as [`IntLit`] mirrors `[int, Type]`.
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

/// The built-in `Int` type constant — `Int : Type`.  A unit literal (the type
/// constant carries no data).  `build` produces the value node `int_marker`
/// and its type the canonical universe; the pair is the shared `[int, Type]`
/// type expression.
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

/// The built-in `string` type constant — `string : Type`.  A unit literal.
/// `build` produces the value node `string_marker` and its type the canonical
/// universe; the pair is the shared `[string, Type]` type expression.
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

/// The built-in `Type` type constant — `Type : Type`.  A unit literal.  Its
/// pair is the canonical self-referential universe node (the single prebuilt
/// composite that must be shared, not rebuilt).
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

// The highlevel program's literal vocabulary: the built-in int & string
// literals and the `Int`/`string`/`Type` type-constant literals, as sibling
// carry variants.  Each type constant has its own literal; each `build`
// rebuilds its value/type nodes fresh per occurrence.
lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum HighProgramLiteral {
    }
    + IntLit as Int;
    + StrLit as Str;
    + IntTypeLit as IntType;
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
            HighProgramLiteral::Str(lit) => lit.build(ctx),
            HighProgramLiteral::IntType(lit) => lit.build(ctx),
            HighProgramLiteral::StringType(lit) => lit.build(ctx),
            HighProgramLiteral::TypeType(lit) => lit.build(ctx),
        }
    }
}

// The 8 kind-marker variants are generated from the registry
// ([`crate::shape::for_each_kind_marker`]) — adding or removing a marker
// touches that one list.  `TypeId` is NOT a kind marker (it carries the
// nominal id a struct marker references) and is spelled out below.
macro_rules! define_type_value {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        /// The highlevel's own value extension — a plain enum of the type constants,
        /// provided whole for the compositions below (and for a language crate
        /// composing its own vocabulary from [`LowValue`] + this).
        ///
        /// Every variant is a *type constant*: its own type is the canonical
        /// universe (`Type : Type`), which makes the composed vocabulary's literal
        /// build a one-arm answer for this whole branch.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub enum TypeValue {
            $(
                $(#[$doc])*
                $variant,
            )*
            /// A nominal type id — a struct type's identity marker, living at
            /// `shape[0]` of a `TypeStruct`-kinded pair.  Equal ids unify,
            /// different ids don't (nominal identity), and an id never unifies
            /// with the structural markers above.
            TypeId(usize),
        }
    };
}
for_each_kind_marker!(define_type_value);

impl TypeValue {
    /// The nominal type id carried by a `TypeId` value, if this is one.
    ///
    /// A composed value vocabulary's `ValueType::type_id` delegates here; the
    /// leaf keeps the one place the id lives.
    pub fn as_type_id(&self) -> Option<usize> {
        match self {
            TypeValue::TypeId(n) => Some(*n),
            _ => None,
        }
    }
}

// The highlevel program's value vocabulary: a flat union of the lowlevel
// structural values and the highlevel type values, each carried whole as
// one sibling variant — one `lichen_utils::enum_ext!` invocation listing
// every layer's enum.  A language crate composes its own vocabulary the
// same way: `+ LowValue as LowValue; + TypeValue as TypeValue;` plus its
// own variants.
lichen_utils::enum_ext! {
    /// The highlevel program's value vocabulary: the lowlevel structural
    /// values and the highlevel type values, as sibling carry variants.
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

// The marker methods are generated from the registry with default bodies:
// any vocabulary carrying the `TypeValue` leaf whole (an `enum_ext!`
// composition, which generates `From<TypeValue>`) gets every marker for
// free; an impl may still override individual methods.
macro_rules! define_value_type_marker_methods {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $(
            $(#[$doc])*
            fn $marker_fn() -> Self {
                Self::from(TypeValue::$variant)
            }
        )*
    };
}

/// The value→type contract a value vocabulary must satisfy to flow through
/// the checker: the type-constant markers it installs, the value→type
/// mapping for constants, and the kind classification the checker's
/// structural type checks dispatch on.  Every value union — the highlevel's
/// own [`HighProgramValue`] or an extended one — implements this; the
/// checker is generic over it.
///
/// The 8 kind-marker methods are registry-derived
/// ([`crate::shape::for_each_kind_marker`]) with default bodies over
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

// The highlevel's own operator extension — a plain enum of the type-level
// computations that have no structural operator form, provided whole for the
// composition below.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TypeOperator {
    /// A fresh nominal type id: each call reads and increments
    /// [`HighGlobal::next_type_id`] and returns a `TypeId(n)` type value.
    /// Nullary — the checker emits it with no operand, so it fires once per
    /// source occurrence and the cached value is reused wherever the struct
    /// type it tags is referenced.
    Fresh,
    /// Binary operators over `[left, right]`.  `Add`/`Sub` compute on
    /// `Int` operands; `Leq`/`Eq` compare and yield `USize(0/1)` — no
    /// `Bool` value exists, the comparison result drives the lazy `Index`
    /// branch of an `if` directly.  `Eq` is the generalized equality
    /// (docs/language-spec.md): it compares any two *same-typed* values
    /// whole (two `Int`s, or two type values — `S::a == Int` is `1`),
    /// while a cross-type comparison is a check-time error (the checker
    /// unifies the operand types; `+ - <=` pin both to `Int`).
    ///
    /// The lowlevel deep-evaluates the operand and gates on its
    /// parameterized subtree before calling `run`, so an unbound operand
    /// (a template parameter during the definition pass) is already the
    /// lazy marker, and `run` stays lazy on any unbound side.
    Add,
    Sub,
    Leq,
    Eq,
}

// --- the highlevel leaves' per-leaf artifact codec --------------------------
//
// `TypeValue` and `TypeOperator` have no arena payload, so their codecs ignore
// the relocation context and are the smallest leaf-codec implementations: an
// exhaustive tag write and an inverse read.  Both sides of the `TypeValue`
// codec are generated from the kind-marker registry
// ([`crate::shape::for_each_kind_marker`]): each entry's tag is the persisted
// artifact tag, a compatibility contract — an existing entry's tag never
// changes and a new marker takes the next unused tag, so the tags are
// deliberately not the list positions (`TypeString` is `7`).  `TypeId` is not
// a kind marker; it keeps tag 8, spelled here — a registry entry claiming 8
// would collide with it as a duplicate match arm and fail to compile.
macro_rules! define_type_value_codec {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        impl TypeValue {
            /// Every kind-marker variant, in registry order — registry-derived,
            /// so the codec round-trip tests iterate the same list the enum
            /// and codec are generated from.  `TypeId` is not a kind marker
            /// and is sampled separately.
            pub const KIND_MARKERS: &[TypeValue] = &[$(TypeValue::$variant),*];
        }

        impl ValueCodec for TypeValue {
            fn write_value<P: Program>(
                w: &mut Writer,
                value: Self,
                _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
            ) {
                match value {
                    $(TypeValue::$variant => w.u8($tag),)*
                    TypeValue::TypeId(n) => {
                        w.u8(8);
                        w.u64(n as u64);
                    }
                }
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

// The one list of the type-level operators: the codec's two sides and the
// exhaustive [`TypeOperator::ALL`] list the round-trip tests iterate are
// generated from it.  Each tag is the persisted artifact tag — the same
// compatibility contract as the kind-marker tags above.  The write side is
// an exhaustive match over the enum, so an enum variant missing from this
// list fails to compile.
macro_rules! define_type_operator_codec {
    ($( $variant:ident = $tag:literal; )*) => {
        impl TypeOperator {
            /// Every variant — generated from the same list as the codec, so
            /// the round-trip tests iterate the codec's full domain.
            pub const ALL: &[TypeOperator] = &[$(TypeOperator::$variant),*];
        }

        impl OperatorCodec for TypeOperator {
            fn write_operator(w: &mut Writer, op: Self) {
                match op {
                    $(TypeOperator::$variant => w.u8($tag),)*
                }
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
}

// The highlevel program's operator vocabulary: a flat union of the
// structural [`LowOperator`] and the highlevel type-level
// [`TypeOperator`], each carried whole as one sibling variant.
// [`HighProgram`]'s second type parameter lets a downstream that needs more
// operators compose its own union over these same leaves and still reuse the
// lowlevel runtime/registry machinery.
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
    ) -> V {
        match self {
            // The structural operators never reach `run`: the VM dispatches
            // them through `AsEnum` before falling through.
            HighProgramOperator::LowOperator(_) => {
                unreachable!("structural operators are dispatched by the VM")
            }
            // The type-level operators are the highlevel's own computation —
            // delegated to the program-generic [`OperatorExt`] impl for
            // [`TypeOperator`], so any composed union reuses the same
            // semantics.
            HighProgramOperator::TypeOperator(op) => op.run(operand, _block, module),
        }
    }
}

/// The highlevel's own type-level operators, dispatched as an extension
/// operator by *any* program whose value vocabulary implements [`ValueType`]
/// and whose global state carries the [`HighGlobal`] component — the shipped
/// `LangProgram`, a plugin-built compiler's program, and the highlevel's own
/// default `HighProgramOperator` program all share this one impl (they differ
/// only in which union wraps the operator).  The semantics are the
/// spec-documented ones: `==` is the generalized equality over any two
/// same-typed values, `+ - <=` are Int-only.
impl<P> OperatorExt<P> for TypeOperator
where
    P: Program,
    P::Value: ValueType,
    P::GlobalExt: AsField<HighGlobal>,
{
    fn run(&self, operand: P::Value, _block: BlockId, module: &mut Module<P>) -> P::Value {
        match self {
            TypeOperator::Fresh => {
                let id = AsField::<HighGlobal>::get_mut(&mut module.global_ext).next_type_id();
                P::Value::type_id_value(id)
            }
            TypeOperator::Add | TypeOperator::Sub | TypeOperator::Leq | TypeOperator::Eq => {
                // The VM already deep-evaluates the operand and gates on its
                // parameterized subtree, so an unbound operand is the lazy
                // marker (the definition pass flags the node).
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return P::Value::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("binary operators expect an operand array of [left, right]")
                };
                let operands = operands.items();
                // An unbound side (an empty slot or the lazy marker) keeps
                // the operator lazy.
                let left = module.node_value(operands[0].node);
                let right = module.node_value(operands[1].node);
                if is_unbound(left) || is_unbound(right) {
                    return P::Value::from(LowValue::Parameterized);
                }
                let (Some(left), Some(right)) = (left, right) else {
                    unreachable!("is_unbound covers the empty slot")
                };
                match self {
                    // A non-USize operand is a *reported* type error, not an
                    // invariant violation: the checker pins both operands to
                    // `Int`, so a wrong shape only arrives here through an
                    // argument unify that already failed (recording the
                    // diagnostic) — stay lazy instead of panicking.
                    TypeOperator::Add | TypeOperator::Sub | TypeOperator::Leq => {
                        let to_usize = |value: &P::Value| match value.as_enum() {
                            Some(LowValue::USize(n)) => Some(n),
                            _ => None,
                        };
                        let (Some(left), Some(right)) = (to_usize(&left), to_usize(&right)) else {
                            return P::Value::from(LowValue::Parameterized);
                        };
                        match self {
                            TypeOperator::Add => {
                                P::Value::from(LowValue::USize(left.wrapping_add(right)))
                            }
                            TypeOperator::Sub => {
                                P::Value::from(LowValue::USize(left.wrapping_sub(right)))
                            }
                            TypeOperator::Leq => {
                                P::Value::from(LowValue::USize((left <= right) as usize))
                            }
                            _ => unreachable!("the Int operators are handled above"),
                        }
                    }
                    // `==` is the generalized equality: it compares the two
                    // values whole (two `Int`s, or two type values).  The
                    // checker unifies the operands' types, so a cross-type
                    // comparison is already a reported error before `run`.
                    TypeOperator::Eq => P::Value::from(LowValue::USize((left == right) as usize)),
                    TypeOperator::Fresh => unreachable!("Fresh is handled above"),
                }
            }
        }
    }
}

/// The highlevel's associated-type collector: what the checker is generic
/// over.  It extends the lowlevel [`Program`] (which carries `Value`,
/// `Operator`, `GlobalExt`, `PackageMeta`) with the one thing the highlevel
/// needs on top — the attribute type carried by an expression's schema
/// ([`Schema`](crate::ir::Schema)).  A language frontend implements this for
/// its own compiled program, plugging in a concrete attribute (e.g.
/// `Perspective`); the checker never names a concrete attribute, only
/// `Self::Attr`.
pub trait HighProgram: Program {
    /// The compile-time attribute type an expression's schema may carry: a
    /// composed attribute *set* ([`AttrSet`]), which also owns the canonical
    /// order the checker lays attributes out in.  `NoAttr` (highlevel's inert
    /// single-attribute set) is the default — a program with no attribute
    /// extension — while a language plugs in its own (e.g. the composed
    /// `Perspective` + `Doc` set).
    type Attr: AttrSet;
    /// The literal vocabulary — a downstream's composed `enum_ext!` union (or
    /// the built-in [`HighProgramLiteral`] for the default).  Every literal
    /// node carries a value of this type; the checker builds it through
    /// [`LiteralExt::build`].
    type Literal: LiteralExt<Self>;
}

/// The highlevel's concrete lowlevel program: a marker generic over the value
/// vocabulary, the operator vocabulary, the attribute type, *and* the literal
/// vocabulary.
///
/// The default [`HighProgramOperator`] is what the checked highlevel builder
/// emits.  A downstream that needs additional lowlevel operators can compose
/// its own operator enum with `lichen_utils::enum_ext!` (carrying
/// [`LowOperator`] and [`TypeOperator`] as siblings, plus its own attribute
/// operators) and use `Module<ProgramImpl<V, MyOperator, MyAttr>>`; the
/// runtime/static-module/registry machinery is then reusable with the extended
/// operator set.
pub struct ProgramImpl<
    V: ValueType = HighProgramValue,
    O: std::fmt::Debug + Copy + PartialEq = HighProgramOperator,
    A: AttrSet = NoAttr,
    L = HighProgramLiteral,
    G: GlobalExt = HighGlobalExt,
>(#[doc(hidden)] pub PhantomData<(V, O, A, L, G)>);

// The marker's `Debug`/`Clone`/`Copy`/`PartialEq` are structural: the single
// `PhantomData` field is `Clone`/`Copy`/`Debug`/`PartialEq` for *any* type
// argument, so the impls carry only the struct's own bounds and never demand
// `G: Debug`/`G: Copy` — keeping `GlobalExt` flexible (it only promises
// `Default`).
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
