//! `@in` — set membership, answering the language's `0`/`1` scalar.
//! See operator-polymorphism.md §3.

use lichen_lowlevel::LowValue;

use lichen_language::compile;
use lichen_language::program::LangValue;

/// `type_of` from the standard library, bound ahead of every probed program.
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
    module
        .evaluate_node_deep(build.root_val, None)
        .expect("the program's root value is undecided")
}

fn usize_of(value: &LangValue) -> usize {
    let LangValue::LowValue(LowValue::USize(n)) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// A set of ordinary **values**: the tested value is compared with the members.
#[test]
fn a_membership_test_compares_a_value_with_a_sets_members() {
    assert_eq!(usize_of(&evaluate("2 @in set{1, 2}")), 1);
    assert_eq!(usize_of(&evaluate("3 @in set{1, 2}")), 0);
    assert_eq!(usize_of(&evaluate("2 @in set{1, 5}")), 0);
}

/// A refinement on a type applies the predicate to the type value, so `in_num`
/// needs no type read at all.
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

/// `ValueExt::value_eq` compares array *handles*, so a class from another
/// module must be compared structurally.
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
