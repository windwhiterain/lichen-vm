//! The disk device registry: the cross-process compiled-artifact store.
//! See docs/notes/artifact-cache.md.
//!
//! # Invariant
//!
//! Type-independent: the module never names a program value or operator — the vocabulary enters
//! only when a caller deserializes the bytes it returns. Artifacts are file-ID keyed and
//! overwritten on recompile, so a file keeps one slot.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::codec::{Reader, Writer};
use crate::module_key::ModuleKey;
use crate::{Hash, hex, sha256};
use sha2::Digest as _;

/// The artifact identity of a compiled package: its source hash folded with each
/// direct dependency's `(key, identity)`.
///
/// # Invariant
///
/// The dependency's *identity*, not its key, is what makes this transitive: a recompile reuses
/// the key, so folding keys alone would serve a frozen artifact whose cross-module references
/// name the dependency's old node layout.
pub fn artifact_hash(source: Hash, deps: &[(ModuleKey, Hash)]) -> Hash {
    use sha2::Sha256;
    let mut hasher = Sha256::new();
    hasher.update(source);
    for (key, identity) in deps {
        hasher.update(key.as_raw().to_le_bytes());
        hasher.update(identity);
    }
    hasher.finalize().into()
}

/// The identity of a dependency this store has no record of: all zeros, which no
/// real identity equals.
const UNKNOWN_IDENTITY: Hash = [0; 32];

/// The stable cache key of a file ID: SHA-256 over the identity string, used as
/// the artifact file name.
pub fn file_id_hash(file_id: &str) -> Hash {
    sha256(file_id.as_bytes())
}

/// The prefix of an embedded lichen source's file ID: `virtual:<name>` names a unit
/// with no place on disk.
const VIRTUAL_PREFIX: &str = "virtual:";

/// The file ID of an embedded lichen source: the identity the device files it
/// under and a dependent records.
pub fn virtual_file_id(name: &str) -> String {
    format!("{VIRTUAL_PREFIX}{name}")
}

/// Whether a file ID names an embedded lichen source (`virtual:<name>`) rather
/// than a file on disk.
pub fn is_virtual_file_id(file_id: &str) -> bool {
    file_id.starts_with(VIRTUAL_PREFIX)
}

/// The name of the embedded source a `virtual:` file ID names; `None` for an
/// on-disk file ID.
pub fn virtual_name(file_id: &str) -> Option<&str> {
    file_id.strip_prefix(VIRTUAL_PREFIX)
}

/// Whether a file ID names a source the cache keeps: a `.lichen` path or a
/// `virtual:` embedded source.
pub fn is_lichen_file_id(file_id: &str) -> bool {
    file_id.ends_with(".lichen") || is_virtual_file_id(file_id)
}

/// One registered artifact's record: its key, source hash, published identity, and
/// direct dependencies.
#[derive(Debug, Clone)]
pub struct Entry {
    pub key: ModuleKey,
    pub source_hash: Hash,
    /// This artifact's identity as published: its source hash folded with the
    /// identities its dependencies had then.
    ///
    /// # Invariant
    ///
    /// A dependency republished since changes the recomputed identity, which is how a stale frozen
    /// artifact is rejected.
    pub artifact: Hash,
    pub deps: Vec<(String, ModuleKey)>,
}

/// The result of a successful verification: the artifact's identity and its
/// dependency list.
#[derive(Debug, Clone)]
pub struct Verified {
    pub key: ModuleKey,
    pub hash: Hash,
    pub deps: Vec<(String, ModuleKey)>,
}

/// The disk store: the key allocator and the file-ID → entry table.
/// See docs/notes/artifact-cache.md.
///
/// # Invariant
///
/// Index mutations (`alloc`, `publish`, `gc`, `remove`) hold the cross-process `mkdir` lock and
/// save by rename; an artifact payload is written without it, atomic by a unique temp name plus a
/// rename; reads lock nothing.
pub struct DeviceRegistry {
    dir: PathBuf,
    next_key: u64,
    free: BTreeSet<u64>,
    entries: HashMap<String, Entry>,
    by_key: HashMap<ModuleKey, String>,
    /// Whether this process already recovered from an unreadable registry file: a
    /// repeat is the same corruption.
    registry_recovered: bool,
}

/// The lock's stale threshold: mutations are millisecond-scale, so an older lock is
/// a crashed holder.
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
            registry_recovered: false,
        };
        registry.reload();
        registry
    }

    fn registry_path(&self) -> PathBuf {
        self.dir.join("registry")
    }
    /// The artifact file for a file ID: `artifacts/<sha256(file_id)>.module`, stable
    /// per file ID.
    fn artifact_path(&self, file_id: &str) -> PathBuf {
        self.dir
            .join("artifacts")
            .join(format!("{}.module", hex(&file_id_hash(file_id))))
    }

    /// Re-read the registry file, replacing the in-memory state: a missing file is a
    /// fresh store, an unreadable one recovers.
    fn reload(&mut self) {
        let Ok(bytes) = std::fs::read(self.registry_path()) else {
            return;
        };
        match parse_registry(&bytes) {
            Ok(state) => {
                self.registry_recovered = false;
                self.by_key = state
                    .entries
                    .iter()
                    .map(|(file_id, entry)| (entry.key, file_id.clone()))
                    .collect();
                self.next_key = state.next_key;
                self.free = state.free;
                self.entries = state.entries;
            }
            // Already recovered here: the file is still unreadable, the same
            // corruption, so the restarted state stands.
            Err(_) if self.registry_recovered => {}
            Err(reason) => self.recover(&reason),
        }
    }

    /// Recover an unreadable registry: preserve it and its artifacts, then start a
    /// fresh, empty entry table.
    ///
    /// # Invariant
    ///
    /// The key space restarts only once no artifact on disk carries an older key: a `ModuleKey` is
    /// a recycled index embedded in artifact bytes, and the registry file was the only map from
    /// file ID to key, so the artifacts are moved aside with it. `next_key` is kept and the free
    /// list dropped, so no key this process handed out is reissued. Nothing is deleted.
    fn recover(&mut self, reason: &str) {
        let registry = quarantine(&self.registry_path());
        let artifacts = quarantine(&self.dir.join("artifacts"));
        let _ = std::fs::create_dir_all(self.dir.join("artifacts"));
        self.free = BTreeSet::new();
        self.entries = HashMap::new();
        self.by_key = HashMap::new();
        self.registry_recovered = true;
        eprintln!(
            "the device registry at {} is unreadable ({reason}); preserved it at {} and the \
             artifacts it described at {}; every artifact recompiles",
            self.registry_path().display(),
            describe(registry),
            describe(artifacts),
        );
    }

    /// Write the registry file, a fixed temp name plus a rename; the registry lock
    /// makes the fixed name safe.
    fn save(&self) {
        let bytes = serialize_registry(self);
        let tmp = self.dir.join("registry.tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, self.registry_path());
        }
    }

    /// Run `f` under the cross-process registry lock: re-read the latest disk state,
    /// mutate, save.
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

    /// Allocate the device key for a file ID, writing a pending entry back so every
    /// process sees it before the compile.
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
                    // A pending entry: an all-zero `source_hash` never verifies, so a
                    // crash before publish is a miss; `artifact` stays zero.
                    source_hash: [0; 32],
                    artifact: UNKNOWN_IDENTITY,
                    deps: Vec::new(),
                },
            );
            registry.by_key.insert(key, file_id.to_string());
            (key, true)
        })
    }

    /// The registry's current on-disk state, read fresh: a write side must agree
    /// with `verify`, which reads from here.
    fn state(&self) -> Option<RegistryState> {
        let bytes = std::fs::read(self.registry_path()).ok()?;
        parse_registry(&bytes).ok()
    }

    /// The identity `file_id`'s artifact will have when published, from a source
    /// hashing to `source_hash`.
    ///
    /// # Invariant
    ///
    /// A caller needs it before publishing, since the frozen artifact carries the identity in its
    /// header and the load rejects a mismatch. It is answered from the registry's own record, not
    /// the caller's view: a `virtual:<name>` dependency need have no record and contributes the
    /// all-zero sentinel on both sides, or every dependent would miss on every run.
    pub fn artifact_identity(&self, source_hash: Hash, deps: &[(String, ModuleKey)]) -> Hash {
        // An unreadable state is a state with no records, which is what `reload`
        // recovers to, so this agrees with `publish`.
        let empty = HashMap::new();
        let entries = &self
            .state()
            .map_or_else(|| empty.clone(), |state| state.entries);
        artifact_hash(source_hash, &dep_identities(entries, deps))
    }

    /// Complete an artifact's record after its compile: its source hash and its
    /// dependency list.
    ///
    /// # Invariant
    ///
    /// `key` matches the pending allocation; the identity is folded from the dependencies'
    /// identities as they stand now, which is the same fold the importer computed.
    pub fn publish(
        &mut self,
        file_id: &str,
        key: ModuleKey,
        source_hash: Hash,
        deps: Vec<(String, ModuleKey)>,
    ) {
        self.with_lock(|registry| {
            let identities = dep_identities(&registry.entries, &deps);
            let entry = registry
                .entries
                .get_mut(file_id)
                .expect("publishing an artifact that was never allocated");
            assert_eq!(entry.key, key, "publishing under a mismatched device key");
            entry.source_hash = source_hash;
            entry.artifact = artifact_hash(source_hash, &identities);
            entry.deps = deps;
        });
    }

    /// The artifact file a file ID is stored in.
    pub fn artifact_file(&self, file_id: &str) -> PathBuf {
        self.artifact_path(file_id)
    }

    /// Write an artifact file, overwriting the file ID's slot.
    ///
    /// # Invariant
    ///
    /// Deliberately not under the registry lock (a payload is far larger than the index): a unique
    /// temp name plus a rename gives the same atomicity, and `fsync` before the rename closes the
    /// crash window. Every failure is swallowed — the cache degrades to a miss — so a failed store
    /// leaves the previous artifact untouched.
    pub fn store_artifact(&mut self, file_id: &str, bytes: &[u8]) {
        let path = self.artifact_path(file_id);
        let nonce = ARTIFACT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp.{}.{nonce}", std::process::id()));
        if write_synced(&tmp, bytes).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Is the artifact for `file_id` up to date against `source`? Walks the recorded
    /// dependency graph, one hash per node.
    ///
    /// # Invariant
    ///
    /// The returned identity is recomputed from the graph as it stands now, never read from the
    /// record: a dependency republished since folds in its new identity, the answer stops matching
    /// the frozen header, and the load rejects it.
    pub fn verify(&self, file_id: &str, source: &[u8]) -> Option<Verified> {
        let state = self.state()?;
        let entry = state.entries.get(file_id)?;
        if sha256(source) != entry.source_hash {
            return None;
        }
        verify_entry(&state, file_id, source, &mut HashSet::new())?;
        let identities = dep_identities(&state.entries, &entry.deps);
        Some(Verified {
            key: entry.key,
            hash: artifact_hash(entry.source_hash, &identities),
            deps: entry.deps.clone(),
        })
    }

    /// Clean the cache: remove every artifact whose file ID is neither a `.lichen`
    /// path nor a `virtual:` one.
    ///
    /// # Invariant
    ///
    /// A dead entry whose key a surviving entry still names is kept: reclaiming that key would hand
    /// it to a different module while an artifact on disk still refers to it.
    pub fn gc(&mut self) -> usize {
        self.with_lock(|registry| {
            let dead: Vec<String> = registry
                .entries
                .keys()
                .filter(|file_id| !is_lichen_file_id(file_id))
                .cloned()
                .collect();
            let mut removed = 0;
            for file_id in dead {
                let Some(entry) = registry.entries.get(&file_id) else {
                    continue;
                };
                let key = entry.key;
                let referenced = registry.entries.iter().any(|(other, other_entry)| {
                    other != &file_id && other_entry.deps.iter().any(|(_, dep)| *dep == key)
                });
                if referenced {
                    continue;
                }
                registry.entries.remove(&file_id);
                registry.by_key.remove(&key);
                registry.free.insert(key.as_raw());
                let _ = std::fs::remove_file(registry.artifact_path(&file_id));
                removed += 1;
            }
            removed
        })
    }

    /// Remove `file_id`'s artifact (its entry, file, and key), unless another
    /// artifact depends on its key.
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

/// A per-process counter making artifact temp names unique within one process;
/// the pid separates processes.
static ARTIFACT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` and flush them to disk, so a crash cannot leave a
/// truncated file behind the following rename.
fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Move `path` aside to the first free `<name>.corrupt[.<n>]` sibling; an earlier
/// quarantine is never overwritten.
fn quarantine(path: &Path) -> Option<PathBuf> {
    if !path.exists() {
        return None;
    }
    let parent = path.parent()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let mut candidate = parent.join(format!("{name}.corrupt"));
    let mut suffix = 0;
    while candidate.exists() {
        suffix += 1;
        candidate = parent.join(format!("{name}.corrupt.{suffix}"));
    }
    std::fs::rename(path, &candidate).ok().map(|()| candidate)
}

/// The identity each recorded dependency contributes to an importer's fold.
///
/// # Invariant
///
/// This is the one answer, shared by `artifact_identity`, `publish` and `verify`; a dependency
/// with no record contributes `UNKNOWN_IDENTITY`, and a mismatch between the sites would make the
/// importer recompile on every run.
fn dep_identities(
    entries: &HashMap<String, Entry>,
    deps: &[(String, ModuleKey)],
) -> Vec<(ModuleKey, Hash)> {
    deps.iter()
        .map(|(dep_file_id, dep_key)| {
            (
                *dep_key,
                entries
                    .get(dep_file_id)
                    .map_or(UNKNOWN_IDENTITY, |dep| dep.artifact),
            )
        })
        .collect()
}

/// Where a quarantine put its bytes — or that there were none to put.
fn describe(preserved: Option<PathBuf>) -> String {
    match preserved {
        Some(path) => path.display().to_string(),
        None => "nothing to move".to_string(),
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
        if is_virtual_file_id(dep_file_id) {
            // An embedded source is compiled into the binary and the store is
            // scoped per plugin set, so its bytes cannot change.
            continue;
        }
        let dep_raw = std::fs::read(dep_file_id).ok()?;
        verify_entry(state, dep_file_id, &dep_raw, visited)?;
    }
    Some(())
}

/// The cross-process registry lock: an exclusive `<dir>/lock` directory, created
/// atomically and removed on release.
///
/// # Invariant
///
/// A lock older than `LOCK_STALE` is a crashed holder and is broken; a wait past `LOCK_WAIT`
/// panics rather than hanging a compile.
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

// Registry file serialization.

struct RegistryState {
    next_key: u64,
    free: BTreeSet<u64>,
    entries: HashMap<String, Entry>,
}

fn serialize_registry(registry: &DeviceRegistry) -> Vec<u8> {
    let mut w = Writer::new();
    w.bytes(b"LCHREG");
    w.u32(3);
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
        w.bytes(&entry.artifact);
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
    // A version-2 registry recorded no artifact identity, so it reads as unreadable
    // and recompiles.
    if r.u32()? != 3 {
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
        let artifact: Hash = r.take(32)?.try_into().expect("32 bytes");
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
                artifact,
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
