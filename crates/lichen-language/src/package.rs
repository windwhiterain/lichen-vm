//! A minimal in-process package store.
//!
//! A package is an ordinary lichen source file whose final expression is the
//! exported value.  Loading one resolves its `@import` directives through
//! this same store (transitive dependencies load first and freeze into the
//! shared registry), compiles it against that shared registry, and freezes
//! the built module — a package that itself imports packages freezes its
//! dependency refs verbatim, absolute from birth, so every importer reads
//! the dependencies' shared payloads in place.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use lichen_highlevel::program::{HighPackageMeta, HighProgram, HighProgramValue};
use lichen_lowlevel::{ModuleKey, Registry, StaticNodeId};

use crate::diag::{Diag, Stage};
use crate::preprocess::preprocess;

/// A loaded package: the path, its registry key, and the static ref to the
/// exported final `[value, type]` pair (the package's final expression).
#[derive(Clone, Debug)]
pub struct PackageHandle {
    pub path: PathBuf,
    pub key: ModuleKey,
    pub export: StaticNodeId,
}

/// The process-local package store: a shared registry plus a path cache.
///
/// The registry is shared with every package and importer, so a package
/// loaded once is used in place by all of them (`packages` is public so a
/// host or test can observe that sharing).
pub struct PackageStore {
    pub registry: Arc<RwLock<Registry<HighProgram<HighProgramValue>>>>,
    pub packages: HashMap<PathBuf, PackageHandle>,
    /// The in-flight load stack (canonical paths) — a package re-entered
    /// while still loading closes an import cycle.
    loading: Vec<PathBuf>,
}

impl PackageStore {
    pub fn new() -> Self {
        let registry = Arc::new(RwLock::new(Registry::new()));
        PackageStore {
            registry,
            packages: HashMap::new(),
            loading: Vec::new(),
        }
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
        let result = self.build_package(&canonical);
        self.loading.pop();
        let handle = result?;
        self.packages.insert(canonical, handle.clone());
        Ok(handle)
    }

    /// Read, resolve, compile, and freeze one package.  Only reached
    /// through [`Self::load_package`], which owns the cache and the loading
    /// stack.
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

        // Compile against the shared registry so the import leaves resolve
        // in place; the module then carries the dependencies' absolute refs
        // into its freeze below.
        let report = crate::compile_with_imports_in(
            &preprocessed.source,
            &preprocessed.imports,
            Some(self.registry()),
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
            .freeze_mapped(&module);
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
    pub fn registry(&self) -> Arc<RwLock<Registry<HighProgram<HighProgramValue>>>> {
        self.registry.clone()
    }
}

impl Default for PackageStore {
    fn default() -> Self {
        Self::new()
    }
}
