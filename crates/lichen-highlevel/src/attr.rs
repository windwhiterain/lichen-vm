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

use lichen_lowlevel::{LowValue, NodeId};

use crate::ir::Loc;
use crate::program::{Ctx, HighProgram, ValueType};

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
    /// expression but carries no constraint.  A label attribute is exempt
    /// from the combine-over-children step and from unify failure — two
    /// differing label values never conflict (an annotation's value replaces,
    /// never merges).  A constraint attribute (e.g. `Perspective`) returns
    /// `false`, keeping its lattice combine/unify behaviour.
    ///
    /// Default `false` — only a metadata attribute overrides it.  The checker
    /// consults this in [`crate::checker::Checker::check_ann`] to choose the
    /// replace path (label) over the combine path (constraint).
    fn is_label(&self) -> bool {
        false
    }
}
