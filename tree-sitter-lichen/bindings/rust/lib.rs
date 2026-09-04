//! Tree-sitter Rust binding for the Lichen grammar.
//!
//! In-tree tests exercise the parser against the sample programs under
//! `crates/lichen-language/examples/programs/`.

use tree_sitter_language::LanguageFn;

extern "C" {
    /// The generated parser entry point: returns a `*const TSLanguage`.
    fn tree_sitter_lichen() -> *const ();
}

/// The raw tree-sitter [`LanguageFn`] for Lichen.
pub fn language() -> LanguageFn {
    // SAFETY: `tree_sitter_lichen` is a valid C symbol produced by
    // `tree-sitter generate` and returns a stable `ts::Language` pointer.
    unsafe { LanguageFn::from_raw(tree_sitter_lichen) }
}

/// The node types JSON for the grammar (used by tooling for schema/docs).
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");
