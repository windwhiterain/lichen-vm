//! The native-plugin contract. See `docs/notes/plugin-taxonomy.md`.

/// The nominal marker of a native plugin (see the [module docs](self)).
///
/// # Invariant
/// Nothing constrains the trait: it has no methods and no generic bound, so the
/// `impl` declares the role rather than checking it. It stays published because
/// a plugin crate implements it out of tree.
pub trait NativePlugin {}

// Unused-import lint does not count doc-link usage; `allow` keeps these bound.
#[allow(unused_imports)]
use crate::attr::AttrExt as _AttrExtDoc;
#[allow(unused_imports)]
use crate::native::{NativeOp as _NativeOpDoc, NativeOps as _NativeOpsDoc};
