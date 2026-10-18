//! The emitted-module cache: one assembled, validated wasm module per kernel id,
//! in memory and on disk.
//!
//! # Invariant
//! The only id in this file is a [`lichen_kernel_ir::KernelId`], so nothing here is a
//! `lichen_lowlevel::Module` or needs the language: a backend that runs an artifact
//! asks this cache and hands back an engine and a module. The bytes come from the
//! caller as a closure, so the assembly — which does read the process kernel
//! registry — stays on the language side while the cache does not.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "wasm")]
mod disk;

/// Compiled modules this process assembled rather than served from either cache.
///
/// # Invariant
/// Test- and measurement-visible on purpose: the value of content-addressed ids
/// (`D15`) is that this stops growing when the same function is compiled again, and a
/// counter is the only way to see that without timing a compile. It counts
/// **assembles**, so a module the disk supplied is not one — this answers what this
/// process built, beside the two that answer what the disk did.
static MODULE_CACHE_MISSES: AtomicUsize = AtomicUsize::new(0);

/// Assembled modules this process read out of the kernel disk cache.
///
/// # Invariant
/// The restart's worth, counted rather than timed: a hit is a module that a process
/// starting now would otherwise assemble from its fragment again, and it is the only
/// reading that says a restart got cheaper.
static MODULE_CACHE_DISK_HITS: AtomicUsize = AtomicUsize::new(0);

/// Assembled modules this process wrote into the kernel disk cache.
///
/// # Invariant
/// Files that landed, not writes that were attempted: a rename is the last step
/// [`disk::store`] takes, so this is what a later process can actually read back.
static MODULE_CACHE_DISK_WRITES: AtomicUsize = AtomicUsize::new(0);

/// See [`MODULE_CACHE_MISSES`].
pub fn module_cache_misses() -> usize {
    MODULE_CACHE_MISSES.load(Ordering::Relaxed)
}

/// See [`MODULE_CACHE_DISK_HITS`].
pub fn module_cache_disk_hits() -> usize {
    MODULE_CACHE_DISK_HITS.load(Ordering::Relaxed)
}

/// See [`MODULE_CACHE_DISK_WRITES`].
pub fn module_cache_disk_writes() -> usize {
    MODULE_CACHE_DISK_WRITES.load(Ordering::Relaxed)
}

/// The directory under a device root that holds the assembled kernels.
///
/// # Invariant
/// A sibling of the device's own `artifacts`, because the two hold different things:
/// an artifact is a compiled package keyed by its source file ID, a kernel is an
/// assembled launch set keyed by the content digest of every fragment in it.
const KERNEL_CACHE_DIR: &str = "kernels";

/// The directory `device_root`'s compiler slot assembles its kernels under.
///
/// # Invariant
/// The last component is `compiler_slot_key(core_repo, plugins)`, the one derivation
/// the package manager and the shipping compiler read too, because this assembler is
/// one of those binaries: bytes another assembler wrote are bytes this one cannot
/// vouch for, and the key is what says the two differ.
pub fn kernel_disk_cache_dir(device_root: &Path, core_repo: &str, plugins: &[String]) -> PathBuf {
    device_root
        .join(KERNEL_CACHE_DIR)
        .join(lichen_utils::cache::compiler_slot_key(core_repo, plugins))
}

/// The kernel disk cache this process reads and writes; `None` while no host named one.
///
/// # Invariant
/// **First set wins.** A process has one assembler and one filesystem view of its
/// cache, so a second root silently serves the first root's modules under the second
/// root's name; configuring again is not a relocation, and the caller is told which
/// directory actually took effect.
static KERNEL_DISK_CACHE: OnceLock<PathBuf> = OnceLock::new();

/// Point the kernel disk cache at `dir`, and report the directory now in effect.
///
/// # Invariant
/// Returns `dir` when this call is the one that set it, and the directory an earlier
/// call set otherwise, so a host composing two roots learns which one the cache is
/// really under instead of assuming its own.
pub fn configure_kernel_disk_cache(dir: PathBuf) -> PathBuf {
    if KERNEL_DISK_CACHE.set(dir.clone()).is_ok() {
        return dir;
    }
    KERNEL_DISK_CACHE.get().cloned().unwrap_or(dir)
}

/// The directory the kernel disk cache is configured under, if a host named one.
pub fn kernel_disk_cache() -> Option<&'static Path> {
    KERNEL_DISK_CACHE.get().map(PathBuf::as_path)
}

/// The map of compiled modules, present only where an engine exists to fill it.
///
/// # Invariant
/// An engine is the only reason a cached module is worth keeping, so without the
/// `wasm` feature there is no key to key it by and no entry to put in it: the
/// counters above stand alone rather than reporting misses against a map that
/// cannot exist. Nothing in this crate or outside it reads it either way.
#[cfg(feature = "wasm")]
mod cache {
    use std::collections::HashMap;
    use std::sync::atomic::Ordering;
    use std::sync::{Mutex, OnceLock};

    use lichen_kernel_ir::KernelId;

    /// The compiled module cache: an assembled, validated wasm module per launch.
    ///
    /// # Invariant
    /// The key is the launch mode and the root [`KernelId`], which suffices because a
    /// registered fragment is immutable and an id is never reused. An entry is *derived*,
    /// so eviction loses nothing a value refers to; the [`wasmi::Engine`] is cached with
    /// its module on purpose, since it owns the translated code and a shared one would
    /// be a second unbounded accumulator.
    static MODULES: OnceLock<Mutex<ModuleCache>> = OnceLock::new();
    fn modules() -> &'static Mutex<ModuleCache> {
        MODULES.get_or_init(Default::default)
    }

    /// How many compiled modules stay resident: a bound, not a policy.
    const MAX_CACHED_MODULES: usize = 64;

    /// One cached module and the engine that compiled it.
    struct CachedModule {
        /// The engine the module was compiled by — see [`MODULES`] for why it is
        /// cached here rather than shared.
        engine: wasmi::Engine,
        module: wasmi::Module,
        /// Insertion stamp: the eviction order, oldest (smallest) first.
        stamp: u64,
    }

    /// The bounded map behind [`MODULES`].
    #[derive(Default)]
    struct ModuleCache {
        entries: HashMap<KernelId, CachedModule>,
        next_stamp: u64,
    }

    impl ModuleCache {
        /// A resident module, cloned out (both are `Arc` handles) so the caller
        /// does not hold the lock while instantiating.
        fn get(&self, key: KernelId) -> Option<(wasmi::Engine, wasmi::Module)> {
            self.entries
                .get(&key)
                .map(|cached| (cached.engine.clone(), cached.module.clone()))
        }

        /// Insert a freshly compiled module, evicting the oldest at the bound.
        /// Replacing a resident key evicts nothing.
        fn insert(&mut self, key: KernelId, engine: wasmi::Engine, module: wasmi::Module) {
            if self.entries.len() >= MAX_CACHED_MODULES
                && !self.entries.contains_key(&key)
                && let Some(oldest) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, cached)| cached.stamp)
                    .map(|(key, _)| *key)
            {
                self.entries.remove(&oldest);
            }
            let stamp = self.next_stamp;
            self.next_stamp += 1;
            self.entries.insert(
                key,
                CachedModule {
                    engine,
                    module,
                    stamp,
                },
            );
        }
    }

    /// The compiled module for `root`, assembling and compiling it on a miss.
    ///
    /// # Invariant
    /// One entry per kernel id, and no mode beside it: both launch paths assemble the
    /// root's relative launch set. The disk file is read only after this map misses,
    /// so a kernel this process holds never touches the filesystem. `assemble` is a
    /// closure because it reads the process kernel registry, which is the language
    /// side's to read; the miss is counted first, so a refusal stays a refusal.
    pub fn cached_module(
        root: KernelId,
        assemble: impl FnOnce() -> Result<Vec<u8>, String>,
    ) -> Result<(wasmi::Engine, wasmi::Module), String> {
        let key = root;
        if let Some(cached) = modules().lock().unwrap().get(key) {
            return Ok(cached);
        }
        // A file this engine refuses is the miss an absent file is.
        if let Some(cached) = super::disk::load(key) {
            modules()
                .lock()
                .unwrap()
                .insert(key, cached.0.clone(), cached.1.clone());
            return Ok(cached);
        }
        super::MODULE_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
        let bytes = assemble()?;
        let engine = wasmi::Engine::default();
        let module = wasmi::Module::new(&engine, &bytes).map_err(|e| e.to_string())?;
        super::disk::store(key, &bytes);
        modules()
            .lock()
            .unwrap()
            .insert(key, engine.clone(), module.clone());
        Ok((engine, module))
    }
}

#[cfg(feature = "wasm")]
pub use cache::cached_module;
