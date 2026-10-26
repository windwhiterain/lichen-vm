//! A function template in a frozen module stays a real function.
//! See function-type-merge.md.

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

/// Compile `source` through a store that already holds the frozen wrapper, and
/// render the result.
fn render_imported_wrapper(module_source: &str, program: &str) -> String {
    let mut store = PackageStore::<LangProgram>::new();
    store
        .register_native(
            "wrap.lichen",
            module_source,
            lichen_highlevel::no_native_ops(),
        )
        .expect("the frozen wrapper compiles");
    lichen_language::run::evaluate_raw(program, None, &mut store)
        .unwrap_or_else(|diags| panic!("expected the program to check and run, got: {diags:?}"))
}

#[test]
fn a_function_template_in_a_frozen_module_is_a_function() {
    let out = render_imported_wrapper(
        "wrap := f => {I := _x => _; f = (i <: I => _); I}",
        "---\nw = import \"wrap.lichen\"\n---\nw.wrap",
    );
    assert!(out.contains("=>"), "the wrapper is a function: {out:?}");
    assert!(!out.contains("->"), "the thin arrow is gone: {out:?}");
    assert!(
        !out.contains("raw[Function"),
        "a function type is not its own two items: {out:?}"
    );

    let mut store = PackageStore::<LangProgram>::new();
    let out = lichen_language::run::evaluate_raw(
        "apply := x = (x : Int => _: Int) => x 1\napply (y : Int => y + y)",
        None,
        &mut store,
    )
    .unwrap_or_else(|diags| panic!("the function template must check, got: {diags:?}"));
    assert_eq!(
        out, "2: Int",
        "the template constrains the function without replacing its body"
    );

    let out = render_imported_wrapper(
        "wrap := f => {f = (x => _); f 1}",
        "---\nw = import \"wrap.lichen\"\n---\nw.wrap (y : Int => y + y)",
    );
    assert_eq!(
        out, "2: Int",
        "a frozen generic function template admits a concrete function"
    );

    let out = render_imported_wrapper(
        "wrap := f => {O := f 1; r := f 1; r = O}",
        "---\nw = import \"wrap.lichen\"\n---\nw.wrap (y : Int => y + y)",
    );
    assert_eq!(out, "2: Int", "an applied result is a reusable template");

    let out = render_imported_wrapper(
        "K := _x => {.native _, .I _, .O _}\nlaunch := k <: K => r => {r = k.O}\nwrap := f => x => {I := _x => _; f = (i <: I => _); x <: I; O := f (I _); r := f x; launch (K{.native _, .I I, .O O}) r}",
        "---\nw = import \"wrap.lichen\"\n---\nw.wrap (y : Int => y + y) 1",
    );
    assert_eq!(out, "2: Int", "a struct field carries the output template");

    let out = render_imported_wrapper(
        "wrap := f => x => {I := _x => _; f = (i <: I => _); x <: I; O := f (I _); r := f x; r = O}",
        "---\nw = import \"wrap.lichen\"\n---\nw.wrap (y : Int => y + y) 1",
    );
    assert_eq!(
        out, "2: Int",
        "the input template drives the output template"
    );
}
