//! Shared end-to-end helpers bound to the bare language's own composition.
//! See tests-do-not-render.md.

#![allow(dead_code, unused_imports)]

use lichen_language::package::PackageStore;
use lichen_language::program::{LangProgram, LangValue};
use lichen_lowlevel::{Module, NodeId};

pub use lichen_language::testing::{
    array_values, float_array, float_of, run_at, type_head, type_is_float, type_is_int,
    type_is_undecided, usize_array, usize_of, values_eq,
};

/// Compile and run `source` (no imports, no store).
pub fn evaluate(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    lichen_language::testing::evaluate::<LangProgram>(source)
}

/// Compile and run `source` through a fresh in-memory store.
pub fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    let mut store = PackageStore::<LangProgram>::new();
    run_at(source, None, &mut store)
}
