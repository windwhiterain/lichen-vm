//! The cache-root resolver: artifact bytes by file ID, and the shipping cache slot.

use super::*;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use lichen_lowlevel::{LocalNodeId, Program, StaticModule};

/// Load and deserialize a file ID's artifact; `modules` must hold its dependencies.
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

/// The Lichen Home `compilers/<toolchain-key>` slot for the shipping vocabulary.
///
/// # Invariant
/// The slot key comes from `lichen_utils::cache::compiler_slot_key`, the one derivation
/// the package manager also calls, so the shipping compiler, the language server and the
/// manager install into the same slot; deriving it here from this crate's version is
/// what let the two sides drift.
pub fn shipping_cache_root() -> PathBuf {
    let key = lichen_utils::cache::compiler_slot_key(lichen_utils::cache::DEFAULT_CORE_REPO, &[]);
    lichendir().join("compilers").join(key)
}
