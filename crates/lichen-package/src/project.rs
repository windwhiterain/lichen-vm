//! The project: a directory of lichen programs whose dependencies are
//! declared per-file as `name = depend "url"` directives in each `@{…@}` block.
//!
//! A [`Project`] is the unit the package manager operates on.  There is no
//! project manifest: a file's `depend` directives name its git sources, the
//! package manager fetches them into the lichen-home source cache
//! ([`crate::git`]), and **compiles by delegating to the compiler binary** —
//! it never compiles a program in-process itself.  The compiler resolves each
//! `depend` against the cache it just populated (see
//! [`lichen_language::preprocess::stage_depends`]), so `import "alias"`
//! resolves into the fetched source.

use std::path::{Path, PathBuf};

use lichen_language::preprocess::{Depend, Directive, block_directives, split_block};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depends_collects_depend_directives() {
        let source = "@{\n  foo = depend \"https://example.com/foo.git\" rev = \"abc\"\n  math = import \"math.lichen\"\n@}\nfoo";
        let deps = Project::depends(source);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].url, "https://example.com/foo.git");
        assert_eq!(deps[0].name, "foo");
        assert_eq!(deps[0].rev.as_deref(), Some("abc"));
        assert!(!deps[0].plugin);
        // The source has no block → no deps.
        assert!(Project::depends("a = 1").is_empty());
    }

    #[test]
    fn plugin_depends_filters_plugin_only() {
        let source = "@{\n  foo = depend \"https://example.com/foo.git\"\n  gpu = depend \"https://example.com/gpu.git\" plugin\n@}\nx";
        let plugins = Project::plugin_depends(source);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].url, "https://example.com/gpu.git");
        assert!(plugins[0].plugin);
    }
}
