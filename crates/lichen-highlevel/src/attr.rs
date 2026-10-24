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
/// The order is the single allocation authority for expr slots: the attribute
/// at order index `i` occupies expr slot `shape::attr_slot(i)`, so no
/// attribute can claim an index that is not its position.
pub trait AttrSet: AttrSpec {
    /// Every attribute this set can carry, in the canonical order — the expr
    /// layout. Checked by [`order_is_canonical`].
    const ORDER: &'static [Self];

    /// This attribute's index in the set's canonical order: `0` for the first,
    /// at expr slot `shape::attr_slot(0)`.
    ///
    /// # Invariant
    /// It is an *order*, not an expr slot: an expression's expr is dense over the
    /// attributes it carries, so a carried attribute's expr slot is its position
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

/// The highlevel's default attribute: `AttrExt` is inert — only the literal
/// hook answers `None`; no schema carries it.
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

/// The inert extension answers only the literal hook (with `None`); every
/// other entry point is unreachable.
impl<P> AttrExt<P> for NoAttr
where
    P: HighProgram,
    P::Value: ValueType,
{
    fn missing_value(&self) -> Option<LowValue> {
        None
    }

    fn combine(&self, _ctx: &mut dyn Ctx<P>, _children: &[NodeId]) -> NodeId {
        unreachable!("the inert attribute set carries no schema")
    }

    fn unify_slots(&self, _ctx: &mut dyn Ctx<P>, _a: NodeId, _b: NodeId, _loc: Loc) {
        unreachable!("the inert attribute set carries no schema")
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
    /// `[missing_value, int]` expr, else a fresh cell.
    fn missing_slot(&self, ctx: &mut dyn Ctx<P>) -> NodeId {
        let value = match self.missing_value() {
            Some(value) => ctx.value_node(P::Value::from(value)),
            None => ctx.fresh(),
        };
        ctx.expr(value, ctx.int_type())
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

    /// The value a **literal** carries in this attribute's position, or `None`
    /// when a literal states nothing there (`@any`).
    ///
    /// # Invariant
    /// A literal is an expression, so it decides its own attributes: this hook is
    /// what it decides with, and it never depends on an annotation around it
    /// (spec §1, "Literals are expressions").
    fn literal_value(&self) -> Option<LowValue> {
        None
    }

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

    /// What a failed [`Self::constraint`] should say; `None` reads the failure's own
    /// value.
    ///
    /// # Invariant
    /// The spelling is a fact of the **annotation**: the same attribute answers the
    /// same spelling on every failure of its template.
    fn assert_spelling(&self) -> Option<crate::diagnostic::AssertSpelling> {
        None
    }

    /// The condition this attribute imposes on the annotated expression's own
    /// value, or `None` when it constrains nothing.
    ///
    /// # Invariant
    /// `value_expr` is the annotated expression's `[value, type]` term and `slot`
    /// its own attribute node on it, whose interior only the attribute reads. The
    /// checker registers the node through its assert channel, which
    /// deep-evaluates and requires `USize(1)`, and the apply clone re-checks the
    /// instantiated condition per call.
    fn constraint(
        &self,
        _ctx: &mut dyn Ctx<P>,
        _value_expr: NodeId,
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
    /// **nested expr** cannot say which attributes that expr carries; it reads the
    /// name through [`expr_label`].
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

/// The **name** `expr` reads as, when one of its attributes names it — the
/// reader for an expr inside a slot.
///
/// # Invariant
/// An expr's arity is in the graph but *which* attribute owns each tail slot is
/// not, so the search asks **every** attribute in canonical order and the first
/// answer wins. It is sound because an attribute answers only about content it
/// recognises as its own. A *static* slot is skipped: an attribute reads a
/// dynamic node.
pub fn expr_label<P>(
    module: &Module<P>,
    expr: NodeId,
    attrs: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
) -> Option<String>
where
    P: HighProgram,
    P::Value: ValueType,
{
    let value = module.node_value(AnyNodeId::Dynamic(expr))?;
    let value = lichen_utils::extend::AsEnum::<LowValue>::as_enum(&value)?;
    for marker in P::Attr::ORDER {
        let Some(AnyNodeId::Dynamic(slot)) =
            module.array_attribute(&value, marker.order_index() as u32)
        else {
            continue;
        };
        if let Some(label) = attrs(marker).label(module, slot) {
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

/// The **value node** of an attribute slot: a `[value, type]` expr's element
/// 0, or the slot itself when already bare.
///
/// # Invariant
/// Comparing two slots instead unifies a `[value, type]` expr against the
/// other side — for a refinement, the predicate's own self-referential
/// `[Function(fid), ↺]` type, whose positional descent walks into its cycle
/// and reports a conflict (`docs/notes/attributes.md` §"the gate must compute
/// its operands"). A slot whose value is not an expr is returned as it stands.
pub fn slot_value_node<P: HighProgram>(ctx: &dyn Ctx<P>, slot: NodeId) -> NodeId
where
    P::Value: ValueType + AsEnum<LowValue>,
{
    let Some(value) = ctx.node_value(slot) else {
        return slot;
    };
    match AsEnum::<LowValue>::as_enum(&value) {
        // SAFETY: `items` is the payload of `ctx.node_value(slot)` — a value
        // of a live node of the checked module.
        Some(LowValue::Array(items)) => match unsafe { items.items() }.first().map(|i| i.node) {
            Some(AnyNodeId::Dynamic(node)) => node,
            _ => slot,
        },
        _ => slot,
    }
}
