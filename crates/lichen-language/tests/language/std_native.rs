//! The `lichen-std-native` plugin, composed into a host and served as a native
//! virtual package.

use lichen_language::package::PackageStore;
use lichen_language::persist::{NoPersist, ProgramCodecOf};
use lichen_lowlevel::{LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// A host over the bare language's leaves plus `lichen-std-native`.
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
        ];
        operators = [
            lichen_lowlevel::LowOperator as LowOperator;
            lichen_highlevel::program::TypeOperator as TypeOperator;
            lichen_perspective::GcdOp as GcdOp;
        ];
        plugins = [ lichen_std_native as lichen_std_native_leaves; ];
    }
}

use host::{LangOperator, LangValue};

/// The frontend/checker's program marker. A local newtype, not an alias, makes
/// the impls below orphan-legal.
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
            LangOperator::SortOp(op) => op.run(operand, block, module),
        }
    }

    // Mirror the composition's deferred dispatch to each leaf.
    fn run_deferred(
        &self,
        operand: Option<NodeId>,
        block: lichen_lowlevel::BlockId,
        module: &mut lichen_lowlevel::Module<HostProgram>,
    ) -> Option<LangValue> {
        match self {
            LangOperator::LowOperator(op) => {
                <lichen_lowlevel::LowOperator as lichen_lowlevel::OperatorExt<HostProgram>>::run_deferred(op, operand, block, module)
            }
            LangOperator::TypeOperator(op) => {
                <lichen_highlevel::program::TypeOperator as lichen_lowlevel::OperatorExt<HostProgram>>::run_deferred(op, operand, block, module)
            }
            LangOperator::GcdOp(op) => {
                <lichen_perspective::GcdOp as lichen_lowlevel::OperatorExt<HostProgram>>::run_deferred(op, operand, block, module)
            }
            LangOperator::SortOp(op) => {
                <lichen_std_native::SortOp as lichen_lowlevel::OperatorExt<HostProgram>>::run_deferred(op, operand, block, module)
            }
        }
    }
}
impl ProgramCodecOf for HostProgram {
    type Codec = NoPersist;
}

// This host names no leaf the lowlevel printer cannot, so it takes the shape's
// default naming.
impl lichen_language::LangProgramShape for HostProgram {}

type DStore = PackageStore<HostProgram>;

/// A fresh in-memory store with the plugin's wrapper registered as the
/// `std.lichen` native virtual package.
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
    let value = module.evaluate_node_deep(build.root_val, None).unwrap();
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
///
/// # Invariant
/// A compound's elements are complete slots, so the canonical reader resolves
/// each one; this host's own value type is the language's.
fn array_values(module: &Module<HostProgram>, value: &LangValue) -> Vec<LangValue> {
    lichen_language::testing::array_values(module, value)
}

/// The `usize` elements of an array value, in order.
///
/// # Invariant
/// An array value's first slot holds the element template, which names the
/// layout and is never user data, so the elements are the rest.
fn usize_array(module: &Module<HostProgram>, value: &LangValue) -> Vec<usize> {
    let mut values = array_values(module, value);
    values.remove(0);
    values.iter().map(usize_of).collect()
}

#[test]
fn std_sort_sorts_a_usize_array() {
    let (module, value, _) = run(r#"
---
  std = import "std.lichen"
---
A := _x => [@any : Int, 3]
std.sort A[3, 1, 2]
"#);
    assert_eq!(
        usize_array(&module, &value),
        vec![1, 2, 3],
        "std.sort produced the sorted elements"
    );
}

#[test]
fn std_sort_is_reusable_and_length_preserving() {
    // `sort` is an ordinary typed function: the length is a fresh cell bound at
    // each apply.
    let (module, value, _) = run(r#"
---
  std = import "std.lichen"
---
A4 := _x => [@any, 4]
A2 := _x => [@any, 2]
(std.sort A4[4, 1, 3, 2], std.sort A2[9, 7])
"#);
    let expr = array_values(&module, &value);
    assert_eq!(
        usize_array(&module, &expr[0]),
        vec![1, 2, 3, 4],
        "the first result keeps its four sorted elements"
    );
    assert_eq!(
        usize_array(&module, &expr[1]),
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
A := _x => [@any, 2]
std.sort A["a", "b"]
"#,
    );
    assert!(
        !diags.is_empty(),
        "std.sort on a string array must be a type error, got: {diags:?}"
    );
}
