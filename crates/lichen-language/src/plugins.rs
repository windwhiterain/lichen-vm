//! The plugin set a compiler binary was composed over.
//!
//! # Invariant
//! The set is a property of the compiled vocabulary, installed once by the
//! binary's `main` before any store runs; a file's `plug` is accepted only when
//! its identity is in it (docs/notes/package-graph.md §3).

use std::sync::OnceLock;

use lichen_preprocess::Depend;

static PLUGIN_SET: OnceLock<Vec<String>> = OnceLock::new();

/// Install this binary's plugin set; a second install reports the set it
/// attempted and leaves the first in place.
pub fn install(plugin_set: Vec<String>) -> Result<(), Vec<String>> {
    PLUGIN_SET.set(plugin_set).map_err(|set| set)
}

/// The plugin set this binary was composed over: empty for the shipping
/// compiler, which composes no `plug`.
pub fn set() -> &'static [String] {
    PLUGIN_SET.get().map(Vec::as_slice).unwrap_or(&[])
}

/// Whether this compiler was composed over the plugin `dep` names.
pub fn composed_over(dep: &Depend) -> bool {
    let key = dep.plugin_key();
    set().iter().any(|entry| entry == &key)
}
