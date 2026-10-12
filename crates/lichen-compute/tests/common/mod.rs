//! Shared end-to-end helpers bound to the compute sub-language.
//! See tests-do-not-render.md.

#![allow(dead_code, unused_imports)]

use lichen_compute::language::{LangProgram, LangValue, new_store};
use lichen_lowlevel::{Module, NodeId};

pub use lichen_language::testing::{
    array_values, float_array, float_of, run_at, type_head, type_is_float, type_is_int,
    type_is_undecided, usize_array, usize_of, values_eq,
};

/// Compile and run `source` (no imports, no store).
pub fn evaluate(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    lichen_language::testing::evaluate::<LangProgram>(source)
}

/// Compile and run `source` through a fresh store with `compute.lichen`
/// registered.
pub fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    let mut store = new_store(None).expect("the embedded compute package registers");
    run_at(source, None, &mut store)
}
