//! The device's persistent store: the registry file plus the artifact
//! cache under a cache directory (the CLI's `~/.lichen`).
//!
//! The [`DeviceRegistry`] is the disk shape of the lowlevel [`Registry`]:
//! it allocates the compact device keys ([`ModuleKey`] indices, reclaimed
//! through a free list) and owns the mapping the lowlevel's registry needs
//! at runtime — key → artifact content hash, key → dependency graph, and
//! path → key aliases.  Every process sharing the cache directory sees the
//! same keys for the same content, so refs serialized as keys resolve
//! identically everywhere.
//!
//! The cache is content-addressed: each compiled package is serialized into
//! `artifacts/<hash>.module`, and the registry file records, per package,
//! the hash of its raw source, the keys of its direct dependencies, and the
//! path it was compiled from.  Loading a package is an *incremental*
//! verification over that recorded dependency graph — a source file hash
//! and an index lookup per node, never a re-parse or a transitive re-hash —
//! and only the chain that actually changed is recompiled.
//!
//! Concurrency: registry mutations (key allocation, publishing, GC) are
//! serialized across processes by a `mkdir` lock with stale-timeout
//! recovery; reads (verification) lock nothing, since saves are atomic
//! renames.  A lost update only costs a recompile, never corruption.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lichen_highlevel::program::{TypeOperator, TypeValue};
use lichen_lowlevel::{
    AnyFunctionId, AnyHandle, ArrayItem, LocalNodeId, LowValue, ModuleKey, Program, StaticFunction,
    StaticFunctionId, StaticFunctionRef, StaticHandle, StaticModule, StaticNode, StaticOperation,
    TableItem,
};
use sha2::Digest as _;

use crate::program::{GcdOp, LangOperator, LangProgram, LangValue};

/// The content hash of an artifact — 32 bytes of SHA-256.
pub type Hash = [u8; 32];

/// SHA-256 over `bytes`.
pub fn sha256(bytes: &[u8]) -> Hash {
    use sha2::Sha256;
    Sha256::digest(bytes).into()
}

/// The artifact hash of a compiled package: the raw source bytes followed
/// by its direct dependency keys in source order.  The hash is transitive —
/// a dependency change changes the importer's hash — and deterministic, so
/// every process computes the same hash for the same source chain.
pub fn artifact_hash(source: &[u8], dep_keys: &[ModuleKey]) -> Hash {
    use sha2::Sha256;
    let mut hasher = Sha256::new();
    hasher.update(source);
    for &key in dep_keys {
        hasher.update(key.as_raw().to_le_bytes());
    }
    hasher.finalize().into()
}

// ---------------------------------------------------------------------------
// The artifact format (`artifacts/<hash>.module`)
//
//   magic "LCHN" | version u32 | key u64 | hash 32B | max_align u64
//   | export u64 | arena_len u64 | arena bytes
//   | node_count u64 | nodes...
//   | function_count u64 | functions...
//
// A node:  value_flag u8, [value], op_flag u8, [op_tag u8, operand_flag u8,
// operand u64], equality (parent/next/tail: flag+u64, size u32),
// parameterized u8.
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

/// The payload alignment of the frozen arena: array item slices (the only
/// payload kind of this vocabulary — `LangValue` carries no ext
/// handle variants).  `StaticModule::from_module` lays out payloads aligned
/// to `max(align_of::<ArrayItem>, P::Value::alignment())`; for the
/// highlevel vocabulary that maximum is `align_of::<ArrayItem>()`.
const ARENA_ALIGN: usize = std::mem::align_of::<ArrayItem>();

/// The vocabulary-specific half of the artifact format.
///
/// The artifact header, node/function frames, arena layout and equality data
/// are generic.  The only vocabulary-dependent parts are the value encoding
/// and the operator encoding; this trait isolates them so a downstream
/// program with extra value/operator variants can reuse the same artifact
/// container by implementing a codec.
pub trait ArtifactCodec<P: Program> {
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

/// The codec for the shipped highlevel language vocabulary.
pub struct HighProgramCodec;

/// The aligned base of a module's arena — the same formula
/// `StaticModule::from_module` used to lay the payloads out, so offsets
/// round-trip exactly.
fn arena_base(arena: &[u8]) -> *const u8 {
    let ptr = arena.as_ptr() as usize;
    let base = (ptr + ARENA_ALIGN - 1) & !(ARENA_ALIGN - 1);
    base as *const u8
}

/// The base-relative offset of a handle's payload pointer.
fn handle_offset<P: Program>(module: &StaticModule<P>, offset: *const u8) -> usize {
    let base = arena_base(&module.arena) as usize;
    let relative = offset as usize - base;
    assert!(
        relative <= module.arena.len(),
        "a handle offset outside its module's arena — broken frozen module"
    );
    relative
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
    serialize_artifact_with(module, modules, hash, export, HighProgramCodec)
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
    w.u32(2); // format version
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
    }
    w.u64(module.functions.len() as u64);
    for function in &module.functions {
        w.u64(function.parameter.index as u64);
        w.u64(function.r#return.index as u64);
        w.u64(function.asserts.len() as u64);
        for &assert in &function.asserts {
            w.u64(assert.index as u64);
        }
    }
    w.into_bytes()
}

impl ArtifactCodec<LangProgram> for HighProgramCodec {
    fn write_value(
        w: &mut Writer,
        value: LangValue,
        modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
    ) {
        write_value(w, value, modules);
    }

    fn read_value(
        r: &mut Reader<'_>,
        self_key: ModuleKey,
        self_arena: &[u8],
        self_base: *const u8,
        modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
    ) -> Result<LangValue, String> {
        read_value(r, self_key, self_arena, self_base, modules)
    }

    fn write_operator(w: &mut Writer, operator: LangOperator) {
        write_operator(w, operator);
    }

    fn read_operator(r: &mut Reader<'_>) -> Result<LangOperator, String> {
        read_operator(r)
    }
}

fn write_value(
    w: &mut Writer,
    value: LangValue,
    modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
) {
    match value {
        LangValue::LowValue(LowValue::USize(n)) => {
            w.u8(0);
            w.u64(n as u64);
        }
        LangValue::LowValue(LowValue::Array(AnyHandle::Static(handle))) => {
            w.u8(1);
            w.u64(handle.module.as_raw());
            let module = &modules[&handle.module];
            let slice = unsafe { &*handle.offset };
            w.u64(handle_offset(module, slice.as_ptr() as *const u8) as u64);
            w.u64(slice.len() as u64);
        }
        LangValue::LowValue(LowValue::Array(AnyHandle::Dynamic(_))) => {
            panic!("serializing a frozen module that carries a dynamic array payload")
        }
        LangValue::LowValue(LowValue::Table(AnyHandle::Static(handle))) => {
            w.u8(12);
            w.u64(handle.module.as_raw());
            let module = &modules[&handle.module];
            let slice = unsafe { &*handle.offset };
            w.u64(handle_offset(module, slice.as_ptr() as *const u8) as u64);
            w.u64(slice.len() as u64);
        }
        LangValue::LowValue(LowValue::Table(AnyHandle::Dynamic(_))) => {
            panic!("serializing a frozen module that carries a dynamic table payload")
        }
        LangValue::LowValue(LowValue::Function(AnyFunctionId::Static(function))) => {
            w.u8(2);
            w.u64(function.module.as_raw());
            w.u64(function.index.0 as u64);
        }
        LangValue::LowValue(LowValue::Function(AnyFunctionId::Dynamic(_))) => {
            panic!("serializing a frozen module that carries a dynamic function ref")
        }
        LangValue::LowValue(LowValue::None) => w.u8(3),
        LangValue::LowValue(LowValue::Parameterized) => w.u8(4),
        LangValue::TypeValue(TypeValue::TypeInt) => w.u8(5),
        LangValue::TypeValue(TypeValue::TypeType) => w.u8(6),
        LangValue::TypeValue(TypeValue::TypeFunction) => w.u8(7),
        LangValue::TypeValue(TypeValue::TypeTuple) => w.u8(8),
        LangValue::TypeValue(TypeValue::TypeArray) => w.u8(9),
        LangValue::TypeValue(TypeValue::TypeStruct) => w.u8(10),
        LangValue::TypeValue(TypeValue::TypeTable) => w.u8(13),
        LangValue::TypeValue(TypeValue::TypeId(n)) => {
            w.u8(11);
            w.u64(n as u64);
        }
        // A compute value (a kernel artifact, a native operator, or the
        // kernel/launch type markers) is runtime-only and never serialized
        // into a frozen, persistent package.
        LangValue::ComputeValue(_) => {
            panic!("serializing a compute value (Kernel/Launch are runtime-only)")
        }
    }
}

fn write_operator(w: &mut Writer, operator: LangOperator) {
    use lichen_lowlevel::LowOperator;
    match operator {
        LangOperator::LowOperator(LowOperator::Index) => w.u8(0),
        LangOperator::LowOperator(LowOperator::Apply) => w.u8(1),
        LangOperator::LowOperator(LowOperator::TableGet) => w.u8(8),
        LangOperator::TypeOperator(TypeOperator::Fresh) => w.u8(3),
        LangOperator::TypeOperator(TypeOperator::Add) => w.u8(4),
        LangOperator::TypeOperator(TypeOperator::Sub) => w.u8(5),
        LangOperator::TypeOperator(TypeOperator::Leq) => w.u8(6),
        LangOperator::TypeOperator(TypeOperator::Eq) => w.u8(7),
        LangOperator::GcdOp(GcdOp::Gcd) => w.u8(9),
        LangOperator::ComputeOperator(_) => {
            panic!("serializing a compute operator (Jit/Launch) into a persistent package");
        }
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
    deserialize_artifact_with(bytes, key, hash, modules, HighProgramCodec)
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
    if r.u32()? != 2 {
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
        nodes.push(StaticNode {
            value,
            operation,
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
        functions.push(StaticFunction {
            parameter,
            r#return,
            asserts,
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

fn read_value(
    r: &mut Reader,
    self_key: ModuleKey,
    self_arena: &[u8],
    self_base: *const u8,
    modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
) -> Result<LangValue, String> {
    Ok(match r.u8()? {
        0 => LangValue::LowValue(LowValue::USize(r.u64()? as usize)),
        1 => {
            let owner = ModuleKey::from_raw(r.u64()?);
            let offset = r.u64()? as usize;
            let len = r.u64()? as usize;
            let (owner_arena, owner_base) = if owner == self_key {
                (self_arena, self_base)
            } else {
                let module = modules.get(&owner).ok_or_else(|| {
                    format!("artifact references unregistered dependency key {owner:?}")
                })?;
                let arena: &[u8] = &module.arena;
                (arena, arena_base(arena))
            };
            let gap = owner_base as usize - owner_arena.as_ptr() as usize;
            if offset + len > owner_arena.len() - gap {
                return Err("artifact handle out of its arena's bounds".into());
            }
            let payload = unsafe { owner_base.add(offset) as *const ArrayItem };
            LangValue::LowValue(LowValue::Array(AnyHandle::Static(
                StaticHandle {
                    module: owner,
                    offset: std::ptr::slice_from_raw_parts(payload, len),
                },
            )))
        }
        2 => {
            let module = ModuleKey::from_raw(r.u64()?);
            let index = r.u64()? as usize;
            LangValue::LowValue(LowValue::Function(AnyFunctionId::Static(
                StaticFunctionRef {
                    module,
                    index: StaticFunctionId(index),
                },
            )))
        }
        3 => LangValue::LowValue(LowValue::None),
        4 => LangValue::LowValue(LowValue::Parameterized),
        12 => {
            let owner = ModuleKey::from_raw(r.u64()?);
            let offset = r.u64()? as usize;
            let len = r.u64()? as usize;
            let (owner_arena, owner_base) = if owner == self_key {
                (self_arena, self_base)
            } else {
                let module = modules.get(&owner).ok_or_else(|| {
                    format!("artifact references unregistered dependency key {owner:?}")
                })?;
                let arena: &[u8] = &module.arena;
                (arena, arena_base(arena))
            };
            let gap = owner_base as usize - owner_arena.as_ptr() as usize;
            if offset + len > owner_arena.len() - gap {
                return Err("artifact handle out of its arena's bounds".into());
            }
            let payload = unsafe { owner_base.add(offset) as *const TableItem };
            LangValue::LowValue(LowValue::Table(AnyHandle::Static(
                StaticHandle {
                    module: owner,
                    offset: std::ptr::slice_from_raw_parts(payload, len),
                },
            )))
        }
        5 => LangValue::TypeValue(TypeValue::TypeInt),
        6 => LangValue::TypeValue(TypeValue::TypeType),
        7 => LangValue::TypeValue(TypeValue::TypeFunction),
        8 => LangValue::TypeValue(TypeValue::TypeTuple),
        9 => LangValue::TypeValue(TypeValue::TypeArray),
        10 => LangValue::TypeValue(TypeValue::TypeStruct),
        13 => LangValue::TypeValue(TypeValue::TypeTable),
        11 => LangValue::TypeValue(TypeValue::TypeId(r.u64()? as usize)),
        tag => return Err(format!("unknown artifact value tag {tag}")),
    })
}

fn read_operator(r: &mut Reader) -> Result<LangOperator, String> {
    use lichen_lowlevel::LowOperator;
    Ok(match r.u8()? {
        0 => LangOperator::LowOperator(LowOperator::Index),
        1 => LangOperator::LowOperator(LowOperator::Apply),
        3 => LangOperator::TypeOperator(TypeOperator::Fresh),
        4 => LangOperator::TypeOperator(TypeOperator::Add),
        5 => LangOperator::TypeOperator(TypeOperator::Sub),
        6 => LangOperator::TypeOperator(TypeOperator::Leq),
        7 => LangOperator::TypeOperator(TypeOperator::Eq),
        8 => LangOperator::LowOperator(LowOperator::TableGet),
        9 => LangOperator::GcdOp(GcdOp::Gcd),
        tag => return Err(format!("unknown artifact operator tag {tag}")),
    })
}

// ---------------------------------------------------------------------------
// The registry file (`<dir>/registry`)
//
//   magic "LCHREG" | version u32 | next_key u64
//   | free: u64 count + u64*
//   | entries: u64 count + per entry:
//       hash 32B, key u64, source_hash 32B, deps: u64 count
//       + per dep (path_len u32 + path bytes, key u64)
//   | paths: u64 count + per alias (path_len u32 + path bytes, hash 32B)
// ---------------------------------------------------------------------------

/// One registered artifact's record: its device key, the hash of the raw
/// source it was compiled from, and its direct dependencies (path + key).
#[derive(Debug, Clone)]
pub struct Entry {
    pub key: ModuleKey,
    pub source_hash: Hash,
    pub deps: Vec<(PathBuf, ModuleKey)>,
}

/// The result of a successful incremental verification: the artifact's
/// identity and its dependency list, ready for loading.
#[derive(Debug, Clone)]
pub struct Verified {
    pub key: ModuleKey,
    pub hash: Hash,
    pub deps: Vec<(PathBuf, ModuleKey)>,
}

/// The disk shape of the device registry: the key allocator (with its free
/// list), the key → content map, and the path → key alias table.  All
/// mutations go through the cross-process `mkdir` lock and are saved
/// atomically; reads ([`Self::verify`]) lock nothing.
pub struct DeviceRegistry {
    dir: PathBuf,
    next_key: u64,
    free: BTreeSet<u64>,
    entries: HashMap<Hash, Entry>,
    by_key: HashMap<ModuleKey, Hash>,
    paths: HashMap<PathBuf, Hash>,
}

/// The lock's stale threshold: registry mutations are millisecond-scale, so
/// a lock older than this is a crashed holder and is broken.
const LOCK_STALE: Duration = Duration::from_secs(10);
const LOCK_WAIT: Duration = Duration::from_secs(30);

impl DeviceRegistry {
    /// Open (or create) the device store rooted at `dir`.
    pub fn open(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(dir.join("artifacts"));
        let mut registry = DeviceRegistry {
            dir,
            next_key: 0,
            free: BTreeSet::new(),
            entries: HashMap::new(),
            by_key: HashMap::new(),
            paths: HashMap::new(),
        };
        registry.reload();
        registry
    }

    fn registry_path(&self) -> PathBuf {
        self.dir.join("registry")
    }
    fn artifact_path(&self, hash: Hash) -> PathBuf {
        self.dir
            .join("artifacts")
            .join(format!("{}.module", hex(&hash)))
    }

    /// Re-read the registry file, replacing the in-memory state.  A missing
    /// or corrupt file leaves the (empty or last-known) state in place —
    /// the next save repairs it.
    fn reload(&mut self) {
        let Ok(bytes) = std::fs::read(self.registry_path()) else {
            return;
        };
        if let Ok(state) = parse_registry(&bytes) {
            self.by_key = state
                .entries
                .iter()
                .map(|(&hash, entry)| (entry.key, hash))
                .collect();
            self.next_key = state.next_key;
            self.free = state.free;
            self.entries = state.entries;
            self.paths = state.paths;
        }
    }

    /// Atomically write the registry file (temp + rename).
    fn save(&self) {
        let bytes = serialize_registry(self);
        let tmp = self.dir.join("registry.tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, self.registry_path());
        }
    }

    /// Run `f` under the cross-process registry lock: re-read the latest
    /// disk state, mutate, save atomically.  Mutations are millisecond-scale
    /// and never evaluate, so the lock is held briefly.
    fn with_lock<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let guard = RegistryLock::acquire(&self.dir);
        self.reload();
        let result = f(self);
        self.save();
        drop(guard);
        result
    }

    /// The number of registered artifacts (tests).
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
    /// The number of path aliases (tests).
    pub fn path_count(&self) -> usize {
        self.paths.len()
    }
    /// The device's cache directory (tests).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Allocate the device key for `hash`: the existing key when the
    /// artifact is already registered, otherwise a reclaimed free key (or
    /// the next fresh index) with a pending entry written back — the
    /// allocation is visible to every process before the (possibly long)
    /// compile starts.  Returns `(key, is_new)`.
    pub fn alloc(&mut self, hash: Hash) -> (ModuleKey, bool) {
        self.with_lock(|registry| {
            if let Some(entry) = registry.entries.get(&hash) {
                return (entry.key, false);
            }
            let key = match registry.free.pop_first() {
                Some(index) => ModuleKey::from_raw(index),
                None => {
                    let index = registry.next_key;
                    registry.next_key += 1;
                    ModuleKey::from_raw(index)
                }
            };
            registry.entries.insert(
                hash,
                Entry {
                    key,
                    // A pending entry: `source_hash` all-zero can never
                    // verify (a real hash is all-zero with probability
                    // 2^-256), so a crash between allocation and publish
                    // reads as a miss and recompiles.
                    source_hash: [0; 32],
                    deps: Vec::new(),
                },
            );
            registry.by_key.insert(key, hash);
            (key, true)
        })
    }

    /// Complete an artifact's record after its compile: its raw-source hash,
    /// its dependency list, and the path alias.  `key` must match the
    /// pending allocation.
    pub fn publish(
        &mut self,
        key: ModuleKey,
        hash: Hash,
        source_hash: Hash,
        deps: Vec<(PathBuf, ModuleKey)>,
        path: PathBuf,
    ) {
        self.with_lock(|registry| {
            let entry = registry
                .entries
                .get_mut(&hash)
                .expect("publishing an artifact that was never allocated");
            assert_eq!(entry.key, key, "publishing under a mismatched device key");
            entry.source_hash = source_hash;
            entry.deps = deps;
            registry.paths.insert(path, hash);
        });
    }

    /// The file an artifact is stored in.
    pub fn artifact_file(&self, hash: Hash) -> PathBuf {
        self.artifact_path(hash)
    }

    /// Write an artifact file (atomic; the file is content-addressed, so
    /// concurrent writers of the same hash write identical bytes).
    pub fn store_artifact(&mut self, hash: Hash, bytes: &[u8]) {
        let path = self.artifact_path(hash);
        let tmp = self.dir.join("artifacts").join("tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// Load and deserialize an artifact.  `modules` must hold every
    /// dependency the artifact's refs name (they are loaded first).
    pub fn load_artifact(
        &self,
        key: ModuleKey,
        hash: Hash,
        modules: &HashMap<ModuleKey, Arc<StaticModule<LangProgram>>>,
    ) -> Result<(StaticModule<LangProgram>, LocalNodeId), String> {
        let bytes = std::fs::read(self.artifact_path(hash))
            .map_err(|e| format!("cannot read cached artifact: {e}"))?;
        deserialize_artifact(&bytes, key, hash, modules)
    }

    /// Incremental verification: is the package at `path` up to date on the
    /// device?  Walks the *recorded* dependency graph (never parses or
    /// re-hashes transitively): each node compares one source-file hash
    /// against its record and recurses into its recorded dependencies.
    /// Returns the artifact identity when the whole graph verifies.
    pub fn verify(&self, path: &Path) -> Option<Verified> {
        let bytes = std::fs::read(self.registry_path()).ok()?;
        let state = parse_registry(&bytes).ok()?;
        let hash = state.paths.get(path).copied()?;
        verify_entry(&state, path, hash, &mut HashSet::new())?;
        let entry = state.entries.get(&hash)?;
        Some(Verified {
            key: entry.key,
            hash,
            deps: entry.deps.clone(),
        })
    }

    /// Explicit garbage collection: mark the entries reachable from the
    /// roots (path aliases whose source file still exists) through the
    /// dependency graph, then delete every unmarked entry — its key is
    /// reclaimed into the free list and its artifact file removed.  Returns
    /// the number of reclaimed artifacts.
    pub fn gc(&mut self) -> usize {
        self.with_lock(|registry| {
            let mut roots: Vec<Hash> = Vec::new();
            for (path, &hash) in &registry.paths {
                if registry.entries.contains_key(&hash) && path.exists() {
                    roots.push(hash);
                }
            }
            let mut alive: HashSet<Hash> = HashSet::new();
            let mut stack = roots;
            while let Some(hash) = stack.pop() {
                if !alive.insert(hash) {
                    continue;
                }
                let Some(entry) = registry.entries.get(&hash) else {
                    continue;
                };
                for (_, dep_key) in &entry.deps {
                    if let Some(&dep_hash) = registry.by_key.get(dep_key) {
                        stack.push(dep_hash);
                    }
                }
            }
            let dead: Vec<Hash> = registry
                .entries
                .keys()
                .filter(|hash| !alive.contains(*hash))
                .copied()
                .collect();
            let removed = dead.len();
            for hash in dead {
                let entry = registry.entries.remove(&hash).expect("the dead entry");
                registry.by_key.remove(&entry.key);
                registry.free.insert(entry.key.as_raw());
                let _ = std::fs::remove_file(registry.artifact_path(hash));
            }
            let keep: HashSet<PathBuf> = registry
                .paths
                .iter()
                .filter(|(_, hash)| registry.entries.contains_key(*hash))
                .map(|(path, _)| path.clone())
                .collect();
            registry.paths.retain(|path, _| keep.contains(path));
            removed
        })
    }

    /// Explicitly remove the package at `path` (its entry, artifact file,
    /// and key) from the device.  Returns whether anything was removed.
    pub fn remove(&mut self, path: &Path) -> bool {
        self.with_lock(|registry| {
            let Some(hash) = registry.paths.remove(path) else {
                return false;
            };
            let Some(entry_key) = registry.entries.get(&hash).map(|entry| entry.key) else {
                return true;
            };
            let referenced = registry
                .entries
                .values()
                .any(|other| other.key != entry_key && other.deps.iter().any(|(_, k)| *k == entry_key));
            let aliased = registry.paths.values().any(|&h| h == hash);
            if !referenced && !aliased {
                registry.entries.remove(&hash);
                registry.by_key.remove(&entry_key);
                registry.free.insert(entry_key.as_raw());
                let _ = std::fs::remove_file(registry.artifact_path(hash));
            }
            true
        })
    }
}

fn verify_entry(
    state: &RegistryState,
    path: &Path,
    hash: Hash,
    visited: &mut HashSet<Hash>,
) -> Option<()> {
    if !visited.insert(hash) {
        return Some(()); // already checked along this walk
    }
    let entry = state.entries.get(&hash)?;
    let raw = std::fs::read(path).ok()?;
    if sha256(&raw) != entry.source_hash {
        return None;
    }
    for (dep_path, dep_key) in &entry.deps {
        let dep_hash = state.paths.get(dep_path).copied()?;
        let dep_entry = state.entries.get(&dep_hash)?;
        if dep_entry.key != *dep_key {
            return None;
        }
        verify_entry(state, dep_path, dep_hash, visited)?;
    }
    Some(())
}

/// The cross-process registry lock: an exclusive directory (`<dir>/lock`),
/// created atomically, removed on release.  A lock whose mtime is older
/// than [`LOCK_STALE`] is a crashed holder and is broken; a wait longer
/// than [`LOCK_WAIT`] panics rather than hanging a compile.
struct RegistryLock(PathBuf);

impl RegistryLock {
    fn acquire(dir: &Path) -> RegistryLock {
        let lock_dir = dir.join("lock");
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match std::fs::create_dir(&lock_dir) {
                Ok(()) => return RegistryLock(lock_dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&lock_dir)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > LOCK_STALE);
                    if stale {
                        let _ = std::fs::remove_dir(&lock_dir);
                        continue;
                    }
                    if Instant::now() > deadline {
                        panic!(
                            "timed out waiting for the device registry lock at {}",
                            lock_dir.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// The device's cache directory: `$LICHEN_HOME` when set, otherwise
/// `~/.lichen`.
pub fn lichendir() -> PathBuf {
    if let Some(home) = std::env::var_os("LICHEN_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home {
        Some(home) => PathBuf::from(home).join(".lichen"),
        None => PathBuf::from(".lichen"),
    }
}

/// The hex encoding of a hash — the artifact file name.
pub fn hex(hash: &Hash) -> String {
    let mut out = String::with_capacity(64);
    for byte in hash {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

// ---------------------------------------------------------------------------
// Registry file serialization
// ---------------------------------------------------------------------------

struct RegistryState {
    next_key: u64,
    free: BTreeSet<u64>,
    entries: HashMap<Hash, Entry>,
    paths: HashMap<PathBuf, Hash>,
}

fn serialize_registry(registry: &DeviceRegistry) -> Vec<u8> {
    let mut w = Writer::new();
    w.bytes(b"LCHREG");
    w.u32(1);
    w.u64(registry.next_key);
    w.u64(registry.free.len() as u64);
    for &index in &registry.free {
        w.u64(index);
    }
    w.u64(registry.entries.len() as u64);
    for (hash, entry) in &registry.entries {
        w.bytes(hash);
        w.u64(entry.key.as_raw());
        w.bytes(&entry.source_hash);
        w.u64(entry.deps.len() as u64);
        for (path, key) in &entry.deps {
            w.path(path);
            w.u64(key.as_raw());
        }
    }
    w.u64(registry.paths.len() as u64);
    for (path, hash) in &registry.paths {
        w.path(path);
        w.bytes(hash);
    }
    w.buf
}

fn parse_registry(bytes: &[u8]) -> Result<RegistryState, String> {
    let mut r = Reader::new(bytes);
    if r.take(6)? != b"LCHREG" {
        return Err("bad registry magic".into());
    }
    if r.u32()? != 1 {
        return Err("unknown registry format version".into());
    }
    let next_key = r.u64()?;
    let mut free = BTreeSet::new();
    for _ in 0..r.u64()? {
        free.insert(r.u64()?);
    }
    let mut entries = HashMap::new();
    for _ in 0..r.u64()? {
        let hash: Hash = r.take(32)?.try_into().expect("32 bytes");
        let key = ModuleKey::from_raw(r.u64()?);
        let source_hash: Hash = r.take(32)?.try_into().expect("32 bytes");
        let mut deps = Vec::new();
        for _ in 0..r.u64()? {
            let path = r.path()?;
            let key = ModuleKey::from_raw(r.u64()?);
            deps.push((path, key));
        }
        entries.insert(
            hash,
            Entry {
                key,
                source_hash,
                deps,
            },
        );
    }
    let mut paths = HashMap::new();
    for _ in 0..r.u64()? {
        let path = r.path()?;
        let hash: Hash = r.take(32)?.try_into().expect("32 bytes");
        paths.insert(path, hash);
    }
    if !r.done() {
        return Err("trailing bytes after the registry".into());
    }
    Ok(RegistryState {
        next_key,
        free,
        entries,
        paths,
    })
}

// ---------------------------------------------------------------------------
// Reader / writer
// ---------------------------------------------------------------------------

pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer { buf: Vec::new() }
    }
    pub fn u8(&mut self, value: u8) {
        self.buf.push(value);
    }
    pub fn u32(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }
    pub fn u64(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }
    pub fn path(&mut self, path: &Path) {
        let bytes = path.to_string_lossy();
        self.u32(bytes.len() as u32);
        self.buf.extend_from_slice(bytes.as_bytes());
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }
    pub fn u8(&mut self) -> Result<u8, String> {
        let byte = *self.buf.get(self.pos).ok_or("truncated artifact")?;
        self.pos += 1;
        Ok(byte)
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }
    pub fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        if self.pos + len > self.buf.len() {
            return Err("truncated artifact".into());
        }
        let bytes = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok(bytes)
    }
    pub fn path(&mut self) -> Result<PathBuf, String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        Ok(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
    }
    pub fn done(&self) -> bool {
        self.pos == self.buf.len()
    }
}
