//! Shared end-to-end helpers bound to the bare language's own composition.
//! See tests-do-not-render.md.

#![allow(dead_code, unused_imports)]

use lichen_language::package::PackageStore;
use lichen_language::program::{LangProgram, LangValue};
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};

pub use lichen_language::testing::{
    array_values, float_array, float_of, run_at, type_head, type_is_float, type_is_int,
    type_is_undecided, usize_array, usize_of, values_eq,
};

/// The `usize` elements of an array value; slot 0 is the layout template.
pub fn usize_array_elements(module: &Module<LangProgram>, value: &LangValue) -> Vec<usize> {
    let mut values = array_values(module, value);
    values.remove(0);
    values.iter().map(usize_of).collect()
}

/// The `usize` fields of a struct value, after its name table and nominal id.
pub fn usize_struct_fields(module: &Module<LangProgram>, value: &LangValue) -> Vec<usize> {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected a struct value, got {value:?}");
    };
    // SAFETY: the value was just produced by the module under test, whose
    // block has not been dropped.
    let fields: Vec<AnyNodeId> = unsafe { array.items() }
        .iter()
        .skip(lichen_highlevel::shape::STRUCT_VALUE_FIELD_BASE)
        .map(|item| item.node)
        .collect();
    fields
        .into_iter()
        .map(|node| {
            let node = match node {
                AnyNodeId::Dynamic(node) => node,
                AnyNodeId::Static(_) => panic!("language values are dynamic"),
            };
            let node = module
                .element_value_node(AnyNodeId::Dynamic(node))
                .unwrap_or(AnyNodeId::Dynamic(node));
            let value = module.node_value(node).expect("a field has a value");
            usize_of(&value)
        })
        .collect()
}

/// Compile and run `source` (no imports, no store).
pub fn evaluate(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    lichen_language::testing::evaluate::<LangProgram>(source)
}

/// Compile and run `source` through a fresh in-memory store.
pub fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    let mut store = PackageStore::<LangProgram>::new();
    run_at(source, None, &mut store)
}
