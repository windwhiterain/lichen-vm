//! The emitted-module cache: one assembled, validated wasm module per kernel id.
//!
//! # Invariant
//! The only id in this file is a [`KernelId`], so nothing here is a
//! `lichen_lowlevel::Module` and nothing here needs the language: a backend that
//! runs an artifact asks this cache and hands back an engine and a module. The
//! bytes come from the caller as a closure, so the assembly — which does read the
//! process kernel registry — stays on the language side while the cache does not.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Compiled modules this process built rather than served from [`MODULES`].
///
/// # Invariant
/// Test- and measurement-visible on purpose: the value of content-addressed ids
/// (`D15`) is that this stops growing when the same function is compiled again, and a
/// counter is the only way to see that without timing a compile.
static MODULE_CACHE_MISSES: AtomicUsize = AtomicUsize::new(0);

/// See [`MODULE_CACHE_MISSES`].
pub fn module_cache_misses() -> usize {
    MODULE_CACHE_MISSES.load(Ordering::Relaxed)
}

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
/// root's relative launch set, so the id decides the module. `assemble` is a
/// closure rather than an argument because it reads the process kernel registry,
/// which is the language side's to read; a miss is counted before it runs, so a
/// refusal is a miss that stayed a refusal.
pub fn cached_module(
    root: KernelId,
    assemble: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<(wasmi::Engine, wasmi::Module), String> {
    let key = root;
    if let Some(cached) = modules().lock().unwrap().get(key) {
        return Ok(cached);
    }
    MODULE_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
    let bytes = assemble()?;
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes).map_err(|e| e.to_string())?;
    modules()
        .lock()
        .unwrap()
        .insert(key, engine.clone(), module.clone());
    Ok((engine, module))
}
