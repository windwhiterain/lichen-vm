//! The device-cache registry: the type-independent half of the persistence layer.
//! See docs/notes/artifact-cache.md.
//!
//! # Invariant
//!
//! It owns everything that never names a program value or operator — the byte codec, `ModuleKey`,
//! the disk `DeviceRegistry` and the hashes — while the artifact content and its vocabulary codec
//! stay in `lichen-language`. Both the compiler and the package manager depend on it, so the
//! package manager can reclaim artifacts without linking the language or VM stack.

pub mod codec;
pub mod device;
pub mod module_key;

pub use lichen_utils::hash::{Hash, hex, sha256};

pub use device::{
    DeviceRegistry, Entry, Verified, artifact_hash, file_id_hash, is_lichen_file_id,
    is_virtual_file_id, virtual_file_id, virtual_name,
};
pub use module_key::ModuleKey;
