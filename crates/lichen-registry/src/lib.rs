//! The device-cache registry: the **type-independent** half of the lichen
//! persistence layer.
//!
//! The compiled-artifact cache and its registry live under a cache directory
//! (the compiler's `~/.lichen`, or a plugin-built compiler's per-plugin-set
//! slot).  The artifact *content*, and the value/operator vocabulary it was
//! encoded with, are **type-dependent** — that half stays in `lichen-language`
//! ([`crate::persist`], the `ArtifactCodec`).  This crate owns everything
//! else, which never mentions a program value or operator:
//!
//! * the binary byte codec ([`codec::Reader`] / [`codec::Writer`]),
//! * the device [`ModuleKey`] (a compact index into the shared registry),
//! * the disk [`DeviceRegistry`]: key allocation (with its free list), the
//!   file-ID → entry table, the cross-process `mkdir` lock, the registry
//!   binary format (`parse`/`serialize`), and the type-independent operations
//!   (`open`, `alloc`, `publish`, `verify`, `gc`, `remove`, `store_artifact`),
//! * the hash helpers ([`Hash`], [`sha256`], [`hex`], [`file_id_hash`],
//!   [`is_lichen_file_id`], [`artifact_hash`]).
//!
//! Both the compiler (`lichen-language`) and the package manager
//! (`lichen-package`) depend on this crate, so the package manager can open a
//! cache's registry and reclaim dead artifacts *without* linking the language
//! or VM stack.  The vocabulary-dependent artifact deserialization
//! (`deserialize_artifact_with`) stays behind in `lichen-language`, reached
//! through [`device::DeviceRegistry::artifact_path`].

pub mod codec;
pub mod device;
pub mod module_key;

pub use lichen_utils::hash::{Hash, hex, sha256};

pub use device::{DeviceRegistry, Entry, Verified, artifact_hash, file_id_hash, is_lichen_file_id};
pub use module_key::ModuleKey;
