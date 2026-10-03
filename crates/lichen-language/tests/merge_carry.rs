//! End-to-end test for the merge carry: a class's decided value reaches the
//! members a *merge* adds to it, not only the members a *write* finds
//! (docs/notes/eval-before-unify.md §3.3).  The assertion is the rendered
//! output line, because the printer is the reader that exposes the defect —
//! the type cell reads `?a` until the carry reaches it.

use lichen_language::run::evaluate;

/// The §2.1 repro, read-first order: the binop's result type is a class the
/// definition pass commits and the apply's result cell joins afterwards.
/// Before the carry the joined cell read undecided forever, and the printer
/// rendered `20: ?a`; the carried value is the class's own `Int`.
#[test]
fn a_merge_carries_the_decided_type_to_a_member_added_after_the_commit() {
    assert_eq!(
        evaluate("f = x => {\n  a = x(0) + x(0)\n  p = x: <Int, Int>\n  a\n}\nf (10, 20)").unwrap(),
        "20: Int"
    );
}
