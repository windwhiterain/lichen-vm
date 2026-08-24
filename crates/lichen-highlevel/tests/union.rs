//! The value and operator unions: `HighProgramValue` is the lowlevel
//! `LowValue` extended with the checker's type values, and
//! `HighProgramOperator` is the lowlevel `LowOperator` extended with the
//! checker's type-level operators.  `From<LowValue>`/`From<LowOperator>`
//! build a structural value/operator, `AsEnum` inspects one, and the
//! extension variants read as `None` — the two halves the lowlevel
//! distinguishes through `as_enum`.

use lichen_highlevel::program::{HighProgramOperator, HighProgramValue};
use lichen_lowlevel::{LowOperator, LowValue};
use lichen_utils::extend::AsEnum;

#[test]
fn structural_values_round_trip_through_from_and_as_enum() {
    let v: HighProgramValue = LowValue::USize(3).into();
    assert_eq!(v, HighProgramValue::USize(3));
    assert_eq!(v.as_enum(), Some(LowValue::USize(3)));
}

#[test]
fn type_values_read_as_none() {
    assert_eq!(HighProgramValue::TypeInt.as_enum(), None);
    assert_eq!(HighProgramValue::TypeId(3).as_enum(), None);
}

#[test]
fn markers_read_as_their_structural_self() {
    assert_eq!(
        HighProgramValue::Parameterized.as_enum(),
        Some(LowValue::Parameterized)
    );
    assert_eq!(HighProgramValue::None.as_enum(), Some(LowValue::None));
}

#[test]
fn structural_operators_round_trip_through_from_and_as_enum() {
    let op: HighProgramOperator = LowOperator::Index.into();
    assert_eq!(op, HighProgramOperator::Index);
    assert_eq!(op.as_enum(), Some(LowOperator::Index));
    assert_eq!(
        HighProgramOperator::Apply.as_enum(),
        Some(LowOperator::Apply)
    );
}

#[test]
fn extension_operators_read_as_none() {
    assert_eq!(HighProgramOperator::IndexTypeDispatch.as_enum(), None);
    assert_eq!(HighProgramOperator::Fresh.as_enum(), None);
    assert_eq!(HighProgramOperator::Add.as_enum(), None);
}
