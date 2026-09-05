//! The project: a directory of lichen programs whose dependencies are
//! declared per-file as `depend "url"` directives in each `@{…@}` block.
//!
//! A [`Project`] is the unit the package manager operates on.  There is no
//! project manifest: a file's `depend` directives name its git sources, the
//! package manager fetches them into the lichen-home source cache
//! ([`crate::git`]), and stages each as a vendored alias on the shared store
//! before compiling — so `import "alias"` in the same file resolves into the
//! fetched source.

use std::path::{Path, PathBuf};

use lichen_language::diag::{Diag, Stage};
use lichen_language::package::PackageStore;
use lichen_language::preprocess::{Depend, Directive, block_directives, split_block};

use crate::git;

/// A lichen project rooted at a directory.
pub struct Project {
    /// The project root (canonicalized).
    pub dir: PathBuf,
}

impl Project {
    /// Load the project rooted at `dir`.
    pub fn load(dir: &Path) -> Result<Project, String> {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        Ok(Project { dir })
    }

    /// The `depend "url"` directives declared in a source's `@{…@}` block.
    pub fn depends(source: &str) -> Vec<Depend> {
        let (interior, _) = split_block(source);
        let Some(interior) = interior else {
            return Vec::new();
        };
        block_directives(interior)
            .into_iter()
            .filter_map(|dir| match dir {
                Directive::Depend {
                    url,
                    name,
                    rev,
                    branch,
                    tag,
                    package,
                    sub,
                    plugin,
                } => Some(Depend {
                    url,
                    name,
                    rev,
                    branch,
                    tag,
                    package,
                    sub,
                    plugin,
                }),
                _ => None,
            })
            .collect()
    }

    /// The native-plugin `depend` directives in a source (those needing a
    /// compiler rebuild).
    pub fn plugin_depends(source: &str) -> Vec<Depend> {
        Self::depends(source)
            .into_iter()
            .filter(|dep| dep.plugin)
            .collect()
    }

    /// A fresh package store backed by the device cache.
    pub fn store(&self) -> PackageStore {
        PackageStore::with_cache_dir(lichen_language::persist::lichendir())
    }

    /// Fetch every `depend` into the lichen-home source cache and register
    /// each as a vendored alias with `store`.
    pub fn stage(&self, store: &mut PackageStore, depends: &[Depend]) -> Result<(), String> {
        for dep in depends {
            let alias = git::alias_of(dep);
            let dir = git::fetch(dep)?;
            store.register_vendored(alias, dir);
        }
        Ok(())
    }

    /// Fetch dependencies (from `source`'s block), stage them, then compile
    /// and run `source` as a project file (its `@import` directives resolve
    /// against `base`).
    pub fn evaluate(&self, source: &str, base: Option<&Path>) -> Result<String, Vec<Diag>> {
        let depends = Self::depends(source);
        let mut store = self.store();
        self.stage(&mut store, &depends)
            .map_err(|e| vec![Diag::new(Stage::Preprocess, (0, 0), e)])?;
        lichen_language::run::evaluate_raw(source, base, &mut store)
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

    /// Fetch dependencies, stage them, then load/freeze the package at `path`,
    /// returning the path and its rendered exported type.
    pub fn build(&self, path: &Path) -> Result<(PathBuf, String), Vec<Diag>> {
        let depends = Self::depends(&std::fs::read_to_string(path).unwrap_or_default());
        let mut store = self.store();
        self.stage(&mut store, &depends)
            .map_err(|e| vec![Diag::new(Stage::Preprocess, (0, 0), e)])?;
        let handle = store.load_package(path)?;
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
    use lichen_language::package::PackageStore;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lichen-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn depends_collects_depend_directives() {
        let source = "@{\n  depend \"https://example.com/foo.git\" as foo rev = \"abc\"\n  math = import \"math.lichen\"\n@}\nfoo";
        let deps = Project::depends(source);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].url, "https://example.com/foo.git");
        assert_eq!(deps[0].name.as_deref(), Some("foo"));
        assert_eq!(deps[0].rev.as_deref(), Some("abc"));
        assert!(!deps[0].plugin);
        // The source has no block → no deps.
        assert!(Project::depends("a = 1").is_empty());
    }

    #[test]
    fn plugin_depends_filters_plugin_only() {
        let source = "@{\n  depend \"https://example.com/foo.git\"\n  depend \"https://example.com/gpu.git\" plugin\n@}\nx";
        let plugins = Project::plugin_depends(source);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].url, "https://example.com/gpu.git");
        assert!(plugins[0].plugin);
    }
}
