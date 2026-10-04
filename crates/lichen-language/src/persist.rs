//! The vocabulary-specific half of the device's persistent store.
//!
//! The type-independent device layer — the disk [`DeviceRegistry`], the
//! registry file format, the binary byte codec, the [`ModuleKey`], and the
//! hash helpers — lives in the leaf [`lichen-registry`] crate (so the package
//! manager can reclaim cache artifacts without linking the VM stack).  This
//! module keeps only what names a program's value/operator vocabulary, in the
//! three siblings it declares below:
//!
//! * `codec` — the [`ArtifactCodec`] / [`ProgramCodecOf`] traits and the
//!   [`NoPersist`] marker,
//! * `container` — the artifact container serialization
//!   ([`serialize_artifact_with`] / [`deserialize_artifact_with`]) and the
//!   shape encoders,
//! * `cache` — [`load_artifact`], which reads a file ID's artifact bytes and
//!   deserializes them with the vocabulary's codec.
//!
//! The registry's [cross-process cache ownership][`crate::persist`] still
//! works exactly as before; only the definitions moved.
//!
//! The cache is content-addressed: each compiled package is serialized into
//! `artifacts/<hash>.module`, and the registry file records, per package, the
//! hash of its raw source, the keys of its direct dependencies, and the path
//! it was compiled from.  Loading a package is an *incremental* verification
//! over that recorded dependency graph — a source file hash and an index
//! lookup per node, never a re-parse or a transitive re-hash — and only the
//! chain that actually changed is recompiled.

mod cache;
mod codec;
mod container;

pub use cache::{load_artifact, shipping_cache_root};
pub use codec::{ArtifactCodec, NoPersist, ProgramCodecOf};
pub use container::{
    deserialize_artifact, deserialize_artifact_with, serialize_artifact, serialize_artifact_with,
};

pub use lichen_lowlevel::codec::{Reader, Writer, arena_align, arena_base};

// The shared hash helpers (`Hash`, `sha256`, `hex`) live in the leaf utility
// crate, so the package manager (which keys its compiler cache by them) does
// not depend on the language crate.  Re-exported here for the existing
// `lichen_language::persist::{Hash, sha256, hex}` paths.
pub use lichen_utils::hash::{Hash, hex, sha256};

// The type-independent device layer (the disk registry, its key/hash helpers)
// lives in `lichen-registry`.  Re-exported here so the existing
// `lichen_language::persist::{ModuleKey, DeviceRegistry, Entry, Verified,
// file_id_hash, is_lichen_file_id, artifact_hash}` paths keep resolving.
pub use lichen_registry::{
    DeviceRegistry, Entry, ModuleKey, Verified, artifact_hash, file_id_hash, is_lichen_file_id,
    is_virtual_file_id, virtual_file_id, virtual_name,
};

// The lichen home / git source cache root live in the preprocessor crate,
// which owns the preprocessor import path.  Re-exported here so the existing
// `lichen_language::persist::{lichendir, sources_root}` paths resolve.
pub use lichen_preprocess::{SOURCES_DIR, lichendir, sources_root};

// ---------------------------------------------------------------------------
// The artifact format (`artifacts/<hash>.module`)
//
//   magic "LCHN" | version u32 | key u64 | hash 32B | max_align u64
//   | body_digest 32B
//   | body
//
// The header ends at `body_digest`; the body is everything after it:
//
//   export u64 | arena_len u64 | arena bytes
//   | node_count u64 | nodes...
//   | function_count u64 | functions...
//
// `body_digest` is the SHA-256 of exactly the body bytes — the bytes after the
// header, and nothing before them (the digest field itself is not covered, of
// course).  The reader verifies it before it reads any body field, so a body
// that was truncated, mis-copied or bit-rotted is a clean "recompile" answer
// instead of a module that loads and is silently wrong.  Field validation
// cannot replace it: a corrupted index can still land inside its declared
// range.
//
// **What the digest is not.**  It is not authenticity: it is not a signature
// and it does not restrain a deliberate writer, because whoever can write the
// artifact file can recompute the digest.  The bound on a deliberate writer is
// memory safety plus total field validation (`P0-1`, `P0-5`, `P0-2` in
// `docs/notes/code-audit.md`), not this field.
//
// A function: parameter u64, return u64, assert_count u64, [asserts u64...],
// node_count u64, [nodes u64...].  The node list is the function's template
// scope in local-index order — the static mirror of `Function::nodes`, so a
// re-homed static closure knows its own scope.
//
// A node:  value_flag u8, [value], op_flag u8, [op_tag u8, operand_flag u8,
// operand u64], equality (parent/next/tail: flag+u64, size u32),
// parameterized u8, low_shape u8 [shape].
//
// Refs (node items, function values, array handles) are written as their
// module's device key plus the local index (or the arena-relative offset
// and length for a handle) — keys are stable across processes, so the
// serialized form needs no relocation on load; the loader only re-resolves
// the arena pointers.  A handle's `offset` field is serialized as its
// base-relative arena offset — a plain number, no pointer semantics — and
// rebuilt against the freshly laid-out arena with the same alignment
// formula the freeze used ([`arena_base`]).
// ---------------------------------------------------------------------------
/// The container's format version — the header layout plus the body encoding.
///
/// The reader accepts only this value, so a change to either half bumps it and
/// retires the artifacts written before the change: they fail the version
/// check and recompile, which is the intended answer, not a compatibility path.
///
/// `5` added the low type's bottom, `LowShape::Unknown` (shape tag 5), so an
/// artifact written before it cannot be read as one: a node's stored low type
/// is now a lattice position rather than a decided shape.
///
/// `7` added the float value to the body's value encoding (`LowValue::Float`,
/// value tag `8`, written as its bits).  That is a change to the encoding half,
/// so the check above retires the artifacts written before it.
///
/// `8` added the struct marker's third field, the field names in definition
/// order (`[TypeId, names, names_in_order]`).  The body's *node* encoding is
/// unchanged — a marker is an ordinary array — but its meaning is not: a
/// 2-field marker is no longer a struct marker, so an artifact written before
/// this would read its struct types as unrecognised shapes rather than fail.
/// The bump is what turns that into the recompile the check above intends.
///
/// `9` moved the `TypeStruct` tag into the struct marker itself: the marker is
/// now the ordinary `[payload, TypeStruct]` pair over a `[TypeId, names,
/// names_in_order]` payload, so a struct kind's marker slot holds a pair where
/// version `8` held the bare payload array.  Again the *node* encoding is
/// unchanged, but an artifact of version `8` would read its struct kinds as
/// unrecognised shapes (its marker has no `TypeStruct` tag), so its struct
/// types and every named read over them would be silently wrong.  Version-`8`
/// artifacts exist outside the source tree (the device cache), so the bump is
/// warranted rather than skipped.
const ARTIFACT_FORMAT_VERSION: u32 = 9;
