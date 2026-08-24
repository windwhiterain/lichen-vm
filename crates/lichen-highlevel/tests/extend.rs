//! The value union is an extension point: `extend_HighProgramValue!`
//! re-splices the highlevel vocabulary (the lowlevel structural values plus
//! the type values) into a new **flat** union with extra variants, and the
//! checker runs on it generically.  This proves the path a language crate
//! would take to add its own value variants — the union stays one flat enum,
//! no wrapper variant, and the extended vocabulary supplies the value→type
//! mapping through [`ValueType`].

use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprKind, IR};
use lichen_highlevel::program::{HighProgramValue, ValueType};
use lichen_lowlevel::{ArrayRef, FunctionId, LowValue, ValueExt};
use lichen_utils::extend::AsEnum;

// A probe extension: a type constant beyond the highlevel's vocabulary.
// The carrier re-splices all of `HighProgramValue`'s variants (including
// the lowlevel structural values) into this flat union — no nesting.
lichen_highlevel::extend_HighProgramValue! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum ProbeValue {
        /// A type constant the highlevel doesn't know — a first-class type
        /// that pairs with `Type`, exactly like `Int` or `Type` itself.
        FloatType,
    }
}

// The extended vocabulary must also satisfy the lowlevel value contract.
// Both impls delegate through `HighProgramValue`, which already implements
// them — the union's shape is flat, only the impls are chained.
impl From<LowValue> for ProbeValue {
    fn from(value: LowValue) -> Self {
        HighProgramValue::from(value).into()
    }
}

impl AsEnum<LowValue> for ProbeValue {
    fn as_enum(&self) -> Option<LowValue> {
        AsEnum::<HighProgramValue>::as_enum(self).and_then(|value| value.as_enum())
    }
}

impl ValueExt for ProbeValue {
    fn is_handle(&self) -> bool {
        false
    }
}

impl ValueType for ProbeValue {
    fn int_marker() -> Self {
        Self::TypeInt
    }
    fn type_marker() -> Self {
        Self::TypeType
    }
    fn function_type_marker() -> Self {
        Self::TypeFunction
    }
    fn tuple_type_marker() -> Self {
        Self::TypeTuple
    }
    fn array_type_marker() -> Self {
        Self::TypeArray
    }
    fn type_struct_marker() -> Self {
        Self::TypeStruct
    }
    fn type_of(&self) -> Self {
        match self {
            ProbeValue::USize(_) => Self::TypeInt,
            ProbeValue::FloatType
            | ProbeValue::TypeInt
            | ProbeValue::TypeType
            | ProbeValue::TypeFunction
            | ProbeValue::TypeTuple
            | ProbeValue::TypeArray
            | ProbeValue::TypeStruct
            | ProbeValue::TypeId(_) => Self::TypeType,
            _ => unreachable!("a structural non-USize value is not a constant"),
        }
    }
    fn type_id(&self) -> Option<usize> {
        match self {
            Self::TypeId(n) => Some(*n),
            _ => None,
        }
    }
    fn type_id_value(n: usize) -> Self {
        Self::TypeId(n)
    }
}

#[test]
fn the_nested_carrier_splices_flat_and_delegates() {
    // The extension splices HighProgramValue's variants into a flat enum —
    // `From<HighProgramValue>` moves them in, `AsEnum` reads them back.
    let v: ProbeValue = HighProgramValue::TypeInt.into();
    assert_eq!(v, ProbeValue::TypeInt);
    assert_eq!(
        AsEnum::<HighProgramValue>::as_enum(&v),
        Some(HighProgramValue::TypeInt)
    );
    // The lowlevel view delegates through HighProgramValue: structural
    // values read back, type values and the extension's own variant read as
    // None.
    let n: ProbeValue = HighProgramValue::USize(3).into();
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
    let five = ir.alloc(ExprKind::Constant(ProbeValue::USize(5)), None);
    let int_t = ir.alloc(ExprKind::Constant(ProbeValue::TypeInt), None);
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
        .array_ids(float_pair)
        .expect("the pair is an array");
    assert_eq!(ids, &[float_value, build.type_expr]);
}

#[test]
fn an_extended_union_reports_type_conflicts() {
    // `5 : Type` is an annotation conflict even on the extended union — the
    // generic checker's diagnostics carry the extended value type.
    let mut ir: IR<ProbeValue> = IR::new();
    let five = ir.alloc(ExprKind::Constant(ProbeValue::USize(5)), None);
    let ty = ir.alloc(ExprKind::Constant(ProbeValue::TypeType), None);
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
