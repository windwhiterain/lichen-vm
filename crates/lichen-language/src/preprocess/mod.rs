//! The preprocessor, re-exported from the isolated [`lichen_preprocess`] crate.
//!
//! The `---...---` block scanner, its mini-frontend, the `Directive` grammar, the
//! `Depend` git-dependency type, and the preprocessor's import path
//! ([`lichen_preprocess::lichendir`] / [`lichen_preprocess::sources_root`])
//! all live in `crates/lichen-preprocess`.  This module is a thin shim that
//! re-exports them (so the existing `lichen_language::preprocess::*` paths
//! resolve unchanged) and pins the vocabulary-bound export handle to the
//! language crate's [`StaticNodeId`].
//!
//! The two orchestrators [`preprocess`] and [`stage_depends`] keep their
//! original signatures — generic over a single program type `P` (the
//! associated-type collector, whose `P::Codec` is the artifact codec) — so the
//! language crate's `PackageStore`, CLI, and run paths call them exactly as
//! before.  Internally they delegate to [`lichen_preprocess`], which only knows
//! the [`lichen_preprocess::ImportResolver`] trait; the language crate's
//! [`PackageStore`] implements that trait.

use std::path::Path;

use lichen_compute::{ComputeOperator, ComputeValue};
use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_lowlevel::StaticNodeId;

use crate::LangProgramShape;
use crate::diag::Diag;
use crate::package::PackageStore;
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

/// Preprocess a project source: cut the leading `---…---` block, resolve its
/// `import` bindings through `store` (whose vendored dependency aliases have
/// already been registered by the compiler against the source cache), and
/// collect the block's string metadata.
///
/// This is the language crate's entry point onto the preprocessor — the
/// ownership seam is that the compiler stages the dependency aliases first (via
/// [`stage_depends`]); everything after is the isolated preprocessor's pure
/// block handling.  The preprocessor only knows [`ImportResolver`]; the
/// [`PackageStore`] implements it.
pub fn preprocess<'a, P>(
    raw: &'a str,
    base: Option<&Path>,
    store: &mut PackageStore<P>,
) -> (Preprocessed<'a>, Vec<Diag<P>>)
where
    P: LangProgramShape,
    P::Value: ValueType + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    let (mut pre, diags) = lichen_preprocess::preprocess::<StaticNodeId, _>(raw, base, store);
    let mut out: Vec<Diag<P>> = diags.into_iter().map(Diag::from_preprocess).collect();
    // **The prelude.**  Every source is seeded with the built-in `core` module,
    // so the operator contract — `Num`, `in_num`, `add`, … — is in scope with no
    // import (`docs/notes/operator-polymorphism.md` §9 Phase 3).  It goes in
    // first, so a program's own `import` or binding of one of those names is
    // resolved later and wins: the prelude is *shadowable*, not reserved.
    //
    // The built-in modules themselves are exempt.  `core` cannot be its own
    // prelude (the load would re-enter itself), and `compute.lichen` is a
    // plugin's private source, compiled against that plugin's own registry —
    // it spells the read it needs and must not depend on a prelude.
    if !is_builtin_source(base) {
        match store.prelude_import() {
            Ok(import) => pre.imports.insert(0, import),
            Err(diags) => out.extend(diags),
        }
    }
    (pre, out)
}

/// Whether `base` names one of the modules the store compiles as part of the
/// language rather than as a source a host handed it.
fn is_builtin_source(base: Option<&Path>) -> bool {
    base.and_then(|path| path.file_name()).is_some_and(|name| {
        name == crate::package::CORE_PATH || name == crate::package::COMPUTE_PATH
    })
}

/// Stage a source's `depend "url"` / `name = plug "url"` directives onto
/// `store` as vendored aliases, resolving each against the lichen-home source
/// cache.  The compiler never fetches git sources itself — see
/// [`lichen_preprocess::stage_depends`].
pub fn stage_depends<P>(store: &mut PackageStore<P>, source: &str) -> Vec<Diag<P>>
where
    P: LangProgramShape,
    P::Value: ValueType + From<ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<ComputeOperator> + 'static,
{
    lichen_preprocess::stage_depends::<StaticNodeId, _>(store, source)
        .into_iter()
        .map(Diag::from_preprocess)
        .collect()
}
