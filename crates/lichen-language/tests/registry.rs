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
fn an_imported_deferred_instantiation_points_at_the_argument_the_caller_passed() {
    // `f`'s body defers the named instantiation `s(.x 1)` — the callee is the
    // lambda's parameter, so it resolves only when a concrete argument arrives
    // — and the miss is recorded on a per-apply clone the importer's checker
    // never saw.  A frozen template's nodes are not this module's, so the clone
    // cannot name its template; its origin is the apply that materialized it,
    // and `Build::apply_edges` turns that into the argument expression the
    // caller passed.  The argument is the bare name `S`, whose use *is* the
    // binder's own expression (`compile.rs`), so the caret sits on `S`'s
    // binding — exactly where a runtime parameter-check failure of a name
    // argument points today.  The imported file's own `.x 1` is not reachable:
    // an ordinary package keeps no source record.
    let dir = temp_dir("imported-deferred");
    write(&dir, "f.lichen", "s => s(.x 1)\n");
    let main = "---f = import \"f.lichen\"---\nS = struct<.y Int>\nf (S)\n";
    let mut store = PackageStore::<LangProgram>::new();
    let err = evaluate_raw(main, Some(&dir), &mut store).unwrap_err();
    let diag = err.first().expect("a refusal");
    assert_eq!(
        diag.check.as_ref().expect("a checker diagnostic").kind,
        DiagKind::TableMiss,
        "{err:?}"
    );
    assert_eq!(
        diag.span,
        Some((2, 5)),
        "the caret is on the argument the caller passed: {err:?}"
    );
}

#[test]
fn an_imported_placeholder_instantiation_resolves_at_the_apply() {
    // `_(.x 1, .y 2)` is an instantiation whose callee is the placeholder, so
    // the struct type is unknown where it stands: later unification with the
    // imported parameter's annotation decides it.  The callee is a static
    // (imported) function, so the deferred instantiation crosses a materialize
    // walk of the frozen body and must still wake when the apply binds the
    // parameter's type cell (`docs/language-spec.md` §3, the deferred
    // instantiation).  Asserted structurally: the instance's own field values
    // in definition order, and the resolved type's fields read back through
    // `type_of` — a struct type's field list and names.  The nominal id is per
    // occurrence, so the importer cannot name it.
    let dir = temp_dir("imported-placeholder");
    write(&dir, "f.lichen", "f = x: struct<.x Int, .y Int> => x\n");
    let main = "---f = import \"f.lichen\"---\ntype_of = x => {t = _; x: t; t}\nv = f.f (_(.x 1, .y 2))\nT = type_of v\n(v, T::x == Int, T::y == Int)\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, _) = common::run_at(main, Some(&dir), &mut store);
    let parts = common::array_values(&module, &value);
    let fields = common::array_values(&module, &parts[0]);
    assert_eq!(common::usize_of(&fields[0]), 1, "the `.x` argument");
    assert_eq!(common::usize_of(&fields[1]), 2, "the `.y` argument");
    assert_eq!(
        common::usize_of(&parts[1]),
        1,
        "the instance's type has field `.x`, typed Int"
    );
    assert_eq!(
        common::usize_of(&parts[2]),
        1,
        "the instance's type has field `.y`, typed Int"
    );
}

#[test]
fn two_applies_of_one_imported_placeholder_instantiation_stay_distinct() {
    // One static function applied twice, each call with its own argument
    // values: the two materialized clones must resolve their placeholder
    // instantiations independently — the first call's binding must not stand
    // in for the second's.
    let dir = temp_dir("imported-placeholder-twice");
    write(&dir, "f.lichen", "f = x: struct<.x Int, .y Int> => x\n");
    let main = "---f = import \"f.lichen\"---(f.f (_(.x 1, .y 2)), f.f (_(.x 3, .y 4)))\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, _) = common::run_at(main, Some(&dir), &mut store);
    let calls = common::array_values(&module, &value);
    let first = common::array_values(&module, &calls[0]);
    let second = common::array_values(&module, &calls[1]);
    assert_eq!(common::usize_of(&first[0]), 1);
    assert_eq!(common::usize_of(&first[1]), 2);
    assert_eq!(common::usize_of(&second[0]), 3);
    assert_eq!(common::usize_of(&second[1]), 4);
}

#[test]
fn a_placeholder_instantiation_deferred_inside_an_imported_body_resolves_at_the_apply() {
    // Here the instantiation stands *in* the imported body, and the annotation
    // that decides its type comes after the read that uses it, so the package
    // freezes a genuinely unresolved instantiation that the apply's
    // materialized clone must resolve.  The instance is a **static** ref into
    // the frozen module — the body's array is proven concrete and referenced
    // in place — so the fields are read *in the program* and the resulting
    // dynamic tuple carries the values: `common::array_values` refuses a
    // static-backed array by design ("language arrays are dynamic").
    let dir = temp_dir("imported-body-deferred");
    write(
        &dir,
        "g.lichen",
        "g = s => { a = _(.x 1, .y 2)\n  p = a : struct<.x Int, .y Int>\n  a }\n",
    );
    let main = "---g = import \"g.lichen\"---\ntype_of = x => {t = _; x: t; t}\nv = g.g 0\nT = type_of v\n(v.x, v.y, T::x == Int, T::y == Int)\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, _) = common::run_at(main, Some(&dir), &mut store);
    let parts = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&parts[0]), 1, "the `.x` argument");
    assert_eq!(common::usize_of(&parts[1]), 2, "the `.y` argument");
    assert_eq!(
        common::usize_of(&parts[2]),
        1,
        "the instance's type has field `.x`, typed Int"
    );
    assert_eq!(
        common::usize_of(&parts[3]),
        1,
        "the instance's type has field `.y`, typed Int"
    );
}

#[test]
fn two_applies_of_one_imported_function_resolve_their_own_struct_type() {
    // The shared-clone hazard: the deferred instantiation's cells live in the
    // *frozen* body, so if a per-apply clone reused the first call's bound
    // cells, two calls resolving **different** struct types through one
    // imported function would contaminate each other.  The imported body's
    // parameter is left open and the caller supplies the type, so each call
    // decides the instantiation for itself.  `struct<.x Int, .y Int>` reads
    // the arguments in `.x, .y` order while `struct<.y Int, .x Int>` reverses
    // them, so the instances' own definition-order values are `(1, 2)` and
    // `(2, 1)`: the per-call type is visible in the value, and a leaked clone
    // would give `(1, 2)` twice, or refuse the second call.
    let dir = temp_dir("static-clone-hazard");
    write(
        &dir,
        "f.lichen",
        "f = x => { a = _(.x 1, .y 2)\n  p = a : x\n  a }\n",
    );
    let main = "---f = import \"f.lichen\"---\nS = struct<.x Int, .y Int>\nT = struct<.y Int, .x Int>\n(f.f (S), f.f (T))\n";
    let mut store = PackageStore::<LangProgram>::new();
    let (module, value, _) = common::run_at(main, Some(&dir), &mut store);
    let calls = common::array_values(&module, &value);
    let first = common::array_values(&module, &calls[0]);
    let second = common::array_values(&module, &calls[1]);
    assert_eq!(common::usize_of(&first[0]), 1, "the first call's `.x`");
    assert_eq!(common::usize_of(&first[1]), 2, "the first call's `.y`");
    assert_eq!(
        common::usize_of(&second[0]),
        2,
        "the second call's `.y` leads its definition order"
    );
    assert_eq!(
        common::usize_of(&second[1]),
        1,
        "the second call's `.x` follows it"
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
