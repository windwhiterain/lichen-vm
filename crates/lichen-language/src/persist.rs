//! The vocabulary-specific half of the device's persistent store.
//!
//! The type-independent device layer — the disk [`DeviceRegistry`], the
//! registry file format, the binary byte codec, the [`ModuleKey`], and the
//! hash helpers — lives in the leaf [`lichen-registry`] crate (so the package
//! manager can reclaim cache artifacts without linking the VM stack).  This
//! module keeps only what names a program's value/operator vocabulary:
//!
//! * the [`ArtifactCodec`] / [`ProgramCodecOf`] traits and the [`NoPersist`]
//!   marker,
//! * the artifact container serialization ([`serialize_artifact_with`] /
//!   [`deserialize_artifact_with`]) and the shape encoders,
//! * [`load_artifact`], which reads a file ID's artifact bytes and
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

use std::collections::HashMap;
use std::sync::Arc;

use lichen_highlevel::program::HighProgram;
pub use lichen_lowlevel::codec::{ARENA_ALIGN, Reader, Writer, arena_base};
use lichen_lowlevel::{
    LocalNodeId, LowShape, Program, StaticFunction, StaticModule, StaticNode, StaticOperation,
};

use crate::program::{LangProgram, ProgramCodec};

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
};

// ---------------------------------------------------------------------------
// The artifact format (`artifacts/<hash>.module`)
//
//   magic "LCHN" | version u32 | key u64 | hash 32B | max_align u64
//   | export u64 | arena_len u64 | arena bytes
//   | node_count u64 | nodes...
//   | function_count u64 | functions...
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

/// The vocabulary-specific half of the artifact format.
///
/// The artifact header, node/function frames, arena layout and equality data
/// are generic.  The only vocabulary-dependent parts are the value encoding
/// and the operator encoding; this trait isolates them so a downstream
/// program with extra value/operator variants can reuse the same artifact
/// container by implementing a codec.
///
/// **Codec contract:** `write_value`/`read_value` (and `write_operator`/
/// `read_operator`) must be exact inverses — every tag the writer emits, the
/// reader must decode to the equal value, and nothing else.  Adding a variant
/// to one side and not the other compiles but silently breaks every cache
/// load (a stored artifact fails to deserialize and is recompiled).  This is
/// enforced by the `codec_roundtrip` test below, which round-trips every
/// value and operator variant; keep the two sides in sync with it.
pub trait ArtifactCodec<P: Program> {
    /// Whether this codec actually persists to a device cache directory.  A
    /// codec that is in-memory only (`NoPersist`) sets this to `false` so the
    /// CLI drives the package store without a cache directory (and so never
    /// reaches a serialize/deserialize path that would panic).
    const PERSISTENT: bool = true;

    /// Write one node value.
    fn write_value(
        w: &mut Writer,
        value: P::Value,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    );

    /// Read one node value.
    fn read_value(
        r: &mut Reader<'_>,
        self_key: ModuleKey,
        self_arena: &[u8],
        self_base: *const u8,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<P::Value, String>;

    /// Write one operation's operator tag.
    fn write_operator(w: &mut Writer, operator: P::Operator);

    /// Read one operation's operator tag.
    fn read_operator(r: &mut Reader<'_>) -> Result<P::Operator, String>;
}

/// A compiled program that carries its own artifact codec, so the language
/// tooling is generic over a single program type `P` (the associated-type
/// collector) rather than the value/operator leaves plus a separate `C` codec.
///
/// A codec is not an associated type of the lowlevel [`Program`] trait — it is a
/// serialization concern layered on top by `lichen-language` — so this trait is
/// the seam that folds the codec into the collector.  The
/// [`lang_compose_vocabulary!`](crate::lang_compose_vocabulary) macro implements
/// it for every composed program, binding `Codec` to the [`ProgramCodec`] that
/// vocabulary emits; a program that never persists uses [`NoPersist`].
pub trait ProgramCodecOf: HighProgram {
    /// The artifact codec for this program (a composed program's
    /// [`crate::program::ProgramCodec`], or [`NoPersist`] for one that never
    /// serializes).
    type Codec: ArtifactCodec<Self> + Default;
}

/// A marker codec for a program that is compiled in memory only and never
/// serialized to the device cache (the package store's in-memory path, and a
/// plugin program whose artifact codec has not been generated yet).  Every
/// method is unreachable — the codec is only ever selected when the store has
/// no cache directory, so `try_reuse`/`build_package` never reach the
/// serialize/deserialize path.
pub struct NoPersist;

impl<P: Program> ArtifactCodec<P> for NoPersist {
    const PERSISTENT: bool = false;

    fn write_value(
        _w: &mut Writer,
        _value: P::Value,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) {
        unreachable!("NoPersist cannot write values — the store has no device cache")
    }

    fn read_value(
        _r: &mut Reader<'_>,
        _self_key: ModuleKey,
        _self_arena: &[u8],
        _self_base: *const u8,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<P::Value, String> {
        unreachable!("NoPersist cannot read values — the store has no device cache")
    }

    fn write_operator(_w: &mut Writer, _operator: P::Operator) {
        unreachable!("NoPersist cannot write operators — the store has no device cache")
    }

    fn read_operator(_r: &mut Reader<'_>) -> Result<P::Operator, String> {
        unreachable!("NoPersist cannot read operators — the store has no device cache")
    }
}

impl Default for NoPersist {
    fn default() -> Self {
        NoPersist
    }
}

/// Serialize `module` (and the arenas its refs point into, via `modules`)
/// into the portable artifact format.  `hash` and `export` are the package
/// metadata the store records alongside the module data.
pub fn serialize_artifact(
    module: &StaticModule<LangProgram>,
    modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
    hash: Hash,
    export: LocalNodeId,
) -> Vec<u8> {
    serialize_artifact_with(module, modules, hash, export, ProgramCodec)
}

/// [`Self::serialize_artifact`] with an explicit [`ArtifactCodec`], for
/// downstream vocabularies that need custom value/operator tags.
pub fn serialize_artifact_with<P, C>(
    module: &StaticModule<P>,
    modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    hash: Hash,
    export: LocalNodeId,
    _codec: C,
) -> Vec<u8>
where
    P: Program,
    C: ArtifactCodec<P>,
{
    let mut w = Writer::new();
    w.bytes(b"LCHN");
    w.u32(3); // format version
    w.u64(module.key.as_raw());
    w.bytes(&hash);
    w.u64(ARENA_ALIGN as u64);
    w.u64(export.index as u64);
    w.u64(module.arena.len() as u64);
    w.bytes(&module.arena);
    w.u64(module.nodes.len() as u64);
    for node in &module.nodes {
        match node.value {
            None => w.u8(0),
            Some(value) => {
                w.u8(1);
                C::write_value(&mut w, value, modules);
            }
        }
        match node.operation {
            None => w.u8(0),
            Some(operation) => {
                w.u8(1);
                C::write_operator(&mut w, operation.operator);
                match operation.operand {
                    None => w.u8(0),
                    Some(operand) => {
                        w.u8(1);
                        w.u64(operand.index as u64);
                    }
                }
            }
        }
        match node.equality.parent {
            None => w.u8(0),
            Some(parent) => {
                w.u8(1);
                w.u64(parent.index as u64);
            }
        }
        match node.equality.next {
            None => w.u8(0),
            Some(next) => {
                w.u8(1);
                w.u64(next.index as u64);
            }
        }
        match node.equality.tail {
            None => w.u8(0),
            Some(tail) => {
                w.u8(1);
                w.u64(tail.index as u64);
            }
        }
        w.u32(node.equality.size);
        w.u8(node.parameterized as u8);
        write_low_shape_opt(&mut w, &node.low_shape);
    }
    w.u64(module.functions.len() as u64);
    for function in &module.functions {
        w.u64(function.parameter.index as u64);
        w.u64(function.r#return.index as u64);
        w.u64(function.asserts.len() as u64);
        for &assert in &function.asserts {
            w.u64(assert.index as u64);
        }
        w.u64(function.nodes.len() as u64);
        for &node in &function.nodes {
            w.u64(node.index as u64);
        }
    }
    w.into_bytes()
}

/// Write an optional [`LowShape`] (the node's stored shape marker).
fn write_low_shape_opt(w: &mut Writer, shape: &Option<LowShape>) {
    match shape {
        None => w.u8(0),
        Some(shape) => {
            w.u8(1);
            write_low_shape(w, shape);
        }
    }
}

fn write_low_shape(w: &mut Writer, shape: &LowShape) {
    match shape {
        LowShape::USize => w.u8(0),
        LowShape::Tuple(items) => {
            w.u8(1);
            w.u64(items.len() as u64);
            for item in items {
                write_low_shape(w, item);
            }
        }
        LowShape::Array(elem, len) => {
            w.u8(2);
            write_low_shape(w, elem);
            w.u64(*len as u64);
        }
        LowShape::Function(param, result) => {
            w.u8(3);
            write_low_shape(w, param);
            write_low_shape(w, result);
        }
        LowShape::Table(key, value) => {
            w.u8(4);
            write_low_shape(w, key);
            write_low_shape(w, value);
        }
    }
}

fn read_low_shape_opt(r: &mut Reader<'_>) -> Result<Option<LowShape>, String> {
    match r.u8()? {
        0 => Ok(None),
        1 => Ok(Some(read_low_shape(r)?)),
        _ => Err("bad low_shape option tag".into()),
    }
}

fn read_low_shape(r: &mut Reader<'_>) -> Result<LowShape, String> {
    match r.u8()? {
        0 => Ok(LowShape::USize),
        1 => {
            let len = r.u64()? as usize;
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(read_low_shape(r)?);
            }
            Ok(LowShape::Tuple(items))
        }
        2 => {
            let elem = Box::new(read_low_shape(r)?);
            let len = r.u64()? as usize;
            Ok(LowShape::Array(elem, len))
        }
        3 => Ok(LowShape::Function(
            Box::new(read_low_shape(r)?),
            Box::new(read_low_shape(r)?),
        )),
        4 => Ok(LowShape::Table(
            Box::new(read_low_shape(r)?),
            Box::new(read_low_shape(r)?),
        )),
        _ => Err("bad low_shape tag".into()),
    }
}

/// Deserialize an artifact.  `key` and `hash` are the expected identity of
/// the file (verified against the header); `modules` supplies the arenas of
/// the artifact's dependencies, which must already be registered — foreign
/// refs resolve through their keys, absolute from birth.  Returns the
/// module and the exported root's local index.
pub fn deserialize_artifact(
    bytes: &[u8],
    key: ModuleKey,
    hash: Hash,
    modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
) -> Result<(StaticModule<LangProgram>, LocalNodeId), String> {
    deserialize_artifact_with(bytes, key, hash, modules, ProgramCodec)
}

/// [`Self::deserialize_artifact`] with an explicit [`ArtifactCodec`], for
/// downstream vocabularies that need custom value/operator tags.
pub fn deserialize_artifact_with<P, C>(
    bytes: &[u8],
    key: ModuleKey,
    hash: Hash,
    modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    _codec: C,
) -> Result<(StaticModule<P>, LocalNodeId), String>
where
    P: Program,
    C: ArtifactCodec<P>,
{
    let mut r = Reader::new(bytes);
    if r.take(4)? != b"LCHN" {
        return Err("bad artifact magic".into());
    }
    if r.u32()? != 3 {
        return Err("unknown artifact format version".into());
    }
    if ModuleKey::from_raw(r.u64()?) != key {
        return Err("artifact key does not match its file".into());
    }
    if r.take(32)? != hash {
        return Err("artifact hash does not match its file".into());
    }
    let max_align = r.u64()? as usize;
    if max_align != ARENA_ALIGN {
        return Err("artifact payload alignment mismatch".into());
    }
    let export = LocalNodeId {
        index: r.u64()? as usize,
    };
    let arena_len = r.u64()? as usize;
    let arena = r.take(arena_len)?.to_vec();
    let base = arena_base(&arena);

    let node_count = r.u64()? as usize;
    let mut nodes: Vec<StaticNode<P>> = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let value = if r.u8()? != 0 {
            Some(C::read_value(&mut r, key, &arena, base, modules)?)
        } else {
            None
        };
        let operation = if r.u8()? != 0 {
            let operator = C::read_operator(&mut r)?;
            let operand = if r.u8()? != 0 {
                Some(LocalNodeId {
                    index: r.u64()? as usize,
                })
            } else {
                None
            };
            Some(StaticOperation { operator, operand })
        } else {
            None
        };
        let parent = if r.u8()? != 0 {
            Some(LocalNodeId {
                index: r.u64()? as usize,
            })
        } else {
            None
        };
        let next = if r.u8()? != 0 {
            Some(LocalNodeId {
                index: r.u64()? as usize,
            })
        } else {
            None
        };
        let tail = if r.u8()? != 0 {
            Some(LocalNodeId {
                index: r.u64()? as usize,
            })
        } else {
            None
        };
        let size = r.u32()?;
        let parameterized = r.u8()? != 0;
        let low_shape = read_low_shape_opt(&mut r)?;
        nodes.push(StaticNode {
            value,
            operation,
            low_shape,
            equality: lichen_utils::disjoint::Meta {
                parent,
                next,
                tail,
                size,
            },
            parameterized,
        });
    }

    let function_count = r.u64()? as usize;
    let mut functions: Vec<StaticFunction> = Vec::with_capacity(function_count);
    for _ in 0..function_count {
        let parameter = LocalNodeId {
            index: r.u64()? as usize,
        };
        let r#return = LocalNodeId {
            index: r.u64()? as usize,
        };
        let assert_count = r.u64()? as usize;
        let mut asserts = Vec::with_capacity(assert_count);
        for _ in 0..assert_count {
            asserts.push(LocalNodeId {
                index: r.u64()? as usize,
            });
        }
        let node_count = r.u64()? as usize;
        let mut nodes = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            nodes.push(LocalNodeId {
                index: r.u64()? as usize,
            });
        }
        functions.push(StaticFunction {
            parameter,
            r#return,
            asserts,
            nodes,
        });
    }
    if !r.done() {
        return Err("trailing bytes after the artifact".into());
    }
    Ok((
        StaticModule {
            key,
            nodes,
            functions,
            arena,
        },
        export,
    ))
}

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

// The lichen home / git source cache root live in the preprocessor crate,
// which owns the preprocessor import path.  Re-exported here so the existing
// `lichen_language::persist::{lichendir, sources_root}` paths resolve.
pub use lichen_preprocess::{SOURCES_DIR, lichendir, sources_root};

// ---------------------------------------------------------------------------
// Codec-round-trip tests: enforce the write/read bijection contract.
//
// `write_value`/`read_value` and `write_operator`/`read_operator` are two
// independent exhaustive `match`es — one over the value/operator *type*, one
// over the *tag byte*.  The compiler can check each side is total, but it
// cannot check that they name the same tag.  A variant added to the write
// side but not the read side (exactly the `TypeString` asymmetry) compiles
// and silently makes every stored artifact underivable.  These tests drive
// every (arena-free) value and every operator through the codec and assert
// the round trip is the identity, so an asymmetry fails the build.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod codec_roundtrip {
    use super::*;
    use crate::program::{LangOperator, LangValue};
    use lichen_highlevel::program::{TypeOperator, TypeValue};
    use lichen_lowlevel::{LowOperator, LowValue};

    /// Encode `v`, then decode it back and return the deserialized value.
    /// Arena-free variants never touch the module map or the (dummy) self
    /// arena/base, so the map is empty and the base is null.
    fn roundtrip_value(v: LangValue) -> LangValue {
        let modules: HashMap<ModuleKey, Arc<StaticModule<LangProgram>>> = HashMap::new();
        let mut w = Writer::new();
        ProgramCodec::write_value(&mut w, v, &modules);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let out = ProgramCodec::read_value(
            &mut r,
            ModuleKey::from_raw(0),
            &[],
            std::ptr::null(),
            &modules,
        )
        .expect("deserializing a value the codec itself wrote");
        assert!(r.done(), "the value codec left trailing bytes");
        out
    }

    /// Every `LangValue` variant that round-trips without a module arena.  The
    /// handle/function-ref variants (array/table/function tags) need a real
    /// frozen module and are exercised at the artifact level by the `persist`
    /// integration tests; this covers every scalar/type/string variant.
    #[test]
    fn every_arena_free_value_round_trips() {
        let values: &[LangValue] = &[
            LangValue::LowValue(LowValue::USize(41)),
            LangValue::LowValue(LowValue::None),
            LangValue::LowValue(LowValue::Parameterized),
            LangValue::LowValue(LowValue::Str("hello")),
            LangValue::TypeValue(TypeValue::TypeInt),
            LangValue::TypeValue(TypeValue::TypeType),
            LangValue::TypeValue(TypeValue::TypeFunction),
            LangValue::TypeValue(TypeValue::TypeTuple),
            LangValue::TypeValue(TypeValue::TypeArray),
            LangValue::TypeValue(TypeValue::TypeStruct),
            LangValue::TypeValue(TypeValue::TypeTable),
            LangValue::TypeValue(TypeValue::TypeString),
            LangValue::TypeValue(TypeValue::TypeId(7)),
            LangValue::ComputeValue(::lichen_compute::ComputeValue::TypeBuffer),
        ];
        for &v in values {
            assert_eq!(roundtrip_value(v), v, "value did not round-trip");
        }
    }

    fn roundtrip_op(op: LangOperator) -> LangOperator {
        let mut w = Writer::new();
        ProgramCodec::write_operator(&mut w, op);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let out = ProgramCodec::read_operator(&mut r)
            .expect("deserializing an operator the codec itself wrote");
        assert!(r.done(), "the operator codec left trailing bytes");
        out
    }

    #[test]
    fn every_operator_round_trips() {
        let ops: &[LangOperator] = &[
            LangOperator::LowOperator(LowOperator::Index),
            LangOperator::LowOperator(LowOperator::Apply),
            LangOperator::LowOperator(LowOperator::TableGet),
            LangOperator::TypeOperator(TypeOperator::Fresh),
            LangOperator::TypeOperator(TypeOperator::Add),
            LangOperator::TypeOperator(TypeOperator::Sub),
            LangOperator::TypeOperator(TypeOperator::Leq),
            LangOperator::TypeOperator(TypeOperator::Eq),
            LangOperator::GcdOp(crate::program::GcdOp::Gcd),
        ];
        for &op in ops {
            assert_eq!(roundtrip_op(op), op, "operator did not round-trip");
        }
    }
}
