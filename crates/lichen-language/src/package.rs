//! The package store: a shared registry plus a path cache, optionally
//! backed by the persistent device store ([`crate::persist`]).
//!
//! A package is an ordinary lichen source file whose final expression is the
//! exported value.  Loading one resolves its `@import` directives through
//! this same store (transitive dependencies load first and freeze into the
//! shared registry), compiles it against that shared registry, and freezes
//! the built module — a package that itself imports packages freezes its
//! dependency refs verbatim, absolute from birth, so every importer reads
//! the dependencies' shared payloads in place.
//!
//! With a cache directory ([`PackageStore::with_cache_dir`], the CLI's
//! `~/.lichen`), a load first runs the device's *incremental verification*
//! over the recorded dependency graph: when the whole graph is up to date,
//! the artifact is loaded from disk (deserialized, registered under its
//! persistent device key) and the compile is skipped entirely.  Only the
//! chain that actually changed is recompiled, and each compiled package is
//! serialized back into the cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lichen_highlevel::program::HighPackageMeta;
use lichen_lowlevel::{ModuleKey, Registry, StaticModule, StaticNodeId};

use crate::diag::{Diag, Stage};
use crate::persist::{self, DeviceRegistry, Hash};
use crate::preprocess::preprocess;
use crate::program::LangProgram;

/// A loaded package: the path, its registry key, and the static ref to the
/// exported final `[value, type]` pair (the package's final expression).
#[derive(Clone, Debug)]
pub struct PackageHandle {
    pub path: PathBuf,
    pub key: ModuleKey,
    pub export: StaticNodeId,
}

/// The process-local package store: a shared registry plus a path cache,
/// optionally backed by the device's persistent store.
///
/// The registry is shared with every package and importer, so a package
/// loaded once is used in place by all of them (`packages` is public so a
/// host or test can observe that sharing).
pub struct PackageStore {
    pub registry: Arc<RwLock<Registry<LangProgram>>>,
    pub packages: HashMap<PathBuf, PackageHandle>,
    /// The in-flight load stack (canonical paths) — a package re-entered
    /// while still loading closes an import cycle.
    loading: Vec<PathBuf>,
    /// The device's cache directory (`None` = in-memory only).
    cache_dir: Option<PathBuf>,
    device: Option<DeviceRegistry>,
    /// The in-memory key allocator — the device registry's counter when no
    /// cache directory is configured (a process-local device).
    next_key: u64,
    /// The in-memory content dedup table: artifact hash → device key.
    content: HashMap<Hash, ModuleKey>,
    /// Packages compiled (not loaded from the device cache) — tests.
    pub compiled: usize,
    /// Packages loaded from the device cache without recompiling — tests.
    pub loaded_from_cache: usize,
}

impl PackageStore {
    /// A purely in-memory store — the pre-cache behavior (tests, the readme
    /// sync, in-process embeddings).  Device keys are allocated from a
    /// process-local counter and nothing is persisted.
    pub fn new() -> Self {
        let registry = Arc::new(RwLock::new(Registry::new()));
        PackageStore {
            registry,
            packages: HashMap::new(),
            loading: Vec::new(),
            cache_dir: None,
            device: None,
            next_key: 0,
            content: HashMap::new(),
            compiled: 0,
            loaded_from_cache: 0,
        }
    }

    /// A store backed by the device's persistent cache rooted at
    /// `cache_dir` (see [`crate::persist::lichendir`]): compiled packages
    /// are serialized into it, and up-to-date packages load from it without
    /// recompiling.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        let device = DeviceRegistry::open(cache_dir.clone());
        let mut store = PackageStore::new();
        store.cache_dir = Some(cache_dir);
        store.device = Some(device);
        store
    }

    /// Explicitly garbage-collect the device cache: reclaim every artifact
    /// not reachable from a path alias whose source file still exists.
    /// Returns the number of reclaimed artifacts.
    pub fn gc(&mut self) -> usize {
        self.device.as_mut().map_or(0, |device| device.gc())
    }

    /// Explicitly remove one package (by its source path) from the device
    /// cache.  Returns whether anything was removed.
    pub fn remove(&mut self, path: &Path) -> bool {
        match std::fs::canonicalize(path) {
            Ok(canonical) => self.device.as_mut().is_some_and(|device| device.remove(&canonical)),
            Err(_) => false,
        }
    }

    /// The device's cache directory, when one is configured.
    pub fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    /// Load (or fetch from cache) the package at `path`, resolving its own
    /// `@import` directives first: each dependency loads (recursively)
    /// before this package compiles, so its refs are absolute from birth
    /// and the freeze below sees their keys already registered.
    pub fn load_package(&mut self, path: &Path) -> Result<PackageHandle, Vec<Diag>> {
        let canonical = match std::fs::canonicalize(path) {
            Ok(canonical) => canonical,
            Err(e) => {
                return Err(vec![Diag::new(
                    Stage::Preprocess,
                    (0, 0),
                    format!("cannot read package {}: {e}", path.display()),
                )]);
            }
        };
        if self.loading.contains(&canonical) {
            return Err(vec![Diag::new(
                Stage::Preprocess,
                (0, 0),
                format!(
                    "circular import: {} is already being loaded",
                    canonical.display()
                ),
            )]);
        }
        if let Some(handle) = self.packages.get(&canonical) {
            return Ok(handle.clone());
        }
        self.loading.push(canonical.clone());
        let result = self.load_package_inner(&canonical);
        self.loading.pop();
        let handle = result?;
        self.packages.insert(canonical, handle.clone());
        Ok(handle)
    }

    /// The load path behind the cache: incremental verification first, then
    /// compile.  Only reached through [`Self::load_package`], which owns the
    /// cache and the loading stack.
    fn load_package_inner(&mut self, canonical: &Path) -> Result<PackageHandle, Vec<Diag>> {
        if let Some(device) = &self.device {
            if let Some(verified) = device.verify(canonical) {
                if let Some(handle) =
                    self.try_reuse(canonical, verified.key, verified.hash, &verified.deps)?
                {
                    return Ok(handle);
                }
                // The artifact file is missing or corrupt — fall through to
                // a fresh compile (the pending allocation is reused).
            }
        }
        self.build_package(canonical)
    }

    /// Reuse an already-registered artifact: ensure its dependencies are
    /// loaded, then serve the resident module, or load the artifact from
    /// the device store when this process has not loaded it yet.  Returns
    /// `Ok(None)` when the artifact cannot be loaded from disk (missing or
    /// corrupt) — the caller recompiles.
    fn try_reuse(
        &mut self,
        canonical: &Path,
        key: ModuleKey,
        hash: Hash,
        deps: &[(PathBuf, ModuleKey)],
    ) -> Result<Option<PackageHandle>, Vec<Diag>> {
        for (dep_path, _) in deps {
            self.load_package(dep_path)?;
        }
        let mut modules: HashMap<ModuleKey, Arc<StaticModule<LangProgram>>> =
            HashMap::new();
        {
            let registry = self.registry.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            match registry.get(key) {
                Some(package) if package.hash == hash => {
                    let export = package
                        .meta
                        .export
                        .expect("a registered package carries its export");
                    return Ok(Some(PackageHandle {
                        path: canonical.to_path_buf(),
                        key,
                        export,
                    }));
                }
                Some(_) => panic!(
                    "device key {key:?} was reclaimed and reallocated while this process still holds the old module — restart the process"
                ),
                None => {}
            }
            for (_, dep_key) in deps {
                modules.insert(
                    *dep_key,
                    registry
                        .get(*dep_key)
                        .expect("a dependency is loaded")
                        .module
                        .clone(),
                );
            }
        }
        let device = self.device.as_ref().expect("the device store");
        let Ok((module, export_index)) = device.load_artifact(key, hash, &modules) else {
            return Ok(None);
        };
        let export = StaticNodeId {
            module: key,
            index: export_index,
        };
        {
            let mut registry = self.registry.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.insert_module(key, hash, module);
            registry.set_package_meta(key, HighPackageMeta { export: Some(export) });
        }
        self.loaded_from_cache += 1;
        Ok(Some(PackageHandle {
            path: canonical.to_path_buf(),
            key,
            export,
        }))
    }

    /// Allocate the device key for an artifact hash: the existing key when
    /// the content is already registered (in memory or on the device),
    /// otherwise a fresh one (reclaimed first, then the next index).
    fn alloc_key(&mut self, hash: Hash) -> (ModuleKey, bool) {
        if let Some(&key) = self.content.get(&hash) {
            return (key, false);
        }
        let (key, is_new) = match &mut self.device {
            Some(device) => device.alloc(hash),
            None => {
                let key = ModuleKey::from_raw(self.next_key);
                self.next_key += 1;
                (key, true)
            }
        };
        self.content.insert(hash, key);
        (key, is_new)
    }

    /// Read, resolve, compile, and freeze one package, serializing it into
    /// the device cache.  Only reached through [`Self::load_package`], which
    /// owns the cache and the loading stack.
    fn build_package(&mut self, canonical: &Path) -> Result<PackageHandle, Vec<Diag>> {
        let source = std::fs::read_to_string(canonical).map_err(|e| {
            vec![Diag::new(
                Stage::Preprocess,
                (0, 0),
                format!("cannot read package {}: {e}", canonical.display()),
            )]
        })?;

        // Resolve the package's own imports through this store: each
        // dependency loads (and freezes) first, recursively.
        let (preprocessed, mut diags) = preprocess(&source, Some(canonical), self);
        if !diags.is_empty() {
            return Err(std::mem::take(&mut diags));
        }

        // The artifact identity: the raw source plus the dependency keys in
        // source order — transitive, so any dependency change re-keys this
        // artifact.  An already-registered identity is reused, not
        // recompiled.
        let dep_keys: Vec<ModuleKey> = preprocessed
            .imports
            .iter()
            .map(|import| import.export.module)
            .collect();
        let hash = persist::artifact_hash(source.as_bytes(), &dep_keys);
        let (key, is_new) = self.alloc_key(hash);
        if !is_new {
            let deps: Vec<(PathBuf, ModuleKey)> = preprocessed
                .imports
                .iter()
                .map(|import| (import.path.clone(), import.export.module))
                .collect();
            if let Some(handle) = self.try_reuse(canonical, key, hash, &deps)? {
                return Ok(handle);
            }
        }

        // Compile against the shared registry so the import leaves resolve
        // in place; the module then carries the dependencies' absolute refs
        // into its freeze below.
        let line_starts = crate::lex::line_starts(&source);
        let report = crate::compile_with_imports_at(
            &preprocessed.code,
            &preprocessed.imports,
            Some(self.registry()),
            preprocessed.code_base,
            &line_starts,
        );
        if !report.diagnostics.is_empty() || report.build.as_ref().is_none_or(|b| !b.ok) {
            return Err(report.diagnostics);
        }
        let build = report.build.unwrap();

        // Fully evaluate the exported value and type before freezing.
        let mut module = build.module;
        module.evaluate_node_deep(build.root_val, None);
        module.evaluate_node_deep(build.root_ty, None);

        let freeze = self
            .registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .freeze_mapped(&module, key, hash);
        let export = StaticNodeId {
            module: freeze.key,
            index: freeze.node_map[&build.root_term],
        };
        self.registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_package_meta(
                freeze.key,
                HighPackageMeta {
                    export: Some(export),
                },
            );

        // Serialize into the device cache: the artifact file (content-
        // addressed) and the registry record (source hash, dependency
        // graph, path alias).
        if let Some(device) = &mut self.device {
            let modules = {
                let registry = self
                    .registry
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut modules: HashMap<
                    ModuleKey,
                    Arc<StaticModule<LangProgram>>,
                > = HashMap::new();
                for (key, package) in registry.iter() {
                    modules.insert(key, package.module.clone());
                }
                modules
            };
            let bytes = persist::serialize_artifact(
                modules[&freeze.key].as_ref(),
                &modules,
                hash,
                export.index,
            );
            device.store_artifact(hash, &bytes);
            let deps: Vec<(PathBuf, ModuleKey)> = preprocessed
                .imports
                .iter()
                .map(|import| (import.path.clone(), import.export.module))
                .collect();
            device.publish(
                key,
                hash,
                persist::sha256(source.as_bytes()),
                deps,
                canonical.to_path_buf(),
            );
        }
        self.compiled += 1;
        Ok(PackageHandle {
            path: canonical.to_path_buf(),
            key: freeze.key,
            export,
        })
    }

    /// Resolve an import path relative to the current source file's directory.
    pub fn resolve_import(
        &mut self,
        base: Option<&Path>,
        import_path: &str,
    ) -> Result<PackageHandle, Diag> {
        let path = Path::new(import_path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            let base_dir = base
                .map(|base| {
                    if base.is_dir() {
                        base.to_path_buf()
                    } else {
                        base.parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| PathBuf::from("."))
                    }
                })
                .unwrap_or_else(|| PathBuf::from("."));
            base_dir.join(path)
        };
        self.load_package(&resolved).map_err(|mut diags| {
            diags.drain(..).next().unwrap_or_else(|| {
                Diag::new(
                    Stage::Preprocess,
                    (0, 0),
                    format!("cannot resolve import '{}'", import_path),
                )
            })
        })
    }

    /// The shared registry, for the importer's checker.
    pub fn registry(&self) -> Arc<RwLock<Registry<LangProgram>>> {
        self.registry.clone()
    }
}

impl Default for PackageStore {
    fn default() -> Self {
        Self::new()
    }
}
