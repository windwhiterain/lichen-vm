//! A written arrow in a frozen module stays the function type it names.
//! See function-type-merge.md.

mod common;

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
fn a_written_arrow_in_a_frozen_module_is_a_function_type() {
    let out = render_imported_wrapper(
        "wrap = f => {I = _; f: I -> _; I}",
        "---\nw = import \"wrap.lichen\"\n---\nw.wrap",
    );
    assert!(
        out.contains("->"),
        "the imported wrapper is a function type: {out:?}"
    );
    assert!(
        !out.contains("raw[Function"),
        "a function type is not its own two items: the parameter's type is the \
         function type the annotation names, so the signature reads as an \
         arrow rather than as `[Function, <signature>]` — got {out:?}"
    );
}
