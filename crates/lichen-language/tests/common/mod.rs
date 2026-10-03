//! Shared helpers for the language end-to-end tests: run a program to its
//! evaluated value and type without going through the value printer, and read
//! structural values back out of the module.  Every assertion built on these
//! is a VM-level value/type comparison, never a rendered string.
//!
//! Each `tests/*.rs` file is its own crate and uses only the helpers it needs,
//! so a helper unused by one file must not warn for that file.

#![allow(dead_code)]

use std::path::Path;

use lichen_highlevel::program::TypeValue;
use lichen_language::compile;
use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_language::program::{LangProgram, LangValue};
use lichen_language::{compile_with_imports_at, lex};
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// Compile and run `source` (no imports), returning the module, the evaluated
/// root value, and the root type node.
pub fn evaluate(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    let report = compile(source);
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    finish(report.build.unwrap())
}

/// Compile and run `source` through a fresh package store, returning the
/// module, the evaluated root value, and the root type node.
pub fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    let mut store = PackageStore::<LangProgram>::new();
    run_at(source, None, &mut store)
}

/// Compile and run `source` through a caller-owned store, returning the
/// module, the evaluated root value, and the root type node.
pub fn run_at(
    source: &str,
    base: Option<&Path>,
    store: &mut PackageStore<LangProgram>,
) -> (Module<LangProgram>, LangValue, NodeId) {
    let (preprocessed, diags) = preprocess(source, base, store);
    assert!(diags.is_empty(), "expected imports to resolve: {diags:?}");
    let line_starts = lex::line_starts(source);
    let report = compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    finish(report.build.unwrap())
}

fn finish(
    build: lichen_highlevel::checker::Build<LangProgram>,
) -> (Module<LangProgram>, LangValue, NodeId) {
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    module.evaluate_node_deep(build.root_ty, None);
    // Mirror `run::render_build`'s refusal gate: a runtime refusal explains a
    // value that never arrived, so a program that produced nothing while a
    // refusal was recorded is a failure, not an empty answer.
    let produced_nothing = matches!(
        AsEnum::<LowValue>::as_enum(&value),
        Some(LowValue::Parameterized | LowValue::Void)
    );
    assert!(
        !(produced_nothing && !module.extension_diagnostics.is_empty()),
        "a runtime refusal was recorded: {:?}",
        module.extension_diagnostics
    );
    (module, value, build.root_ty)
}

/// The `usize` scalar behind a value.
pub fn usize_of(value: &LangValue) -> usize {
    let LangValue::LowValue(LowValue::USize(n)) = value else {
        panic!("expected a usize value, got {value:?}");
    };
    *n
}

/// The `f32` scalar behind a value.
pub fn float_of(value: &LangValue) -> f32 {
    let LangValue::LowValue(LowValue::Float(f)) = value else {
        panic!("expected a float value, got {value:?}");
    };
    *f
}

/// The element values of an array (or tuple) value, evaluated in order.
pub fn array_values(module: &Module<LangProgram>, value: &LangValue) -> Vec<LangValue> {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected an array value, got {value:?}");
    };
    // SAFETY: `array` is the payload of the value just produced by `module`,
    // whose block has not been dropped.
    unsafe { array.items() }
        .iter()
        .map(|item| match item.node {
            AnyNodeId::Dynamic(node) => module
                .node_value(AnyNodeId::Dynamic(node))
                .expect("an array element has a value"),
            AnyNodeId::Static(_) => panic!("language arrays are dynamic"),
        })
        .collect()
}

/// The `usize` elements of an array value, in order.
pub fn usize_array(module: &Module<LangProgram>, value: &LangValue) -> Vec<usize> {
    array_values(module, value).iter().map(usize_of).collect()
}

/// The `f32` elements of an array value, in order.
pub fn float_array(module: &Module<LangProgram>, value: &LangValue) -> Vec<f32> {
    array_values(module, value).iter().map(float_of).collect()
}

/// Structural equality of two evaluated values, possibly from different
/// modules: scalars compare field-wise (floats by their bits), arrays compare
/// element by element.  This is the value-level equality the printer used to
/// spell out as a string; it never compares rendered text.
pub fn values_eq(
    a: (&Module<LangProgram>, &LangValue),
    b: (&Module<LangProgram>, &LangValue),
) -> bool {
    match (a.1.as_enum(), b.1.as_enum()) {
        (Some(LowValue::USize(x)), Some(LowValue::USize(y))) => x == y,
        (Some(LowValue::Float(x)), Some(LowValue::Float(y))) => x.to_bits() == y.to_bits(),
        (Some(LowValue::Str(x)), Some(LowValue::Str(y))) => x == y,
        (Some(LowValue::Array(x)), Some(LowValue::Array(y))) => {
            // SAFETY: `x`/`y` are payloads of live values from `a.0`/`b.0`; this
            // comparator releases no block.
            let xs = unsafe { x.items() };
            let ys = unsafe { y.items() };
            xs.len() == ys.len()
                && xs.iter().zip(ys).all(|(xi, yi)| {
                    let xv = a.0.node_value(xi.node).unwrap();
                    let yv = b.0.node_value(yi.node).unwrap();
                    values_eq((a.0, &xv), (b.0, &yv))
                })
        }
        _ => *a.1 == *b.1,
    }
}

/// The head marker of an atomic type node — the marker an atomic `[head, K]`
/// type pair renders as (`Int`, `Float`, …).  A node that is not an atomic
/// type (an unbound cell, a compound type, or no value) answers `None`.
pub fn type_head(module: &Module<LangProgram>, node: NodeId) -> Option<LangValue> {
    let value = module.node_value(AnyNodeId::Dynamic(node))?;
    match value.as_enum() {
        Some(LowValue::Array(array)) => {
            // SAFETY: `array` is the payload of a value read from a live node of
            // `module`; this helper releases no block.
            let items = unsafe { array.items() };
            if items.len() == 2 {
                module.node_value(items[0].node)
            } else {
                None
            }
        }
        _ => Some(value),
    }
}

/// Whether an atomic type node's head is the `Int` marker.
pub fn type_is_int(module: &Module<LangProgram>, node: NodeId) -> bool {
    type_head(module, node) == Some(LangValue::TypeValue(TypeValue::TypeInt))
}

/// Whether an atomic type node's head is the `Float` marker.
pub fn type_is_float(module: &Module<LangProgram>, node: NodeId) -> bool {
    type_head(module, node) == Some(LangValue::TypeValue(TypeValue::TypeFloat))
}

/// Whether a type node is an undecided cell (what the printer spells `?` or a
/// named `?a`): its value is the lazy marker, or it holds nothing at all.
pub fn type_is_undecided(module: &Module<LangProgram>, node: NodeId) -> bool {
    match module.node_value(AnyNodeId::Dynamic(node)) {
        None => true,
        Some(value) => matches!(value.as_enum(), Some(LowValue::Parameterized)),
    }
}
