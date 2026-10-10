//! The preprocessor, re-exported from the [`lichen_preprocess`] crate.
//! See docs/notes/preprocessor-isolation.md.

use std::path::Path;

use lichen_compute::{ComputeOperator, ComputeValue};
use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_lowlevel::StaticNodeId;

use crate::LangProgramShape;
use crate::diag::Diag;
use crate::package::PackageStore;
use crate::program::GcdOp;

/// The block scanner, directive helpers and preprocessor data types.
pub use lichen_preprocess::{
    Depend, Directive, PreprocessDiag, block_depends, block_directives, block_metadata, depend_of,
    split_block,
};
pub use lichen_preprocess::{ImportResolver, ResolvedPackage};

/// The language crate pins the export handle to its static node id.
pub type ResolvedImport = lichen_preprocess::ResolvedImport<StaticNodeId>;
/// The language crate pins the preprocessor's output to its static node id.
pub type Preprocessed<'a> = lichen_preprocess::Preprocessed<'a, StaticNodeId>;

/// Preprocess a project source: cut the `---…---` block, resolve its imports.
///
/// # Invariant
///
/// The compiler stages dependency aliases first ([`stage_depends`]); the
/// preprocessor only knows [`ImportResolver`], which [`PackageStore`] implements.
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
    // The prelude goes in first, so a program's own name wins: it is shadowable.

    // The built-in modules are exempt: `core` cannot be its own prelude, and
    // `compute.lichen` must not depend on one.
    if !is_builtin_source(base) {
        match store.prelude_import() {
            Ok(import) => pre.imports.insert(0, import),
            Err(diags) => out.extend(diags),
        }
    }
    (pre, out)
}

/// Whether `base` names a module the language compiles itself.
fn is_builtin_source(base: Option<&Path>) -> bool {
    base.and_then(|path| path.file_name()).is_some_and(|name| {
        name == crate::package::CORE_PATH || name == crate::package::COMPUTE_PATH
    })
}

/// Stage a source's `depend`/`plug` directives onto `store` as vendored aliases.
///
/// # Invariant
///
/// The compiler never fetches git sources itself.
/// See [`lichen_preprocess::stage_depends`].
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
