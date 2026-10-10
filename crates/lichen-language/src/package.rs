//! The package store: a shared registry plus a path cache, device-backed.
//! See docs/notes/packages.md.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lichen_compute::WRAPPER_SOURCE;
use lichen_highlevel::checker::Build;
use lichen_highlevel::native::NativeOps;
use lichen_highlevel::program::{HighPackageMeta, PackageSource, TypeOperator, ValueType};
use lichen_lowlevel::{LocalNodeId, ModuleKey, NodeId, Registry, StaticModule, StaticNodeId};
use lichen_preprocess::{ImportResolver, PreprocessDiag, ResolvedPackage};

use crate::LangProgramShape;
use crate::diag::{Diag, Stage};
use crate::persist::{self, DeviceRegistry, Hash, ProgramCodecOf};
use crate::preprocess::{ResolvedImport, preprocess};
use crate::program::GcdOp;

mod vendored;
use vendored::{vendored_alias, vendored_entry_file};

/// The virtual path of the `lichen-compute` native package.
pub(crate) const COMPUTE_PATH: &str = "compute.lichen";

/// The virtual path of the built-in `core` module — the language's prelude.
pub(crate) const CORE_PATH: &str = "core.lichen";

/// The `core` module's source: the operator contract, written in lichen.
///
/// # Invariant
///
/// It is the prelude because an operator's contract is a fact about the
/// language: every source sees these names, and a program that wants different
/// ones shadows them (a later binder wins over the seeded import).
const CORE_SOURCE: &str = include_str!("core.lichen");

/// The names [`CORE_SOURCE`] binds at the top level, in source order.
fn core_exports() -> Vec<String> {
    let tokens = crate::lex::lex(CORE_SOURCE).tokens;
    let parsed = crate::parse::parse(&tokens);
    parsed
        .program
        .statements
        .iter()
        .filter_map(|bs| match &bs.stmt {
            crate::ast::Stmt::Binding(binding) => Some(binding.name.clone()),
            crate::ast::Stmt::Expr(_) => None,
        })
        .collect()
}

/// Whether `import` is the built-in prelude ([`CORE_PATH`]) rather than a program's.
///
/// # Invariant
///
/// A prelude entry carries a synthetic span because no directive produced it, so
/// a reader that shows a program's own names skips it; a written `import` keeps
/// its directive's span.
pub fn is_prelude_import(import: &ResolvedImport) -> bool {
    import
        .path
        .file_name()
        .is_some_and(|name| name == CORE_PATH)
}

/// Every node of a checked built-in that has a source position, paired with it.
///
/// # Invariant
///
/// Read before the module freezes — a frozen module carries values, not spans —
/// and the refs are mapped through the freeze afterwards.
fn located_nodes<P>(
    build: &Build<P>,
    span_index: Option<&crate::compile::SpanIndex>,
) -> Vec<(NodeId, (u32, u32))>
where
    P: LangProgramShape,
    P::Value: ValueType,
{
    let Some(span_index) = span_index else {
        return Vec::new();
    };
    build
        .node_edges
        .iter()
        .filter_map(|(node, loc)| {
            let span = span_index.get(loc.expr.0 as usize).copied().flatten()?;
            Some((*node, span))
        })
        .collect()
}

/// The `(name, term)` pairs [`CORE_SOURCE`] exposes directly.
///
/// # Invariant
///
/// The names come from the source in the same order as the build's statements,
/// so a mismatch is reported rather than quietly truncated; the pairs are read
/// before the module moves into the freeze, and mapped to static refs after.
fn core_terms<P>(build: &Build<P>) -> Result<Vec<(String, NodeId)>, String>
where
    P: LangProgramShape,
    P::Value: ValueType,
{
    let names = core_exports();
    let statements = &build.ir.stmt_roots;
    if names.len() != statements.len() {
        return Err(format!(
            "the core module binds {} names but the build has {} statements",
            names.len(),
            statements.len()
        ));
    }
    names
        .into_iter()
        .zip(statements.iter())
        .map(|(name, &statement)| {
            let term = build
                .state
                .get(statement.0 as usize)
                .and_then(|state| state.term)
                .ok_or_else(|| format!("the core binding `{name}` has no pair"))?;
            Ok((name, term))
        })
        .collect()
}

/// Map [`core_terms`] through a freeze: each binding's pair becomes the static
/// ref `direct` binds the name to.
fn core_direct(
    terms: Vec<(String, NodeId)>,
    key: ModuleKey,
    node_map: &std::collections::HashMap<NodeId, LocalNodeId>,
) -> Result<Vec<(String, StaticNodeId)>, String> {
    terms
        .into_iter()
        .map(|(name, term)| {
            let index = node_map
                .get(&term)
                .copied()
                .ok_or_else(|| format!("the core binding `{name}` was not frozen"))?;
            Ok((name, StaticNodeId { module: key, index }))
        })
        .collect()
}

/// The `lichen-compute` plugin's private native registry.
///
/// # Invariant
///
/// Attached only to the compilation of `compute.lichen`, so `$jit`/`$launch`
/// resolve privately and a second plugin registering its own `$jit` never
/// collides.
fn compute_native_ops<P>() -> NativeOps<P>
where
    P: LangProgramShape,
    P::Value: ValueType + From<lichen_compute::ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    lichen_compute::compute_native_ops!(P)
}

/// A loaded package: the path, its registry key, and the export's static ref.
#[derive(Clone, Debug)]
pub struct PackageHandle {
    pub path: PathBuf,
    pub key: ModuleKey,
    pub export: StaticNodeId,
    /// Extra `(name, export)` bindings a package exposes; empty for an ordinary one.
    pub direct: Vec<(String, StaticNodeId)>,
}

/// The process-local package store: a shared registry plus a path cache.
///
/// # Invariant
///
/// The registry is shared with every package and importer, so a package loaded
/// once is used in place by all of them. Generic over the program type `P`.
pub struct PackageStore<P: ProgramCodecOf> {
    pub registry: Arc<RwLock<Registry<P>>>,
    pub packages: HashMap<PathBuf, PackageHandle>,
    /// The in-flight load stack; a re-entry closes an import cycle.
    loading: Vec<PathBuf>,
    /// Native virtual packages, keyed by the import path they are served at.
    native: HashMap<PathBuf, PackageHandle>,
    /// Vendored dependency directories, keyed by the import alias they resolve
    /// through (see [`Self::register_vendored`]).
    vendored: HashMap<String, PathBuf>,
    /// The device's cache directory (`None` = in-memory only).
    cache_dir: Option<PathBuf>,
    device: Option<DeviceRegistry>,
    /// The in-memory key allocator, used when no cache directory is configured.
    next_key: u64,
    /// The artifact codec `P::Codec` is a type-level marker (the codec value is
    /// `P::Codec::default()` at use).
    _codec: PhantomData<<P as ProgramCodecOf>::Codec>,
    /// Packages compiled (not loaded from the device cache) — tests.
    pub compiled: usize,
    /// Packages loaded from the device cache without recompiling — tests.
    pub loaded_from_cache: usize,
}

// The minimal impl: construction, cache-dir plumbing, and the vendored registry.
impl<P: ProgramCodecOf> PackageStore<P> {
    /// A purely in-memory store: keys come from a process-local counter.
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

    /// A store backed by the device's persistent cache rooted at `cache_dir`.
    pub fn with_cache_dir(cache_dir: PathBuf) -> Self {
        let device = DeviceRegistry::open(cache_dir.clone());
        let mut store = PackageStore::new();
        store.cache_dir = Some(cache_dir);
        store.device = Some(device);
        store
    }

    /// A store backed by an already-open device registry at `cache_dir`.
    ///
    /// # Invariant
    ///
    /// Every mutation re-reads the registry under the cross-process lock, so a
    /// long-lived handle observes another process's writes as a fresh open would.
    pub fn with_device(cache_dir: PathBuf, device: DeviceRegistry) -> Self {
        let mut store = PackageStore::new();
        store.cache_dir = Some(cache_dir);
        store.device = Some(device);
        store
    }

    /// Take the open device registry back out; `None` for an in-memory store.
    pub fn into_device(self) -> Option<DeviceRegistry> {
        self.device
    }

    /// Garbage-collect the device cache, returning the number of reclaimed artifacts.
    pub fn gc(&mut self) -> usize {
        self.device.as_mut().map_or(0, |device| device.gc())
    }

    /// Remove one package (by its source path) from the device cache.
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

    /// Register a vendored dependency directory under an import alias.
    pub fn register_vendored(&mut self, alias: impl Into<String>, dir: PathBuf) {
        self.vendored.insert(alias.into(), dir);
    }

    /// Whether `alias` is a registered vendored dependency.
    pub fn is_vendored(&self, alias: &str) -> bool {
        self.vendored.contains_key(alias)
    }

    /// The shared registry, for the importer's checker.
    pub fn registry(&self) -> Arc<RwLock<Registry<P>>> {
        self.registry.clone()
    }

    /// Allocate the device key for a file ID: the existing one, or a fresh one.
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

    /// The device file ID of an import path this store resolved.
    ///
    /// # Invariant
    ///
    /// An on-disk package's canonical path is its file ID; a registered embedded
    /// source is filed under `virtual:<name>`, because its import path is a
    /// display path, not its identity.
    fn dependency_file_id(&self, path: &Path) -> String {
        let registered = path
            .file_name()
            .map(Path::new)
            .and_then(|name| self.native.get(name));
        match registered {
            Some(handle) => persist::virtual_file_id(&handle.path.to_string_lossy()),
            None => path.to_string_lossy().into_owned(),
        }
    }
}

// The compute-bounds impl: load, compile, freeze and serialize.
impl<P> PackageStore<P>
where
    P: LangProgramShape,
    P::Value: ValueType + From<lichen_compute::ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    /// Load (or cache-fetch) the package at `path`, resolving its imports first.
    ///
    /// # Invariant
    ///
    /// Each dependency loads recursively before this package compiles, so its
    /// refs are absolute from birth and the freeze sees their keys registered.
    pub fn load_package(&mut self, path: &Path) -> Result<PackageHandle, Vec<Diag<P>>> {
        // A registered native virtual package: served by its file name, no disk file.
        if let Some(file_name) = path.file_name()
            && let Some(handle) = self.native.get(Path::new(file_name))
        {
            return Ok(handle.clone());
        }
        // The `lichen-compute` native package self-registers on first import.
        if path.file_name().is_some_and(|n| n == "compute.lichen") {
            let handle = self
                .register_compute()
                .map_err(|e| vec![Diag::unattributed(Stage::Preprocess, e)])?;
            return Ok(handle);
        }
        // The built-in `core` module, self-registering the same way.
        if path.file_name().is_some_and(|n| n == CORE_PATH) {
            let handle = self
                .register_core()
                .map_err(|e| vec![Diag::unattributed(Stage::Preprocess, e)])?;
            return Ok(handle);
        }
        // Only `.lichen` files are packages; any other extension is rejected.
        if path.extension().is_none_or(|ext| ext != "lichen") {
            return Err(vec![Diag::unattributed(
                Stage::Preprocess,
                format!(
                    "cannot load package {}: only .lichen files are packages",
                    path.display()
                ),
            )]);
        }
        let canonical = match std::fs::canonicalize(path) {
            Ok(canonical) => canonical,
            Err(e) => {
                return Err(vec![Diag::io(format!(
                    "cannot read package {}: {e}",
                    path.display()
                ))]);
            }
        };
        if self.loading.contains(&canonical) {
            return Err(vec![Diag::unattributed(
                Stage::Preprocess,
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

    /// Register the `lichen-compute` native package from its embedded wrapper.
    ///
    /// # Invariant
    ///
    /// It is compiled against the plugin's private native registry and always
    /// fresh in memory: its `Kernel` values are runtime-only, so the artifact
    /// format cannot serialize it.
    fn register_compute(&mut self) -> Result<PackageHandle, String> {
        // Reuse an already-registered module, so one registry holds it once.
        if let Some(handle) = self.registered_builtin(COMPUTE_PATH) {
            self.native
                .insert(PathBuf::from(COMPUTE_PATH), handle.clone());
            return Ok(handle);
        }
        let source = WRAPPER_SOURCE;
        let (preprocessed, mut diags) = preprocess(source, Some(Path::new(COMPUTE_PATH)), self);
        if !diags.is_empty() {
            return Err(diags
                .drain(..)
                .map(|d| d.message)
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let line_starts = crate::lex::line_starts(preprocessed.code);
        let report = crate::compile_with_imports_at::<P>(
            preprocessed.code,
            &preprocessed.imports,
            Some(self.registry()),
            preprocessed.code_base,
            &line_starts,
            compute_native_ops::<P>(),
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

        // The nodes' positions, read before the module moves into the freeze.
        let located = located_nodes(&build, report.span_index.as_ref());
        // Fully evaluate the exported value and type before freezing.
        let mut module = build.module;
        module.evaluate_node_deep(build.root_val, None);
        module.evaluate_node_deep(build.root_ty, None);

        // The embedded compute package has no imports, so its identity is its
        // own source hash alone.
        let hash = persist::artifact_hash(persist::sha256(source.as_bytes()), &[]);
        let (key, _is_new) = self.alloc_key(&persist::virtual_file_id(COMPUTE_PATH));
        let freeze = self
            .registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .freeze_mapped(&module, key, hash);
        let export = StaticNodeId {
            module: freeze.key,
            index: freeze.node_map[&build.root_term],
        };
        let package_source = self.builtin_source(COMPUTE_PATH, source, &located, &freeze.node_map);
        self.registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_package_meta(
                freeze.key,
                HighPackageMeta {
                    export: Some(export),
                    source: Some(package_source),
                    ..Default::default()
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

    /// The path a built-in module's source is exposed at.
    fn builtin_path(&self, name: &str) -> PathBuf {
        match &self.cache_dir {
            Some(root) => root.join("builtin").join(name),
            None => PathBuf::from(name),
        }
    }

    /// Materialize a built-in's source under the cache root, returning its path.
    ///
    /// # Invariant
    ///
    /// Rewritten only when the bytes differ, and a no-op without a cache root:
    /// the store's writes never fail a compile.
    fn materialize_builtin(&self, name: &str, source: &str) -> PathBuf {
        let path = self.builtin_path(name);
        if self.cache_dir.is_none() {
            return path;
        }
        if std::fs::read_to_string(&path).ok().as_deref() != Some(source) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, source);
        }
        path
    }

    /// The source record of a built-in the store just froze: its file and node spans.
    fn builtin_source(
        &self,
        name: &str,
        source: &str,
        located: &[(NodeId, (u32, u32))],
        node_map: &HashMap<NodeId, LocalNodeId>,
    ) -> PackageSource {
        let path = self.materialize_builtin(name, source);
        let mut spans: Vec<(usize, (u32, u32))> = located
            .iter()
            .filter_map(|(node, span)| Some((node_map.get(node)?.index, *span)))
            .collect();
        spans.sort_unstable();
        // One position per frozen node: the first recorded wins, so a node the
        // build located twice does not shadow itself.
        spans.dedup_by_key(|(index, _)| *index);
        PackageSource {
            path,
            code: std::sync::Arc::from(source),
            spans,
        }
    }

    /// A built-in module's source record, if this registry holds one.
    pub fn package_source(&self, key: ModuleKey) -> Option<PackageSource> {
        let registry = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry
            .get(key)
            .and_then(|package| package.meta.source.clone())
    }

    /// A built-in module this registry already holds, as a handle.
    ///
    /// # Invariant
    ///
    /// The recorded meta carries what a handle needs: the exported pair and the
    /// directly-exposed names.
    fn registered_builtin(&mut self, path: &str) -> Option<PackageHandle> {
        let (key, _is_new) = self.alloc_key(&persist::virtual_file_id(path));
        let registry = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let package = registry.get(key)?;
        let export = package.meta.export?;
        Some(PackageHandle {
            path: PathBuf::from(path),
            key,
            export,
            direct: package.meta.direct.clone(),
        })
    }

    /// Register the built-in `core` module from its embedded source.
    ///
    /// # Invariant
    ///
    /// Its `direct` list is what makes it a prelude rather than a package: every
    /// top-level name it binds becomes a base-scope name, and the record value
    /// holds its fields in definition order, so the source-order names pair with
    /// the frozen field nodes positionally.
    fn register_core(&mut self) -> Result<PackageHandle, String> {
        // Reuse when the registry already holds the module: one key, one artifact.
        if let Some(handle) = self.registered_builtin(CORE_PATH) {
            self.native.insert(PathBuf::from(CORE_PATH), handle.clone());
            return Ok(handle);
        }
        let source = CORE_SOURCE;
        let (preprocessed, mut diags) = preprocess(source, Some(Path::new(CORE_PATH)), self);
        if !diags.is_empty() {
            return Err(diags
                .drain(..)
                .map(|d| d.message)
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let line_starts = crate::lex::line_starts(preprocessed.code);
        let report = crate::compile_with_imports_at::<P>(
            preprocessed.code,
            &preprocessed.imports,
            Some(self.registry()),
            preprocessed.code_base,
            &line_starts,
            lichen_highlevel::native::no_native_ops::<P>(),
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
        // Read the names' pairs and the nodes' positions before the freeze.
        let terms = core_terms(&build)?;
        let located = located_nodes(&build, report.span_index.as_ref());
        let mut module = build.module;
        module.evaluate_node_deep(build.root_val, None);
        module.evaluate_node_deep(build.root_ty, None);
        // Its identity is its source hash alone; it imports nothing of its own.
        let hash = persist::artifact_hash(persist::sha256(source.as_bytes()), &[]);
        let (key, _is_new) = self.alloc_key(&persist::virtual_file_id(CORE_PATH));
        let freeze = self
            .registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .freeze_mapped(&module, key, hash);
        let export = StaticNodeId {
            module: freeze.key,
            index: freeze.node_map[&build.root_term],
        };
        let direct = core_direct(terms, freeze.key, &freeze.node_map)?;
        let package_source = self.builtin_source(CORE_PATH, source, &located, &freeze.node_map);
        self.registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_package_meta(
                freeze.key,
                HighPackageMeta {
                    export: Some(export),
                    direct: direct.clone(),
                    source: Some(package_source),
                },
            );
        let handle = PackageHandle {
            path: PathBuf::from(CORE_PATH),
            key: freeze.key,
            export,
            direct,
        };
        self.native.insert(PathBuf::from(CORE_PATH), handle.clone());
        // Not counted in `compiled`: the prelude is the language compiling itself.
        Ok(handle)
    }

    /// The prelude import: the built-in `core` module, bound as its bare names.
    ///
    /// # Invariant
    ///
    /// A later binder of the same name wins, which is what makes the prelude
    /// shadowable rather than reserved.
    pub fn prelude_import(&mut self) -> Result<ResolvedImport, Vec<Diag<P>>> {
        let handle = self.load_package(Path::new(CORE_PATH))?;
        Ok(ResolvedImport {
            // The module is also reachable as `core`, seeded first so it can be shadowed.
            name: "core".to_string(),
            // A seeded import has no source position: no directive produced it.
            span: (1, 1),
            export: handle.export,
            path: handle.path,
            direct: handle.direct,
        })
    }

    /// The load path behind the cache: incremental verification first, then compile.
    fn load_package_inner(&mut self, canonical: &Path) -> Result<PackageHandle, Vec<Diag<P>>> {
        let file_id = canonical.to_string_lossy().into_owned();
        let source = std::fs::read_to_string(canonical).map_err(|e| {
            vec![Diag::io(format!(
                "cannot read package {}: {e}",
                canonical.display()
            ))]
        })?;
        if let Some(device) = &self.device
            && let Some(verified) = device.verify(&file_id, source.as_bytes())
            && let Some(handle) = self.try_reuse(
                canonical,
                &file_id,
                verified.key,
                verified.hash,
                &verified.deps,
            )?
        {
            return Ok(handle);
        }
        // The artifact file is missing or corrupt — fall through to
        // a fresh compile (the pending allocation is reused).
        self.build_package(canonical, source)
    }

    /// Reuse an already-registered artifact, or load it from the device store.
    ///
    /// # Invariant
    ///
    /// `Ok(None)` means the artifact cannot be loaded from disk (missing or
    /// corrupt), and the caller recompiles.
    fn try_reuse(
        &mut self,
        canonical: &Path,
        file_id: &str,
        key: ModuleKey,
        hash: Hash,
        deps: &[(String, ModuleKey)],
    ) -> Result<Option<PackageHandle>, Vec<Diag<P>>> {
        for (dep_file_id, _) in deps {
            // A recorded embedded dependency is filed as a `virtual:<name>` id.
            let name = persist::virtual_name(dep_file_id).unwrap_or(dep_file_id.as_str());
            self.load_package(Path::new(name))?;
        }
        let mut modules: HashMap<ModuleKey, Arc<StaticModule<P>>> = HashMap::new();
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
            crate::persist::load_artifact::<P, P::Codec>(device, file_id, key, hash, &modules)
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
                    ..Default::default()
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

    /// Read, resolve, compile and freeze one package, serializing it to the cache.
    fn build_package(
        &mut self,
        canonical: &Path,
        source: String,
    ) -> Result<PackageHandle, Vec<Diag<P>>> {
        let file_id = canonical.to_string_lossy().into_owned();

        // Resolve the package's own imports through this store: each
        // dependency loads (and freezes) first, recursively.
        let (preprocessed, mut diags) = preprocess(&source, Some(canonical), self);
        if !diags.is_empty() {
            return Err(std::mem::take(&mut diags));
        }

        // The artifact identity: the source hash, then each dependency's own.
        let deps: Vec<(String, ModuleKey)> = preprocessed
            .imports
            .iter()
            .map(|import| (self.dependency_file_id(&import.path), import.export.module))
            .collect();
        let source_hash = persist::sha256(source.as_bytes());
        let hash = self.artifact_identity(source_hash, &deps);
        // A file ID is compiled once and overwritten: its key is stable.
        let (key, _is_new) = self.alloc_key(&file_id);

        // Compile against the shared registry so the import refs resolve in place.
        let line_starts = crate::lex::line_starts(&source);
        let report = crate::compile_with_imports_at::<P>(
            preprocessed.code,
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

        // The freeze replaces the slot: the registry may still hold the old artifact.
        let freeze = self
            .registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .freeze_mapped_replacing(&module, key, hash);
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
                    ..Default::default()
                },
            );

        // Serialize into the device cache and record the source hash and deps.
        if let Some(device) = &mut self.device {
            let modules = {
                let registry = self
                    .registry
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut modules: HashMap<ModuleKey, Arc<StaticModule<P>>> = HashMap::new();
                for (key, package) in registry.iter() {
                    modules.insert(key, package.module.clone());
                }
                modules
            };
            // A codec refusal leaves the package uncached rather than failing the build.
            if let Ok(bytes) = persist::serialize_artifact_with::<P, P::Codec>(
                modules[&freeze.key].as_ref(),
                &modules,
                hash,
                export.index,
                P::Codec::default(),
            ) {
                device.store_artifact(&file_id, &bytes);
                device.publish(&file_id, key, persist::sha256(source.as_bytes()), deps);
            }
        }
        self.compiled += 1;
        Ok(PackageHandle {
            path: canonical.to_path_buf(),
            key: freeze.key,
            export,
            direct: Vec::new(),
        })
    }

    /// The artifact identity this unit's source and recorded dependencies produce.
    ///
    /// # Invariant
    ///
    /// Every side must agree on the fold, or the cache misses on every run rather
    /// than once; a cache-backed store asks the device, an in-memory store its own
    /// registry.
    fn artifact_identity(&self, source_hash: Hash, deps: &[(String, ModuleKey)]) -> Hash {
        match &self.device {
            Some(device) => device.artifact_identity(source_hash, deps),
            None => {
                let identities = self.dependency_identities(deps);
                persist::artifact_hash(source_hash, &identities)
            }
        }
    }

    /// The recorded identity of each dependency in an in-memory store.
    ///
    /// # Invariant
    ///
    /// A dependency this store has no record of contributes the all-zero
    /// sentinel, which no real identity equals.
    fn dependency_identities(&self, deps: &[(String, ModuleKey)]) -> Vec<(ModuleKey, Hash)> {
        let registry = self
            .registry
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        deps.iter()
            .map(|(_, dep_key)| {
                (
                    *dep_key,
                    registry
                        .get(*dep_key)
                        .map_or([0; 32], |package| package.hash),
                )
            })
            .collect()
    }

    /// Resolve an import path relative to the current source file's directory.
    pub fn resolve_import(
        &mut self,
        base: Option<&Path>,
        import_path: &str,
    ) -> Result<PackageHandle, Diag<P>> {
        // A vendored alias resolves against its registered directory.
        if let Some((alias, rest)) = vendored_alias(import_path)
            && let Some(dir) = self.vendored.get(alias)
        {
            let resolved = match rest {
                Some(rest) => dir.join(rest),
                None => vendored_entry_file::<P>(dir, alias)?,
            };
            return self.load_package(&resolved).map_err(first_diagnostic);
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
        self.load_package(&resolved).map_err(first_diagnostic)
    }
}

/// The diagnostic a failed package load reports: the load's own first.
///
/// # Invariant
///
/// A failed build always carries at least one (see [`crate::build_report`]), so
/// this seam needs no fallback message of its own.
fn first_diagnostic<P: lichen_lowlevel::Program>(mut diags: Vec<Diag<P>>) -> Diag<P> {
    diags
        .drain(..)
        .next()
        .expect("a failed package load reports a diagnostic")
}

// The native-package registration impl: compile a plugin's embedded source.
impl<P> PackageStore<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    /// Register a native virtual package: compile `source` against its own ops.
    ///
    /// # Invariant
    ///
    /// The embedded source is a complete package (no block, no imports), so it
    /// compiles directly against the registry rather than through the
    /// compute-bounded preprocessor.
    pub fn register_native(
        &mut self,
        virtual_path: &str,
        source: &str,
        native_ops: NativeOps<P>,
    ) -> Result<PackageHandle, String> {
        let line_starts = crate::lex::line_starts(source);
        let report = crate::compile_with_imports_at::<P>(
            source,
            &[],
            Some(self.registry()),
            0,
            &line_starts,
            native_ops,
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

        // Embedded source with no imports: its identity is its own source hash.
        let hash = persist::artifact_hash(persist::sha256(source.as_bytes()), &[]);
        let (key, _is_new) = self.alloc_key(&persist::virtual_file_id(virtual_path));
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
                    ..Default::default()
                },
            );
        // File it under its file name, so `import "name"` resolves to it.
        let name = Path::new(virtual_path)
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(virtual_path));
        let handle = PackageHandle {
            path: PathBuf::from(virtual_path),
            key: freeze.key,
            export,
            direct: Vec::new(),
        };
        self.native.insert(name, handle.clone());
        self.compiled += 1;
        Ok(handle)
    }
}
/// The import-resolution seam the preprocessor consumes.
impl<P> ImportResolver<StaticNodeId> for PackageStore<P>
where
    P: LangProgramShape,
    P::Value: ValueType + From<lichen_compute::ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    fn resolve_import(
        &mut self,
        base: Option<&Path>,
        import_path: &str,
    ) -> Result<ResolvedPackage<StaticNodeId>, PreprocessDiag> {
        PackageStore::resolve_import(self, base, import_path)
            .map_err(|d| PreprocessDiag {
                span: d.span,
                message: d.message,
            })
            .map(|handle| ResolvedPackage {
                export: handle.export,
                path: handle.path,
                direct: handle.direct,
            })
    }

    fn register_vendored(&mut self, alias: String, dir: PathBuf) {
        PackageStore::register_vendored(self, alias, dir);
    }
}

impl<P: ProgramCodecOf> Default for PackageStore<P> {
    fn default() -> Self {
        Self::new()
    }
}
