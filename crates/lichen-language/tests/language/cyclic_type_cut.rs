//! The readings `…` and `?` share, and the cut that bounds a walk.
//! See docs/language-spec.md §5.

use lichen_language::compile;
use lichen_render::print_type;

/// The type the front end gives `source`'s root expression, as printed.
fn root_type(source: &str) -> String {
    let report = compile(source);
    assert!(report.ok(), "{source} must check: {:?}", report.diagnostics);
    let build = report.build.expect("a build");
    print_type(&build.module, build.root_ty)
}

/// Reading one: `?a` printed twice is one class; naming cells moves it to `?b`.
#[test]
fn a_repeated_class_cell_is_one_unification_class() {
    // "a repeated `?a` says two slots are one unification class" — spec §5.
    assert_eq!(root_type("f = x => x; f"), "?a -> ?a");
}

/// The §7.6 lazy spine: the cut; one level short prints `tuple{Float, tuple{Float, …}}`.
#[test]
fn a_coinductive_spine_is_cut_at_the_first_revisited_node() {
    // "the walk cuts the spine at the first node it meets again" — spec §5.
    assert_eq!(
        root_type("f = x => [x, @shallow f (x + 1.0)]; inf = f 0.0; inf"),
        "tuple{Float, tuple{Float, tuple{Float, …}}}"
    );
}

/// The §7.6 alternating pair, on the open list's spelling; `?a, …` moves first.
#[test]
fn a_two_name_cycle_stops_on_the_open_field_lists_spelling() {
    // "the `…` in `tuple{?a, …}` says the field list is **undecided**" — spec §5.
    assert_eq!(
        root_type("f = x => [x, @shallow g x]; g = y => [y, @shallow f y]; inf = f 0.0; inf"),
        "tuple{Float, tuple{Float, tuple{Float, tuple{?a, …}}}}"
    );
}

/// The §7.6 three-name cycle: four heads and return (code-audit P4-10).
#[test]
fn a_three_name_cycle_terminates_at_its_first_revisit() {
    // "it fires at the first revisit and a cycle of any length terminates" — §5.
    let printed =
        root_type("A = struct{.g B}; B = struct{.g C}; C = struct{.g A}; f = x : A => 0; f");
    assert_eq!(
        printed,
        "struct{.g struct{.g struct{.g struct{.g …}}}} -> Int"
    );
    assert_eq!(printed.matches("struct{").count(), 4);
    assert_eq!(printed.matches('…').count(), 1);
}
