//! The value and operator unions: `HighProgramValue` is a flat union of the
//! lowlevel `LowValue` and the highlevel `TypeValue` (each carried whole as
//! one sibling variant), and `HighProgramOperator` the same for `LowOperator`
//! and `TypeOperator`.  `From` builds a layer's value, `AsEnum` reads that
//! layer's branch back, and every other branch reads as `None` — the two
//! halves the lowlevel distinguishes through `as_enum`.  Both unions have
//! several `AsEnum` impls, so the views are spelled `AsEnum::<..>::as_enum`.

use lichen_highlevel::program::{HighProgramOperator, HighProgramValue, TypeOperator, TypeValue};
use lichen_lowlevel::{LowOperator, LowValue};
use lichen_utils::extend::AsEnum;

#[test]
fn structural_values_round_trip_through_from_and_as_enum() {
    let v: HighProgramValue = LowValue::USize(3).into();
    assert_eq!(v, HighProgramValue::LowValue(LowValue::USize(3)));
    assert_eq!(AsEnum::<LowValue>::as_enum(&v), Some(LowValue::USize(3)));
}

#[test]
fn type_values_read_as_none_through_the_lowlevel_view() {
    assert_eq!(
        AsEnum::<LowValue>::as_enum(&HighProgramValue::TypeValue(TypeValue::TypeInt)),
        None
    );
    assert_eq!(
        AsEnum::<LowValue>::as_enum(&HighProgramValue::TypeValue(TypeValue::TypeId(3))),
        None
    );
    // Each layer also views its own branch.
    assert_eq!(
        AsEnum::<TypeValue>::as_enum(&HighProgramValue::TypeValue(TypeValue::TypeInt)),
        Some(TypeValue::TypeInt)
    );
}

#[test]
fn markers_read_as_their_structural_self() {
    assert_eq!(
        AsEnum::<LowValue>::as_enum(&HighProgramValue::LowValue(LowValue::Parameterized)),
        Some(LowValue::Parameterized)
    );
    assert_eq!(
        AsEnum::<LowValue>::as_enum(&HighProgramValue::LowValue(LowValue::None)),
        Some(LowValue::None)
    );
}

#[test]
fn structural_operators_round_trip_through_from_and_as_enum() {
    let op: HighProgramOperator = LowOperator::Index.into();
    assert_eq!(op, HighProgramOperator::LowOperator(LowOperator::Index));
    assert_eq!(
        AsEnum::<LowOperator>::as_enum(&op),
        Some(LowOperator::Index)
    );
    assert_eq!(
        AsEnum::<LowOperator>::as_enum(&HighProgramOperator::LowOperator(LowOperator::Apply)),
        Some(LowOperator::Apply)
    );
}

#[test]
fn extension_operators_read_as_none() {
    assert_eq!(
        AsEnum::<LowOperator>::as_enum(&HighProgramOperator::TypeOperator(
            TypeOperator::IndexTypeDispatch
        )),
        None
    );
    assert_eq!(
        AsEnum::<LowOperator>::as_enum(&HighProgramOperator::TypeOperator(TypeOperator::Fresh)),
        None
    );
    assert_eq!(
        AsEnum::<LowOperator>::as_enum(&HighProgramOperator::TypeOperator(TypeOperator::Add)),
        None
    );
}
