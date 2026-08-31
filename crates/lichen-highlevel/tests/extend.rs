//! The value vocabulary is an extension point: one `enum_ext!` invocation
//! lists every layer's enum directly — the lowlevel structural values, the
//! highlevel type values, and the downstream's own variants — composing them
//! as sibling carry variants of one flat union, and the checker runs on it
//! generically.  This proves the path a language crate would take to add its
//! own value variants: the `From`/`AsEnum` glue for every layer is generated
//! by the macro, and the extended vocabulary supplies the value→type mapping
//! through [`ValueType`].

use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprKind, IR};
use lichen_highlevel::program::{TypeValue, ValueType};
use lichen_lowlevel::{AnyNodeId, LowValue, NodeId, ValueExt};
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
    fn type_of(&self) -> Self {
        match self {
            // Both type-constant branches — the extension's own and the
            // highlevel's — have the canonical universe as their type.
            ProbeValue::FloatType | ProbeValue::TypeValue(_) => Self::type_marker(),
            ProbeValue::LowValue(LowValue::USize(_)) => Self::int_marker(),
            ProbeValue::LowValue(_) => {
                unreachable!("a structural non-USize value is not a constant")
            }
        }
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
    let mut ir: IR<ProbeValue> = IR::new();
    let float_ty = ir.alloc(ExprKind::Constant(ProbeValue::FloatType), None);
    let five = ir.alloc(ExprKind::Constant(LowValue::USize(5).into()), None);
    let int_t = ir.alloc(
        ExprKind::Constant(ProbeValue::TypeValue(TypeValue::TypeInt)),
        None,
    );
    let ann = ir.alloc(
        ExprKind::Annotation {
            value: five,
            r#type: int_t,
        },
        None,
    );
    // A tuple so every expression above is reachable from the root (the
    // checker only compiles what the root references).
    let tuple = ir.alloc_tuple(&[float_ty, ann], None);
    ir.set_root(tuple);
    let build = Checker::build(ir);
    assert!(build.ok, "the extended-union program must check");
    let float_pair = build.term[float_ty.0 as usize].unwrap();
    let float_value = build.val[float_ty.0 as usize].unwrap();
    assert_eq!(
        build.module.nodes[float_value].value,
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
    let mut ir: IR<ProbeValue> = IR::new();
    let five = ir.alloc(ExprKind::Constant(LowValue::USize(5).into()), None);
    let ty = ir.alloc(
        ExprKind::Constant(ProbeValue::TypeValue(TypeValue::TypeType)),
        None,
    );
    let ann = ir.alloc(
        ExprKind::Annotation {
            value: five,
            r#type: ty,
        },
        None,
    );
    ir.set_root(ann);
    let build = Checker::build(ir);
    assert!(!build.ok);
    assert!(
        build
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagKind::Annotation)
    );
}
