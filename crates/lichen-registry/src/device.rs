//! The disk device registry: the content-addressed compiled-artifact cache.
//!
//! A **file ID** is a compiled unit's identity: an on-disk file's canonical
//! path, or `virtual:<name>` for an embedded source.  Artifacts are stored at
//! `artifacts/<sha256(file_id)>.module` and **overwritten** when the file is
//! recompiled, so a frequently modified file keeps exactly one cache slot.
//!
//! This module is **type-independent**: it never names a program value or
//! operator.  The vocabulary only enters when a caller loads an artifact's
//! bytes and deserializes them with a codec (see
//! `lichen-language::persist::load_artifact`, which reads the path from
//! [`DeviceRegistry::artifact_file`]).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::codec::{Reader, Writer};
use crate::module_key::ModuleKey;
use crate::{Hash, hex, sha256};
use sha2::Digest as _;

/// The artifact hash of a compiled package: the raw source bytes followed by
/// its direct dependency keys in source order.  The hash is transitive — a
/// dependency change changes the importer's hash — and deterministic, so every
/// process computes the same hash for the same source chain.
pub fn artifact_hash(source: &[u8], dep_keys: &[ModuleKey]) -> Hash {
    use sha2::Sha256;
    let mut hasher = Sha256::new();
    hasher.update(source);
    for &key in dep_keys {
        hasher.update(key.as_raw().to_le_bytes());
    }
    hasher.finalize().into()
}

/// The stable cache key of a file ID: SHA-256 over the identity string.  Used
/// as the artifact file name, so the same file ID always occupies the same
/// cache slot — recompiling a modified file overwrites it.
pub fn file_id_hash(file_id: &str) -> Hash {
    sha256(file_id.as_bytes())
}

/// Whether a file ID names a lichen source the cache should keep: an on-disk
/// `.lichen` file path, or a `virtual:` embedded lichen source.
pub fn is_lichen_file_id(file_id: &str) -> bool {
    file_id.ends_with(".lichen") || file_id.starts_with("virtual:")
}

/// One registered artifact's record: its device key, the hash of the raw
/// source it was compiled from, and its direct dependencies (file ID + key).
#[derive(Debug, Clone)]
pub struct Entry {
    pub key: ModuleKey,
    pub source_hash: Hash,
    pub deps: Vec<(String, ModuleKey)>,
}

/// The result of a successful incremental verification: the artifact's
/// identity and its dependency list, ready for loading.
#[derive(Debug, Clone)]
pub struct Verified {
    pub key: ModuleKey,
    pub hash: Hash,
    pub deps: Vec<(String, ModuleKey)>,
}

/// The disk shape of the device registry: the key allocator (with its free
/// list) and the file-ID → entry table.  A **file ID** is a compiled unit's
/// identity: an on-disk file's canonical path, or `virtual:<name>` for an
/// embedded source.  Artifacts are stored at `artifacts/<sha256(file_id)>.module`
/// and **overwritten** when the file is recompiled, so a frequently modified
/// file keeps exactly one cache slot.  All mutations go through the
/// cross-process `mkdir` lock and are saved atomically; reads ([`Self::verify`])
/// lock nothing.
pub struct DeviceRegistry {
    dir: PathBuf,
    next_key: u64,
    free: BTreeSet<u64>,
    entries: HashMap<String, Entry>,
    by_key: HashMap<ModuleKey, String>,
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
        };
        registry.reload();
        registry
    }

    fn registry_path(&self) -> PathBuf {
        self.dir.join("registry")
    }
    /// The artifact file for a file ID: `artifacts/<sha256(file_id)>.module` —
    /// stable per file ID, so recompiling a file overwrites its slot.
    fn artifact_path(&self, file_id: &str) -> PathBuf {
        self.dir
            .join("artifacts")
            .join(format!("{}.module", hex(&file_id_hash(file_id))))
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
                .map(|(file_id, entry)| (entry.key, file_id.clone()))
                .collect();
            self.next_key = state.next_key;
            self.free = state.free;
            self.entries = state.entries;
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
    /// The device's cache directory (tests).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Allocate the device key for a file ID: the existing key when the file
    /// is already registered (recompiles reuse it, overwriting the slot),
    /// otherwise a reclaimed free key (or the next fresh index) with a pending
    /// entry written back — visible to every process before the (possibly
    /// long) compile starts.  Returns `(key, is_new)`.
    pub fn alloc(&mut self, file_id: &str) -> (ModuleKey, bool) {
        self.with_lock(|registry| {
            if let Some(entry) = registry.entries.get(file_id) {
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
                file_id.to_string(),
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
            registry.by_key.insert(key, file_id.to_string());
            (key, true)
        })
    }

    /// Complete an artifact's record after its compile: its raw-source hash
    /// and its dependency list (file ID + key).  `key` must match the pending
    /// allocation.
    pub fn publish(
        &mut self,
        file_id: &str,
        key: ModuleKey,
        source_hash: Hash,
        deps: Vec<(String, ModuleKey)>,
    ) {
        self.with_lock(|registry| {
            let entry = registry
                .entries
                .get_mut(file_id)
                .expect("publishing an artifact that was never allocated");
            assert_eq!(entry.key, key, "publishing under a mismatched device key");
            entry.source_hash = source_hash;
            entry.deps = deps;
        });
    }

    /// The artifact file a file ID is stored in.
    pub fn artifact_file(&self, file_id: &str) -> PathBuf {
        self.artifact_path(file_id)
    }

    /// Write an artifact file (atomic, overwriting the file ID's slot).
    pub fn store_artifact(&mut self, file_id: &str, bytes: &[u8]) {
        let path = self.artifact_path(file_id);
        let tmp = self.dir.join("artifacts").join("tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// Incremental verification: is the artifact for `file_id` up to date
    /// against its current source?  Walks the *recorded* dependency graph
    /// (never parses or re-hashes transitively beyond one source hash per
    /// node): each node compares one source-file hash against its record and
    /// recurses into its recorded dependencies.  Returns the artifact identity
    /// when the whole graph verifies.
    pub fn verify(&self, file_id: &str, source: &[u8]) -> Option<Verified> {
        let bytes = std::fs::read(self.registry_path()).ok()?;
        let state = parse_registry(&bytes).ok()?;
        let entry = state.entries.get(file_id)?;
        if sha256(source) != entry.source_hash {
            return None;
        }
        verify_entry(&state, file_id, source, &mut HashSet::new())?;
        let dep_keys: Vec<ModuleKey> = entry.deps.iter().map(|(_, key)| *key).collect();
        Some(Verified {
            key: entry.key,
            hash: artifact_hash(source, &dep_keys),
            deps: entry.deps.clone(),
        })
    }

    /// Clean the device cache: remove every artifact whose file ID is **not**
    /// a lichen file path (`.lichen`) and **not** a virtual lichen-file path
    /// (`virtual:`) — i.e. keep exactly the on-disk and embedded lichen
    /// sources, and prune anything else.  The kept artifacts stay keyed by
    /// their file ID (a `.lichen` source is kept even when its slot is
    /// overwritten by a recompile).  Returns the number of removed artifacts.
    pub fn gc(&mut self) -> usize {
        self.with_lock(|registry| {
            let dead: Vec<String> = registry
                .entries
                .keys()
                .filter(|file_id| !is_lichen_file_id(file_id))
                .cloned()
                .collect();
            let removed = dead.len();
            for file_id in dead {
                let entry = registry.entries.remove(&file_id).expect("the dead entry");
                registry.by_key.remove(&entry.key);
                registry.free.insert(entry.key.as_raw());
                let _ = std::fs::remove_file(registry.artifact_path(&file_id));
            }
            removed
        })
    }

    /// Explicitly remove the artifact for `file_id` (its entry, artifact file,
    /// and key) from the device — keeping it when another artifact depends on
    /// its key.  Returns whether anything was removed.
    pub fn remove(&mut self, file_id: &str) -> bool {
        self.with_lock(|registry| {
            let Some(entry_key) = registry.entries.get(file_id).map(|e| e.key) else {
                return false;
            };
            let referenced = registry.entries.iter().any(|(other, other_entry)| {
                other != file_id && other_entry.deps.iter().any(|(_, key)| *key == entry_key)
            });
            if !referenced {
                registry.entries.remove(file_id);
                registry.by_key.remove(&entry_key);
                registry.free.insert(entry_key.as_raw());
                let _ = std::fs::remove_file(registry.artifact_path(file_id));
            }
            true
        })
    }
}

fn verify_entry(
    state: &RegistryState,
    file_id: &str,
    source: &[u8],
    visited: &mut HashSet<String>,
) -> Option<()> {
    if !visited.insert(file_id.to_string()) {
        return Some(()); // already checked along this walk
    }
    let entry = state.entries.get(file_id)?;
    if sha256(source) != entry.source_hash {
        return None;
    }
    for (dep_file_id, dep_key) in &entry.deps {
        let dep_entry = state.entries.get(dep_file_id)?;
        if dep_entry.key != *dep_key {
            return None;
        }
        let dep_raw = std::fs::read(dep_file_id).ok()?;
        verify_entry(state, dep_file_id, &dep_raw, visited)?;
    }
    Some(())
}

/// The cross-process registry lock: an exclusive directory (`<dir>/lock`),
/// created atomically, removed on release.  A lock whose mtime is older than
/// [`LOCK_STALE`] is a crashed holder and is broken; a wait longer than
/// [`LOCK_WAIT`] panics rather than hanging a compile.
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

// ---------------------------------------------------------------------------
// Registry file serialization
// ---------------------------------------------------------------------------

struct RegistryState {
    next_key: u64,
    free: BTreeSet<u64>,
    entries: HashMap<String, Entry>,
}

fn serialize_registry(registry: &DeviceRegistry) -> Vec<u8> {
    let mut w = Writer::new();
    w.bytes(b"LCHREG");
    w.u32(2);
    w.u64(registry.next_key);
    w.u64(registry.free.len() as u64);
    for &index in &registry.free {
        w.u64(index);
    }
    w.u64(registry.entries.len() as u64);
    for (file_id, entry) in &registry.entries {
        w.path(Path::new(file_id));
        w.u64(entry.key.as_raw());
        w.bytes(&entry.source_hash);
        w.u64(entry.deps.len() as u64);
        for (dep_file_id, dep_key) in &entry.deps {
            w.path(Path::new(dep_file_id));
            w.u64(dep_key.as_raw());
        }
    }
    w.into_bytes()
}

fn parse_registry(bytes: &[u8]) -> Result<RegistryState, String> {
    let mut r = Reader::new(bytes);
    if r.take(6)? != b"LCHREG" {
        return Err("bad registry magic".into());
    }
    if r.u32()? != 2 {
        return Err("unknown registry format version".into());
    }
    let next_key = r.u64()?;
    let mut free = BTreeSet::new();
    for _ in 0..r.u64()? {
        free.insert(r.u64()?);
    }
    let mut entries = HashMap::new();
    for _ in 0..r.u64()? {
        let file_id = r.path()?.to_string_lossy().into_owned();
        let key = ModuleKey::from_raw(r.u64()?);
        let source_hash: Hash = r.take(32)?.try_into().expect("32 bytes");
        let mut deps = Vec::new();
        for _ in 0..r.u64()? {
            let dep_file_id = r.path()?.to_string_lossy().into_owned();
            let dep_key = ModuleKey::from_raw(r.u64()?);
            deps.push((dep_file_id, dep_key));
        }
        entries.insert(
            file_id,
            Entry {
                key,
                source_hash,
                deps,
            },
        );
    }
    if !r.done() {
        return Err("trailing bytes after the registry".into());
    }
    Ok(RegistryState {
        next_key,
        free,
        entries,
    })
}
