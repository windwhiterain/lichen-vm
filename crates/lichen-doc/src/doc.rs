//! The `Doc` attribute: a **label** that attaches a metadata value (a struct
//! instance the user builds — by convention a `Doc` struct with `.name` and
//! `.description` fields) to any expression, spelled `? expr`.
//!
//! `Doc` is a label, not a constraint.  Unlike `Perspective` (whose slot value
//! unifies under the divisibility lattice and is checked at every apply), a
//! `Doc` carries no constraint:
//!
//! - [`AttrExt::combine`] returns *no doc* — a compound's doc is its own
//!   annotation, never a meet of its children's docs.
//! - [`AttrExt::unify_slots`] **propagates** the doc onto the other side and
//!   never reports a failure: it attempts a real unify (so an unbound doc cell
//!   binds to the concrete doc — the doc *passes from one to another*), and
//!   when two already-concrete docs differ, [`AttrExt::is_subtype`] is `true`
//!   so the mismatch is suppressed (the existing doc is kept — the override
//!   case).  `is_subtype` is the attribute's *only* lever for "never
//!   conflicts" — the checker never special-cases a label's unification.
//! - [`AttrExt::is_label`] is `true`, so `Doc` contributes no constraint slot;
//!   the label's runtime slot is the annotation value's `[value, type]` term
//!   pair (so the renderer can walk the value's type chain).  Because a label
//!   is metadata, the `?` expression *is* the value that rides the expression,
//!   so a later `? b` overrides an earlier `? a` naturally.
//!
//! The doc value is just any first-class lichen value (a struct instance), so
//! the type system validates it like any other value.  The checker's slot for
//! the label is the `?` expression's `[value, type]` term pair — the uniform
//! slot shape shared with a constraint (a perspective's lattice value sits in
//! the same pair's element 0) — so the renderer can walk the value's whole
//! type chain: a doc's *field names* come from the struct type, not a
//! hardcoded shape.  That spelling renders through the program-generic
//! [`lichen_render::render_struct_fields_named`].

use lichen_highlevel::attr::{AttrExt, AttrExtRegistry, AttrSpec};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::Loc;
use lichen_highlevel::program::{Ctx, HighProgram, ValueType};
use lichen_lowlevel::{LowValue, Module, NodeId};
use lichen_render::render_struct_fields_named;
use lichen_utils::extend::AsEnum;

/// The doc attribute marker.  Carries no data — the doc's *value* is a
/// runtime node (a user-made struct instance).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Doc;

impl AttrSpec for Doc {}

/// The attribute-extension registry mapping the [`Doc`] marker to its label
/// behaviour.  A host composes [`Doc`] into its attribute vocabulary and
/// passes this to the checker's attribute machinery.
pub fn doc_attr_ext<P>() -> AttrExtRegistry<P, Doc>
where
    P: HighProgram,
    P::Value: ValueType + AsEnum<LowValue>,
{
    Box::new(|_: &Doc| -> &'static dyn AttrExt<P> { &Doc })
}

impl<P> AttrExt<P> for Doc
where
    P: HighProgram,
    P::Value: ValueType + AsEnum<LowValue>,
{
    /// The value read for an *absent* occurrence: *no doc* — spelled as **no
    /// value at all**, because an absent doc is an *unbound cell* by intent:
    /// a real unify binds it, so a doc passes from one side to the other.
    fn missing_value(&self) -> Option<LowValue> {
        None
    }

    /// A doc never combines over its children: a compound's doc is its own
    /// annotation, not a meet of its children's.  Returns an unbound cell —
    /// the per-site no-doc marker.
    fn combine(&self, ctx: &mut dyn Ctx<P>, _children: &[NodeId]) -> NodeId {
        ctx.fresh()
    }

    /// Propagate the doc and never fail: a real unify (an unbound doc cell
    /// binds to the concrete doc — the doc *passes from one to another*), then
    /// `is_subtype` is `true` so two differing concrete docs never conflict
    /// (the existing doc is kept — the override case).
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc) {
        ctx.check_unify_relaxed(a, b, loc, DiagKind::Attribute, &|ctx, value, declared| {
            // `is_subtype` is always `true` for a doc, so the relaxed unify
            // never reports a mismatch — the existing doc is kept (override).
            self.is_subtype(ctx, value, declared)
        });
    }

    /// A doc is a label: an annotation's value replaces, never merges.
    fn is_label(&self) -> bool {
        true
    }

    /// Two differing docs are always compatible — never an error.
    fn is_subtype(&self, _ctx: &dyn Ctx<P>, _sub: NodeId, _super: NodeId) -> bool {
        true
    }

    /// A doc spells `? <named fields>` — the slot is the annotation value
    /// expression's `[value, type]` term pair, so the field *names* come from
    /// the value's struct type chain (never a hardcoded shape).
    fn render(
        &self,
        module: &Module<P>,
        slot: NodeId,
        _attrs: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
    ) -> Option<String> {
        // The render slot is the `?` expression's `[value, type]` pair.
        let pair = self.slot_value(module, slot)?;
        let LowValue::Array(items) = pair else {
            return None;
        };
        // SAFETY: `items` is the payload of the value read from the live node
        // `slot` of `module`.
        let items = unsafe { items.items() };
        let value = items.first()?.node;
        let ty = items.get(1)?.node;
        let fields = render_struct_fields_named(module, value, ty)?;
        Some(format!("? {fields}"))
    }

    /// A **string** doc *names* the value it attaches to: `in_num ? "in_num" =
    /// v => …` gives the predicate the name `in_num`, and that name is the one
    /// thing a value that cannot print itself — a function — can be spelled by
    /// (`docs/notes/operator-polymorphism.md` §8.1).  The answer is the bare
    /// name; a labelled value *reads* as `?name`, which is the caller's
    /// spelling ([`lichen_render::value_label`]).  A struct doc describes
    /// instead of naming, so it answers `None` here and spells through
    /// [`Self::render`].
    fn label(&self, module: &Module<P>, slot: NodeId) -> Option<String> {
        let pair = self.slot_value(module, slot)?;
        let LowValue::Array(items) = pair else {
            return None;
        };
        // SAFETY: `items` is the payload of the value read from the live node
        // `slot` of `module`.
        let items = unsafe { items.items() };
        let value = items.first()?.node;
        match module.node_value(value).and_then(|v| v.as_enum()) {
            Some(LowValue::Str(name)) => Some(name.to_string()),
            _ => None,
        }
    }
}
