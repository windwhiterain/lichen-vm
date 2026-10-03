//! The package store: a shared registry plus a path cache, optionally
//! backed by the persistent device store ([`crate::persist`]).
//!
//! Vendored path resolution lives in the sibling module `vendored`.
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
use lichen_highlevel::checker::Build;
use lichen_highlevel::native::NativeOps;
use lichen_highlevel::program::{HighPackageMeta, TypeOperator, ValueType};
use lichen_lowlevel::{LocalNodeId, ModuleKey, NodeId, Registry, StaticModule, StaticNodeId};
use lichen_preprocess::{ImportResolver, PreprocessDiag, ResolvedPackage};

use crate::LangProgramShape;
use crate::diag::{Diag, Stage};
use crate::persist::{self, DeviceRegistry, Hash, ProgramCodecOf};
use crate::preprocess::{ResolvedImport, preprocess};
use crate::program::GcdOp;

mod vendored;
use vendored::{vendored_alias, vendored_entry_file};

/// The virtual path of the `lichen-compute` native package.  Imported as
/// `compute.lichen`, it is served from a registered native module (see
/// [`PackageStore::register_compute`]) rather than a source file on disk.
pub(crate) const COMPUTE_PATH: &str = "compute.lichen";

/// The virtual path of the built-in **`core`** module — the language's
/// **prelude**: imported implicitly into every source a host compiles (see
/// [`PackageStore::prelude_import`]), and explicitly as `core = import "core"`
/// when a program wants the module *value* rather than its names.  Served from a
/// registered built-in module (see [`PackageStore::register_core`]), never from a
/// source file on disk.
pub(crate) const CORE_PATH: &str = "core.lichen";

/// The `core` module's source: the operator **contract**, written in lichen —
/// the class domain `Num`, the predicate `in_num` that consults it, and one
/// binding per polymorphic operator, each refining its operands' **classes**
/// with `x : (_ ! in_num)` (`docs/notes/operator-polymorphism.md` §3, §9).
///
/// It is the prelude because an operator's contract is a fact about the
/// language, not about a program: every source sees these names without an
/// import, and a program that wants different ones shadows them (a later binder
/// wins over a seeded import, and the prelude is seeded first).
const CORE_SOURCE: &str = include_str!("core.lichen");

/// The names [`CORE_SOURCE`] binds at the top level, in source order — the
/// record fields of its export struct.  A record's *value* carries its fields in
/// definition order, which is what pairs a name with the field's frozen node.
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

/// Whether `import` is the built-in **prelude** ([`CORE_PATH`]) rather than an
/// import a program wrote.
///
/// The distinction matters to a reader that shows a program's *own* names: the
/// prelude is seeded into every source (`crate::preprocess::preprocess`), and its
/// entries carry a synthetic span because no directive produced them — so an
/// editor's document symbols, completions, and hover skip them, while a written
/// `import` stays a document definition (its span is the directive's).
pub fn is_prelude_import(import: &ResolvedImport) -> bool {
    import
        .path
        .file_name()
        .is_some_and(|name| name == CORE_PATH)
}

/// The `(name, term)` pairs [`CORE_SOURCE`] exposes **directly**: one per
/// top-level binding, each the binding's own `[value, type]` pair.
///
/// A record's *value* holds its fields' **values** — their types live in the
/// struct's kind — so the pair a name must resolve to is not recoverable from
/// the frozen struct: it is the checked build's term for that binding.  The names
/// come from the source in the same order, so a mismatch is a bug in this pairing
/// rather than a quietly missing name — it is reported, not truncated.
///
/// Read **before** the module is moved into the freeze, and mapped to static refs
/// afterwards (see [`core_direct`]).
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

/// The `lichen-compute` plugin's private native registry, built by the
/// plugin over a host's concrete program marker.  Attached only to the
/// compilation of `compute.lichen`, so `$jit`/`$launch` resolve privately — a
/// second plugin registering its own `$jit` never collides.  The plugin itself
/// is program-generic; only this composition site names the program marker.
fn compute_native_ops<P>() -> NativeOps<P>
where
    P: LangProgramShape,
    P::Value: ValueType + From<lichen_compute::ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    lichen_compute::compute_native_ops!(P)
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
/// host or test can observe that sharing).  Generic over a single program
/// type `P` (the associated-type collector; its `P::Codec` is the artifact
/// codec — [`persist::NoPersist`] for an in-memory store).
pub struct PackageStore<P: ProgramCodecOf> {
    pub registry: Arc<RwLock<Registry<P>>>,
    pub packages: HashMap<PathBuf, PackageHandle>,
    /// The in-flight load stack (canonical paths) — a package re-entered
    /// while still loading closes an import cycle.
    loading: Vec<PathBuf>,
    /// Native virtual packages — a package name served from a registered
    /// native module instead of a disk file, keyed by the import path
    /// (`compute.lichen`, `std.lichen`, …).  See
    /// [`Self::register_compute`] and [`Self::register_native`].
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
    /// The artifact codec `P::Codec` is a type-level marker (the codec value is
    /// `P::Codec::default()` at use).
    _codec: PhantomData<<P as ProgramCodecOf>::Codec>,
    /// Packages compiled (not loaded from the device cache) — tests.
    pub compiled: usize,
    /// Packages loaded from the device cache without recompiling — tests.
    pub loaded_from_cache: usize,
}

// The minimal impl: construction, cache-dir plumbing, and the vendored
// registry — none of which touch a compute value/operator or the artifact
// codec.  These need only that `P` is a program carrying its codec.
impl<P: ProgramCodecOf> PackageStore<P> {
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

    /// A store backed by an already-open device registry at `cache_dir`.  For a
    /// host that keeps one registry handle across requests (the language
    /// server) instead of reopening — and reparsing — the registry file each
    /// time: the handle carries the same cache directory
    /// [`Self::with_cache_dir`] would have opened.  Every mutation re-reads the
    /// registry under the cross-process lock and every verification reads it
    /// back, so a long-lived handle observes another process's writes exactly
    /// as a freshly opened one would.
    pub fn with_device(cache_dir: PathBuf, device: DeviceRegistry) -> Self {
        let mut store = PackageStore::new();
        store.cache_dir = Some(cache_dir);
        store.device = Some(device);
        store
    }

    /// Take the open device registry back out, so a host can hold it across
    /// requests (see [`Self::with_device`]).  `None` for an in-memory store.
    pub fn into_device(self) -> Option<DeviceRegistry> {
        self.device
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
    pub fn registry(&self) -> Arc<RwLock<Registry<P>>> {
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

    /// The device file ID of an import path this store resolved: an on-disk
    /// package's canonical path is its file ID, while a registered embedded
    /// source is filed under `virtual:<name>` (see [`Self::register_native`]
    /// and [`Self::register_compute`]) — the import path is that source's
    /// display path, not its identity, so the device cannot verify the
    /// dependency by it.
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

// The compute-bounds impl: load/compile/freeze/serialize, which compile the
// `compute.lichen` native package and run the program's imports through the
// shared store.  These need the compute value/operator coercions (and the
// `GcdOp`/`TypeOperator`/`'static`/codec bundle) because they call
// `compile_with_imports_at`, `compute_native_ops`, and the artifact codec.
impl<P> PackageStore<P>
where
    P: LangProgramShape,
    P::Value: ValueType + From<lichen_compute::ComputeValue> + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    /// Load (or fetch from cache) the package at `path`, resolving its own
    /// `@import` directives first: each dependency loads (recursively)
    /// before this package compiles, so its refs are absolute from birth
    /// and the freeze below sees their keys already registered.
    pub fn load_package(&mut self, path: &Path) -> Result<PackageHandle, Vec<Diag<P>>> {
        // A registered native virtual package (`compute.lichen`, `std.lichen`,
        // …): served from the in-memory registry, never a disk file.  A host
        // that registered one (the package-manager plug: a plugin's embedded
        // source compiled against its private native registry) is served here
        // by its file name.
        if let Some(file_name) = path.file_name()
            && let Some(handle) = self.native.get(Path::new(file_name))
        {
            return Ok(handle.clone());
        }
        // The `lichen-compute` native package: served from a registered
        // module, not a disk file.  It self-registers on first import.
        if path.file_name().is_some_and(|n| n == "compute.lichen") {
            let handle = self
                .register_compute()
                .map_err(|e| vec![Diag::unattributed(Stage::Preprocess, e)])?;
            return Ok(handle);
        }
        // The built-in `core` module — the prelude.  Served from a registered
        // module too, and self-registering the same way, so the implicit prelude
        // import and an explicit `import "core"` are one code path.
        if path.file_name().is_some_and(|n| n == CORE_PATH) {
            let handle = self
                .register_core()
                .map_err(|e| vec![Diag::unattributed(Stage::Preprocess, e)])?;
            return Ok(handle);
        }
        // Only `.lichen` files are packages.  Reject any other extension up
        // front so the cache invariant holds by construction — an artifact's
        // file ID is always a `.lichen` path (or a `virtual:` path for an
        // embedded source), which is exactly what the `gc` "clean" rule keeps.
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
        // Reuse an already-registered module, like [`Self::register_core`]: a
        // host that builds a store per run over one registry must not freeze the
        // same content twice.
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

    /// A built-in module this registry already holds, as a handle — the reuse
    /// path for a host that builds a store per run over one shared registry (see
    /// [`Self::register_core`]).  The recorded meta carries what a handle needs:
    /// the exported pair and the directly-exposed names.
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

    /// Register the built-in **`core`** module: compile its embedded source into
    /// a frozen module, file it in the shared registry, and remember the handle
    /// so `core.lichen` is served from here (no disk file).  Unlike
    /// [`Self::register_compute`], `core` calls no native operator: it is
    /// ordinary lichen, so it compiles against the **empty** native registry.
    ///
    /// Its `direct` list is what makes it a *prelude* rather than a package:
    /// every name it binds at the top level is bound as a base-scope **name**
    /// ([`crate::preprocess::ResolvedImport`]'s `direct`), the mechanism the
    /// import path already carries for a package that exposes names alongside its
    /// module value.  A record's *value* holds its fields in definition order, so
    /// the names (read from the source) pair with the frozen field nodes
    /// positionally.
    fn register_core(&mut self) -> Result<PackageHandle, String> {
        // **Reuse** when this registry already holds the module.  A host may build
        // a store per run over one shared registry — the editor's worker does
        // exactly that — and recompiling would freeze the same content under the
        // same key, which the registry refuses, one key naming one artifact.  The
        // recorded meta carries what a handle needs, so the later store adopts the
        // earlier one's module.
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
        // The names' pairs are read from the checked build, before the module
        // moves into the freeze (`core_terms`).
        let terms = core_terms(&build)?;
        let mut module = build.module;
        module.evaluate_node_deep(build.root_val, None);
        module.evaluate_node_deep(build.root_ty, None);
        // The module imports nothing of its own, so its identity is its source
        // hash alone.  The prelude it *is* is not a dependency of the artifact.
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
        self.registry
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_package_meta(
                freeze.key,
                HighPackageMeta {
                    export: Some(export),
                    direct: direct.clone(),
                },
            );
        let handle = PackageHandle {
            path: PathBuf::from(CORE_PATH),
            key: freeze.key,
            export,
            direct,
        };
        self.native.insert(PathBuf::from(CORE_PATH), handle.clone());
        // **Not counted** in [`Self::compiled`].  That counter observes the
        // packages a *program* asked for (an explicit `import`, a `depend`), which
        // is what the cache and identity tests measure; the prelude is the
        // language compiling itself, and every store pays it exactly once.
        Ok(handle)
    }

    /// The **prelude import**: the built-in `core` module, bound as the bare
    /// names it exports.  Every source a host compiles is seeded with it (see
    /// [`crate::preprocess::preprocess`]), so the operator contract — `Num`,
    /// `in_num`, `add`, … — is in scope without an import.  A later binder of the
    /// same name wins, which is what makes the prelude *shadowable* rather than
    /// reserved.
    pub fn prelude_import(&mut self) -> Result<ResolvedImport, Vec<Diag<P>>> {
        let handle = self.load_package(Path::new(CORE_PATH))?;
        Ok(ResolvedImport {
            // The module is also reachable as `core` — the same handle serves an
            // explicit `import "core"` — seeded first, so a program's own binder
            // of that name shadows it.
            name: "core".to_string(),
            // A seeded import has no source position — the resolver records the
            // span of the directive it came from, and this one came from none.
            span: (1, 1),
            export: handle.export,
            path: handle.path,
            direct: handle.direct,
        })
    }

    /// The load path behind the cache: incremental verification first, then
    /// compile.  Only reached through [`Self::load_package`], which owns the
    /// cache and the loading stack.
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
    ) -> Result<Option<PackageHandle>, Vec<Diag<P>>> {
        for (dep_file_id, _) in deps {
            // A recorded embedded dependency is filed as a `virtual:<name>`
            // file ID; the store serves it by the name it registered (see
            // [`Self::dependency_file_id`]).
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

    /// Read, resolve, compile, and freeze one package, serializing it into
    /// the device cache.  Only reached through [`Self::load_package`], which
    /// owns the cache and the loading stack; the source is already read.
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

        // The artifact identity: the raw source hash, then each dependency's
        // own identity.  Transitive by construction — a dependency's identity
        // is the identity it was built as, so a change anywhere in the import
        // closure changes this one, and the frozen artifact this key writes
        // into its header stops matching the one a later verification computes.
        //
        // The recorded dependency identity is the file ID, not the import's
        // display path (they differ for an embedded source).
        let deps: Vec<(String, ModuleKey)> = preprocessed
            .imports
            .iter()
            .map(|import| (self.dependency_file_id(&import.path), import.export.module))
            .collect();
        let source_hash = persist::sha256(source.as_bytes());
        let hash = self.artifact_identity(source_hash, &deps);
        // A file ID is compiled once and overwritten: the key is stable per
        // file, so recompiling a changed file reuses the same slot.
        let (key, _is_new) = self.alloc_key(&file_id);

        // Compile against the shared registry so the import leaves resolve
        // in place; the module then carries the dependencies' absolute refs
        // into its freeze below.
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

        // The freeze **replaces** the slot: a file ID is compiled once and
        // overwritten, so recompiling a changed file reuses the same key — and
        // the store's registry is the caller's (a session's cells are filed in it,
        // because their imports must resolve there), so the previous compile's
        // artifact for this file is still resident when this one runs.
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

        // Serialize into the device cache under the file ID slot (overwritten
        // on recompile), and record the source hash + dependency graph.
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
            // A codec refusal means this package has no artifact form, not that
            // the package is wrong.  The shape is ordinary code: a package whose
            // top level `$jit`s holds a live kernel, and a kernel is a
            // process-local registry handle.  The program runs either way, so
            // the answer is to leave the package **uncached** — the same
            // degradation every other failure in this store takes — rather than
            // to fail a compile that would have succeeded.
            //
            // The pending device entry `alloc_key` wrote stays unpublished, and
            // an unpublished entry can never verify, so the refusal is
            // permanent for this file rather than a one-off miss.
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

    /// The artifact identity this unit's source and recorded dependencies will
    /// produce — the fold every side must agree on, or the cache misses on
    /// every run instead of once (see [`persist::artifact_hash`]).
    ///
    /// A cache-backed store asks the device, because the device's record is
    /// what a later verification recomputes from; an in-memory store has no
    /// artifact to key and nothing to be consistent with across loads, so its
    /// own registry answers.
    fn artifact_identity(&self, source_hash: Hash, deps: &[(String, ModuleKey)]) -> Hash {
        match &self.device {
            Some(device) => device.artifact_identity(source_hash, deps),
            None => {
                let identities = self.dependency_identities(deps);
                persist::artifact_hash(source_hash, &identities)
            }
        }
    }

    /// The recorded identity of each dependency in an **in-memory** store: what
    /// its module was frozen as.  A dependency this store has no record of
    /// contributes the all-zero sentinel, which no real identity equals.
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
        // A vendored dependency alias: `import "alias"` or `import "alias/rest"`
        // resolves against the vendored directory registered under `alias`
        // (see [`Self::register_vendored`]).  A bare `alias` names the
        // dependency's entry package; `alias/rest` resolves `rest` relative to
        // the vendored directory.  Only tried when the alias is registered and
        // is not a file-like path (a leading segment ending in `.lichen`).
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

/// The diagnostic a failed package load reports: the load's own first.  A
/// failed build always carries at least one (see [`crate::build_report`]), so
/// this seam needs no fallback message of its own.
fn first_diagnostic<P: lichen_lowlevel::Program>(mut diags: Vec<Diag<P>>) -> Diag<P> {
    diags
        .drain(..)
        .next()
        .expect("a failed package load reports a diagnostic")
}

// The native-package registration impl: compile a plugin's embedded lichen
// source against its private native-op registry and serve it as a virtual
// package.  It needs only the bounds `compile_with_imports_at` requires — the
// store's compute/`ComputeValue` leaves are NOT needed — so the package-manager
// plug (register any native plugin source) stays plugin-agnostic.
impl<P> PackageStore<P>
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    /// Register a native virtual package: compile `source` (a plugin's
    /// embedded lichen wrapper) against that plugin's *private* native-op
    /// registry and serve it at `virtual_path` (its file name), so
    /// `import "virtual_path"` resolves to it without a disk file.
    ///
    /// This is the package-manager plug: a host that pulls a native plugin
    /// compiles the plugin's `WRAPPER_SOURCE` here and files it in the store's
    /// native-package registry, exactly as `register_compute` does for
    /// `lichen-compute` — but over a *caller-supplied* registry, so the plugin
    /// stays program-generic and the caller names only the crate and its
    /// program marker.
    ///
    /// The embedded source is a complete package (no `---…---` block, no
    /// imports), so it compiles directly against the registry rather than
    /// through [`preprocess`](crate::preprocess::preprocess) — which is
    /// compute-bounded and would force a non-compute host to carry
    /// [`lichen_compute::ComputeValue`]/[`lichen_compute::ComputeOperator`].
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

        // A registered native package is compiled from the source embedded in
        // this binary and has no imports, so its identity is its own source
        // hash alone.
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
        // File it under its file name so `load_package` serves it by the name
        // an `import "name"` resolves to (the path's own file name).
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
/// The import-resolution seam for the preprocessor, implemented by the package
/// store.  [`ImportResolver`] only knows the vocabulary-agnostic
/// [`ResolvedPackage`] (export/path/direct); the store adapts its own
/// `PackageHandle`/`Diag` to it, so the isolated preprocessor never names a
/// program marker.
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
