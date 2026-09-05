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
//! hardcoded shape.

use lichen_highlevel::attr::{AttrExt, AttrSpec};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::Loc;
use lichen_highlevel::program::{Ctx, HighProgram, ValueType};
use lichen_lowlevel::{LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

use crate::render::render_struct_fields_named;

/// The doc attribute marker.  Carries no data — the doc's *value* is a
/// runtime node (a user-made struct instance).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Doc;

impl AttrSpec for Doc {}

/// The attribute-extension registry mapping the [`Doc`] marker to its label
/// behaviour.  A host composes [`Doc`] into its attribute vocabulary and
/// passes this to the checker's attribute machinery.
pub fn doc_attr_ext<P>() -> Box<dyn Fn(&Doc) -> &'static dyn AttrExt<P>>
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
    /// The slot this attribute occupies below the `[value, type]` head — a
    /// second attribute after `Perspective`, so the pair is
    /// `[value, type, persp, doc]` in the (future) multi-attribute layout.
    fn slot(&self) -> usize {
        3
    }

    /// The value read for an *absent* occurrence: *no doc*.
    fn missing_value(&self) -> LowValue {
        LowValue::None
    }

    /// A doc never combines over its children: a compound's doc is its own
    /// annotation, not a meet of its children's.  Returns the shared no-doc
    /// value.
    fn combine(&self, ctx: &mut dyn Ctx<P>, _children: &[NodeId]) -> NodeId {
        ctx.value_node(P::Value::from(LowValue::None))
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
    fn render(&self, module: &Module<P>, slot: NodeId) -> Option<String> {
        // The render slot is the `?` expression's `[value, type]` pair.
        let pair = self.slot_value(module, slot)?;
        let LowValue::Array(items) = pair else {
            return None;
        };
        let items = items.items();
        let value = items.first()?.node;
        let ty = items.get(1)?.node;
        let fields = render_struct_fields_named(module, value, ty)?;
        Some(format!("? {fields}"))
    }
}
