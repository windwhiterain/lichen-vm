//! The cache-root resolver: read a file ID's artifact bytes with the
//! vocabulary's codec, and name the shipping compiler's cache slot.

use super::*;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use lichen_lowlevel::{LocalNodeId, Program, StaticModule};

/// Load and deserialize a file ID's artifact.  `modules` must hold every
/// dependency the artifact's refs name (they are loaded first).  The codec
/// `C` decodes the value/operator variants of the program `P`.
pub fn load_artifact<P, C>(
    device: &DeviceRegistry,
    file_id: &str,
    key: ModuleKey,
    hash: Hash,
    modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
) -> Result<(StaticModule<P>, LocalNodeId), String>
where
    P: Program,
    C: ArtifactCodec<P> + Default,
{
    let bytes = std::fs::read(device.artifact_file(file_id))
        .map_err(|e| format!("cannot read cached artifact: {e}"))?;
    deserialize_artifact_with::<P, C>(&bytes, key, hash, modules, C::default())
}

/// The Lichen Home `compilers/<toolchain-key>` slot for the **shipping**
/// (empty plugin-set) vocabulary.
///
/// The slot key is derived by `lichen_utils::cache::compiler_slot_key`, the one
/// derivation the package manager also calls (for an empty plugin set it
/// computes `compiler_cache::key(DEFAULT_REPO, &[])`), so the shipping compiler
/// and language server cache under the same `compilers/<key>` slot the package
/// manager installs them into.  Deriving it here a second time from this
/// crate's own version is exactly what let the two sides drift.
pub fn shipping_cache_root() -> PathBuf {
    let key = lichen_utils::cache::compiler_slot_key(lichen_utils::cache::DEFAULT_CORE_REPO, &[]);
    lichendir().join("compilers").join(key)
}
