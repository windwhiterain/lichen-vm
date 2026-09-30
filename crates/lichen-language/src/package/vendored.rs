//! Vendored dependency directories: an import path's first segment resolved to
//! a directory the host registered, and that directory's entry package file.

use super::*;
/// Split a potential vendored alias from an import path: `"foo"` →
/// `("foo", None)`, `"foo/rest"` → `("foo", Some("rest"))`.  A leading
/// segment that ends in `.lichen` is a file name, not an alias (so a relative
/// import like `"math.lichen"` never hits the vendored map).
pub(super) fn vendored_alias(import_path: &str) -> Option<(&str, Option<&str>)> {
    let (first, rest) = match import_path.find('/') {
        Some(i) => (&import_path[..i], Some(&import_path[i + 1..])),
        None => (import_path, None),
    };
    if first.is_empty() || first.ends_with(".lichen") {
        return None;
    }
    Some((first, rest))
}

/// The entry package file of a vendored dependency directory: a `_.lichen`,
/// then `<alias>.lichen`, then the directory's sole `.lichen` file.  An
/// ambiguous (many) or absent package is a diagnostic, not a guess.
pub(super) fn vendored_entry_file<P: lichen_lowlevel::Program>(
    dir: &Path,
    alias: &str,
) -> Result<PathBuf, Diag<P>> {
    let lib = dir.join("_.lichen");
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
        // A directory that cannot be read at all is a filesystem failure, not
        // "this dependency has no entry package": keep the error.
        Err(e) => {
            return Err(Diag::io(format!(
                "cannot read vendored dependency '{alias}' at {}: {e}",
                dir.display()
            )));
        }
    };
    files.sort();
    match files.len() {
        1 => Ok(files.into_iter().next().expect("one file")),
        0 => Err(Diag::unattributed(
            Stage::Preprocess,
            format!(
                "vendored dependency '{alias}' has no .lichen entry package (no _.lichen, \
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
            Err(Diag::unattributed(
                Stage::Preprocess,
                format!(
                    "vendored dependency '{alias}' is ambiguous: pick one of {names} (or add a _.lichen)"
                ),
            ))
        }
    }
}

#[cfg(test)]
mod vendored_tests {
    use super::*;
    use crate::program::LangProgram;

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
        std::fs::write(foo.join("_.lichen"), "42").unwrap();
        let mut store = PackageStore::<LangProgram>::new();
        store.register_vendored("foo", foo.clone());
        let handle = store.resolve_import(None, "foo").unwrap();
        assert_eq!(
            handle.path,
            std::fs::canonicalize(foo.join("_.lichen")).unwrap()
        );
    }

    #[test]
    fn vendored_alias_resolves_subpath() {
        let dir = tempdir("sub");
        let foo = dir.join("deps").join("foo");
        std::fs::create_dir_all(&foo).unwrap();
        std::fs::write(foo.join("_.lichen"), "1").unwrap();
        std::fs::write(foo.join("other.lichen"), "2").unwrap();
        let mut store = PackageStore::<LangProgram>::new();
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
        let mut store = PackageStore::<LangProgram>::new();
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
        let mut store = PackageStore::<LangProgram>::new();
        store.register_vendored("foo", foo);
        let err = store.resolve_import(None, "foo").unwrap_err();
        assert!(err.message.contains("ambiguous"), "{}", err.message);
    }
}
