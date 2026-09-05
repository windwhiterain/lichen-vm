//! The preprocessor, re-exported from the isolated [`lichen_preprocess`] crate.
//!
//! The `@{...@}` block scanner, its mini-frontend, the `Directive` grammar, the
//! `Depend` git-dependency type, and the preprocessor's import path
//! ([`lichen_preprocess::lichendir`] / [`lichen_preprocess::sources_root`])
//! all live in `crates/lichen-preprocess`.  This module is a thin shim that
//! re-exports them (so the existing `liche_language::preprocess::*` paths
//! resolve unchanged) and pins the vocabulary-bound export handle to the
//! language crate's [`StaticNodeId`].
//!
//! The two orchestrators [`preprocess`] and [`stage_depends`] keep their
//! original signatures — generic over the program's value/operator vocabularies
//! `V`/`O` and its artifact codec `C` — so the language crate's `PackageStore`,
//! CLI, and run paths call them exactly as before.  Internally they delegate to
//! [`lichen_preprocess`], which only knows the [`lichen_preprocess::ImportResolver`]
//! trait; the language crate's [`PackageStore`] implements that trait.

use std::path::Path;

use lichen_compute::{ComputeOperator, ComputeValue};
use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_lowlevel::{LowOperator, OperatorExt, StaticNodeId};
use lichen_utils::extend::AsEnum;

use crate::CompiledProgram;
use crate::diag::Diag;
use crate::package::PackageStore;
use crate::persist::ArtifactCodec;
use crate::program::GcdOp;

/// The block scanner / directive helpers and the preprocessor's data types,
/// re-exported from the isolated `lichen-preprocess` crate.
pub use lichen_preprocess::{
    Depend, Directive, PreprocessDiag, block_depends, block_directives, block_metadata, depend_of,
    split_block,
};
pub use lichen_preprocess::{ImportResolver, ResolvedPackage};

/// The language crate pins the preprocessor's export handle to its static node
/// id (see [`lichen_lowlevel::StaticNodeId`]).
pub type ResolvedImport = lichen_preprocess::ResolvedImport<StaticNodeId>;
/// The language crate pins the preprocessor's output to its static node id.
pub type Preprocessed<'a> = lichen_preprocess::Preprocessed<'a, StaticNodeId>;

/// Preprocess a project source: cut the leading `@{…@}` block, resolve its
/// `import` bindings through `store` (whose vendored dependency aliases have
/// already been registered by the compiler against the source cache), and
/// collect the block's string metadata.
///
/// This is the language crate's entry point onto the preprocessor — the
/// ownership seam is that the compiler stages the dependency aliases first (via
/// [`stage_depends`]); everything after is the isolated preprocessor's pure
/// block handling.  The preprocessor only knows [`ImportResolver`]; the
/// [`PackageStore`] implements it.
pub fn preprocess<'a, V, O, C>(
    raw: &'a str,
    base: Option<&Path>,
    store: &mut PackageStore<V, O, C>,
) -> (Preprocessed<'a>, Vec<Diag<CompiledProgram<V, O>>>)
where
    V: ValueType + From<ComputeValue> + 'static,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq
        + From<GcdOp>
        + From<TypeOperator>
        + From<ComputeOperator>
        + 'static,
    C: ArtifactCodec<CompiledProgram<V, O>> + Default,
{
    let (pre, diags) = lichen_preprocess::preprocess::<StaticNodeId, _>(raw, base, store);
    (pre, diags.into_iter().map(Diag::from_preprocess).collect())
}

/// Stage a source's `depend "url"` / `name = plug "url"` directives onto
/// `store` as vendored aliases, resolving each against the lichen-home source
/// cache.  The compiler never fetches git sources itself — see
/// [`lichen_preprocess::stage_depends`].
pub fn stage_depends<V, O, C>(
    store: &mut PackageStore<V, O, C>,
    source: &str,
) -> Vec<Diag<CompiledProgram<V, O>>>
where
    V: ValueType + From<ComputeValue> + 'static,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq
        + From<GcdOp>
        + From<TypeOperator>
        + From<ComputeOperator>
        + 'static,
    C: ArtifactCodec<CompiledProgram<V, O>> + Default,
{
    lichen_preprocess::stage_depends::<StaticNodeId, _>(store, source)
        .into_iter()
        .map(Diag::from_preprocess)
        .collect()
}
