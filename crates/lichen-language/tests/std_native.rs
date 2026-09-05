//! End-to-end tests for the `lichen-std-native` plugin, exercised **the way a
//! package manager pulls a native plugin into a compiler**: compose the host
//! vocabulary over the plugin (`lang_compose_vocabulary!`), register the
//! plugin's embedded `std.lichen` wrapper as a native virtual package, then
//! import it and run.  This pins that a plugin-built compiler can serve the
//! plugin's typed wrapper source (a real `[Int, len] -> [Int, len]` sort, not
//! the opaque native application) and that `std.sort` really sorts.
//!
//! The plugin is **not** wired into the shipping compiler's vocabulary: this
//! composition is test-local, exactly as a package-manager-generated compiler
//! would substitute the plugin set into the manifest.

use lichen_language::CompiledProgram;
use lichen_language::package::PackageStore;
use lichen_language::persist::NoPersist;

/// The plugin-built compiler's vocabulary: the language's leaves plus the
/// `lichen-std-native` plugin (its leaves come in via the `plugins` arm).
mod host {
    lichen_language::lang_compose_vocabulary! {
        attrs = [
            lichen_perspective::Perspective as Perspective;
            lichen_doc::Doc as Doc;
        ]
        [ P::Operator: From<lichen_perspective::GcdOp> ];
        values = [
            lichen_lowlevel::LowValue as LowValue;
            lichen_highlevel::program::TypeValue as TypeValue;
            lichen_compute::ComputeValue as ComputeValue;
        ];
        operators = [
            lichen_lowlevel::LowOperator as LowOperator;
            lichen_highlevel::program::TypeOperator as TypeOperator;
            lichen_perspective::GcdOp as GcdOp;
            lichen_compute::ComputeOperator as ComputeOperator;
        ];
        plugins = [ lichen_std_native as lichen_std_native_leaves; ];
    }
}

use host::{LangOperator, LangValue};

/// The program marker the frontend/checker drive over the composed vocabulary.
///
/// [`liche_language::CompiledProgram`] fixes the attribute set to the language's
/// `LangAttr`, so the composed operator vocabulary must implement
/// [`lichen_lowlevel::OperatorExt`] for *that* program (the `lang_compose_vocabulary!`
/// macro generates it only for its own `host::LangProgram`, which carries the
/// test-local `host::LangAttr`).  A package-manager-built compiler reuses the
/// same shipping `LangAttr`, so this impl belongs to the plugin-built host, not
/// the plugin.
type HostProgram = CompiledProgram<LangValue, LangOperator>;

impl lichen_lowlevel::OperatorExt<HostProgram> for LangOperator {
    fn run(
        &self,
        operand: <HostProgram as lichen_lowlevel::Program>::Value,
        block: lichen_lowlevel::BlockId,
        module: &mut lichen_lowlevel::Module<HostProgram>,
    ) -> <HostProgram as lichen_lowlevel::Program>::Value {
        match self {
            LangOperator::LowOperator(op) => op.run(operand, block, module),
            LangOperator::TypeOperator(op) => op.run(operand, block, module),
            LangOperator::GcdOp(op) => op.run(operand, block, module),
            LangOperator::ComputeOperator(op) => op.run(operand, block, module),
            LangOperator::SortOp(op) => op.run(operand, block, module),
        }
    }
}

type DStore = PackageStore<LangValue, LangOperator, NoPersist>;

/// A fresh in-memory store with the plugin's embedded wrapper registered as the
/// `std.lichen` native virtual package — the package-manager plug: compile the
/// wrapper source against the plugin's private native registry and serve it by
/// name, with no disk file.
fn new_store() -> DStore {
    let mut store = PackageStore::<LangValue, LangOperator, NoPersist>::new();
    store
        .register_native(
            "std.lichen",
            lichen_std_native::WRAPPER_SOURCE,
            lichen_std_native::lichen_std_native_ops!(HostProgram),
        )
        .expect("std.lichen must compile and register as a native package");
    store
}

/// Compile, check, and run `source`, returning the rendered `value: type`.
fn run(source: &str) -> String {
    let mut store = new_store();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .unwrap_or_else(|diags| panic!("expected {source:?} to check and run, got: {diags:?}"))
}

/// Compile `source` and assert it *fails*; return the rendered diagnostics.
fn fail(source: &str) -> Vec<String> {
    let mut store = new_store();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .expect_err("expected this program to fail")
        .into_iter()
        .map(|d| d.message)
        .collect()
}

#[test]
fn std_sort_sorts_a_usize_array() {
    let out = run(r#"
@{
  std = import "std.lichen"
@}
std.sort [3, 1, 2]
"#);
    assert_eq!(out, "[1, 2, 3]: Int<3>", "std.sort produced: {out:?}");
}

#[test]
fn std_sort_is_reusable_and_length_preserving() {
    // The wrapper's `sort` is an ordinary typed function: applying it several
    // times over arrays of different lengths is fine (the length is a fresh
    // cell bound at each apply), and the result keeps the length.
    let out = run(r#"
@{
  std = import "std.lichen"
@}
(std.sort [4, 1, 3, 2], std.sort [9, 7])
"#);
    assert_eq!(
        out, "([1, 2, 3, 4], [7, 9]): <Int<4>, Int<2>>",
        "produced: {out:?}"
    );
}

#[test]
fn std_sort_rejects_a_non_array() {
    // The array gate pins the argument to `[Int, len]`; a scalar fails.
    let diags = fail(
        r#"
@{
  std = import "std.lichen"
@}
std.sort 5
"#,
    );
    assert!(
        !diags.is_empty(),
        "std.sort 5 must be a type error, got diagnostics: {diags:?}"
    );
}

#[test]
fn std_sort_rejects_a_non_int_array() {
    // A `[string]` array is not a `[usize]` array, so the gate rejects it.
    let diags = fail(
        r#"
@{
  std = import "std.lichen"
@}
std.sort ["a", "b"]
"#,
    );
    assert!(
        !diags.is_empty(),
        "std.sort on a string array must be a type error, got: {diags:?}"
    );
}
