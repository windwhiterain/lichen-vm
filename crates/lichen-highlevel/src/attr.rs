//! The attribute extension point: `AttrExt`'s behaviour and the set's
//! canonical order. See `docs/notes/attributes.md`.

use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};

use crate::ir::Loc;
use crate::program::{Ctx, HighProgram, ValueType};
use lichen_utils::extend::AsEnum;

/// The marker bound every attribute type must satisfy: a `Copy` token,
/// deduplicated by equality in the `Schema`'s tail.
pub trait AttrSpec: Clone + Copy + PartialEq + Eq + std::fmt::Debug + 'static {}

/// A composed attribute **set** — what a [`Schema`](crate::ir::Schema) tail
/// holds — and its **canonical order**.
    ///
    /// # Invariant
    /// The order is the single allocation authority for pair slots: the attribute
    /// at order index `i` occupies pair slot `shape::attr_slot(i)`, so no
    /// attribute can claim an index that is not its position.
pub trait AttrSet: AttrSpec {
    /// Every attribute this set can carry, in the canonical order — the pair
    /// layout. Checked by [`order_is_canonical`].
    const ORDER: &'static [Self];

    /// This attribute's index in the set's canonical order: `0` for the first,
    /// at pair slot `shape::attr_slot(0)`.
    ///
    /// # Invariant
    /// It is an *order*, not a pair slot: an expression's pair is dense over the
    /// attributes it carries, so a carried attribute's pair slot is its position
    /// in that expression's schema tail.
    fn order_index(&self) -> usize;
}

/// Whether a set's order is well formed: each attribute's
/// [`AttrSet::order_index`] is its position in [`AttrSet::ORDER`].
pub fn order_is_canonical<A: AttrSet>() -> bool {
    A::ORDER
        .iter()
        .enumerate()
        .all(|(position, attr)| attr.order_index() == position)
}

/// The highlevel's default attribute: its `AttrExt` is never reached, so the
/// checker's machinery is inert.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NoAttr;
impl AttrSpec for NoAttr {}
impl AttrSet for NoAttr {
    /// The inert set holds one attribute and no schema carries it, so the index
    /// is never read.
    const ORDER: &'static [Self] = &[NoAttr];

    /// The first (and only) attribute of the inert set.
    fn order_index(&self) -> usize {
        0
    }
}

/// The compile-time lowering behaviour of one attribute. See
/// `docs/notes/attributes.md`.
pub trait AttrExt<P: HighProgram>
where
    P::Value: ValueType,
{
    /// The value read for an *absent* occurrence; `None` means an **undecided
    /// cell**, which a unify binds.
    fn missing_value(&self) -> Option<LowValue>;

    /// The slot read for an absent occurrence: the uniform
    /// `[missing_value, int]` pair, else a fresh cell.
    fn missing_slot(&self, ctx: &mut dyn Ctx<P>) -> NodeId {
        let value = match self.missing_value() {
            Some(value) => ctx.value_node(P::Value::from(value)),
            None => ctx.fresh(),
        };
        ctx.pair(value, ctx.int_type())
    }

    /// Whether absent occurrences may read one shared missing-slot node rather
    /// than a fresh node per site.
    ///
    /// # Invariant
    /// A shareable missing value must be **concrete**: reconciling two slots is a
    /// real unify, which writes whichever side is undecided. A concrete node is
    /// only ever read; an undecided one is written by whichever occurrence
    /// unified first, and every later occurrence would read the bound value.
    fn share_missing_slot(&self) -> bool {
        false
    }

    /// Combine the sub-expressions' slots into one node; `children` are compiled,
    /// pre-padded with [`Self::missing_value`].
    fn combine(&self, ctx: &mut dyn Ctx<P>, children: &[NodeId]) -> NodeId;

    /// Unify two attribute slots: `a` is the found side, `b` the expected one,
    /// at the source-blind `loc`.
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc);

    /// The optional subtype relation on this attribute's slot values, consulted
    /// after a failed unify.
    ///
    /// # Invariant
    /// An undecided value returns `false`: the checker suppresses the error
    /// whenever the relation holds, so a conservative answer keeps the check
    /// honest.
    fn is_subtype(&self, _ctx: &dyn Ctx<P>, _sub: NodeId, _super: NodeId) -> bool {
        false
    }

    /// Whether this attribute is a **label** — metadata carrying no constraint,
    /// hence no apply-time constraint slot.
    fn is_label(&self) -> bool {
        false
    }

    /// The condition this attribute imposes on the annotated expression's own
    /// value, or `None` when it constrains nothing.
    ///
    /// # Invariant
    /// `value_pair` is the annotated expression's `[value, type]` term and `slot`
    /// its own attribute node on it, whose interior only the attribute reads. The
    /// checker registers the node through its assert channel, which
    /// deep-evaluates and requires `USize(1)`, and the apply clone re-checks the
    /// instantiated condition per call.
    fn constraint(
        &self,
        _ctx: &mut dyn Ctx<P>,
        _value_pair: NodeId,
        _slot: NodeId,
    ) -> Option<NodeId> {
        None
    }

    /// The **name** this attribute gives the value it attaches to, or `None`.
    ///
    /// # Invariant
    /// The answer is the bare name: how a *labelled value* reads (`?name`) is the
    /// caller's spelling, not the attribute's. An attribute that *describes*
    /// rather than names answers `None` and spells through [`Self::render`].
    fn label(&self, _module: &Module<P>, _slot: NodeId) -> Option<String> {
        None
    }

    /// Render this attribute's slot value in the language's own syntax, or `None`
    /// when it cannot be spelled.
    ///
    /// # Invariant
    /// `attrs` is the composed extension registry, because a slot that holds a
    /// **nested pair** cannot say which attributes that pair carries; it reads the
    /// name through [`pair_label`].
    fn render(
        &self,
        _module: &Module<P>,
        _slot: NodeId,
        _attrs: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
    ) -> Option<String> {
        None
    }

    /// The slot value of an attribute node as a `LowValue`: an empty slot is
    /// `None`, `LowValue::Error` is concrete.
    fn slot_value(&self, module: &Module<P>, slot: NodeId) -> Option<LowValue> {
        module
            .node_value(AnyNodeId::Dynamic(slot))
            .and_then(|v| v.as_enum())
    }
}

/// The **name** `pair` reads as, when one of its attributes names it — the
/// reader for a pair inside a slot.
    ///
    /// # Invariant
    /// A pair's arity is in the graph but *which* attribute owns each tail slot is
    /// not, so the search asks **every** attribute in canonical order and the first
    /// answer wins. It is sound because an attribute answers only about content it
    /// recognises as its own. A *static* slot is skipped: an attribute reads a
    /// dynamic node.
pub fn pair_label<P>(
    module: &Module<P>,
    pair: NodeId,
    attrs: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
) -> Option<String>
where
    P: HighProgram,
    P::Value: ValueType,
{
    // SAFETY: `pair` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { crate::shape::array_items(module, AnyNodeId::Dynamic(pair)) }?;
    for item in items.iter().skip(crate::shape::PAIR_ATTR_BASE) {
        let AnyNodeId::Dynamic(slot) = item.node else {
            continue;
        };
        if let Some(label) = P::Attr::ORDER
            .iter()
            .find_map(|marker| attrs(marker).label(module, slot))
        {
            return Some(label);
        }
    }
    None
}

/// The attribute-extension registry: a marker to the [`AttrExt`] holding its
/// behaviour for a build's lifetime.
    ///
    /// # Invariant
    /// The checker takes it as `Option<AttrExtRegistry<P, P::Attr>>`: `None` is a
    /// build with no attribute extension, and a marker it cannot resolve is a
    /// reported diagnostic, never a panic.
pub type AttrExtRegistry<P, Attr> = Box<dyn Fn(&Attr) -> &'static dyn AttrExt<P>>;

/// The **value node** of an attribute slot: a `[value, type]` pair's element
/// 0, or the slot itself when already bare.
    ///
    /// # Invariant
    /// Comparing two slots instead unifies a `[value, type]` pair against the
    /// other side — for a refinement, the predicate's own self-referential
    /// `[Function(fid), ↺]` type, whose positional descent walks into its cycle
    /// and reports a conflict (`docs/notes/attributes.md` §"the gate must compute
    /// its operands"). A slot whose value is not a pair is returned as it stands.
pub fn slot_value_node<P: HighProgram>(ctx: &dyn Ctx<P>, slot: NodeId) -> NodeId
where
    P::Value: ValueType + AsEnum<LowValue>,
{
    let Some(value) = ctx.class_value(slot) else {
        return slot;
    };
    match AsEnum::<LowValue>::as_enum(&value) {
        // SAFETY: `items` is the payload of `ctx.class_value(slot)` — a value
        // of a live node of the checked module.
        Some(LowValue::Array(items)) => match unsafe { items.items() }.first().map(|i| i.node) {
            Some(AnyNodeId::Dynamic(node)) => node,
            _ => slot,
        },
        _ => slot,
    }
}
