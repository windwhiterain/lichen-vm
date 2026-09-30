//! The attribute extension point.
//!
//! A [`Schema`](crate::ir::Schema) names *which* compile-time attribute an
//! expression carries; an attribute's **lowering behaviour** lives in an
//! [`AttrExt`], and *where* it sits in the pair is the composed attribute
//! set's business ([`AttrSet::order_index`]).  The checker is
//! attribute-agnostic: it reads the schema, builds the runtime pair at exactly
//! the schema's arity, and pads an absent attribute with the extension's
//! `missing_slot` at every unify site — it never names "perspective = gcd,
//! missing = 0".  Every attribute's slot is the annotation value's
//! `[value, type]` term pair (the uniform shape): a perspective's lattice
//! value is that pair's element 0, a doc's metadata is the whole pair.  Those
//! semantics live only in a concrete attribute (in a language layer, e.g.
//! `Perspective` in `lichen-perspective`) and in the operator it emits.
//!
//! A concrete attribute is a marker implementing [`AttrSpec`]; highlevel ships
//! two of them — `NoAttr` (the default, an empty attribute whose extension is
//! never reached) and the trait plumbing — while a language adds its own.
//! A marker supplies *behaviour* only: it never names a slot number.  The
//! order a set of attributes occupies the pair in is the **canonical
//! attribute order**, declared once by the composition (see
//! [`AttrSet`]) and read by everyone who lays attributes out.

use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};

use crate::ir::Loc;
use crate::program::{Ctx, HighProgram, ValueType};
use lichen_utils::extend::AsEnum;

/// The marker bound every attribute type must satisfy: a plain, hashable,
/// interning-friendly token (the [`Schema`](crate::ir::Schema)`::tail` entries
/// are deduplicated by equality).  `Copy` is required because a program's
/// attribute type travels in a `Copy` program marker.
pub trait AttrSpec: Clone + Copy + PartialEq + Eq + std::fmt::Debug + 'static {}

/// A composed attribute **set** — the type a [`Schema`](crate::ir::Schema)
/// tail holds (one entry per attached attribute) — together with its
/// **canonical order**.
///
/// The order is the single allocation authority for pair slots: the attribute
/// at order index `i` occupies pair slot [`shape::attr_slot(i)`], and the
/// frontend's tail, the checker's slot merge, the apply-time attribute check
/// and every reader agree because they all ask the set instead of keeping
/// their own list.  A composition implements this by *deriving* the index
/// from its manifest's `attrs` list (see
/// `lichen_language::lang_compose_vocabulary!`), which is what makes slot
/// assignment collision-free by construction: no attribute can claim an index
/// that is not its position, so adding one is a one-list edit.
///
/// [`shape::attr_slot(i)`]: crate::shape::attr_slot
pub trait AttrSet: AttrSpec {
    /// Every attribute this set can carry, in the canonical order.  The
    /// attribute at index `i` occupies pair slot [`shape::attr_slot(i)`], so
    /// this slice *is* the pair layout.  A composition emits it from its
    /// manifest list; the invariant that makes it collision-free is that
    /// [`Self::order_index`] is an attribute's position in it (checked by
    /// [`order_is_canonical`]).
    ///
    /// [`shape::attr_slot(i)`]: crate::shape::attr_slot
    const ORDER: &'static [Self];

    /// This attribute's index in the set's canonical order — `0` for the first
    /// attribute, which sits at pair slot [`shape::attr_slot(0)`].  It is an
    /// *order*, not a pair slot: an expression's pair is dense over the
    /// attributes it actually carries, so the pair slot of a carried
    /// attribute is its position in that expression's schema tail.
    ///
    /// [`shape::attr_slot(0)`]: crate::shape::attr_slot
    fn order_index(&self) -> usize;
}

/// Whether a set's canonical order is well formed: every attribute's
/// [`AttrSet::order_index`] is its position in [`AttrSet::ORDER`], so no two
/// attributes of the set can claim the same slot.  A generated set asserts this
/// at build time (a `const`-evaluated check in the composition macro); a
/// hand-written set is checked in debug builds when a checker is built over it.
pub fn order_is_canonical<A: AttrSet>() -> bool {
    A::ORDER
        .iter()
        .enumerate()
        .all(|(position, attr)| attr.order_index() == position)
}

/// The highlevel's default attribute: a program with no attribute extension.
/// Its `AttrExt` is never reached — no schema carries it — so the checker's
/// attribute machinery is inert.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NoAttr;
impl AttrSpec for NoAttr {}
impl AttrSet for NoAttr {
    /// The inert set holds exactly one attribute, so its order is the
    /// single-member list; no schema ever carries it, so the index is never
    /// read.
    const ORDER: &'static [Self] = &[NoAttr];

    /// The first (and only) attribute of the inert set.
    fn order_index(&self) -> usize {
        0
    }
}

/// The compile-time lowering behaviour of one attribute.
///
/// The checker knows only the *shape* — "an attribute combines over its
/// children (their meet), an absent occurrence reads `missing_value`, and two
/// slots unify by `unify_slots`".  Every concrete operation an attribute needs
/// is supplied here, by the layer that defines the attribute.
///
/// **No layout here.**  Which pair slot an attribute occupies is *not* a
/// property of its behaviour: it is its position in the composed set's
/// canonical order ([`AttrSet::order_index`]), so a plugin supplies semantics
/// and the composition assigns the slot.
pub trait AttrExt<P: HighProgram>
where
    P::Value: ValueType,
{
    /// The value read for an *absent* occurrence of this attribute.  A
    /// perspective reads `USize(0)`: neutral in `gcd`, concrete in equality
    /// unify.
    fn missing_value(&self) -> LowValue;

    /// The slot node read for an *absent* occurrence of this attribute.
    ///
    /// The default builds a `[missing_value, int]` term pair — the uniform
    /// slot shape every attribute uses (the checker always pushes the
    /// annotation value's `[value, type]` pair, so an absent slot must be the
    /// same shape).  A perspective therefore reads `[0, int]` (its missing
    /// lattice value in value position).  An attribute whose missing value is
    /// not a term expression overrides this.
    fn missing_slot(&self, ctx: &mut dyn Ctx<P>) -> NodeId {
        let value = ctx.value_node(P::Value::from(self.missing_value()));
        ctx.pair(value, ctx.int_type())
    }

    /// Whether every occurrence of an absent attribute may read **one** shared
    /// missing-slot node, installed once per build, instead of a fresh node per
    /// site.  The attribute decides this about its own value; the checker only
    /// asks and caches the answer.
    ///
    /// **The contract a shareable missing value must satisfy: it is concrete.**
    /// Reconciling two slots is a real unification ([`Self::unify_slots`] goes
    /// through `check_unify_relaxed`), and a unify *writes* whichever side is
    /// unbound.  A concrete node is only ever read, so one node can serve every
    /// occurrence; an unbound one would be written by whichever occurrence
    /// unified first, and every later occurrence would read the bound value.
    ///
    /// That is the whole difference between the two attributes highlevel ships
    /// against: a perspective's absent form is the constant `0` (the lattice's
    /// meet identity), so it shares; a doc's absent form is an **unbound cell**,
    /// which a unify binds on purpose so a doc passes from one side to the other
    /// ([`lichen_doc`]), so it must be fresh per site — sharing it would let the
    /// first bind poison every later read.
    ///
    /// Defaults to `false`: an attribute opts in by stating that its missing
    /// value is concrete, and an attribute that says nothing behaves exactly as
    /// before.
    fn share_missing_slot(&self) -> bool {
        false
    }

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
    /// expression but carries no constraint.  The checker's slot handling is
    /// uniform — every attribute's runtime pair slot is the annotation value's
    /// `[value, type]` term pair (a constraint reads its lattice value from
    /// element 0; a label's renderer walks the value's whole type chain) — so
    /// `is_label` selects only the *semantic* difference: a label contributes
    /// no apply-time constraint slot and is never validated against the
    /// provider, while a constraint is.  Whether a label *conflicts* is
    /// decided solely by [`Self::is_subtype`] — a metadata attribute overrides
    /// it to `true`, so the checker never has to special-case a label's
    /// unification.  A constraint attribute (e.g. `Perspective`) returns
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
    /// for [`Self::render`].  Returns the value as a `LowValue` enum.  Only
    /// the unbound marker is filtered (an unbound slot spells nothing); a
    /// computed nothing ([`LowValue::Void`]) is a concrete slot value and
    /// passes through.
    fn slot_value(&self, module: &Module<P>, slot: NodeId) -> Option<LowValue> {
        module
            .node_value(AnyNodeId::Dynamic(slot))
            .and_then(|v| v.as_enum())
            .filter(|v| !matches!(v, LowValue::Parameterized))
    }
}
