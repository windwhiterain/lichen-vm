//! The **refinement** attribute: a predicate that must evaluate to `1`. See
//! `docs/notes/operator-polymorphism.md` §3.1.

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

use crate::attr::{AttrExt, AttrExtRegistry, AttrSpec, slot_value_node};
use crate::diagnostic::DiagKind;
use crate::ir::Loc;
use crate::program::{Ctx, HighProgram, ValueType};

/// The refinement attribute's marker; its value is the predicate's own node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Refinement;

impl AttrSpec for Refinement {}

/// The attribute-extension registry mapping [`Refinement`] to its behaviour.
pub fn refinement_attr_ext<P>() -> AttrExtRegistry<P, Refinement>
where
    P: HighProgram,
    P::Value: ValueType + AsEnum<LowValue>,
    P::Operator: From<LowOperator>,
{
    Box::new(|_: &Refinement| -> &'static dyn AttrExt<P> { &Refinement })
}

impl<P> AttrExt<P> for Refinement
where
    P: HighProgram,
    P::Value: ValueType + AsEnum<LowValue>,
    P::Operator: From<LowOperator>,
{
    /// *No refinement* — **no value at all**: a concrete one conflicts with
    /// every predicate.
    fn missing_value(&self) -> Option<LowValue> {
        None
    }

    /// A fresh undecided cell: a refinement never combines over its children —
    /// it is its own annotation.
    fn combine(&self, ctx: &mut dyn Ctx<P>, _children: &[NodeId]) -> NodeId {
        ctx.fresh()
    }

    /// Reconcile two slots with a **plain unify**: differing predicates conflict.
    /// See `docs/notes/attributes.md`.
    ///
    /// # Invariant
    /// The comparison is of the two slots' **value** nodes
    /// ([`slot_value_node`]), not of the slots: the predicate is a slot's `[value,
    /// type]` pair's element 0, and comparing slots would unify that pair against
    /// the predicate's own self-referential `[Function(fid), ↺]` type, so the
    /// descent walks into the cycle and refuses a refinement that should pass.
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc) {
        let a = slot_value_node(ctx, a);
        let b = slot_value_node(ctx, b);
        ctx.check_unify(a, b, loc, DiagKind::Attribute);
    }

    /// The refinement's enforcement: `predicate value`, whose result the assert
    /// channel requires to be `1`.
    ///
    /// # Invariant
    /// The slot is the predicate expression's `[value, type]` pair, so element 0
    /// is the predicate's own value node — no re-wrapping, and the apply's
    /// subject is the very function the user wrote.
    fn constraint(&self, ctx: &mut dyn Ctx<P>, value_pair: NodeId, slot: NodeId) -> Option<NodeId> {
        let pair = ctx.class_value(slot)?;
        let Some(LowValue::Array(items)) = pair.as_enum() else {
            return None;
        };
        // SAFETY: `items` is the payload of a value read from this build's
        // module, which outlives the read.
        let items = unsafe { items.items() };
        let predicate = match items.first()?.node {
            AnyNodeId::Dynamic(predicate) => predicate,
            // A *static* predicate is frozen in another module; the slot is
            // always built here, so this cannot arise.
            AnyNodeId::Static(_) => return None,
        };
        let result = ctx.fresh();
        let operands = ctx.array_node(&[predicate, value_pair, result]);
        let apply = ctx.op_node(P::Operator::from(LowOperator::Apply), Some(operands));
        // An apply node **is** its `[value, type]` pair: the value is element
        // 0, the pair is neither `1` nor *lazy*.
        let zero = ctx.value_node(P::Value::from(LowValue::USize(0)));
        let read = ctx.array_node(&[apply, zero]);
        Some(ctx.op_node(P::Operator::from(LowOperator::Index), Some(read)))
    }

    /// The refinement spells `! <the predicate's name>`, found by the general
    /// [`pair_label`] search.
    fn render(
        &self,
        module: &Module<P>,
        slot: NodeId,
        attrs: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
    ) -> Option<String> {
        Some(format!(
            "! {}",
            crate::attr::pair_label(module, slot, attrs)?
        ))
    }
}
