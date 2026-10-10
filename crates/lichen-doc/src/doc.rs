//! The `Doc` attribute, `? expr`: a label attaching a metadata value to any
//! expression.  See docs/notes/attributes.md.

use lichen_highlevel::attr::{AttrExt, AttrExtRegistry, AttrSpec};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::Loc;
use lichen_highlevel::program::{Ctx, HighProgram, ValueType};
use lichen_lowlevel::{LowValue, Module, NodeId};
use lichen_render::render_struct_fields_named;
use lichen_utils::extend::AsEnum;

/// The doc attribute marker: carries no data, because the doc's value is a
/// runtime node (a user-made struct instance).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Doc;

impl AttrSpec for Doc {}

/// Map the [`Doc`] marker to its label behaviour, for a host that composed
/// [`Doc`] into its attribute vocabulary.
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
    /// An absent doc is an undecided cell by intent: a real unify binds it, so
    /// a doc passes from one side to the other.
    fn missing_value(&self) -> Option<LowValue> {
        None
    }

    /// A compound's doc is its own annotation, never a meet of its children's.
    fn combine(&self, ctx: &mut dyn Ctx<P>, _children: &[NodeId]) -> NodeId {
        ctx.fresh()
    }

    /// Unify a real cell for an undecided doc, else keep the existing doc: two
    /// differing concrete docs are never a conflict.
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc) {
        ctx.check_unify_relaxed(a, b, loc, DiagKind::Attribute, &|ctx, value, declared| {
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

    /// Spell `? <named fields>` from the pair's struct type chain, so the names
    /// are the value's, never a hardcoded shape.
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

    /// A string doc names the value it attaches to; a struct doc describes
    /// instead.  See docs/notes/operator-polymorphism.md.
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
