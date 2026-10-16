//! Driving a program end-to-end and reading the result without the printer.
//! See docs/notes/tests-do-not-render.md.
//!
//! # Invariant
//!
//! How a host reads a program's result is one thing here, not one thing per
//! host: a suite over one composition and a suite over another differ only in
//! the program they name, never in how they read a value. A host binds these
//! to its own program marker (see `tests/common`); nothing here names one.

use std::path::Path;

use lichen_highlevel::program::{TypeOperator, TypeValue, ValueType};
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

use crate::LangProgramShape;
use crate::package::PackageStore;
use crate::preprocess::preprocess;
use crate::program::GcdOp;
use crate::{compile_with_imports_at, lex};

/// Compile and run `source` (no imports), returning the module, the evaluated
/// root value, and the root type node.
pub fn evaluate<P>(source: &str) -> (Module<P>, P::Value, NodeId)
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let line_starts = lex::line_starts(source);
    let report = compile_with_imports_at::<P>(
        source,
        &[],
        None,
        0,
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

/// Compile and run `source` through a caller-owned store.
pub fn run_at<P>(
    source: &str,
    base: Option<&Path>,
    store: &mut PackageStore<P>,
) -> (Module<P>, P::Value, NodeId)
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let (preprocessed, diags) = preprocess(source, base, store);
    assert!(diags.is_empty(), "expected imports to resolve: {diags:?}");
    let line_starts = lex::line_starts(source);
    let report = compile_with_imports_at::<P>(
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

fn finish<P>(build: lichen_highlevel::checker::Build<P>) -> (Module<P>, P::Value, NodeId)
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let mut module = build.module;
    let value = module.evaluate_node_deep(build.root_val, None);
    let _ = module.evaluate_node_deep(build.root_ty, None);
    // The CLI reports every runtime refusal (`docs/notes/language-toolchain.md`), while this helper still asserts an older, quieter rule, so a suite built through it is laxer than the CLI about a refusal recorded beside an answer.
    let produced_nothing = !crate::run::answer_arrived(&module, value);
    assert!(
        !(produced_nothing && !module.extension_diagnostics.is_empty()),
        "a runtime refusal was recorded: {:?}",
        module.extension_diagnostics
    );
    (
        module,
        value.expect("the program's root value is undecided"),
        build.root_ty,
    )
}

/// The `usize` scalar behind a value.
pub fn usize_of<V: ValueType>(value: &V) -> usize {
    match AsEnum::<LowValue>::as_enum(value) {
        Some(LowValue::USize(n)) => n,
        _ => panic!("expected a usize value, got {value:?}"),
    }
}

/// The `f32` scalar behind a value.
pub fn float_of<V: ValueType>(value: &V) -> f32 {
    match AsEnum::<LowValue>::as_enum(value) {
        Some(LowValue::Float(f)) => f,
        _ => panic!("expected a float value, got {value:?}"),
    }
}

/// The element values of an array (or tuple) value, evaluated in order.
pub fn array_values<P>(module: &Module<P>, value: &P::Value) -> Vec<P::Value>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
{
    let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(value) else {
        panic!("expected an array value, got {value:?}")
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
pub fn usize_array<P>(module: &Module<P>, value: &P::Value) -> Vec<usize>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
{
    array_values(module, value).iter().map(usize_of).collect()
}

/// The `f32` elements of an array value, in order.
pub fn float_array<P>(module: &Module<P>, value: &P::Value) -> Vec<f32>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
{
    array_values(module, value).iter().map(float_of).collect()
}

/// Structural equality of two values: scalars field-wise, floats by bits,
/// arrays element by element.
pub fn values_eq<P>(a: (&Module<P>, &P::Value), b: (&Module<P>, &P::Value)) -> bool
where
    P: LangProgramShape,
    P::Value: ValueType + PartialEq + 'static,
{
    match (
        AsEnum::<LowValue>::as_enum(a.1),
        AsEnum::<LowValue>::as_enum(b.1),
    ) {
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
                    // No value is a refusal, not a `false`: two unevaluated values must
                    // never compare equal.
                    let xv =
                        a.0.node_value(xi.node)
                            .unwrap_or_else(|| panic!("left element {:?} has no value", xi.node));
                    let yv =
                        b.0.node_value(yi.node)
                            .unwrap_or_else(|| panic!("right element {:?} has no value", yi.node));
                    values_eq((a.0, &xv), (b.0, &yv))
                })
        }
        _ => a.1 == b.1,
    }
}

/// The head marker of an atomic type node. A node that is not an atomic type
/// answers `None`.
pub fn type_head<P>(module: &Module<P>, node: NodeId) -> Option<P::Value>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
{
    let value = module.node_value(AnyNodeId::Dynamic(node))?;
    match AsEnum::<LowValue>::as_enum(&value) {
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
pub fn type_is_int<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
{
    type_head(module, node) == Some(<P::Value as From<TypeValue>>::from(TypeValue::TypeInt))
}

/// Whether an atomic type node's head is the `Float` marker.
pub fn type_is_float<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
{
    type_head(module, node) == Some(<P::Value as From<TypeValue>>::from(TypeValue::TypeFloat))
}

/// Whether a type node is an undecided cell: it holds nothing at all.
pub fn type_is_undecided<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
{
    module.node_value(AnyNodeId::Dynamic(node)).is_none()
}
