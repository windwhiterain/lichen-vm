//! The package manager's preprocessor.
//!
//! This is the crate that **owns the preprocessor import path**: it is where
//! a project's `@{…@}` block is cut out and its `import "…"` directives are
//! resolved, with git-fetched dependencies staged onto that path.
//!
//! The byte scan and the block's mini-frontend live in
//! `crates/lichen-language/src/preprocess/` (the language crate owns the
//! grammar and the block *syntax*).  This module is the **front-line owner**
//! of the workflow: it re-exports the block scanner / directive helpers a
//! tool uses, and it drives the end-to-end preprocess for a project whose
//! dependencies come from git.  Before the language crate's `preprocess` runs,
//! [`crate::project::Project::stage`] registers every fetched dependency's
//! vendored directory with the shared [`PackageStore`]
//! ([`PackageStore::register_vendored`]), so `import "dep"` /
//! `import "dep/rest"` resolves into the fetched clone
//! ([`lichen_language::package::PackageStore::resolve_import`]).
//!
//! Re-exported here (from `lichen_language::preprocess`) so a consumer of the
//! package manager never imports the language crate's module directly for the
//! scanner: [`split_block`], [`block_directives`], [`block_metadata`],
//! [`Directive`], [`Preprocessed`], [`ResolvedImport`].

pub use lichen_language::preprocess::{
    Directive, Preprocessed, ResolvedImport, block_directives, block_metadata, split_block,
};

use std::path::Path;

use lichen_language::diag::Diag;
use lichen_language::package::PackageStore;

/// Preprocess a project source: cut the leading `@{…@}` block, resolve its
/// `import` bindings through `store` (whose vendored dependency aliases have
/// already been registered by the package manager), and collect the block's
/// string metadata.
///
/// This is the package manager's entry point onto the language crate's
/// preprocessor — the ownership seam is that the caller stages the dependency
/// aliases first ([`crate::project::Project::stage`]); everything after is the
/// language crate's pure block handling.
pub fn preprocess<'a>(
    source: &'a str,
    base: Option<&Path>,
    store: &mut PackageStore,
) -> (Preprocessed<'a>, Vec<Diag>) {
    lichen_language::preprocess::preprocess(source, base, store)
}
