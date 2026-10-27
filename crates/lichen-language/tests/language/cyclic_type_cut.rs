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

/// Reading one: a function value's type is the shared `FunctionType` marker.
#[test]
fn a_function_values_type_is_the_shared_marker() {
    // A function value's type names no function: it is the shared marker.
    assert_eq!(root_type("f := x => x; f"), "TypeFunction");
}

/// The §7.6 lazy spine: the cut; one level short prints `tuple{Float, tuple{Float, …}}`.
#[test]
fn a_coinductive_spine_is_cut_at_the_first_revisited_node() {
    // "the walk cuts the spine at the first node it meets again" — spec §5.
    assert_eq!(
        root_type("A := _x => [@any, 2]\nf := x => A[x, @shallow f (x + 1.0)]; inf := f 0.0; inf"),
        "Array"
    );
}

/// A mutual cycle through a call result is that call's own signature instance.
#[test]
fn a_two_name_cycle_through_a_call_result_is_instantiated() {
    // The call result is the per-call instance; the shared cut is not here.
    assert_eq!(
        root_type(
            "A := _x => [@any, 2]\nf := x => A[x, @shallow g x]; g := y => A[y, @shallow f y]; inf := f 0.0; inf"
        ),
        "Array"
    );
}

/// The §7.6 three-name cycle: four heads and return (code-audit P4-10).
#[test]
fn a_three_name_cycle_terminates_at_its_first_revisit() {
    // The cycle stops at its first revisit; the struct values keep their shape.
    let printed = root_type("A := {.g B}; B := {.g C}; C := {.g A}; f := x <: (_x => A) => 0; f");
    // The struct cycle is not in the type at all: the printer walks none.
    assert_eq!(printed, "TypeFunction");
    assert_eq!(printed.matches("Struct").count(), 0);
    assert_eq!(printed.matches('…').count(), 0);
}
