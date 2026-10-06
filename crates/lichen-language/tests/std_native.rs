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

use lichen_language::package::PackageStore;
use lichen_language::persist::{NoPersist, ProgramCodecOf};
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// The plugin-built compiler's vocabulary: the language's leaves plus the
/// `lichen-std-native` plugin (its leaves come in via the `plugins` arm).
///
/// The composition's own `LangProgram` (carrying the test-local `host::LangAttr`)
/// is unused here — the test rebinds the host vocabulary under the shipping
/// `program::LangAttr` below, which is what the frontend/checker speak in.
mod host {
    #![allow(dead_code)] // the composition's own LangProgram/ProgramCodec are
    // unused — see the module doc.
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
/// [`lichen_language::program::LangAttr`] fixes the attribute set to the
/// language's shipping `LangAttr`, so the composed operator vocabulary must
/// implement [`lichen_lowlevel::OperatorExt`] for *that* program — the
/// `lang_compose_vocabulary!` macro generates it only for its own
/// `host::LangProgram`, which carries the test-local `host::LangAttr`.  A
/// package-manager-built compiler reuses the same shipping `LangAttr`, so this
/// impl belongs to the plugin-built host, not the plugin.  A **local newtype**
/// (rather than an alias to the foreign
/// [`lichen_highlevel::program::ProgramImpl`]) is what makes the
/// `Program`/`HighProgram`/`OperatorExt`/`ProgramCodecOf` impls below
/// orphan-legal from this crate.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct HostProgram(
    ::lichen_highlevel::program::ProgramImpl<
        LangValue,
        LangOperator,
        lichen_language::program::LangAttr,
    >,
);

impl lichen_lowlevel::Program for HostProgram {
    type Value = LangValue;
    type Operator = LangOperator;
    type GlobalExt = lichen_highlevel::program::HighGlobalExt;
    type PackageMeta = lichen_highlevel::program::HighPackageMeta;
}

impl lichen_highlevel::program::HighProgram for HostProgram {
    type Attr = lichen_language::program::LangAttr;
    type Literal = lichen_highlevel::program::HighProgramLiteral;
}

impl lichen_lowlevel::OperatorExt<HostProgram> for LangOperator {
    fn run(
        &self,
        operand: <HostProgram as lichen_lowlevel::Program>::Value,
        block: lichen_lowlevel::BlockId,
        module: &mut lichen_lowlevel::Module<HostProgram>,
    ) -> Option<<HostProgram as lichen_lowlevel::Program>::Value> {
        match self {
            LangOperator::LowOperator(op) => op.run(operand, block, module),
            LangOperator::TypeOperator(op) => op.run(operand, block, module),
            LangOperator::GcdOp(op) => op.run(operand, block, module),
            LangOperator::ComputeOperator(op) => op.run(operand, block, module),
            LangOperator::SortOp(op) => op.run(operand, block, module),
        }
    }
}

// The store is generic over a single program `P` that carries its own codec;
// `HostProgram` (the shipping `LangAttr`) is the plugin-built compiler's
// marker, so it binds `NoPersist` (an in-memory store) as its codec.
impl ProgramCodecOf for HostProgram {
    type Codec = NoPersist;
}

type DStore = PackageStore<HostProgram>;

/// A fresh in-memory store with the plugin's embedded wrapper registered as the
/// `std.lichen` native virtual package — the package-manager plug: compile the
/// wrapper source against the plugin's private native registry and serve it by
/// name, with no disk file.
fn new_store() -> DStore {
    let mut store = PackageStore::<HostProgram>::new();
    store
        .register_native(
            "std.lichen",
            lichen_std_native::WRAPPER_SOURCE,
            lichen_std_native::lichen_std_native_ops!(HostProgram),
        )
        .expect("std.lichen must compile and register as a native package");
    store
}

/// Compile, check, and run `source`, returning the module, the evaluated root
/// value, and the root type node.
fn run(source: &str) -> (Module<HostProgram>, LangValue, NodeId) {
    let mut store = new_store();
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    assert!(diags.is_empty(), "expected imports to resolve: {diags:?}");
    let line_starts = lichen_language::lex::line_starts(source);
    let report = lichen_language::compile_with_imports_at::<HostProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    assert!(
        report.ok(),
        "expected {source:?} to check and run, got: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    module.evaluate_node_deep(build.root_ty, None);
    (module, value, build.root_ty)
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

/// The `usize` scalar behind a value.
fn usize_of(value: &LangValue) -> usize {
    match AsEnum::<LowValue>::as_enum(value) {
        Some(LowValue::USize(n)) => n,
        _ => panic!("expected a usize value, got {value:?}"),
    }
}

/// The element values of an array (or tuple) value, evaluated in order.
fn array_values(module: &Module<HostProgram>, value: &LangValue) -> Vec<LangValue> {
    match AsEnum::<LowValue>::as_enum(value) {
        Some(LowValue::Array(array)) => unsafe { array.items() }
            .iter()
            .map(|item| match item.node {
                AnyNodeId::Dynamic(node) => module
                    .node_value(AnyNodeId::Dynamic(node))
                    .expect("an array element has a value"),
                AnyNodeId::Static(_) => panic!("language arrays are dynamic"),
            })
            .collect(),
        _ => panic!("expected an array value, got {value:?}"),
    }
}

/// The `usize` elements of an array value, in order.
fn usize_array(module: &Module<HostProgram>, value: &LangValue) -> Vec<usize> {
    array_values(module, value).iter().map(usize_of).collect()
}

#[test]
fn std_sort_sorts_a_usize_array() {
    let (module, value, _) = run(r#"
---
  std = import "std.lichen"
---
std.sort [3, 1, 2]
"#);
    assert_eq!(
        usize_array(&module, &value),
        vec![1, 2, 3],
        "std.sort produced the sorted elements"
    );
}

#[test]
fn std_sort_is_reusable_and_length_preserving() {
    // The wrapper's `sort` is an ordinary typed function: applying it several
    // times over arrays of different lengths is fine (the length is a fresh
    // cell bound at each apply), and the result keeps the length.
    let (module, value, _) = run(r#"
---
  std = import "std.lichen"
---
(std.sort [4, 1, 3, 2], std.sort [9, 7])
"#);
    let pair = array_values(&module, &value);
    assert_eq!(
        usize_array(&module, &pair[0]),
        vec![1, 2, 3, 4],
        "the first result keeps its four sorted elements"
    );
    assert_eq!(
        usize_array(&module, &pair[1]),
        vec![7, 9],
        "the second result keeps its two sorted elements"
    );
}

#[test]
fn std_sort_rejects_a_non_array() {
    // The array gate pins the argument to `[Int, len]`; a scalar fails.
    let diags = fail(
        r#"
---
  std = import "std.lichen"
---
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
---
  std = import "std.lichen"
---
std.sort ["a", "b"]
"#,
    );
    assert!(
        !diags.is_empty(),
        "std.sort on a string array must be a type error, got: {diags:?}"
    );
}
