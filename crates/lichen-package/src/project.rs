//! The project: a directory of lichen programs plus its manifest and its
//! git-fetched dependencies.
//!
//! A [`Project`] is the unit the package manager operates on.  It loads the
//! manifest, fetches every git dependency into the project's vendor dir, and
//! stages the dependencies onto the preprocessor import path (via
//! [`PackageStore::register_vendored`]) before compiling.  Dependencies are
//! fetched once per run and shared through the store, so importing a
//! dependency resolves against the single vendored clone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lichen_language::diag::{Diag, Stage};
use lichen_language::package::PackageStore;

use crate::git;
use crate::manifest::{Dependency, Manifest};

/// A lichen project rooted at a directory.
pub struct Project {
    /// The project root (canonicalized).
    pub dir: PathBuf,
    /// The project manifest (empty when there is none).
    pub manifest: Manifest,
}

impl Project {
    /// Load the project rooted at `dir` (its manifest, when present).  A
    /// malformed manifest is an error.
    pub fn load(dir: &Path) -> Result<Project, String> {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let manifest = Manifest::load(&dir)?;
        Ok(Project { dir, manifest })
    }

    /// The dependency set, keyed by import alias.
    pub fn deps(&self) -> &BTreeMap<String, Dependency> {
        &self.manifest.deps
    }

    /// The native-plugin dependencies (those that need a compiler rebuild).
    pub fn plugin_deps(&self) -> impl Iterator<Item = (&String, &Dependency)> {
        self.manifest.deps.iter().filter(|(_, dep)| dep.is_plugin())
    }

    /// Fetch every git dependency into its vendor dir, returning the
    /// alias → vendored-dir map.  Idempotent: existing clones are `fetch`ed
    /// and checked out, missing ones are cloned.
    pub fn fetch_all(&self) -> Result<BTreeMap<String, PathBuf>, String> {
        let mut out = BTreeMap::new();
        for (alias, dep) in &self.manifest.deps {
            let dir = git::fetch(&self.dir, alias, dep)?;
            out.insert(alias.clone(), dir);
        }
        Ok(out)
    }

    /// Stage every dependency's vendored dir with `store`, so
    /// `import "alias"` / `import "alias/rest"` resolve into it.
    pub fn stage(&self, store: &mut PackageStore) {
        for (alias, _dep) in &self.manifest.deps {
            let dir = git::dep_dir(&self.dir, alias);
            store.register_vendored(alias.clone(), dir);
        }
    }

    /// A fresh package store backed by the device cache.
    pub fn store(&self) -> PackageStore {
        PackageStore::with_cache_dir(lichen_language::persist::lichendir())
    }

    /// Fetch dependencies, stage them, then compile and run the lichen program
    /// at `path`, returning its rendered output.
    pub fn run(&self, path: &Path) -> Result<String, Vec<Diag>> {
        let source = std::fs::read_to_string(path).map_err(|e| {
            vec![Diag::new(
                Stage::Preprocess,
                (0, 0),
                format!("cannot read {}: {e}", path.display()),
            )]
        })?;
        self.evaluate(&source, Some(path))
    }

    /// Fetch dependencies, stage them, then compile and run `source` as a
    /// project file (its `@import` directives resolve against `base`).
    pub fn evaluate(&self, source: &str, base: Option<&Path>) -> Result<String, Vec<Diag>> {
        self.fetch_all()
            .map_err(|e| vec![Diag::new(Stage::Preprocess, (0, 0), e)])?;
        let mut store = self.store();
        self.stage(&mut store);
        lichen_language::run::evaluate_raw(source, base, &mut store)
    }

    /// Fetch dependencies, stage them, then load/freeze the package at `path`
    /// (its exports), returning the path and its rendered exported type.
    pub fn build(&self, path: &Path) -> Result<(PathBuf, String), Vec<Diag>> {
        self.fetch_all()
            .map_err(|e| vec![Diag::new(Stage::Preprocess, (0, 0), e)])?;
        let mut store = self.store();
        self.stage(&mut store);
        let handle = store.load_package(path)?;
        // The exported type: re-import the file itself and print the type, as
        // the compiler's `build` subcommand does.
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let source = format!("@{{\n  _pkg = import \"{name}\"\n@}}\n_pkg\n");
        let output = lichen_language::run::evaluate_raw(&source, Some(path), &mut store)?;
        let ty = output.split(": ").nth(1).unwrap_or(&output).to_string();
        Ok((handle.path.clone(), ty))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Dependency, Manifest, PROJECT_DIR, Package};
    use lichen_language::package::PackageStore;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lichen-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stage_registers_vendored_alias() {
        let dir = tempdir("stage");
        // A manifest dependency `foo` from git.
        let manifest = Manifest {
            package: Package {
                name: "demo".into(),
                version: None,
            },
            deps: [(
                "foo".into(),
                Dependency {
                    git: "https://example.com/foo.git".into(),
                    rev: None,
                    branch: None,
                    tag: None,
                    package: None,
                    plugin: false,
                },
            )]
            .into_iter()
            .collect(),
        };
        // The vendored clone (as if `lichen fetch` had produced it).
        let vendored = dir.join(PROJECT_DIR).join("deps").join("foo");
        std::fs::create_dir_all(&vendored).unwrap();
        std::fs::write(vendored.join("lib.lichen"), "42").unwrap();

        let project = Project {
            dir: dir.clone(),
            manifest,
        };
        let mut store = PackageStore::new();
        project.stage(&mut store);
        assert!(store.is_vendored("foo"));
        let handle = store.resolve_import(None, "foo").unwrap();
        assert_eq!(
            handle.path,
            std::fs::canonicalize(vendored.join("lib.lichen")).unwrap()
        );
    }

    #[test]
    fn no_deps_stage_is_a_noop() {
        let dir = tempdir("nodeps");
        let project = Project {
            dir: dir.clone(),
            manifest: Manifest::default(),
        };
        let mut store = PackageStore::new();
        project.stage(&mut store);
        // No aliases registered.
        assert!(!store.is_vendored("anything"));
        // A relative import still resolves on its own.
        std::fs::write(dir.join("math.lichen"), "3").unwrap();
        let handle = store
            .resolve_import(Some(&dir.join("main.lichen")), "math.lichen")
            .unwrap();
        assert_eq!(
            handle.path,
            std::fs::canonicalize(dir.join("math.lichen")).unwrap()
        );
    }
}
