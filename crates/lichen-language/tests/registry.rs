//! Integration tests for the language-registry plan: preprocessing,
//! package store, and importing frozen modules.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

mod common;

use lichen_highlevel::diagnostic::DiagKind;
use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;
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
    let main = "---x = import \"pkg.lichen\"---x\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, root_ty) = common::run_at(main, Some(&dir), &mut store);
    assert_eq!(common::usize_of(&value), 42);
    assert!(common::type_is_int(&module, root_ty));
}

#[test]
fn imports_and_applies_a_function_package() {
    let dir = temp_dir("function");
    write(&dir, "f.lichen", "x => x + 1\n");
    let main = "---f = import \"f.lichen\"---f 41\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, root_ty) = common::run_at(main, Some(&dir), &mut store);
    assert_eq!(common::usize_of(&value), 42);
    assert!(common::type_is_int(&module, root_ty));
}

#[test]
fn imports_a_struct_type_and_instantiates_it() {
    let dir = temp_dir("struct");
    write(&dir, "s.lichen", "struct<.f Int>\n");
    let main = "---s = import \"s.lichen\"---s(5,)\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, _) = common::run_at(main, Some(&dir), &mut store);
    assert_eq!(
        common::usize_of(&common::array_values(&module, &value)[0]),
        5,
        "the struct instance is the one-element tuple (5,)"
    );
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
    write(
        &dir,
        "middle.lichen",
        "---inc = import \"inner.lichen\"---x => inc x\n",
    );
    let main = "---f = import \"middle.lichen\"---f 41\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, root_ty) = common::run_at(main, Some(&dir), &mut store);
    assert_eq!(common::usize_of(&value), 42);
    assert!(common::type_is_int(&module, root_ty));
    // Both packages loaded exactly once, into the one shared registry.
    assert_eq!(store.packages.len(), 2);
}

#[test]
fn transitive_struct_types_flow_through_packages() {
    // A struct type defined in the inner package, instantiated in the
    // middle one, read by name in the importer: the nominal id and the frozen
    // type travel across two freeze boundaries.
    let dir = temp_dir("transitive-struct");
    write(&dir, "inner.lichen", "struct<.f Int>\n");
    write(
        &dir,
        "middle.lichen",
        "---S = import \"inner.lichen\"---S(41,)\n",
    );
    let main = "---v = import \"middle.lichen\"---v.f\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, root_ty) = common::run_at(main, Some(&dir), &mut store);
    assert_eq!(common::usize_of(&value), 41);
    assert!(common::type_is_int(&module, root_ty));
}

#[test]
fn diamond_imports_load_each_package_once() {
    // main imports b and c; both import a.  The store loads a once (cache),
    // so b and c share one frozen artifact of a through the shared registry.
    let dir = temp_dir("diamond");
    write(&dir, "a.lichen", "42\n");
    write(&dir, "b.lichen", "---a = import \"a.lichen\"---a + 1\n");
    write(&dir, "c.lichen", "---a = import \"a.lichen\"---a + 2\n");
    let main = "---b = import \"b.lichen\"\nc = import \"c.lichen\"---(b, c)\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, _) = common::run_at(main, Some(&dir), &mut store);
    let elements = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&elements[0]), 43);
    assert_eq!(common::usize_of(&elements[1]), 44);
    assert_eq!(
        store.packages.len(),
        3,
        "a loads once despite two importers"
    );
}

#[test]
fn circular_imports_are_diagnosed() {
    // a imports b, b imports a — the load stack re-enters a and reports the
    // cycle.  The message carries the chain (a → b → a); the caret sits on
    // the main file's own directive, the one location it can act on.
    let dir = temp_dir("cycle");
    write(&dir, "a.lichen", "---b = import \"b.lichen\"---b\n");
    write(&dir, "b.lichen", "---a = import \"a.lichen\"---a\n");
    let main = "---x = import \"a.lichen\"---x\n";
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert!(
        err.iter()
            .any(|d| d.message.contains("circular import") && d.span == Some((1, 4))),
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
    let main = "---x = import \"inner.lichen\"---x\n";
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert!(
        err.iter()
            .any(|d| d.message.contains("cannot load package 'inner.lichen'")
                && d.message.contains("unresolved name 'y'")),
        "the diagnostic names the failing package and its cause: {err:?}"
    );
    assert!(
        err.iter()
            .any(|d| d.message.contains("cannot load package") && d.span == Some((1, 4))),
        "the caret sits on the @import directive, not the package's line 2: {err:?}"
    );
}

#[test]
fn a_package_whose_last_statement_is_a_raw_read_reports_the_package_own_failure() {
    // `[1, 2]<0>` is a raw read of a runtime array: the element is not a
    // `[value, type]` pair, so the package's *own* build rejects it and the
    // import reports that.  (The export used to reach the importer as a bare
    // read operation instead of a pair and be refused there by the
    // import-export guard; the read now builds the pair every expression's term
    // is, so the failure is the honest one.)
    let dir = temp_dir("raw-export");
    write(&dir, "raw.lichen", "[1, 2]<0>\n");
    let main = "---x = import \"raw.lichen\"---x\n";
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert!(
        err.iter()
            .any(|d| d.message.contains("cannot load package 'raw.lichen'")),
        "the package's own failure is the one reported: {err:?}"
    );
    assert_eq!(
        err[0].span,
        Some((1, 4)),
        "the caret is on the import directive"
    );
}

#[test]
fn a_failure_inside_the_prelude_is_attributed_to_its_own_file() {
    // The prelude's contract is a built-in module's source, so the condition a
    // failure names belongs to *that* file: the diagnostic carries the file and
    // the position of the line that wrote the condition, which is what makes a
    // refusal navigable instead of "could not be attributed"
    // (`docs/notes/core-prelude.md`).  The module is materialized on disk when a
    // store has a cache root, and named by its own path when it has none.
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw("add [\"a\", \"b\"]\n", None, &mut store).unwrap_err();
    let attributed: Vec<_> = err.iter().filter(|d| d.file.is_some()).collect();
    assert!(
        !attributed.is_empty(),
        "a prelude failure names the file it came from: {err:?}"
    );
    let diag = attributed[0];
    let source = diag.file.as_ref().unwrap();
    assert!(
        source.path.ends_with("core.lichen"),
        "the built-in's own path: {:?}",
        source.path
    );
    let (line, _col) = diag.span.expect("a position in that file");
    assert!(
        source.code.lines().count() >= line as usize,
        "the position is inside the kept source: {line}"
    );
    // The line it names is the contract's own line — the one that spelled
    // `in_num` — not the program's.
    assert!(
        source
            .code
            .lines()
            .nth(line as usize - 1)
            .is_some_and(|text| text.contains("in_num")),
        "the position names the contract's line"
    );
}

#[test]
fn a_failed_assert_in_an_imported_package_still_reports_a_diagnostic() {
    // The imported body's assert is cloned into the importer's module with the
    // *imported* module's node as its template, so this build has no expression
    // to attribute the failure to and renders nothing for it.  The report
    // invariant — a failed build always carries a diagnostic — is what keeps
    // that from surfacing as an error with an empty diagnostic list.
    let dir = temp_dir("imported-assert");
    write(&dir, "pkg.lichen", "x => @assert (x == 1)\n");
    let main = "---f = import \"pkg.lichen\"---f 2\n";
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert_eq!(err.len(), 1, "a failure never renders as nothing: {err:?}");
    let check = err[0].check.as_ref().expect("a checker diagnostic");
    assert_eq!(check.kind, DiagKind::UnattributedFailure);
    assert!(check.loc().is_none(), "there is no expression to blame");
}

#[test]
fn an_unattributable_failure_in_a_dependency_names_the_package() {
    // Here the failing assert belongs to `b`'s own build, so the failure
    // reaches the importer through the package-load seam instead of through the
    // importer's own build.  That seam takes the load's first diagnostic, which
    // the report invariant guarantees exists.
    let dir = temp_dir("dependency-unattributed");
    write(&dir, "c.lichen", "x => @assert (x == 1)\n");
    write(&dir, "b.lichen", "---f = import \"c.lichen\"---f 2\n");
    let main = "---x = import \"b.lichen\"---x\n";
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert_eq!(
        err.len(),
        1,
        "the load's own diagnostic is the one reported: {err:?}"
    );
    assert!(
        err[0].message.contains("cannot load package 'b.lichen'")
            && err[0].message.contains("could not be attributed"),
        "the import diagnostic carries the failing build's own message: {}",
        err[0].message
    );
}

#[test]
fn a_raw_read_of_a_non_tuple_container_in_a_package_is_refused_by_kind() {
    // `[[1]]<0>` reads a component of an array — not of a type value — so the
    // package's own build refuses it where it stands, stating the tuple kind.
    // The package's own failure is the one reported, through the import: an
    // honest refusal rather than a panic inside the importer's checker.
    //
    // This used to be the out-of-bounds *slot* read (the container's element
    // was a one-element array whose type slot 1 is missing).  That shape is
    // unreachable now that the container's type must be the tuple kind: a
    // tuple-kinded value's components are always `[value, type]` pairs.
    let dir = temp_dir("short-export");
    write(&dir, "short.lichen", "[[1]]<0>\n");
    let main = "---x = import \"short.lichen\"---x\n";
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert!(
        err.iter().any(|d| d
            .message
            .contains("expected TypeTuple, found array<array<Int, 1>, 1>")),
        "the package's own failure is the one reported: {err:?}"
    );
    assert_eq!(
        err[0].span,
        Some((1, 4)),
        "the caret is on the import directive"
    );
}

#[test]
fn package_store_caches_loaded_packages() {
    let dir = temp_dir("cache");
    let pkg = write(&dir, "pkg.lichen", "42\n");
    let mut store = PackageStore::<LangProgram>::new();
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
    let mut store = PackageStore::<LangProgram>::new();
    let (_, first, _) = common::run_at(
        "---f = import \"pkg.lichen\"---f 41\n",
        Some(&dir),
        &mut store,
    );
    let (_, second, _) = common::run_at(
        "---f = import \"pkg.lichen\"---f 1\n",
        Some(&dir),
        &mut store,
    );
    assert_eq!(common::usize_of(&first), 42);
    assert_eq!(common::usize_of(&second), 2);
    assert_eq!(
        store.packages.len(),
        1,
        "the package loaded once for both files"
    );
}

#[test]
fn imported_type_error_is_reported_without_panicking() {
    let dir = temp_dir("typeerror");
    write(&dir, "n.lichen", "42\n");
    let main = "---n = import \"n.lichen\"---n 1\n";
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    assert!(
        err.iter().any(|d| d.message.contains("found Int")),
        "diagnostics should render the imported non-function type error: {err:?}"
    );
}

#[test]
fn exports_are_stored_on_the_registered_package() {
    let dir = temp_dir("exports");
    let pkg = write(&dir, "pkg.lichen", "42\n");
    let mut store = PackageStore::<LangProgram>::new();
    let handle = store.load_package(&pkg).unwrap();
    let registered = store.registry.read().unwrap();
    let package = registered.get(handle.key).unwrap();
    assert_eq!(
        package.meta.export,
        Some(handle.export),
        "the registry entry must carry the export ref for future import/disk persistence"
    );
}
