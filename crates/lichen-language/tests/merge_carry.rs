//! End-to-end test for the merge carry: a class's decided value reaches the
//! members a *merge* adds to it, not only the members a *write* finds
//! (docs/notes/eval-before-unify.md §3.3).  The value and type are asserted
//! structurally: the type cell reads `?a` until the carry reaches it, then
//! reads the class's own `Int`.

mod common;

use lichen_language::program::LangValue;
use lichen_lowlevel::LowValue;

/// The §2.1 repro, read-first order: the binop's result type is a class the
/// definition pass commits and the apply's result cell joins afterwards.
/// Before the carry the joined cell read undecided forever; the carried value
/// is the class's own `Int`.
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
