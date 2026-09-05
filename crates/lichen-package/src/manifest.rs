//! The project manifest: `package` metadata plus the dependency set the
//! package manager resolves.  It lives at `.lichen/package.toml` in the
//! project root (or `package.toml`, searched in that order).
//!
//! Dependencies are git-sourced and recorded as a map from the import alias
//! to a [`Dependency`] describing where to fetch it.  A dependency is either
//! a *lic* package (`.lichen` files, resolved onto the preprocessor import
//! path) or a *native plugin* (a Rust crate that extends the compiler's
//! vocabulary — see [`crate::plugin`]), which additionally triggers a
//! compiler rebuild.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The project manifest file name, relative to the project root.
pub const MANIFEST_NAME: &str = "package.toml";

/// The directory under the project root that holds the manifest and the
/// vendored dependencies.
pub const PROJECT_DIR: &str = ".lichen";

/// The manifest: package metadata + dependencies.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Manifest {
    #[serde(default)]
    pub package: Package,
    /// The dependency set, keyed by import alias.
    #[serde(default, rename = "dependencies")]
    pub deps: BTreeMap<String, Dependency>,
}

/// The `[package]` table: a name and optional version.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Package {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A git-sourced dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// The git repository URL.
    pub git: String,
    /// A pinned revision, branch, or tag to check out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The Rust crate's package name, for a native-plugin dependency (used
    /// when generating the compiler rebuild).  Defaults to the alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// A native plugin: extending the compiler's value/operator vocabulary,
    /// so importing it requires the compiler to be rebuilt.
    #[serde(default)]
    pub plugin: bool,
}

impl Dependency {
    /// The revision specifier git should check out: an explicit `rev`, else
    /// `branch`, else `tag`.  `None` leaves the clone's default HEAD.
    pub fn checkout(&self) -> Option<&str> {
        self.rev
            .as_deref()
            .or(self.branch.as_deref())
            .or(self.tag.as_deref())
    }

    /// The Rust crate package name a native-plugin dependency is built under.
    pub fn crate_name(&self, alias: &str) -> String {
        self.package.clone().unwrap_or_else(|| alias.to_string())
    }

    /// Whether this dependency is a native plugin (needs a compiler rebuild).
    pub fn is_plugin(&self) -> bool {
        self.plugin
    }
}

impl Manifest {
    /// Load a manifest from `dir`, searching `.lichen/package.toml` then
    /// `package.toml`.  A missing file is a default (empty) manifest — the
    /// package manager treats a project without one as a plain directory of
    /// programs.  A *malformed* manifest is an error (it is surfaced to the
    /// user, not silently replaced with an empty one).
    pub fn load(dir: &Path) -> Result<Manifest, String> {
        match Self::path_in(dir) {
            Some(path) => {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
            }
            None => Ok(Manifest::default()),
        }
    }

    /// Save a manifest to `dir/.lichen/package.toml`, creating the dir.
    pub fn save(&self, dir: &Path) -> Result<PathBuf, String> {
        let lichen_dir = dir.join(PROJECT_DIR);
        std::fs::create_dir_all(&lichen_dir)
            .map_err(|e| format!("cannot create {}: {e}", lichen_dir.display()))?;
        let path = lichen_dir.join(MANIFEST_NAME);
        let text =
            toml::to_string_pretty(self).map_err(|e| format!("cannot serialize manifest: {e}"))?;
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }

    /// The manifest path inside `dir`, if one exists.
    pub fn path_in(dir: &Path) -> Option<PathBuf> {
        let nested = dir.join(PROJECT_DIR).join(MANIFEST_NAME);
        if nested.is_file() {
            return Some(nested);
        }
        let flat = dir.join(MANIFEST_NAME);
        flat.is_file().then_some(flat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lichen-manifest-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempdir("round-trip");
        let mut manifest = Manifest {
            package: Package {
                name: "demo".into(),
                version: Some("0.1.0".into()),
            },
            deps: Default::default(),
        };
        manifest.deps.insert(
            "foo".into(),
            Dependency {
                git: "https://example.com/foo.git".into(),
                rev: Some("abc123".into()),
                branch: None,
                tag: None,
                package: Some("foo-crate".into()),
                plugin: true,
            },
        );
        let path = manifest.save(&dir).unwrap();
        assert!(path.is_file());
        let loaded = Manifest::load(&dir).unwrap();
        assert_eq!(loaded.package.name, "demo");
        assert_eq!(loaded.deps.len(), 1);
        let dep = &loaded.deps["foo"];
        assert_eq!(dep.git, "https://example.com/foo.git");
        assert_eq!(dep.rev.as_deref(), Some("abc123"));
        assert_eq!(dep.crate_name("foo"), "foo-crate");
        assert!(dep.is_plugin());
        // The `version` field round-trips too.
        assert_eq!(loaded.package.version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn checkout_prefers_rev_then_branch_then_tag() {
        let dep = Dependency {
            git: "x".into(),
            rev: Some("r".into()),
            branch: Some("b".into()),
            tag: Some("t".into()),
            package: None,
            plugin: false,
        };
        assert_eq!(dep.checkout(), Some("r"));
        let dep = Dependency {
            git: "x".into(),
            rev: None,
            branch: Some("b".into()),
            tag: Some("t".into()),
            package: None,
            plugin: false,
        };
        assert_eq!(dep.checkout(), Some("b"));
        let dep = Dependency {
            git: "x".into(),
            rev: None,
            branch: None,
            tag: Some("t".into()),
            package: None,
            plugin: false,
        };
        assert_eq!(dep.checkout(), Some("t"));
    }
}
