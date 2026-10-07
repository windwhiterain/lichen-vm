//! A **written arrow in a frozen module** stays the function type it names.
//!
//! `A -> B` compiles to a real lambda (a function), and a function's type is
//! the function itself (`f : f`, the self-referential `[Function(fid), ↺]`).
//! Annotating a parameter with one therefore unifies the parameter's *type
//! cell* with that function type — the two are one class, and the cell's value
//! is the function type.
//!
//! A frozen module flattens the class into its one value, so the materialized
//! cell carries the function type **as a value**: `[Function(fid), t]` where
//! `t` is the frozen node that *is* the cycle, not the cell.  Reading that as
//! "a function type" is the same relation as reading it by class — the type
//! level says the cell and the function type are one — and this test is where
//! that reading is pinned, in the language layer, with no compute and no
//! operator involved.
//!
//! Measured before: the wrapper printed
//! `Function: raw[Function, raw[?a, ?b] -> raw[?c, ?d]] -> ?b` — the parameter's
//! type dumped as its two items because the printer asked whether slot 1 was
//! the *cell* rather than whether it is a function type.
//! `docs/notes/function-type-merge.md` has the reproduction and the search.

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
