//! The language's concrete program: the highlevel's value vocabulary and
//! attribute/operator extension, composed into one [`LangProgram`].
//!
//! The highlevel is attribute-agnostic — it names only the *shape* of "an
//! attribute combines over children" — so the concrete pieces live here:
//! [`Perspective`] (the only attribute, a divisibility lattice) and its
//! combine operator [`GcdOp::Gcd`] (an n-ary gcd meet).  The language then
//! composes these with the highlevel's `LowValue`/`TypeValue`/`LowOperator`/
//! `TypeOperator` leaves into a single flat vocabulary via
//! [`lichen_utils::enum_ext!`].
//!
//! [`LangProgram`] is the program marker the whole frontend checks with — the
//! `P` of `Module<P>`/`Registry<P>`/`Checker<P>`, with `Value = HighProgramValue`,
//! `Operator = LangOperator`, and `Attr = Perspective`.

use lichen_highlevel::attr::{AttrExt, AttrSpec};
use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::Loc;
use lichen_highlevel::program::{
    HighGlobal, ProgramImpl, TypeOperator, TypeValue, ValueType,
};
use lichen_lowlevel::{LowOperator, LowValue, Module, NodeId, OperatorExt, ValueExt};
use lichen_utils::compose::AsField;
use lichen_utils::extend::AsEnum;

use crate::compute::{ComputeOperator, ComputeValue};

/// The language's own operators: the perspective combine.
///
/// A plain enum, provided whole for the [`LangOperator`] composition below —
/// the same leaf shape as the highlevel's [`TypeOperator`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GcdOp {
    /// n-ary `gcd` over the operand array.  An empty operand array (a leaf
    /// with no sub-expressions) evaluates to `0` — the meet identity / top,
    /// so `gcd(n, 0) = n` makes it neutral.  A lazy operand yields
    /// `Parameterized`, like `Add`/`Sub`.
    Gcd,
}

// The language program's operator vocabulary: a flat union of the
// structural [`LowOperator`], the highlevel's [`TypeOperator`], the
// language's own [`GcdOp`], and the compute [`ComputeOperator`] — each carried
// whole as one sibling variant.
lichen_utils::enum_ext! {
    /// The language program's operator vocabulary.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum LangOperator {
    }
    + LowOperator as LowOperator;
    + TypeOperator as TypeOperator;
    + GcdOp as GcdOp;
    + ComputeOperator as ComputeOperator;
}

lichen_utils::enum_ext! {
    /// The language program's value vocabulary.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum LangValue {
    }
    + LowValue as LowValue;
    + TypeValue as TypeValue;
    + ComputeValue as ComputeValue;
}

impl ValueExt for LangValue {
    fn is_handle(&self) -> bool {
        false
    }
}

// The compute vocabulary's only new type-constant marker is `TypeKernel`
// (a kernel's type mirror is `[signature, [TypeKernel, Type]]`); every other
// marker delegates to the highlevel type values.
impl ValueType for LangValue {
    fn int_marker() -> Self {
        Self::TypeValue(TypeValue::TypeInt)
    }
    fn type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeType)
    }
    fn function_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeFunction)
    }
    fn tuple_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeTuple)
    }
    fn array_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeArray)
    }
    fn type_struct_marker() -> Self {
        Self::TypeValue(TypeValue::TypeStruct)
    }
    fn table_type_marker() -> Self {
        Self::TypeValue(TypeValue::TypeTable)
    }
    fn type_id(&self) -> Option<usize> {
        match self {
            Self::TypeValue(TypeValue::TypeId(n)) => Some(*n),
            _ => None,
        }
    }
    fn type_id_value(n: usize) -> Self {
        Self::TypeValue(TypeValue::TypeId(n))
    }
}

/// The perspective attribute: a plain non-negative integer whose lattice is
/// divisibility (`a ⊑ b ⟺ a | b`), meet (`gcd`), top (`0`, "uniform over all
/// threads", the `∞` fold), and bottom (`1`).  A node with an unannotated
/// perspective is *missing*, which in GPU code means "not expressed
/// per-thread" — i.e. uniform over all threads — so it IS the top, `0`.  A
/// `# p`-annotated leaf uses `p`; an unannotated leaf contributes `0` (the
/// top, neutral in `gcd`).  Stage 1 uses only `gcd` and `0`.
///
/// The marker carries no data — the perspective's *value* is a runtime node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Perspective;

impl AttrSpec for Perspective {}

/// The language's concrete program: `Value = LangValue`,
/// `Operator = LangOperator`, `Attr = Perspective`.
pub type LangProgram = ProgramImpl<LangValue, LangOperator, Perspective>;

/// The attribute-extension registry for [`LangProgram`]: maps the
/// `Perspective` marker to its lowering behavior.  The checker consults this
/// at the schema-driven lowering sites, never naming a concrete attribute.
pub fn persp_attr_ext() -> Box<dyn Fn(&Perspective) -> &'static dyn AttrExt<LangProgram>> {
    Box::new(|_: &Perspective| -> &'static dyn AttrExt<LangProgram> { &Perspective })
}

/// `gcd` over the divisibility lattice.  `gcd(n, 0) = n` (the meet identity),
/// `gcd(0, 0) = 0`.
fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

impl OperatorExt<LangProgram> for LangOperator {
    fn run(
        &self,
        operand: <LangProgram as lichen_lowlevel::Program>::Value,
        block: lichen_lowlevel::BlockId,
        module: &mut Module<LangProgram>,
    ) -> <LangProgram as lichen_lowlevel::Program>::Value {
        match self {
            // The structural operators never reach `run`: the VM dispatches
            // them through `AsEnum` before falling through.
            LangOperator::LowOperator(_) => {
                unreachable!("structural operators are dispatched by the VM")
            }
            LangOperator::TypeOperator(TypeOperator::Fresh) => {
                let id = AsField::<HighGlobal>::get_mut(&mut module.global_ext).next_type_id();
                                LangValue::type_id_value(id)
            }
            LangOperator::TypeOperator(
                TypeOperator::Add | TypeOperator::Sub | TypeOperator::Leq | TypeOperator::Eq,
            ) => {
                // The VM already deep-evaluates the operand and gates on its
                // parameterized subtree, so an unbound operand is the lazy
                // marker (the definition pass flags the node).
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return LangValue::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("binary operators expect an operand array of [left, right]")
                };
                let operands = operands.items();
                // A non-USize operand is a *reported* type error, not an
                // invariant violation: the checker pins both operands to
                // `Int`, so a wrong shape only arrives here through an
                // argument unify that already failed (recording the
                // diagnostic) — stay lazy instead of panicking.
                let Some(left) = module
                    .node_value(operands[0].node)
                    .and_then(|value| value.as_enum())
                    .and_then(|value| match value {
                        LowValue::USize(n) => Some(n),
                        _ => None,
                    })
                else {
                    return LangValue::from(LowValue::Parameterized);
                };
                let Some(right) = module
                    .node_value(operands[1].node)
                    .and_then(|value| value.as_enum())
                    .and_then(|value| match value {
                        LowValue::USize(n) => Some(n),
                        _ => None,
                    })
                else {
                    return LangValue::from(LowValue::Parameterized);
                };
                match self {
                    LangOperator::TypeOperator(TypeOperator::Add) => {
                        LangValue::from(LowValue::USize(left.wrapping_add(right)))
                    }
                    LangOperator::TypeOperator(TypeOperator::Sub) => {
                        LangValue::from(LowValue::USize(left.wrapping_sub(right)))
                    }
                    LangOperator::TypeOperator(TypeOperator::Leq) => {
                        LangValue::from(LowValue::USize((left <= right) as usize))
                    }
                    LangOperator::TypeOperator(TypeOperator::Eq) => {
                        LangValue::from(LowValue::USize((left == right) as usize))
                    }
                    _ => unreachable!("all binary operators are handled above"),
                }
            }
            LangOperator::GcdOp(GcdOp::Gcd) => {
                // The operand is the array of the children's attribute slots,
                // pre-padded with the missing value (`0`) by the checker.  A
                // lazy operand (an unbound parameter) stays lazy.
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return LangValue::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("Gcd expects an operand array")
                };
                let mut acc = 0;
                for item in operands.items() {
                    let Some(n) = module
                        .node_value(item.node)
                        .and_then(|value| value.as_enum())
                        .and_then(|value| match value {
                            LowValue::USize(n) => Some(n),
                            _ => None,
                        })
                    else {
                        return LangValue::from(LowValue::Parameterized);
                    };
                    acc = gcd(acc, n);
                }
                LangValue::from(LowValue::USize(acc))
            }
            LangOperator::ComputeOperator(op) => op.run(operand, block, module),
        }
    }
}

impl AttrExt<LangProgram> for Perspective {
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
    fn combine(&self, checker: &mut Checker<LangProgram>, children: &[NodeId]) -> NodeId {
        let operands = checker.array_node(checker.current_block, children);
        checker.op_node(
            checker.current_block,
            LangOperator::from(GcdOp::Gcd),
            Some(operands),
        )
    }

    /// Perspective unify: two slots must unify, or — when they differ — the
    /// declared (`b`) must be a *subtype* of the value's (`a`) under the
    /// divisibility order.  An absent side reads `0` before this is called.
    fn unify_slots(
        &self,
        checker: &mut Checker<LangProgram>,
        a: NodeId,
        b: NodeId,
        loc: Loc,
    ) {
        checker.check_unify_relaxed(a, b, loc, DiagKind::Attribute, |checker, value, declared| {
            // `a` (first) is the value's actual perspective, `b` (second) the
            // declared one.  A value uniform over `value` threads is usable
            // where `declared` is required iff `declared | value` (an aligned
            // `value`-group can be partitioned into `declared`-groups, so
            // uniform-`value` implies uniform-`declared`).  `0` (no
            // perspective) matches only `0`.
            self.is_subtype(checker, declared, value)
        });
    }

    /// `sub ⊑ super ⟺ sub | super` under the divisibility order, where `0`
    /// is the top ("uniform over all threads", the `∞` fold): `sub = 0` is
    /// satisfied only by `super = 0`, and `super = 0` satisfies any `sub`.
    /// Implementation reads the two slot values (an unbound value — a
    /// runtime-dependent perspective — is not a subtype).
    fn is_subtype(&self, checker: &Checker<LangProgram>, sub: NodeId, sup: NodeId) -> bool {
        let (Some(sub), Some(sup)) = (checker.class_value(sub), checker.class_value(sup)) else {
            return false;
        };
        let (Some(LowValue::USize(sub)), Some(LowValue::USize(sup))) =
            (sub.as_enum(), sup.as_enum())
        else {
            return false;
        };
        divides(sub, sup)
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
fn divides(sub: usize, sup: usize) -> bool {
    if sub == 0 {
        sup == 0
    } else {
        sup % sub == 0
    }
}

#[cfg(test)]
#[path = "tests/program_tests.rs"]
mod tests;
