//! The package manager's preprocessor surface, re-exported from the isolated
//! [`lichen_preprocess`] crate.
//!
//! The `@{…@}` block scanner, its mini-frontend, the `Directive` grammar, the
//! `Depend` git-dependency type, and the preprocessor's import path
//! ([`lichen_preprocess::lichendir`] / [`lichen_preprocess::sources_root`])
//! live in `crates/lichen-preprocess`.  The package manager **owns the
//! preprocessor import path** the way it always has — it is what fetches every
//! `depend` into the lichen-home source cache ([`crate::git`]) before the
//! compiler resolves the vendored aliases — but the scanner and the grammar are
//! shared from that one crate, not duplicated here.
//!
//! Re-exported here so a consumer of the package manager never imports the
//! preprocessor crate's module directly for the scanner: [`split_block`],
//! [`block_directives`], [`block_metadata`], [`block_depends`],
//! [`Directive`], [`Depend`], [`Preprocessed`], [`ResolvedImport`],
//! [`PreprocessDiag`], and the [`ImportResolver`] seam.

pub use lichen_preprocess::{
    Depend, Directive, ImportResolver, PreprocessDiag, Preprocessed, ResolvedImport,
    ResolvedPackage, block_depends, block_directives, block_metadata, depend_of, split_block,
};
