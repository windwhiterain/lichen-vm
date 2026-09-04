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
use lichen_lowlevel::{BlockId, LowValue, Module, NodeId, OperatorExt, Program};
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
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
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
    if sub == 0 {
        sup == 0
    } else {
        sup % sub == 0
    }
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
                if matches!(AsEnum::<LowValue>::as_enum(&operand), Some(LowValue::Parameterized)) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("Gcd expects an operand array");
                };
                let mut acc = 0;
                for item in operands.items() {
                    let Some(n) = module
                        .node_value(item.node)
                        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
                        .and_then(|value| match value {
                            LowValue::USize(n) => Some(n),
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
    /// (a first attribute → 2, so the pair is `[value, type, persp]`).
    fn slot(&self) -> usize {
        2
    }

    /// `0` — neutral in `gcd` (the meet identity), concrete in equality
    /// unify.
    fn missing_value(&self) -> LowValue {
        LowValue::USize(0)
    }

    /// Perspective combine: a single `Gcd` op node over the children's
    /// attribute slots (the n-ary gcd meet).  An absent child reads `0`
    /// (padded by the checker), which is neutral, so `(1 # 4 + 2) # 4`
    /// derives `gcd(4, 0) = 4`.
    fn combine(&self, ctx: &mut dyn Ctx<P>, children: &[NodeId]) -> NodeId {
        let operands = ctx.array_node(children);
        ctx.op_node(P::Operator::from(GcdOp::Gcd), Some(operands))
    }

    /// Perspective unify: two slots must unify, or — when they differ — the
    /// declared (`b`) must be a *subtype* of the value's (`a`) under the
    /// divisibility order.  An absent side reads `0` before this is called.
    fn unify_slots(&self, ctx: &mut dyn Ctx<P>, a: NodeId, b: NodeId, loc: Loc) {
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
    /// Implementation reads the two slot values (an unbound value — a
    /// runtime-dependent perspective — is not a subtype).
    fn is_subtype(&self, ctx: &dyn Ctx<P>, sub: NodeId, sup: NodeId) -> bool {
        let (Some(sub), Some(sup)) = (ctx.class_value(sub), ctx.class_value(sup)) else {
            return false;
        };
        let (Some(LowValue::USize(sub)), Some(LowValue::USize(sup))) =
            (AsEnum::<LowValue>::as_enum(&sub), AsEnum::<LowValue>::as_enum(&sup))
        else {
            return false;
        };
        divides(sub, sup)
    }

    /// A leaf perspective spells `# n`; a compound's gcd meet (or an unbound
    /// value) has no single spelling and is not shown.
    fn render(&self, module: &Module<P>, slot: NodeId) -> Option<String> {
        match self.slot_value(module, slot)? {
            LowValue::USize(n) => Some(format!("# {n}")),
            _ => None,
        }
    }
}
