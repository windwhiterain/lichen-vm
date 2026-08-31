//! Integration tests for the language-registry plan: preprocessing,
//! package store, and importing frozen modules.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lichen_language::package::PackageStore;
use lichen_language::run::evaluate_raw;

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lichen-registry-{name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn imports_an_integer_package() {
    let dir = temp_dir("integer");
    write(&dir, "pkg.lichen", "42\n");
    let main = "@import \"pkg.lichen\" as x;\nx\n";
    let mut store = PackageStore::new();
    let out = evaluate_raw(main, Some(&dir), &mut store).unwrap();
    assert_eq!(out, "42: Int");
}

#[test]
fn imports_and_applies_a_function_package() {
    let dir = temp_dir("function");
    write(&dir, "f.lichen", "x => x + 1\n");
    let main = "@import \"f.lichen\" as f;\nf 41\n";
    let mut store = PackageStore::new();
    let out = evaluate_raw(main, Some(&dir), &mut store).unwrap();
    assert_eq!(out, "42: Int");
}

#[test]
fn imports_a_struct_type_and_instantiates_it() {
    let dir = temp_dir("struct");
    write(&dir, "s.lichen", "struct<Int>\n");
    let main = "@import \"s.lichen\" as s;\ns(5,)\n";
    let mut store = PackageStore::new();
    let out = evaluate_raw(main, Some(&dir), &mut store).unwrap();
    assert_eq!(out, "(5,): struct<Int>");
}

#[test]
fn transitive_imports_apply_across_modules() {
    // inner → middle → main: middle imports inner and exports a function
    // whose body applies the import; the main file applies middle's export.
    // The apply path materializes middle's template, whose baked values
    // reference inner's module — cross-module refs carried verbatim through
    // middle's freeze.
    let dir = temp_dir("transitive");
    write(&dir, "inner.lichen", "x => x + 1\n");
    write(&dir, "middle.lichen", "@import \"inner.lichen\" as inc\nx => inc x\n");
    let main = "@import \"middle.lichen\" as f\nf 41\n";
    let mut store = PackageStore::new();
    let out = evaluate_raw(main, Some(&dir), &mut store).unwrap();
    assert_eq!(out, "42: Int");
    // Both packages loaded exactly once, into the one shared registry.
    assert_eq!(store.packages.len(), 2);
}

#[test]
fn transitive_struct_types_flow_through_packages() {
    // A struct type defined in the inner package, instantiated in the
    // middle one, indexed in the importer: the nominal id and the frozen
    // type travel across two freeze boundaries.
    let dir = temp_dir("transitive-struct");
    write(&dir, "inner.lichen", "struct<Int>\n");
    write(&dir, "middle.lichen", "@import \"inner.lichen\" as S\nS(41,)\n");
    let main = "@import \"middle.lichen\" as v\nv(0)\n";
    let mut store = PackageStore::new();
    let out = evaluate_raw(main, Some(&dir), &mut store).unwrap();
    assert_eq!(out, "41: Int");
}

#[test]
fn diamond_imports_load_each_package_once() {
    // main imports b and c; both import a.  The store loads a once (cache),
    // so b and c share one frozen artifact of a through the shared registry.
    let dir = temp_dir("diamond");
    write(&dir, "a.lichen", "42\n");
    write(&dir, "b.lichen", "@import \"a.lichen\" as a\na + 1\n");
    write(&dir, "c.lichen", "@import \"a.lichen\" as a\na + 2\n");
    let main = "@import \"b.lichen\" as b\n@import \"c.lichen\" as c\n(b, c)\n";
    let mut store = PackageStore::new();
    let out = evaluate_raw(main, Some(&dir), &mut store).unwrap();
    assert_eq!(out, "(43, 44): <Int, Int>");
    assert_eq!(store.packages.len(), 3, "a loads once despite two importers");
}

#[test]
fn circular_imports_are_diagnosed() {
    // a imports b, b imports a — the load stack re-enters a and reports the
    // cycle.  The message carries the chain (a → b → a); the caret sits on
    // the main file's own directive, the one location it can act on.
    let dir = temp_dir("cycle");
    write(&dir, "a.lichen", "@import \"b.lichen\" as b\nb\n");
    write(&dir, "b.lichen", "@import \"a.lichen\" as a\na\n");
    let main = "@import \"a.lichen\" as x\nx\n";
    let mut store = PackageStore::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert!(
        err.iter()
            .any(|d| d.message.contains("circular import") && d.span == Some((1, 1))),
        "the cycle must be diagnosed at the main file's directive: {err:?}"
    );
}

#[test]
fn a_failing_dependency_is_reported_at_the_import_directive() {
    // inner fails to resolve `y` at its own line 2; the main file's
    // diagnostic points at its own @import line (not inner's coordinates)
    // and names the package that failed to load.
    let dir = temp_dir("failing-dep");
    write(&dir, "inner.lichen", "42\ny\n");
    let main = "@import \"inner.lichen\" as x\nx\n";
    let mut store = PackageStore::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert!(
        err.iter().any(|d| d.message.contains("cannot load package 'inner.lichen'")
            && d.message.contains("unresolved name 'y'")),
        "the diagnostic names the failing package and its cause: {err:?}"
    );
    assert!(
        err.iter().any(|d| d.message.contains("cannot load package") && d.span == Some((1, 1))),
        "the caret sits on the @import directive, not the package's line 2: {err:?}"
    );
}

#[test]
fn package_store_caches_loaded_packages() {
    let dir = temp_dir("cache");
    let pkg = write(&dir, "pkg.lichen", "42\n");
    let mut store = PackageStore::new();
    let a = store.load_package(&pkg).unwrap();
    let b = store.load_package(&pkg).unwrap();
    assert_eq!(a.key, b.key);
    assert_eq!(a.export, b.export);
}

#[test]
fn two_importers_share_one_package_through_one_store() {
    // Two files importing the same package through one store: the package
    // freezes once, and both importer modules resolve its refs through the
    // same registry key.
    let dir = temp_dir("shared");
    write(&dir, "pkg.lichen", "x => x + 1\n");
    let mut store = PackageStore::new();
    let first = evaluate_raw("@import \"pkg.lichen\" as f\nf 41\n", Some(&dir), &mut store).unwrap();
    let second = evaluate_raw("@import \"pkg.lichen\" as f\nf 1\n", Some(&dir), &mut store).unwrap();
    assert_eq!(first, "42: Int");
    assert_eq!(second, "2: Int");
    assert_eq!(store.packages.len(), 1, "the package loaded once for both files");
}

#[test]
fn imported_type_error_is_reported_without_panicking() {
    let dir = temp_dir("typeerror");
    write(&dir, "n.lichen", "42\n");
    let main = "@import \"n.lichen\" as n;\nn 1\n";
    let mut store = PackageStore::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert!(
        err.iter()
            .any(|d| d.message.contains("found Int")),
        "diagnostics should render the imported non-function type error: {err:?}"
    );
}

#[test]
fn exports_are_stored_on_the_registered_package() {
    let dir = temp_dir("exports");
    let pkg = write(&dir, "pkg.lichen", "42\n");
    let mut store = PackageStore::new();
    let handle = store.load_package(&pkg).unwrap();
    let registered = store.registry.read().unwrap();
    let package = registered.get(handle.key).unwrap();
    assert_eq!(
        package.meta.export,
        Some(handle.export),
        "the registry entry must carry the export ref for future import/disk persistence"
    );
}
