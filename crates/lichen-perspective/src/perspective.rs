//! The program-generic perspective semantics: the attribute marker and its
//! divisibility-lattice lowering, plus the n-ary `gcd` operator leaf.
//!
//! Nothing here names a concrete host program — every entry point (the
//! [`AttrExt`] impl, the [`OperatorExt`] impl, the [`persp_attr_ext`] registry)
//! is bounded only by the associated-type constraints a host satisfies when its
//! composed value/operator vocabularies carry [`Perspective`] and [`GcdOp`]
//! alongside the structural leaves.  The host composes those leaves and passes
//! the registry to its checker; the codesign (grammar, IR schema tail, persist
//! discriminator) stays in the host language layer.

use lichen_highlevel::attr::{AttrExt, AttrSpec};
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::Loc;
use lichen_highlevel::program::{Ctx, HighProgram, ValueType};
use lichen_lowlevel::{AnyNodeId, BlockId, LowValue, Module, NodeId, OperatorExt, Program};
use lichen_utils::extend::AsEnum;

/// The perspective's operator leaf: the n-ary `gcd` meet.
///
/// A plain enum, provided whole for a host's operator-vocabulary composition —
/// the same leaf shape as the highlevel's `TypeOperator`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GcdOp {
    /// n-ary `gcd` over the operand array.  An empty operand array (a leaf
    /// with no sub-expressions) evaluates to `0` — the meet identity / top,
    /// so `gcd(n, 0) = n` makes it neutral.  A lazy operand yields
    /// `Parameterized`, like `Add`/`Sub`.
    Gcd,
}

/// The perspective attribute marker: a plain non-negative integer whose
/// lattice is divisibility (`a ⊑ b ⟺ a | b`), meet (`gcd`), top (`0`,
/// "uniform over all threads", the `∞` fold), and bottom (`1`).  A node with
/// an unannotated perspective is *missing*, which in GPU code means "not
/// expressed per-thread" — i.e. uniform over all threads — so it IS the top,
/// `0`.  A `# p`-annotated leaf uses `p`; an unannotated leaf contributes `0`
/// (the top, neutral in `gcd`).  Stage 1 uses only `gcd` and `0`.
///
/// The marker carries no data — the perspective's *value* is a runtime node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Perspective;

impl AttrSpec for Perspective {}

/// `gcd` over the divisibility lattice.  `gcd(n, 0) = n` (the meet identity),
/// `gcd(0, 0) = 0`.
pub fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Whether `sub` divides `sup` in the divisibility lattice — the subtype
/// order `sub ⊑ sup ⟺ sub | sup`.
///
/// `0` is the lattice **top**, "uniform over all threads" (the `∞` fold: a
/// value with no `#` is not expressed per-thread in GPU code, so it is
/// uniform across every thread).  Since Rust integers have no `∞`, the top
/// is encoded `0` — every `n | 0`, and `gcd(n, 0) = n` (the meet identity).
///
/// - `sub = 0` (declared uniform-over-all): only a uniform-over-all value
///   satisfies it, so `divides(0, sup) ⟺ sup == 0`.
/// - `sub > 0`: `sub | sup ⟺ sup % sub == 0`.  `sup = 0` (a uniform-over-all
///   value) satisfies any requirement, since `0 % sub == 0`.
pub fn divides(sub: usize, sup: usize) -> bool {
    if sub == 0 { sup == 0 } else { sup % sub == 0 }
}

/// The attribute-extension registry for a host program `P`: maps the
/// [`Perspective`] marker to its lowering behaviour.  A host composes
/// [`Perspective`] into its vocabulary and passes this to the checker's
/// attribute machinery.
///
/// The closure returns the single [`Perspective`] marker coerced to a
/// `&'static` extension — the checker sees a uniform attribute and never names
/// a concrete lattice.
pub fn persp_attr_ext<P>() -> Box<dyn Fn(&Perspective) -> &'static dyn AttrExt<P>>
where
    P: HighProgram,
    P::Value: ValueType + AsEnum<LowValue>,
    P::Operator: From<GcdOp>,
{
    Box::new(|_: &Perspective| -> &'static dyn AttrExt<P> { &Perspective })
}

/// `GcdOp::run` — the VM dispatch for the injected `Gcd` operator.
///
/// The operand is the array of the children's attribute slots, pre-padded with
/// the missing value (`0`) by the checker.  A lazy operand (an unbound
/// parameter) stays lazy.
impl<P> OperatorExt<P> for GcdOp
where
    P: Program,
    P::Value: AsEnum<LowValue> + From<LowValue>,
{
    fn run(&self, operand: P::Value, _block: BlockId, module: &mut Module<P>) -> P::Value {
        match self {
            GcdOp::Gcd => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("Gcd expects an operand array");
                };
                let mut acc = 0;
                for item in operands.items() {
                    // A child slot is a `[value, type]` term pair; the lattice
                    // value is its element 0.  A bare value (an un-annotated
                    // edge that never became a pair) is accepted too.
                    let Some(n) = module
                        .node_value(item.node)
                        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
                        .and_then(|value| match value {
                            LowValue::USize(n) => Some(n),
                            LowValue::Array(items) => {
                                let elem0 = items.items().first()?;
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
                        return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                    };
                    acc = gcd(acc, n);
                }
                <P::Value as From<LowValue>>::from(LowValue::USize(acc))
            }
        }
    }
}

/// The attribute-extension lowering of the [`Perspective`] marker: the slot it
/// occupies, its missing value, its `gcd` combine, and the divisibility subtype
/// order.
///
/// Program-generic: `combine` builds the operator over `P::Operator` (which the
/// host's vocabulary carries via a `From<GcdOp>` leaf), and the subtype reads
/// the two slot values through the curated [`Ctx`].
impl<P> AttrExt<P> for Perspective
where
    P: HighProgram,
    P::Value: ValueType + AsEnum<LowValue>,
    P::Operator: From<GcdOp>,
{
    /// The slot this attribute occupies below the `[value, type]` head
    /// (a first attribute → 2, so the pair is
    /// `[value, type, [persp value, persp type]]` — the slot is itself a
    /// `[value, type]` term pair, the uniform slot shape).
    fn slot(&self) -> usize {
        2
    }

    /// `0` — neutral in `gcd` (the meet identity), concrete in equality
    /// unify.  The slot's absent form is `[0, int]` (see
    /// [`AttrExt::missing_slot`]).
    fn missing_value(&self) -> LowValue {
        LowValue::USize(0)
    }

    /// Perspective combine: a single `Gcd` op node over the children's
    /// attribute slots (the n-ary gcd meet), wrapped as a `[value, type]`
    /// term pair — the uniform slot shape.  An absent child reads `[0, int]`
    /// (padded by the checker), whose value `0` is neutral, so
    /// `(1 # 4 + 2) # 4` derives `gcd(4, 0) = 4`.  The `Gcd` operator reads
    /// each child's lattice value from its pair's element 0.
    fn combine(&self, ctx: &mut dyn Ctx<P>, children: &[NodeId]) -> NodeId {
        let operands = ctx.array_node(children);
        let gcd = ctx.op_node(P::Operator::from(GcdOp::Gcd), Some(operands));
        ctx.pair(gcd, ctx.int_type())
    }

    /// Perspective unify: two slots must unify, or — when they differ — the
    /// declared (`b`) must be a *subtype* of the value's (`a`) under the
    /// divisibility order.  An absent side reads `0` before this is called.
    ///
    /// Each slot is a `[value, type]` term pair; the lattice value is element
    /// 0.  The unify compares the *values* (so the diagnostic and the subtype
    /// read bare `4`/`8`, not `[4, Int]`/`[8, Int]`).
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc) {
        let a = slot_value_node(ctx, a);
        let b = slot_value_node(ctx, b);
        ctx.check_unify_relaxed(a, b, loc, DiagKind::Attribute, &|ctx, value, declared| {
            // `a` (first) is the value's actual perspective, `b` (second) the
            // declared one.  A value uniform over `value` threads is usable
            // where `declared` is required iff `declared | value` (an aligned
            // `value`-group can be partitioned into `declared`-groups, so
            // uniform-`value` implies uniform-`declared`).  `0` (no
            // perspective) matches only `0`.
            self.is_subtype(ctx, declared, value)
        });
    }

    /// `sub ⊑ super ⟺ sub | super` under the divisibility order, where `0`
    /// is the top ("uniform over all threads", the `∞` fold): `sub = 0` is
    /// satisfied only by `super = 0`, and `super = 0` satisfies any `sub`.
    /// Implementation reads the two slot values — each slot is a
    /// `[value, type]` term pair, so the lattice value is its element 0 (a
    /// bare value — an un-annotated edge — is accepted too).  An unbound
    /// value (a runtime-dependent perspective) is not a subtype.
    fn is_subtype(&self, ctx: &dyn Ctx<P>, sub: NodeId, sup: NodeId) -> bool {
        let value_of = |node: NodeId| -> Option<usize> {
            let value = ctx.class_value(node)?;
            match AsEnum::<LowValue>::as_enum(&value) {
                Some(LowValue::USize(n)) => Some(n),
                Some(LowValue::Array(items)) => match items.items().first()?.node {
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

    /// A leaf perspective spells `# n`; a compound's gcd meet (or an unbound
    /// value) has no single spelling and is not shown.  The slot is a
    /// `[value, type]` term pair, so the lattice value is its element 0.
    fn render(&self, module: &Module<P>, slot: NodeId) -> Option<String> {
        let slot_value = self.slot_value(module, slot)?;
        let n = match slot_value {
            LowValue::USize(n) => n,
            LowValue::Array(items) => match module
                .node_value(items.items().first()?.node)
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

/// The lattice-value node of a slot: a `[value, type]` term pair's element 0
/// (the bare value), or the slot itself when it is already bare — an
/// un-annotated edge that never became a pair.  Used so the apply-time check
/// and its diagnostics read the bare `4`, not the whole `[4, Int]`.
fn slot_value_node<P: HighProgram>(ctx: &dyn Ctx<P>, slot: NodeId) -> NodeId
where
    P::Value: ValueType + AsEnum<LowValue>,
{
    let Some(value) = ctx.class_value(slot) else {
        return slot;
    };
    match AsEnum::<LowValue>::as_enum(&value) {
        Some(LowValue::Array(items)) => match items.items().first().map(|item| item.node) {
            Some(AnyNodeId::Dynamic(n)) => n,
            _ => slot,
        },
        _ => slot,
    }
}
