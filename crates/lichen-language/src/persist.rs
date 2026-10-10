//! The vocabulary-specific half of the device's persistent store.
//! See docs/notes/artifact-cache.md.

mod cache;
mod codec;
mod container;

pub use cache::{load_artifact, shipping_cache_root};
pub use codec::{ArtifactCodec, NoPersist, ProgramCodecOf};
pub use container::{
    deserialize_artifact, deserialize_artifact_with, serialize_artifact, serialize_artifact_with,
};

pub use lichen_lowlevel::codec::{Reader, Writer, arena_align, arena_base};

// The hash helpers live in `lichen-utils`; re-exported for the old paths.
pub use lichen_utils::hash::{Hash, hex, sha256};

// The device layer lives in `lichen-registry`; re-exported for the old paths.
pub use lichen_registry::{
    DeviceRegistry, Entry, ModuleKey, Verified, artifact_hash, file_id_hash, is_lichen_file_id,
    is_virtual_file_id, virtual_file_id, virtual_name,
};

// The home/source roots live in `lichen-preprocess`; re-exported for old paths.
pub use lichen_preprocess::{SOURCES_DIR, lichendir, sources_root};

// The artifact byte format and the version history are documented in
// docs/notes/artifact-cache.md.
/// The container's format version — the header layout plus the body encoding.
///
/// # Invariant
///
/// The reader accepts only this value, so a change to either half bumps it and
/// retires the artifacts written before it: they fail the version check and
/// recompile. See docs/notes/artifact-cache.md.
const ARTIFACT_FORMAT_VERSION: u32 = 10;
