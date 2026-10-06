//! The **refinement** attribute: a predicate on an expression's own value,
//! required to evaluate to `1`.
//!
//! `docs/notes/operator-polymorphism.md` §3 is the design.  In one paragraph:
//! an operator's contract is not a type but a refinement — classical
//! `{v | p v}`, with the base type left as the ordinary inference cell — and it
//! is carried as this attribute, whose slot holds **exactly one predicate
//! function**.  One is enough because a conjunction is one function written with
//! `*` (`0 * 1 = 0`): `x : Int ! (v => v > 3) ! (v => v < 10)` lowers to a
//! single `v => (v > 3) * (v < 10)`.
//!
//! # The four decisions, and why
//!
//! - **Nothing propagates.**  `combine` returns a fresh unbound cell rather than
//!   deriving a refinement from the children's, which is what a *lattice*
//!   attribute (`Perspective`) does.  It cannot: the frontend **folds**
//!   computations — an `a == b` on known operands is folded away, a `let` is
//!   substituted, constant arithmetic is evaluated — so by the time a parent is
//!   built, the children that produced its value may no longer exist as
//!   expressions.  Inferring which predicates a computed value satisfies is a
//!   solver's job.  A refinement therefore exists only where it was written.
//! - **An absent refinement is an unbound cell** (no value), not a
//!   concrete "no refinement" value, and it is **not shared** across sites.
//!   Both follow from the reconciliation being a *plain unify*:
//!   [`AttrExt::share_missing_slot`]'s contract says a unify writes whichever
//!   side is unbound, so a concrete absent value could be shared and an unbound
//!   one must not be; and `unify(Error, predicate)` would conflict, so no
//!   refinement could ever pass from one side to the other.  Being an unbound
//!   cell is what lets an annotation's predicate flow into an argument's slot —
//!   the propagation the language already has for types.
//! - **Reconciliation is a plain unify**, so requiring the *same* refinement
//!   rather than a weaker one is over-strict.  Deliberately so: weakening needs
//!   implication between predicates (`fact ⊨ requirement`), which is the
//!   subtyping this language does not have.  A unify never wrongly *accepts*,
//!   so the strictness costs expressiveness and buys soundness.  This is the one
//!   row where the refinement differs from [`lichen_doc::Doc`], which resolves
//!   its slot with an always-true subtype so a later doc overrides.
//! - **Enforcement is the assert channel**, through [`AttrExt::constraint`]: the
//!   predicate applied to the annotated value, registered like any assert.  No
//!   new lowlevel mechanism — the association "which value does this constraint
//!   refine?" *is* the condition node, because this attribute applied the
//!   predicate to that value.

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

use crate::attr::{AttrExt, AttrExtRegistry, AttrSpec};
use crate::diagnostic::DiagKind;
use crate::ir::Loc;
use crate::program::{Ctx, HighProgram, ValueType};

/// The refinement attribute's marker.  Carries no data — the refinement's
/// *value* is the predicate expression's own runtime node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Refinement;

impl AttrSpec for Refinement {}

/// The attribute-extension registry mapping the [`Refinement`] marker to its
/// behaviour.  A host composes [`Refinement`] into its attribute vocabulary and
/// passes this to the checker's attribute machinery.
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
    /// *No refinement* — spelled as **no value at all**, because an absent
    /// refinement is an **unbound cell** by intent: the plain unify that
    /// reconciles two slots binds it, so a refinement passes from one side to
    /// the other.  See the module docs for why it cannot be a concrete value.
    fn missing_value(&self) -> Option<LowValue> {
        None
    }

    /// A refinement never combines over its children: it is its own annotation,
    /// not a meet of its children's.  Returns a fresh unbound cell — the
    /// per-site no-refinement marker (per-site, because a unify may bind it).
    fn combine(&self, ctx: &mut dyn Ctx<P>, _children: &[NodeId]) -> NodeId {
        ctx.fresh()
    }

    /// Reconcile two refinement slots with a **plain unify**: the same predicate
    /// is required, and two differing predicates conflict.  Over-strict by
    /// decision — the alternative is predicate implication, which this language
    /// has no subtyping to express.
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc) {
        ctx.check_unify(a, b, loc, DiagKind::Attribute);
    }

    /// The refinement's enforcement: `predicate value`, an ordinary lichen
    /// apply whose result the assert channel requires to be `1`.
    ///
    /// The slot is the predicate expression's `[value, type]` pair, so element 0
    /// is the predicate's own value node — no re-wrapping, and the node identity
    /// is preserved so the apply's subject is the very function the user wrote.
    fn constraint(&self, ctx: &mut dyn Ctx<P>, value_pair: NodeId, slot: NodeId) -> Option<NodeId> {
        let pair = ctx.class_value(slot)?;
        let Some(LowValue::Array(items)) = pair.as_enum() else {
            return None;
        };
        // SAFETY: `items` is the payload of the value read from the live node
        // `slot` of this build's module, which outlives the read.
        let items = unsafe { items.items() };
        let predicate = match items.first()?.node {
            AnyNodeId::Dynamic(predicate) => predicate,
            // A *static* predicate is a frozen node of another module; the
            // annotation's own slot is always built in this module, so this
            // cannot arise, and staying silent is the conservative answer rather
            // than inventing a node.
            AnyNodeId::Static(_) => return None,
        };
        let result = ctx.fresh();
        let operands = ctx.array_node(&[predicate, value_pair, result]);
        let apply = ctx.op_node(P::Operator::from(LowOperator::Apply), Some(operands));
        // An apply node **is** its return `[value, type]` pair, so the
        // condition's own value is that pair's element 0 — the same read the
        // checker's `value_of` builds.  Registering the pair itself would hand
        // the assert channel an array, which is neither `1` nor *lazy*, and it
        // would report a failure even while the applied value is still open.
        let zero = ctx.value_node(P::Value::from(LowValue::USize(0)));
        let read = ctx.array_node(&[apply, zero]);
        Some(ctx.op_node(P::Operator::from(LowOperator::Index), Some(read)))
    }

    /// The refinement spells `! <the predicate's name>`.  The slot holds the
    /// predicate's own pair, so the name is the *predicate's* label — found
    /// through the general [`pair_label`] search, because which attributes that
    /// pair carries is not in the graph.  A predicate with no labelled doc
    /// spells nothing: a function value has no source form, and a made-up one
    /// could not be spelled back.
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

// The refinement's spelling is `! <the predicate's name>`: the slot holds the
// predicate's own pair, so the name is the *predicate's* label, found through
// the general [`pair_label`] search (`docs/notes/operator-polymorphism.md`
// §8.1).  A predicate with no labelled doc spells nothing — honest, because a
// function value has no source form to print, and a made-up one could not be
// spelled back.
