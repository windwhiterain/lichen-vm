//! The perspective semantics: the attribute marker, its lattice lowering, and
//! the n-ary `gcd` operator leaf.

use lichen_highlevel::attr::{AttrExt, AttrExtRegistry, AttrSpec, slot_value_node};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::Loc;
use lichen_highlevel::program::{Ctx, HighProgram, ValueType};
use lichen_lowlevel::codec::{OperatorCodec, Reader, Writer};
use lichen_lowlevel::{AnyNodeId, BlockId, LowValue, Module, NodeId, OperatorExt, Program};
use lichen_utils::extend::AsEnum;

/// The perspective's operator leaf: the n-ary `gcd` meet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GcdOp {
    /// n-ary `gcd` over the operand array; an empty array is `0`, the meet
    /// identity.  A lazy operand declines.
    Gcd,
}

// The perspective leaf's per-leaf artifact codec: the single `Gcd` operator.
impl OperatorCodec for GcdOp {
    fn write_operator(w: &mut Writer, op: Self) -> Result<(), String> {
        match op {
            GcdOp::Gcd => w.u8(0),
        }
        Ok(())
    }

    fn read_operator(r: &mut Reader<'_>) -> Result<Self, String> {
        match r.u8()? {
            0 => Ok(GcdOp::Gcd),
            tag => Err(format!("unknown gcd-operator tag {tag}")),
        }
    }
}

/// The perspective attribute marker; `# p` is uniform over `p` aligned threads.
///
/// # Invariant
///
/// The lattice is divisibility: meet is `gcd`, top is `0` (uniform over all
/// threads, the `∞` fold), bottom is `1`.  An unannotated value is the top.
/// The marker carries no data — the perspective's value is a runtime node.  See
/// docs/notes/attributes.md.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Perspective;

impl AttrSpec for Perspective {}

/// `gcd` over the divisibility lattice: the meet.  `gcd(n, 0) = n`.
pub fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Whether `sub` divides `sup`: the subtype order `sub ⊑ sup ⟺ sub | sup`.
///
/// # Invariant
///
/// `0` is the top, so `sub = 0` holds only for `sup = 0`; `sup = 0` holds for
/// any `sub`.
pub fn divides(sub: usize, sup: usize) -> bool {
    if sub == 0 {
        sup == 0
    } else {
        sup.is_multiple_of(sub)
    }
}

/// Map the [`Perspective`] marker to its lowering behaviour, for a host that
/// composed the marker into its vocabulary.
pub fn persp_attr_ext<P>() -> AttrExtRegistry<P, Perspective>
where
    P: HighProgram,
    P::Value: ValueType + AsEnum<LowValue>,
    P::Operator: From<GcdOp>,
{
    Box::new(|_: &Perspective| -> &'static dyn AttrExt<P> { &Perspective })
}

/// `GcdOp::run` — the VM dispatch for the injected `Gcd` operator.
///
/// # Invariant
///
/// The operand is the array of the children's attribute slots, pre-padded with
/// the missing value `0` by the checker; a lazy operand stays lazy.
impl<P> OperatorExt<P> for GcdOp
where
    P: Program,
    P::Value: AsEnum<LowValue> + From<LowValue>,
{
    fn run(&self, operand: P::Value, _block: BlockId, module: &mut Module<P>) -> Option<P::Value> {
        // `None` is the trait's spelling of "undecided"; the closure gives the
        // arm's early returns one type to agree on.
        (|| match self {
            GcdOp::Gcd => {
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("Gcd expects an operand array");
                };
                let mut acc = 0;
                // SAFETY: `operands` is the payload of the operand value the
                // VM just evaluated, so its block outlives this run.
                for item in unsafe { operands.items() } {
                    // A child slot is a `[value, type]` pair; its element 0 is
                    // the lattice value.  A bare value is accepted too.
                    let Some(n) = module
                        .node_value(item.node)
                        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
                        .and_then(|value| match value {
                            LowValue::USize(n) => Some(n),
                            LowValue::Array(items) => {
                                // SAFETY: `items` is the payload of a value
                                // read from a live node of `module` just above.
                                let elem0 = unsafe { items.items() }.first()?;
                                match module
                                    .node_value(elem0.node)
                                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                                {
                                    Some(LowValue::USize(n)) => Some(n),
                                    _ => None,
                                }
                            }
                            _ => None,
                        })
                    else {
                        return None;
                    };
                    acc = gcd(acc, n);
                }
                Some(<P::Value as From<LowValue>>::from(LowValue::USize(acc)))
            }
        })()
    }
}

/// The attribute-extension lowering of the [`Perspective`] marker.
impl<P> AttrExt<P> for Perspective
where
    P: HighProgram,
    P::Value: ValueType + AsEnum<LowValue>,
    P::Operator: From<GcdOp>,
{
    /// `0`, the meet identity, so it stays concrete under unify; the absent
    /// slot is `[0, int]` ([`AttrExt::missing_slot`]).
    fn missing_value(&self) -> Option<LowValue> {
        Some(LowValue::USize(0))
    }

    /// The absent form is the concrete constant `0`, so reconciliation reads
    /// it and one node serves every absent occurrence.
    fn share_missing_slot(&self) -> bool {
        true
    }

    /// Perspective combine: a `Gcd` node over the children's `[value, type]`
    /// pairs.  An absent child is `0`, neutral in gcd.
    fn combine(&self, ctx: &mut dyn Ctx<P>, children: &[NodeId]) -> NodeId {
        let operands = ctx.array_node(children);
        let gcd = ctx.op_node(P::Operator::from(GcdOp::Gcd), Some(operands));
        ctx.pair(gcd, ctx.int_type())
    }

    /// Perspective unify: the slots must unify, and two differing values hold
    /// when the declared is a subtype of the value's.
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc) {
        let a = slot_value_node(ctx, a);
        let b = slot_value_node(ctx, b);
        ctx.check_unify_relaxed(a, b, loc, DiagKind::Attribute, &|ctx, value, declared| {
            // A value uniform over `value` threads is usable where `declared`
            // is required iff `declared | value`.
            self.is_subtype(ctx, declared, value)
        });
    }

    /// The subtype order: `sub ⊑ super ⟺ sub | super`.
    ///
    /// # Invariant
    ///
    /// `0` is the top: `sub = 0` holds only for `super = 0`, and `super = 0`
    /// holds for any `sub`.  An undecided value is not a subtype.
    fn is_subtype(&self, ctx: &dyn Ctx<P>, sub: NodeId, sup: NodeId) -> bool {
        let value_of = |node: NodeId| -> Option<usize> {
            let value = ctx.class_value(node)?;
            match AsEnum::<LowValue>::as_enum(&value) {
                Some(LowValue::USize(n)) => Some(n),
                // SAFETY: `items` is the payload of `ctx.class_value(node)` —
                // a value of a live node of the checked module.
                Some(LowValue::Array(items)) => match unsafe { items.items() }.first()?.node {
                    AnyNodeId::Dynamic(n) => {
                        let elem0 = ctx.class_value(n)?;
                        match AsEnum::<LowValue>::as_enum(&elem0) {
                            Some(LowValue::USize(n)) => Some(n),
                            _ => None,
                        }
                    }
                    AnyNodeId::Static(_) => None,
                },
                _ => None,
            }
        };
        let (Some(sub), Some(sup)) = (value_of(sub), value_of(sup)) else {
            return false;
        };
        divides(sub, sup)
    }

    /// A leaf perspective spells `# n`; a compound's meet has no spelling.
    /// Element 0 of the slot is the lattice value.
    fn render(
        &self,
        module: &Module<P>,
        slot: NodeId,
        _attrs: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
    ) -> Option<String> {
        let slot_value = self.slot_value(module, slot)?;
        let n = match slot_value {
            LowValue::USize(n) => n,
            LowValue::Array(items) => match module
                // SAFETY: `items` is the payload of a value read from the
                // live node `slot` of `module` just above.
                .node_value(unsafe { items.items() }.first()?.node)
                .and_then(|v| v.as_enum())
            {
                Some(LowValue::USize(n)) => n,
                _ => return None,
            },
            _ => return None,
        };
        Some(format!("# {n}"))
    }
}
