//! Value and operator unions; each layer's view is `AsEnum::<..>::as_enum`.

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
        AsEnum::<LowValue>::as_enum(&HighProgramValue::LowValue(LowValue::Error)),
        Some(LowValue::Error)
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
        AsEnum::<LowOperator>::as_enum(&HighProgramOperator::TypeOperator(TypeOperator::Fresh)),
        None
    );
    assert_eq!(
        AsEnum::<LowOperator>::as_enum(&HighProgramOperator::TypeOperator(TypeOperator::Add)),
        None
    );
}
