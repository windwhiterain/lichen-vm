//! The `Doc` attribute: a **label** that attaches a metadata value (a `Doc`
//! struct instance — `struct<.name string, .description string>`) to any
//! expression, spelled `? doc{…}`.
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
//!   case).
//! - [`AttrExt::is_label`] is `true`, so an annotation's `? b` **replaces** any
//!   existing doc `a` outright (the `check_ann` label branch).
//!
//! A `Doc` value is just a `Doc`-shaped struct instance, so unlike a scalar
//! perspective it is first-class lichen data that the type system validates.

use lichen_highlevel::attr::{AttrExt, AttrSpec};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::Loc;
use lichen_highlevel::program::{Ctx, HighProgram, ValueType};
use lichen_lowlevel::{LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// The doc attribute marker.  Carries no data — the doc's *value* is a
/// runtime node (a `Doc` struct instance).
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
        ctx.check_unify_relaxed(a, b, loc, DiagKind::Attribute, &|_ctx, _value, _declared| true);
    }

    /// A doc is a label: an annotation's value replaces, never merges.
    fn is_label(&self) -> bool {
        true
    }

    /// Two differing docs are always compatible — never an error.
    fn is_subtype(&self, _ctx: &dyn Ctx<P>, _sub: NodeId, _super: NodeId) -> bool {
        true
    }

    /// A doc spells `? doc{ … }` with its field values (a `Doc` struct
    /// instance's positional field tuple).
    fn render(&self, module: &Module<P>, slot: NodeId) -> Option<String> {
        let value = self.slot_value(module, slot)?;
        let LowValue::Array(items) = value else {
            return None;
        };
        let fields: Vec<String> = items
            .items()
            .iter()
            .filter_map(|item| match module.node_value(item.node).and_then(|v| v.as_enum()) {
                Some(LowValue::Str(s)) => Some(format!("\"{s}\"")),
                Some(LowValue::USize(n)) => Some(n.to_string()),
                _ => None,
            })
            .collect();
        Some(format!("? doc{{ {} }}", fields.join(", ")))
    }
}
