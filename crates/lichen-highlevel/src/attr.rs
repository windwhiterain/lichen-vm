//! The attribute extension point.
//!
//! A [`Schema`](crate::ir::Schema) names *which* compile-time attribute an
//! expression carries; an attribute's **lowering behaviour** lives in an
//! [`AttrExt`].  The checker is attribute-agnostic: it reads the schema,
//! builds the runtime pair at exactly the schema's arity, and pads an absent
//! attribute with the extension's `missing_value` at every unify site — it
//! never names "perspective = gcd, missing = 0".  Those semantics live only in
//! a concrete attribute (in a language layer, e.g. `Perspective` in
//! `lichen-language`) and in the operator it emits.
//!
//! A concrete attribute is a marker implementing [`AttrSpec`]; highlevel ships
//! two of them — `NoAttr` (the default, an empty attribute whose extension is
//! never reached) and the trait plumbing — while a language adds its own.

use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};

use crate::ir::Loc;
use crate::program::{Ctx, HighProgram, ValueType};
use lichen_utils::extend::AsEnum;

/// The marker bound every attribute type must satisfy: a plain, hashable,
/// interning-friendly token (the [`Schema`](crate::ir::Schema)`::tail` entries
/// are deduplicated by equality).  `Copy` is required because a program's
/// attribute type travels in a `Copy` program marker.
pub trait AttrSpec: Clone + Copy + PartialEq + Eq + std::fmt::Debug + 'static {}

/// The highlevel's default attribute: a program with no attribute extension.
/// Its `AttrExt` is never reached — no schema carries it — so the checker's
/// attribute machinery is inert.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NoAttr;
impl AttrSpec for NoAttr {}

/// The compile-time lowering behaviour of one attribute.
///
/// The checker knows only the *shape* — "an attribute combines over its
/// children (their meet), an absent occurrence reads `missing_value`, and two
/// slots unify by `unify_slots`".  Every concrete operation an attribute needs
/// is supplied here, by the layer that defines the attribute.
pub trait AttrExt<P: HighProgram>
where
    P::Value: ValueType,
{
    /// The slot this attribute occupies below the `[value, type]` head
    /// (a first attribute → 2, so the pair is `[value, type, attr]`).
    fn slot(&self) -> usize;

    /// The value read for an *absent* occurrence of this attribute.  A
    /// perspective reads `USize(0)`: neutral in `gcd`, concrete in equality
    /// unify.
    fn missing_value(&self) -> LowValue;

    /// Combine the direct sub-expressions' attribute slots into one node
    /// (a perspective → the language's meet operator over the operand array, a
    /// lazy operand → `Parameterized`).  `children` are the already-compiled
    /// child slots, pre-padded with [`Self::missing_value`].  Built through
    /// the curated [`Ctx`], never raw lowlevel nodes.
    fn combine(&self, ctx: &mut dyn Ctx<P>, children: &[NodeId]) -> NodeId;

    /// Unify two attribute slots.  Receives the *found/value* side as `a` and
    /// the *expected/declared* side as `b`.  A perspective's default impl is
    /// an equality unify; an attribute that defines [`Self::is_subtype`] may
    /// call [`Ctx::check_unify_relaxed`] instead so a subtype (not just an
    /// exact match) passes.  `loc` is the source-blind location of the check.
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc);

    /// The optional subtype relation on this attribute's slot values: whether
    /// `sub` is a subtype of `super` under its lattice.  The checker consults
    /// it after a failed equality unify ([`Ctx::check_unify_relaxed`]); if
    /// it holds, the failure's error is suppressed and the check passes.
    ///
    /// Default `false` — no subtyping, exact equality is required.  A
    /// concrete attribute (e.g. `Perspective`) overrides it to relax its
    /// apply/`# p` check from equality to a partial order.  Implementations
    /// read the two slot values with [`Ctx::class_value`]; an unbound
    /// value (a runtime-dependent perspective) should return `false`, so the
    /// check stays conservative.
    fn is_subtype(&self, _ctx: &dyn Ctx<P>, _sub: NodeId, _super: NodeId) -> bool {
        false
    }

    /// Whether this attribute is a **label**: metadata that attaches to an
    /// expression but carries no constraint.  A label contributes no apply-time
    /// constraint slot, and its runtime pair slot is the annotation value's
    /// `[value, type]` term pair (so its renderer can walk the value's type
    /// chain).  Whether a label *conflicts* is decided solely by
    /// [`Self::is_subtype`] — a metadata attribute overrides it to `true`, so
    /// the checker never has to special-case a label's unification, only its
    /// metadata slot.  A constraint attribute (e.g. `Perspective`) returns
    /// `false`, keeping its lattice combine/unify behaviour.
    ///
    /// Default `false` — only a metadata attribute overrides it.  The checker
    /// consults this in [`crate::checker::Checker::check_ann`] to choose the
    /// metadata-slot path (label) over the provider/unify path (constraint).
    fn is_label(&self) -> bool {
        false
    }

    /// Render this attribute's slot value in the language's own syntax
    /// (`# 4`, `? name = "five"`), or `None` when it cannot be spelled (an
    /// unbound or runtime-dependent value, or an attribute with no display).
    /// The output printers use it to show the attributes an expression actually
    /// carries: they iterate the expression's schema tail and render every
    /// *present* attribute, so an un-annotated expression spells nothing.
    ///
    /// Default `None` — an attribute that does not override it is not shown.
    fn render(&self, _module: &Module<P>, _slot: NodeId) -> Option<String> {
        None
    }

    /// The slot value of an attribute node, read from the module — a helper
    /// for [`Self::render`].  Returns the value as a `LowValue` enum.
    fn slot_value(&self, module: &Module<P>, slot: NodeId) -> Option<LowValue> {
        module
            .node_value(AnyNodeId::Dynamic(slot))
            .and_then(|v| v.as_enum())
            .filter(|v| !matches!(v, LowValue::None | LowValue::Parameterized))
    }
}
