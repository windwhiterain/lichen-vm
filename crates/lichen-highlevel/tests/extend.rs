//! The value and literal vocabularies are extension points: one `enum_ext!`
//! invocation lists every layer's enum directly — the lowlevel structural
//! values, the highlevel type values, the downstream's own variants — as
//! sibling carry variants of one flat union, and the checker runs on it
//! generically.  This proves the path a language crate would take to add its
//! own value variants (via [`ValueType`]) and its own literal structs (via
//! [`LiteralExt`]), composing the built-in int/type-constant literals with a
//! downstream literal the same way an operator vocabulary composes.

use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprKind, IR};
use lichen_highlevel::NoAttr;
use lichen_highlevel::program::{
    Ctx, HighProgramOperator, IntLit, IntTypeLit, LiteralBuild, LiteralExt, ProgramImpl,
    TypeOperator, TypeTypeLit, TypeValue, ValueType,
};
use lichen_lowlevel::{
    AnyNodeId, BlockId, LowOperator, LowValue, Module, NodeId, OperatorExt, Program, ValueExt,
};
use lichen_utils::extend::AsEnum;

// A probe extension: a type constant beyond the highlevel's vocabulary.
// The composed union carries the lowlevel and highlevel layers as sibling
// variants — flat, no nesting — and gains its own `FloatType`.
lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum ProbeValue {
        /// A type constant the highlevel doesn't know — a first-class type
        /// that pairs with `Type`, exactly like `Int` or `Type` itself.
        FloatType,
    }
    + LowValue as LowValue;
    + TypeValue as TypeValue;
}

/// The dynamic node behind an item ref — the checker builds only dynamic graphs.
fn dyn_node(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => unreachable!("checker graphs are dynamic"),
    }
}

impl ValueExt for ProbeValue {
    fn is_handle(&self) -> bool {
        false
    }
}

impl ValueType for ProbeValue {
    fn int_marker() -> Self {
        Self::TypeValue(TypeValue::TypeInt)
    }
    fn string_marker() -> Self {
        Self::TypeValue(TypeValue::TypeString)
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

// The probe literal vocabulary: the highlevel's built-in literal structs
// compose with the downstream's own (here a `FloatLit` that stores nothing
// and builds the `FloatType` marker paired with `Type`) — the same
// composition a operator vocabulary uses.  A downstream composes via
// `enum_ext!` and implements `LiteralExt` for the composed enum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatLit;

impl<P> LiteralExt<P> for FloatLit
where
    P: Program,
    P::Value: From<ProbeValue>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>) -> LiteralBuild {
        let value_node = ctx.value_node(P::Value::from(ProbeValue::FloatType));
        let ty = ctx.universe();
        let pair = ctx.pair(value_node, ty);
        LiteralBuild {
            pair,
            value: value_node,
            ty,
        }
    }
}

lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum ProbeLiteral {
    }
    + IntLit as Int;
    + IntTypeLit as IntType;
    + TypeTypeLit as TypeType;
    + FloatLit as Float;
}

/// The probe program marker: the probe value and literal vocabularies with no
/// attribute extension.
pub type ProbeProgram = ProgramImpl<ProbeValue, ProbeOperator, NoAttr, ProbeLiteral>;

impl LiteralExt<ProbeProgram> for ProbeLiteral {
    fn build(&self, ctx: &mut dyn Ctx<ProbeProgram>) -> LiteralBuild {
        match self {
            ProbeLiteral::Int(lit) => lit.build(ctx),
            ProbeLiteral::IntType(lit) => lit.build(ctx),
            ProbeLiteral::TypeType(lit) => lit.build(ctx),
            ProbeLiteral::Float(lit) => lit.build(ctx),
        }
    }
}

#[test]
fn the_carry_variants_wrap_and_view() {
    // Each layer's From wraps into its own branch; AsEnum reads it back.
    let v: ProbeValue = TypeValue::TypeInt.into();
    assert_eq!(v, ProbeValue::TypeValue(TypeValue::TypeInt));
    assert_eq!(AsEnum::<TypeValue>::as_enum(&v), Some(TypeValue::TypeInt));
    // The lowlevel view reads only the LowValue branch: structural values
    // round-trip, every other branch reads as None.
    let n: ProbeValue = LowValue::USize(3).into();
    assert_eq!(n, ProbeValue::LowValue(LowValue::USize(3)));
    assert_eq!(AsEnum::<LowValue>::as_enum(&n), Some(LowValue::USize(3)));
    assert_eq!(AsEnum::<LowValue>::as_enum(&v), None);
    assert_eq!(AsEnum::<LowValue>::as_enum(&ProbeValue::FloatType), None);
}

#[test]
fn the_checker_runs_on_an_extended_union() {
    // `FloatType : Type` — the new type constant is a first-class type: its
    // pair is a fresh `[FloatType, Type]` with the canonical universe as its
    // type slot.  And `5 : Int` checks as usual on the extended vocabulary.
    let mut ir: IR<NoAttr, ProbeLiteral> = IR::new();
    let float_ty = ir.alloc(ExprKind::Literal(ProbeLiteral::Float(FloatLit)), None);
    let five = ir.alloc(ExprKind::Literal(ProbeLiteral::Int(IntLit(5))), None);
    let int_t = ir.alloc(ExprKind::Literal(ProbeLiteral::IntType(IntTypeLit)), None);
    let ann = ir.alloc(
        ExprKind::Annotation {
            value: five,
            r#type: Some(int_t),
            attribute: None,
        },
        None,
    );
    // A tuple so every expression above is reachable from the root (the
    // checker only compiles what the root references).
    let tuple = ir.alloc_tuple(&[float_ty, ann], None);
    ir.set_root(tuple);
    let build = Checker::<ProbeProgram>::build(ir);
    assert!(build.ok, "the extended-union program must check");
    let float_pair = build.term[float_ty.0 as usize].unwrap();
    let float_value = build.val[float_ty.0 as usize].unwrap();
    assert_eq!(
        build.module.node_value(AnyNodeId::Dynamic(float_value)),
        Some(ProbeValue::FloatType)
    );
    assert_eq!(build.ty[float_ty.0 as usize], Some(build.type_expr));
    let ids = build
        .module
        .array_items(float_pair)
        .expect("the pair is an array")
        .iter()
        .map(|item| dyn_node(item.node))
        .collect::<Vec<_>>();
    assert_eq!(ids, &[float_value, build.type_expr]);
}

#[test]
fn an_extended_union_reports_type_conflicts() {
    // `5 : Type` is an annotation conflict even on the extended union — the
    // generic checker's diagnostics carry the extended value type.
    let mut ir: IR<NoAttr, ProbeLiteral> = IR::new();
    let five = ir.alloc(ExprKind::Literal(ProbeLiteral::Int(IntLit(5))), None);
    let ty = ir.alloc(ExprKind::Literal(ProbeLiteral::TypeType(TypeTypeLit)), None);
    let ann = ir.alloc(
        ExprKind::Annotation {
            value: five,
            r#type: Some(ty),
            attribute: None,
        },
        None,
    );
    ir.set_root(ann);
    let build = Checker::<ProbeProgram>::build(ir);
    assert!(!build.ok);
    assert!(
        build
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagKind::Annotation)
    );
}
// A probe operator vocabulary: the same extension shape a downstream language


// would use when it needs operators beyond the highlevel's own set.
lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum ProbeOperator {
    }
    + LowOperator as LowOperator;
    + TypeOperator as TypeOperator;
}

impl<V: ValueType, L> OperatorExt<ProgramImpl<V, ProbeOperator, NoAttr, L>> for ProbeOperator
where
    L: std::fmt::Debug + Copy + PartialEq,
{
    fn run(
        &self,
        _operand: V,
        _block: BlockId,
        _module: &mut Module<ProgramImpl<V, ProbeOperator, NoAttr, L>>,
    ) -> V {
        unreachable!("the probe operator is only used to prove the type composes")
    }
}

#[test]
fn the_program_marker_accepts_a_composed_operator_vocabulary() {
    // A `Module` can be bound to the highlevel value vocabulary with a
    // downstream operator union; the lowlevel runtime machinery no longer
    // requires the operator set to be exactly `HighProgramOperator`.
    let _module = Module::<ProbeProgram>::new();
    assert!(HighProgramOperator::LowOperator(LowOperator::Apply)
        == HighProgramOperator::LowOperator(LowOperator::Apply));
}
