//! The value union: `HighProgramValue` is the lowlevel `LowValue` extended
//! with the checker's type values.  `From<LowValue>` builds a structural
//! value, `AsEnum<LowValue>` inspects one, and the type values read as
//! `None` — the two halves the lowlevel distinguishes through `as_enum`.

use lichen_highlevel::program::HighProgramValue;
use lichen_lowlevel::LowValue;
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
