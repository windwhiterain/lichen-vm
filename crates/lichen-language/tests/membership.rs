//! `@in` — set membership (`docs/notes/operator-polymorphism.md` §3).
//!
//! Every assertion is a VM-level value comparison: a membership test answers the
//! language's `0`/`1` scalar, so it is read back as a `USize` and no rendered
//! type string is inspected.

use lichen_lowlevel::LowValue;

use lichen_language::compile;
use lichen_language::program::LangValue;

/// The standard library's `type_of` (`lichen-std/_.lichen`), bound ahead of
/// every probed program: `compile` takes a bare source with no package store to
/// import from.
const TYPE_OF: &str = "type_of = x => {t = _; x: t; t}\n";

fn evaluate(source: &str) -> LangValue {
    let report = compile(source);
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    let build = report.build.expect("a checked program has a build");
    let mut module = build.module;
    module.evaluate_node_deep(build.root_val, None)
}

fn usize_of(value: &LangValue) -> usize {
    let LangValue::LowValue(LowValue::USize(n)) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// A set of ordinary **values**: the tested value is compared with the members,
/// so `2` is a member of `set{1, 2}` and `3` is not.
#[test]
fn a_membership_test_compares_a_value_with_a_sets_members() {
    assert_eq!(usize_of(&evaluate("2 @in set{1, 2}")), 1);
    assert_eq!(usize_of(&evaluate("3 @in set{1, 2}")), 0);
    assert_eq!(usize_of(&evaluate("2 @in set{1, 5}")), 0);
}

/// A refinement written **on a type** — `x : (_ ! in_num)` — is a **class**
/// refinement: the predicate is applied to the type value, so `in_num` needs no
/// type read at all.  The class is open in the template, so each application
/// re-checks its own class: `Int` and `Float` pass, a `string` does not.
#[test]
fn a_refinement_written_on_a_type_refines_the_class() {
    let open = "Num = set{Int, Float}\nin_num = t => t @in Num\nf = x => { x : (_ ! in_num); x }\n";
    let checked = |argument: &str| compile(&format!("{open}f {argument}")).ok();
    assert!(checked("5"), "`Int` is in the domain");
    assert!(
        checked("1.5"),
        "`Float` is in the domain, and the open class re-checks per application"
    );
    assert!(!checked("\"a\""), "`string` is not in the domain");

    // A *concrete* class is enforced at the definition, not per application.
    let closed = |class: &str| {
        format!(
            "Num = set{{Int, Float}}\nin_num = t => t @in Num\ng = x => {{ x : ({class} ! in_num); x }}\ng 5"
        )
    };
    assert_eq!(usize_of(&evaluate(&closed("Int"))), 5);
    assert!(!compile(&closed("string")).ok(), "`string` is not in `Num`");
}

/// A set of **type values** — the class domain a contract is written over.  The
/// members and the tested value are both type values, and they are compared
/// structurally: `ValueExt::value_eq` cannot answer this, because it compares
/// array *handles*, so a class node out of another module would never match.
#[test]
fn a_membership_test_matches_a_type_value_against_a_class_domain() {
    let probe = |argument: &str| {
        evaluate(&format!(
            "{TYPE_OF}Num = set{{Int, Float}}\ntype_of {argument} @in Num"
        ))
    };
    assert_eq!(usize_of(&probe("5")), 1, "the class `Int` is a member");
    assert_eq!(usize_of(&probe("1.5")), 1, "the class `Float` is a member");
    assert_eq!(
        usize_of(&probe("\"a\"")),
        0,
        "a `string` is not a numeric class"
    );
}
