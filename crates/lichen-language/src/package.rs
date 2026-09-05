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
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lichen_compute::WRAPPER_SOURCE;
use lichen_highlevel::native::NativeOps;
use lichen_highlevel::program::{HighPackageMeta, TypeOperator, ValueType};
use lichen_lowlevel::{LowOperator, ModuleKey, OperatorExt, Registry, StaticModule, StaticNodeId};
use lichen_utils::extend::AsEnum;

use crate::CompiledProgram;
use crate::diag::{Diag, Stage};
use crate::persist::{self, ArtifactCodec, DeviceRegistry, Hash};
use crate::preprocess::preprocess;
use crate::program::GcdOp;

/// The virtual path of the `lichen-compute` native package.  Imported as
/// `compute.lichen`, it is served from a registered native module (see
/// [`PackageStore::register_compute`]) rather than a source file on disk.
const COMPUTE_PATH: &str = "compute.lichen";

/// The `lichen-compute` plugin's private native registry, built by the
/// plugin over a host's concrete program marker.  Attached only to the
/// compilation of `compute.lichen`, so `$jit`/`$launch` resolve privately — a
/// second plugin registering its own `$jit` never collides.  The plugin itself
/// is program-generic; only this composition site names the program marker.
fn compute_native_ops<V, O>() -> NativeOps<CompiledProgram<V, O>>
where
    V: ValueType + From<lichen_compute::ComputeValue> + 'static,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq
        + From<GcdOp>
        + From<TypeOperator>
        + From<lichen_compute::ComputeOperator>
        + 'static,
{
    lichen_compute::compute_native_ops!(CompiledProgram<V, O>)
}

/// A loaded package: the path, its registry key, and the static ref to the
/// exported final `[value, type]` pair (the package's final expression).
#[derive(Clone, Debug)]
pub struct PackageHandle {
    pub path: PathBuf,
    pub key: ModuleKey,
    pub export: StaticNodeId,
    /// Extra `(name, export)` bindings a package exposes directly, so `import`
    /// can bind them as names (the compute package's `jit`/`launch`/`Kernel`).
    /// Empty for an ordinary package.
    pub direct: Vec<(String, StaticNodeId)>,
}

/// The process-local package store: a shared registry plus a path cache,
/// optionally backed by the device's persistent store.
///
/// The registry is shared with every package and importer, so a package
/// loaded once is used in place by all of them (`packages` is public so a
/// host or test can observe that sharing).  Generic over the value/operator
/// vocabularies `V`/`O` (the language's attribute set is fixed) and the
/// artifact codec `C` (`[`persist::NoPersist`]` for an in-memory store).
pub struct PackageStore<
    V: ValueType,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq,
    C = persist::HighProgramCodec,
> {
    pub registry: Arc<RwLock<Registry<CompiledProgram<V, O>>>>,
    pub packages: HashMap<PathBuf, PackageHandle>,
    /// The in-flight load stack (canonical paths) — a package re-entered
    /// while still loading closes an import cycle.
    loading: Vec<PathBuf>,
    /// Native virtual packages — a package name served from a registered
    /// native module (the `lichen-compute` package) instead of a disk file,
    /// keyed by the import path (`compute.lichen`).  See
    /// [`Self::register_compute`].
    native: HashMap<PathBuf, PackageHandle>,
    /// Vendored dependencies, keyed by the import alias the package manager
    /// resolves `import "alias"` / `import "alias/rest"` through.  A vendored
    /// alias maps to a directory of `.lichen` package files (a git-fetched
    /// dependency); the bare alias resolves to the directory's entry package,
    /// and a suffixed path resolves relative to it.  See
    /// [`Self::register_vendored`] and [`Self::resolve_import`].
    vendored: HashMap<String, PathBuf>,
    /// The device's cache directory (`None` = in-memory only).
    cache_dir: Option<PathBuf>,
    device: Option<DeviceRegistry>,
    /// The in-memory key allocator — the device registry's counter when no
    /// cache directory is configured (a process-local device).
    next_key: u64,
    /// The artifact codec `C` is a type-level marker (the codec value is
    /// `C::default()` at use).
    _codec: PhantomData<C>,
    /// Packages compiled (not loaded from the device cache) — tests.
    pub compiled: usize,
    /// Packages loaded from the device cache without recompiling — tests.
    pub loaded_from_cache: usize,
}

// The minimal impl: construction, cache-dir plumbing, and the vendored
// registry — none of which touch a compute value/operator or the artifact
// codec.  These need only that `CompiledProgram<V, O>` is a program.
impl<V, O, C> PackageStore<V, O, C>
where
    V: ValueType,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq,
{
    /// A purely in-memory store — the pre-cache behavior (tests, the readme
    /// sync, in-process embeddings).  Device keys are allocated from a
    /// process-local counter and nothing is persisted.
    pub fn new() -> Self {
        let registry = Arc::new(RwLock::new(Registry::new()));
        PackageStore {
            registry,
            packages: HashMap::new(),
            loading: Vec::new(),
            native: HashMap::new(),
            vendored: HashMap::new(),
            cache_dir: None,
            device: None,
            next_key: 0,
            _codec: PhantomData,
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
            Ok(canonical) => self
                .device
                .as_mut()
                .is_some_and(|device| device.remove(&canonical.to_string_lossy())),
            Err(_) => false,
        }
    }

    /// The device's cache directory, when one is configured.
    pub fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    /// Register a vendored dependency directory under an import alias, so
    /// `import "alias"` / `import "alias/rest"` resolve into it.  The package
    /// manager registers one alias per git-fetched dependency before compiling.
    pub fn register_vendored(&mut self, alias: impl Into<String>, dir: PathBuf) {
        self.vendored.insert(alias.into(), dir);
    }

    /// Whether `alias` is a registered vendored dependency.
    pub fn is_vendored(&self, alias: &str) -> bool {
        self.vendored.contains_key(alias)
    }

    /// The shared registry, for the importer's checker.
    pub fn registry(&self) -> Arc<RwLock<Registry<CompiledProgram<V, O>>>> {
        self.registry.clone()
    }

    /// Allocate the device key for a file ID: the existing key when the file
    /// is already registered (recompiles reuse it, overwriting the slot),
    /// otherwise a fresh one (reclaimed first, then the next index).
    fn alloc_key(&mut self, file_id: &str) -> (ModuleKey, bool) {
        match &mut self.device {
            Some(device) => device.alloc(file_id),
            None => {
                let key = ModuleKey::from_raw(self.next_key);
                self.next_key += 1;
                (key, true)
            }
        }
    }
}

// The compute-bounds impl: load/compile/freeze/serialize, which compile the
// `compute.lichen` native package and run the program's imports through the
// shared store.  These need the compute value/operator coercions (and the
// `GcdOp`/`TypeOperator`/`'static`/codec bundle) because they call
// `compile_with_imports_at`, `compute_native_ops`, and the artifact codec.
impl<V, O, C> PackageStore<V, O, C>
where
    V: ValueType + From<lichen_compute::ComputeValue> + 'static,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq
        + From<GcdOp>
        + From<TypeOperator>
        + From<lichen_compute::ComputeOperator>
        + 'static,
    C: ArtifactCodec<CompiledProgram<V, O>> + Default,
{
    /// Load (or fetch from cache) the package at `path`, resolving its own
    /// `@import` directives first: each dependency loads (recursively)
    /// before this package compiles, so its refs are absolute from birth
    /// and the freeze below sees their keys already registered.
    pub fn load_package(
        &mut self,
        path: &Path,
    ) -> Result<PackageHandle, Vec<Diag<CompiledProgram<V, O>>>> {
        // The `lichen-compute` native package: served from a registered
        // module, not a disk file.
        if path.file_name().is_some_and(|n| n == "compute.lichen") {
            if let Some(handle) = self.native.get(Path::new(COMPUTE_PATH)) {
                return Ok(handle.clone());
            }
            let handle = self
                .register_compute()
                .map_err(|e| vec![Diag::new(Stage::Preprocess, (0, 0), e)])?;
            return Ok(handle);
        }
        // Only `.lichen` files are packages.  Reject any other extension up
        // front so the cache invariant holds by construction — an artifact's
        // file ID is always a `.lichen` path (or a `virtual:` path for an
        // embedded source), which is exactly what the `gc` "clean" rule keeps.
        if path.extension().is_none_or(|ext| ext != "lichen") {
            return Err(vec![Diag::new(
                Stage::Preprocess,
                (0, 0),
                format!(
                    "cannot load package {}: only .lichen files are packages",
                    path.display()
                ),
            )]);
        }
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

    /// Register the `lichen-compute` native package: compile its embedded
    /// wrapper source into a frozen module, file it in the shared registry,
    /// and remember the handle so `compute.lichen` imports are served from
    /// here (no disk file).  The wrapper is compiled against the plugin's
    /// *private* native registry — the only compilation that resolves its
    /// `$jit`/`$launch` calls.  Its frozen module carries runtime-only
    /// `Kernel` values (see `plugin-taxonomy.md`), which the artifact format
    /// deliberately cannot serialize, so it is always compiled fresh in
    /// memory rather than cached on the device.
    fn register_compute(&mut self) -> Result<PackageHandle, String> {
        let source = WRAPPER_SOURCE;
        let (preprocessed, mut diags) = preprocess(source, Some(Path::new(COMPUTE_PATH)), self);
        if !diags.is_empty() {
            return Err(diags
                .drain(..)
                .map(|d| d.message)
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let line_starts = crate::lex::line_starts(&preprocessed.code);
        let report = crate::compile_with_imports_at::<V, O>(
            &preprocessed.code,
            &preprocessed.imports,
            Some(self.registry()),
            preprocessed.code_base,
            &line_starts,
            compute_native_ops::<V, O>(),
        );
        if !report.diagnostics.is_empty() || report.build.as_ref().is_none_or(|b| !b.ok) {
            return Err(report
                .diagnostics
                .into_iter()
                .map(|d| d.message)
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let build = report.build.unwrap();

        // Fully evaluate the exported value and type before freezing.
        let mut module = build.module;
        module.evaluate_node_deep(build.root_val, None);
        module.evaluate_node_deep(build.root_ty, None);

        let hash = persist::artifact_hash(source.as_bytes(), &[]);
        let (key, _is_new) = self.alloc_key("virtual:compute.lichen");
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
        let handle = PackageHandle {
            path: PathBuf::from(COMPUTE_PATH),
            key: freeze.key,
            export,
            direct: Vec::new(),
        };
        self.native
            .insert(PathBuf::from(COMPUTE_PATH), handle.clone());
        self.compiled += 1;
        Ok(handle)
    }

    /// The load path behind the cache: incremental verification first, then
    /// compile.  Only reached through [`Self::load_package`], which owns the
    /// cache and the loading stack.
    fn load_package_inner(
        &mut self,
        canonical: &Path,
    ) -> Result<PackageHandle, Vec<Diag<CompiledProgram<V, O>>>> {
        let file_id = canonical.to_string_lossy().into_owned();
        let source = std::fs::read_to_string(canonical).map_err(|e| {
            vec![Diag::new(
                Stage::Preprocess,
                (0, 0),
                format!("cannot read package {}: {e}", canonical.display()),
            )]
        })?;
        if let Some(device) = &self.device {
            if let Some(verified) = device.verify(&file_id, source.as_bytes()) {
                if let Some(handle) = self.try_reuse(
                    canonical,
                    &file_id,
                    verified.key,
                    verified.hash,
                    &verified.deps,
                )? {
                    return Ok(handle);
                }
                // The artifact file is missing or corrupt — fall through to
                // a fresh compile (the pending allocation is reused).
            }
        }
        self.build_package(canonical, source)
    }

    /// Reuse an already-registered artifact: ensure its dependencies are
    /// loaded, then serve the resident module, or load the artifact from
    /// the device store when this process has not loaded it yet.  Returns
    /// `Ok(None)` when the artifact cannot be loaded from disk (missing or
    /// corrupt) — the caller recompiles.
    fn try_reuse(
        &mut self,
        canonical: &Path,
        file_id: &str,
        key: ModuleKey,
        hash: Hash,
        deps: &[(String, ModuleKey)],
    ) -> Result<Option<PackageHandle>, Vec<Diag<CompiledProgram<V, O>>>> {
        for (dep_file_id, _) in deps {
            self.load_package(Path::new(dep_file_id))?;
        }
        let mut modules: HashMap<ModuleKey, Arc<StaticModule<CompiledProgram<V, O>>>> =
            HashMap::new();
        {
            let registry = self
                .registry
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                        direct: Vec::new(),
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
        let Ok((module, export_index)) =
            device.load_artifact::<CompiledProgram<V, O>, C>(file_id, key, hash, &modules)
        else {
            return Ok(None);
        };
        let export = StaticNodeId {
            module: key,
            index: export_index,
        };
        {
            let mut registry = self
                .registry
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.insert_module(key, hash, module);
            registry.set_package_meta(
                key,
                HighPackageMeta {
                    export: Some(export),
                },
            );
        }
        self.loaded_from_cache += 1;
        Ok(Some(PackageHandle {
            path: canonical.to_path_buf(),
            key,
            export,
            direct: Vec::new(),
        }))
    }

    /// Read, resolve, compile, and freeze one package, serializing it into
    /// the device cache.  Only reached through [`Self::load_package`], which
    /// owns the cache and the loading stack; the source is already read.
    fn build_package(
        &mut self,
        canonical: &Path,
        source: String,
    ) -> Result<PackageHandle, Vec<Diag<CompiledProgram<V, O>>>> {
        let file_id = canonical.to_string_lossy().into_owned();

        // Resolve the package's own imports through this store: each
        // dependency loads (and freezes) first, recursively.
        let (preprocessed, mut diags) = preprocess(&source, Some(canonical), self);
        if !diags.is_empty() {
            return Err(std::mem::take(&mut diags));
        }

        // The artifact identity: the raw source plus the dependency keys in
        // source order — transitive, so a dependency change re-keys this
        // artifact.
        let dep_keys: Vec<ModuleKey> = preprocessed
            .imports
            .iter()
            .map(|import| import.export.module)
            .collect();
        let hash = persist::artifact_hash(source.as_bytes(), &dep_keys);
        // A file ID is compiled once and overwritten: the key is stable per
        // file, so recompiling a changed file reuses the same slot.
        let (key, _is_new) = self.alloc_key(&file_id);

        // Compile against the shared registry so the import leaves resolve
        // in place; the module then carries the dependencies' absolute refs
        // into its freeze below.
        let line_starts = crate::lex::line_starts(&source);
        let report = crate::compile_with_imports_at::<V, O>(
            &preprocessed.code,
            &preprocessed.imports,
            Some(self.registry()),
            preprocessed.code_base,
            &line_starts,
            lichen_highlevel::no_native_ops(),
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

        // Serialize into the device cache under the file ID slot (overwritten
        // on recompile), and record the source hash + dependency graph.
        if let Some(device) = &mut self.device {
            let modules = {
                let registry = self
                    .registry
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut modules: HashMap<ModuleKey, Arc<StaticModule<CompiledProgram<V, O>>>> =
                    HashMap::new();
                for (key, package) in registry.iter() {
                    modules.insert(key, package.module.clone());
                }
                modules
            };
            let bytes = persist::serialize_artifact_with::<CompiledProgram<V, O>, C>(
                modules[&freeze.key].as_ref(),
                &modules,
                hash,
                export.index,
                C::default(),
            );
            device.store_artifact(&file_id, &bytes);
            let deps: Vec<(String, ModuleKey)> = preprocessed
                .imports
                .iter()
                .map(|import| {
                    (
                        import.path.to_string_lossy().into_owned(),
                        import.export.module,
                    )
                })
                .collect();
            device.publish(&file_id, key, persist::sha256(source.as_bytes()), deps);
        }
        self.compiled += 1;
        Ok(PackageHandle {
            path: canonical.to_path_buf(),
            key: freeze.key,
            export,
            direct: Vec::new(),
        })
    }

    /// Resolve an import path relative to the current source file's directory.
    pub fn resolve_import(
        &mut self,
        base: Option<&Path>,
        import_path: &str,
    ) -> Result<PackageHandle, Diag<CompiledProgram<V, O>>> {
        // A vendored dependency alias: `import "alias"` or `import "alias/rest"`
        // resolves against the vendored directory registered under `alias`
        // (see [`Self::register_vendored`]).  A bare `alias` names the
        // dependency's entry package; `alias/rest` resolves `rest` relative to
        // the vendored directory.  Only tried when the alias is registered and
        // is not a file-like path (a leading segment ending in `.lichen`).
        if let Some((alias, rest)) = vendored_alias(import_path) {
            if let Some(dir) = self.vendored.get(alias) {
                let resolved = match rest {
                    Some(rest) => dir.join(rest),
                    None => vendored_entry_file::<CompiledProgram<V, O>>(dir, alias)?,
                };
                return self.load_package(&resolved).map_err(|mut diags| {
                    diags.drain(..).next().unwrap_or_else(|| {
                        Diag::new(
                            Stage::Preprocess,
                            (0, 0),
                            format!("cannot resolve vendored import '{}'", import_path),
                        )
                    })
                });
            }
        }
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
}

/// Split a potential vendored alias from an import path: `"foo"` →
/// `("foo", None)`, `"foo/rest"` → `("foo", Some("rest"))`.  A leading
/// segment that ends in `.lichen` is a file name, not an alias (so a relative
/// import like `"math.lichen"` never hits the vendored map).
fn vendored_alias(import_path: &str) -> Option<(&str, Option<&str>)> {
    let (first, rest) = match import_path.find('/') {
        Some(i) => (&import_path[..i], Some(&import_path[i + 1..])),
        None => (import_path, None),
    };
    if first.is_empty() || first.ends_with(".lichen") {
        return None;
    }
    Some((first, rest))
}

/// The entry package file of a vendored dependency directory: a `lib.lichen`,
/// then `<alias>.lichen`, then the directory's sole `.lichen` file.  An
/// ambiguous (many) or absent package is a diagnostic, not a guess.
fn vendored_entry_file<P: lichen_lowlevel::Program>(
    dir: &Path,
    alias: &str,
) -> Result<PathBuf, Diag<P>> {
    let lib = dir.join("lib.lichen");
    if lib.is_file() {
        return Ok(lib);
    }
    let aliased = dir.join(format!("{alias}.lichen"));
    if aliased.is_file() {
        return Ok(aliased);
    }
    let mut files = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "lichen"))
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    files.sort();
    match files.len() {
        1 => Ok(files.into_iter().next().expect("one file")),
        0 => Err(Diag::new(
            Stage::Preprocess,
            (0, 0),
            format!(
                "vendored dependency '{alias}' has no .lichen entry package (no lib.lichen, \
                 {alias}.lichen, or a single .lichen file)"
            ),
        )),
        _ => {
            let names = files
                .into_iter()
                .map(|f| {
                    f.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>()
                .join(", ");
            Err(Diag::new(
                Stage::Preprocess,
                (0, 0),
                format!(
                    "vendored dependency '{alias}' is ambiguous: pick one of {names} (or add a lib.lichen)"
                ),
            ))
        }
    }
}

impl<V, O, C> Default for PackageStore<V, O, C>
where
    V: ValueType,
    O: OperatorExt<CompiledProgram<V, O>>
        + AsEnum<LowOperator>
        + From<LowOperator>
        + std::fmt::Debug
        + Copy
        + PartialEq,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod vendored_tests {
    use super::*;
    use crate::program::{LangOperator, LangValue};

    fn tempdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lichen-vendored-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn vendored_alias_resolves_to_entry_package() {
        let dir = tempdir("entry");
        let foo = dir.join("deps").join("foo");
        std::fs::create_dir_all(&foo).unwrap();
        std::fs::write(foo.join("lib.lichen"), "42").unwrap();
        let mut store = PackageStore::<LangValue, LangOperator>::new();
        store.register_vendored("foo", foo.clone());
        let handle = store.resolve_import(None, "foo").unwrap();
        assert_eq!(
            handle.path,
            std::fs::canonicalize(foo.join("lib.lichen")).unwrap()
        );
    }

    #[test]
    fn vendored_alias_resolves_subpath() {
        let dir = tempdir("sub");
        let foo = dir.join("deps").join("foo");
        std::fs::create_dir_all(&foo).unwrap();
        std::fs::write(foo.join("lib.lichen"), "1").unwrap();
        std::fs::write(foo.join("other.lichen"), "2").unwrap();
        let mut store = PackageStore::<LangValue, LangOperator>::new();
        store.register_vendored("foo", foo.clone());
        let handle = store.resolve_import(None, "foo/other.lichen").unwrap();
        assert_eq!(
            handle.path,
            std::fs::canonicalize(foo.join("other.lichen")).unwrap()
        );
    }

    #[test]
    fn non_vendored_relative_import_does_not_hit_alias() {
        let dir = tempdir("plain");
        std::fs::write(dir.join("math.lichen"), "3").unwrap();
        let mut store = PackageStore::<LangValue, LangOperator>::new();
        store.register_vendored("foo", dir.join("deps").join("foo"));
        let base = dir.join("main.lichen");
        let handle = store.resolve_import(Some(&base), "math.lichen").unwrap();
        assert_eq!(
            handle.path,
            std::fs::canonicalize(dir.join("math.lichen")).unwrap()
        );
    }

    #[test]
    fn ambiguous_vendored_dir_is_diagnosed() {
        let dir = tempdir("ambig");
        let foo = dir.join("deps").join("foo");
        std::fs::create_dir_all(&foo).unwrap();
        std::fs::write(foo.join("a.lichen"), "1").unwrap();
        std::fs::write(foo.join("b.lichen"), "2").unwrap();
        let mut store = PackageStore::<LangValue, LangOperator>::new();
        store.register_vendored("foo", foo);
        let err = store.resolve_import(None, "foo").unwrap_err();
        assert!(err.message.contains("ambiguous"), "{}", err.message);
    }
}
