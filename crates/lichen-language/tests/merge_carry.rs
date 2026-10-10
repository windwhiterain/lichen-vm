//! A class's decided value reaches a *merge*'s members, not only a *write*'s.
//! See docs/notes/eval-before-unify.md §3.3.

mod common;

use lichen_language::program::LangValue;
use lichen_lowlevel::LowValue;

/// The definition pass commits the class; the apply's join is the merge that
/// has to carry it to the member the join adds.
#[test]
fn a_merge_carries_the_decided_type_to_a_member_added_after_the_commit() {
    let (module, value, root_ty) =
        common::evaluate("f = x => {\n  a = x(0) + x(0)\n  p = x: <Int, Int>\n  a\n}\nf (10, 20)");
    assert_eq!(
        value,
        LangValue::LowValue(LowValue::USize(20)),
        "the carried value is the class's own 20"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the carried type is the class's own Int"
    );
}
