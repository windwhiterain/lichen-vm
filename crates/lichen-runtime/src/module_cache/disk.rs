//! The kernel disk cache: one file per launch set, under a compiler slot.
//!
//! # Invariant
//! A file is named by the launch set's **content** identity — every fragment it
//! reaches, in visit order — and not by the root [`KernelId`], because a module is a
//! function of that set alone: the binding facts and the device a run answers on are
//! absent from it because the emitter reads neither, and naming them would split two
//! identical modules into two files.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use lichen_kernel_ir::KernelId;
use lichen_utils::hash::{Hash, hex, sha256};

use crate::kernel_registry::launch_set_digest;

/// The content hash a payload carries ahead of its bytes.
const DIGEST_LEN: usize = 32;

/// The file the launch set `key` assembles to lives in.
fn module_path(dir: &Path, key: &Hash) -> PathBuf {
    dir.join(format!("{}.wasm", hex(key)))
}

/// The name a write lands in before it is renamed over [`module_path`].
fn temporary_path(dir: &Path, key: &Hash) -> PathBuf {
    dir.join(format!("{}.wasm.tmp", hex(key)))
}

/// `root`'s module, read out of the disk cache and validated, or `None` for a miss.
///
/// # Invariant
/// The payload is the module's own content hash ahead of its bytes, and both that hash
/// and `wasmi::Module::new` must accept the file: the directory is a writable shared
/// space, so a file either check refuses is a miss rather than a module that might be
/// somebody else's.
pub fn load(root: KernelId) -> Option<(wasmi::Engine, wasmi::Module)> {
    let dir = super::kernel_disk_cache()?;
    let key = launch_set_digest(root)?;
    let payload = std::fs::read(module_path(dir, &key)).ok()?;
    let (digest, bytes) = payload.split_at_checked(DIGEST_LEN)?;
    if sha256(bytes) != digest {
        return None;
    }
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, bytes).ok()?;
    super::MODULE_CACHE_DISK_HITS.fetch_add(1, Ordering::Relaxed);
    Some((engine, module))
}

/// Write `bytes` as the module for the launch set rooted at `root`, for a later
/// process to read instead of assembling.
///
/// # Invariant
/// The payload lands in a temporary name and is renamed over the final one, so a
/// concurrent reader sees a whole file or none of it; every failure here leaves the
/// caller assembling as if no cache existed, and a directory that is not there yet is
/// created on the first write.
pub fn store(root: KernelId, bytes: &[u8]) {
    let Some(dir) = super::kernel_disk_cache() else {
        return;
    };
    let Some(key) = launch_set_digest(root) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut payload = Vec::with_capacity(DIGEST_LEN + bytes.len());
    payload.extend_from_slice(&sha256(bytes));
    payload.extend_from_slice(bytes);
    let temporary = temporary_path(dir, &key);
    if std::fs::write(&temporary, &payload).is_err() {
        return;
    }
    if std::fs::rename(&temporary, module_path(dir, &key)).is_err() {
        return;
    }
    super::MODULE_CACHE_DISK_WRITES.fetch_add(1, Ordering::Relaxed);
}
