//! Device-persistence tests: the `~/.lichen` cache — cross-store round
//! trips, incremental recompilation, stable/reclaimed device keys, file-ID
//! keyed overwrite-on-recompile, explicit GC, crash recovery, and
//! corrupt-artifact self-healing.  Unlike a content-addressed cache, each
//! compiled file keeps exactly one cache slot (keyed by its file ID), so
//! recompiling a modified file overwrites it rather than accumulating.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lichen_language::diag::Stage;
use lichen_language::package::PackageStore;
use lichen_language::persist::{
    DeviceRegistry, artifact_hash, deserialize_artifact, file_id_hash, hex, sha256,
};
mod common;

use lichen_language::program::{LangProgram, LangValue};
use lichen_lowlevel::LowValue;

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lichen-persist-{name}-{}-{nonce}",
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

/// The handle of the loaded package whose path ends with `name`.
fn handle_of(
    store: &PackageStore<LangProgram>,
    name: &str,
) -> lichen_language::package::PackageHandle {
    store
        .packages
        .values()
        .find(|handle| handle.path.file_name().is_some_and(|n| n == name))
        .unwrap_or_else(|| panic!("{name} was not loaded"))
        .clone()
}

#[test]
fn the_shipping_slot_is_the_one_the_package_manager_installs_into() {
    // The compiler locates its cache root here; `lichen` (lichen-package)
    // installs the shipping toolchain into `<lichendir>/compilers/<its own
    // key>`.  Both call `lichen_utils::cache::compiler_slot_key`, and this is
    // the one test that spans the two crates: a second derivation — or the
    // same derivation over a different repository — on either side fails here.
    let installed = lichen_package::compiler_cache::key(lichen_package::DEFAULT_REPO, &[])
        .expect("the empty plugin set needs no fetched source");
    let expected = lichen_language::persist::lichendir()
        .join("compilers")
        .join(installed);
    assert_eq!(
        lichen_language::persist::shipping_cache_root(),
        expected,
        "the slot the compiler reads must be the slot the package manager writes"
    );
}

#[test]
fn cache_round_trip_across_stores() {
    // A transitive chain compiles once, then a fresh store over the same
    // cache directory loads the whole chain from disk — same output, same
    // device keys, zero compiles.
    let dir = temp_dir("roundtrip");
    write(&dir, "inner.lichen", "x => x + 1\n");
    write(
        &dir,
        "middle.lichen",
        "---inc = import \"inner.lichen\"---x => inc x\n",
    );
    let main_path = write(
        &dir,
        "main.lichen",
        "---f = import \"middle.lichen\"---f 41\n",
    );
    let cache = dir.join("cache");

    let mut store1 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let source = fs::read_to_string(&main_path).unwrap();
    let (_, value1, _) = common::run_at(&source, Some(&dir), &mut store1);
    assert_eq!(common::usize_of(&value1), 42);
    assert_eq!(
        store1.compiled, 2,
        "the first load compiles the two packages of the chain"
    );
    assert_eq!(store1.loaded_from_cache, 0);

    let mut store2 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let (_, value2, _) = common::run_at(&source, Some(&dir), &mut store2);
    assert_eq!(common::usize_of(&value2), 42);
    assert_eq!(store2.compiled, 0, "a cache hit compiles nothing");
    assert_eq!(
        store2.loaded_from_cache, 2,
        "the package chain loads from the device cache"
    );
    // Keys are stable across processes: both stores resolved the same
    // artifacts under the same device keys.
    assert_eq!(
        handle_of(&store1, "inner.lichen").key,
        handle_of(&store2, "inner.lichen").key
    );
    assert_eq!(
        handle_of(&store1, "middle.lichen").key,
        handle_of(&store2, "middle.lichen").key
    );
}

#[test]
fn incremental_recompile_only_touches_the_changed_chain() {
    // A → B → C.  Changing B recompiles B and A only; C is verified through
    // the recorded dependency graph and loads from the cache unchanged.
    let dir = temp_dir("incremental");
    write(&dir, "c.lichen", "40\n");
    write(&dir, "b.lichen", "---c = import \"c.lichen\"---c + 1\n");
    let a_path = write(&dir, "a.lichen", "---b = import \"b.lichen\"---b + 1\n");
    let cache = dir.join("cache");

    let mut store1 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let a1 = store1.load_package(&a_path).unwrap();
    assert_eq!(store1.compiled, 3);
    let c_key = handle_of(&store1, "c.lichen").key;

    write(&dir, "b.lichen", "---c = import \"c.lichen\"---c + 2\n");
    let mut store2 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let a2 = store2.load_package(&a_path).unwrap();
    assert_eq!(store2.compiled, 2, "only B and A recompile");
    assert_eq!(store2.loaded_from_cache, 1, "C loads from the cache");
    assert_eq!(
        a2.key, a1.key,
        "A's key is stable: the file ID's slot is overwritten, not reallocated"
    );
    assert_eq!(
        handle_of(&store2, "c.lichen").key,
        c_key,
        "C did not recompile"
    );
}

#[test]
fn recompile_reuses_the_device_key() {
    // Deleting every artifact file forces a recompile, but the registry
    // record — and therefore the device key — survives.
    let dir = temp_dir("rekey");
    write(&dir, "pkg.lichen", "42\n");
    let cache = dir.join("cache");
    let mut store1 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let h1 = store1.load_package(&dir.join("pkg.lichen")).unwrap();

    for entry in fs::read_dir(cache.join("artifacts")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let mut store2 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let h2 = store2.load_package(&dir.join("pkg.lichen")).unwrap();
    assert_eq!(store2.compiled, 1, "the missing artifact recompiles");
    assert_eq!(h1.key, h2.key, "the device key is stable across recompiles");
}

#[test]
fn corrupt_artifact_rebuilds_cleanly() {
    // A truncated artifact file reads as a miss and recompiles; the result
    // is identical and the file heals.
    let dir = temp_dir("corrupt");
    write(&dir, "pkg.lichen", "x => x + 1\n");
    let cache = dir.join("cache");
    let mut store1 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let h1 = store1.load_package(&dir.join("pkg.lichen")).unwrap();

    for entry in fs::read_dir(cache.join("artifacts")).unwrap() {
        fs::write(entry.unwrap().path(), b"LCHN\x00\x00\x00\x01garbage").unwrap();
    }
    let mut store2 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let h2 = store2.load_package(&dir.join("pkg.lichen")).unwrap();
    assert_eq!(store2.compiled, 1, "the corrupt artifact recompiles");
    assert_eq!(h1.key, h2.key);

    // The chain is usable again: a third store loads the healed cache.
    let mut store3 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    store3.load_package(&dir.join("pkg.lichen")).unwrap();
    assert_eq!(store3.compiled, 0, "the healed artifact loads from cache");
}

#[test]
fn identical_content_gets_separate_file_id_slots() {
    // Two paths with identical content are distinct files (distinct file IDs),
    // so each is compiled and cached in its own slot — no content dedup.
    let dir = temp_dir("nodedupe");
    write(&dir, "a.lichen", "7\n");
    write(&dir, "b.lichen", "7\n");
    let cache = dir.join("cache");
    let mut store = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let a = store.load_package(&dir.join("a.lichen")).unwrap();
    let b = store.load_package(&dir.join("b.lichen")).unwrap();
    assert_eq!(store.compiled, 2, "each file compiles separately");
    assert_ne!(a.key, b.key, "distinct files get distinct keys");
    assert_eq!(
        fs::read_dir(cache.join("artifacts")).unwrap().count(),
        2,
        "two slots, one per file ID"
    );
}

#[test]
fn a_corrupted_body_is_rejected_by_the_header_digest() {
    // The header's body digest is verified before any body field is read, so a
    // body corrupted in place is a clean miss (the store recompiles) rather
    // than a module that loads and is silently wrong.  The flipped byte is a
    // letter of a string literal: valid UTF-8, inside no length or index, so
    // no field parser can reject it — only the digest can.
    const MARKER: &str = "artifact-body-digest-marker";
    let dir = temp_dir("bodydigest");
    let path = write(&dir, "pkg.lichen", &format!("\"{MARKER}\"\n"));
    let cache = dir.join("cache");
    let mut store = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let handle = store.load_package(&path).unwrap();

    let source = fs::read_to_string(&path).unwrap();
    let file_id = fs::canonicalize(&path)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let bytes = fs::read(
        cache
            .join("artifacts")
            .join(format!("{}.module", hex(&file_id_hash(&file_id)))),
    )
    .unwrap();
    // Every source is seeded with the prelude, so a package's artifact records
    // the built-in `core` module as a dependency.  The device gives an *embedded*
    // dependency — one with no artifact record of its own, like `core` and
    // `compute` — the all-zero sentinel, on both sides of the fold, so the
    // importer's cache still hits (`DeviceRegistry::artifact_identity`).
    let prelude = store.prelude_import().unwrap();
    let hash = artifact_hash(
        sha256(source.as_bytes()),
        &[(prelude.export.module, [0u8; 32])],
    );
    let modules = HashMap::new();

    // A valid artifact the writer produced still loads: a digest read or
    // computed over the wrong bytes would break every cache load.
    let (module, export) = deserialize_artifact(&bytes, handle.key, hash, &modules)
        .expect("the artifact the writer produced must load");
    assert!(export.index < module.nodes.len());
    assert!(
        module
            .nodes
            .iter()
            .any(|node| node.value == Some(LangValue::LowValue(LowValue::Str(MARKER)))),
        "the loaded module carries the program's string literal"
    );

    // The writer emits the string literal's bytes into the body exactly once,
    // so the flip below lands in the body and changes no length or index.
    let at = bytes
        .windows(MARKER.len())
        .position(|window| window == MARKER.as_bytes())
        .expect("the string literal's bytes are in the artifact body");
    assert_eq!(
        bytes
            .windows(MARKER.len())
            .filter(|window| *window == MARKER.as_bytes())
            .count(),
        1,
        "the marker occurs once, so the flip lands in the string literal"
    );
    let mut corrupt = bytes.clone();
    corrupt[at] ^= 0x20;

    let Err(error) = deserialize_artifact(&corrupt, handle.key, hash, &modules) else {
        panic!("a corrupted body must not load");
    };
    assert!(
        error.contains("digest"),
        "the body digest rejects it, not a field parser: {error}"
    );
}

#[test]
fn gc_cleans_only_non_lichen_and_non_virtual_slots() {
    // `gc` is a "clean": it keeps artifacts whose file ID is a lichen file
    // path or a virtual lichen-file path, and removes anything else.  Since
    // `load_package` rejects non-`.lichen` files, a stray non-lichen file ID
    // can only enter the registry out-of-band (here, directly) — gc prunes it.
    let dir = temp_dir("clean");
    let keep_path = write(&dir, "keep.lichen", "42\n");
    let cache = dir.join("cache");

    // Inject a non-lichen file ID directly into the device registry.
    let mut device = DeviceRegistry::open(cache.clone());
    let (junk_key, is_new) = device.alloc("junk.txt");
    assert!(is_new);
    drop(device);

    let mut store = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let keep = store.load_package(&keep_path).unwrap();
    assert_eq!(store.compiled, 1);
    assert_eq!(fs::read_dir(cache.join("artifacts")).unwrap().count(), 1);

    let removed = store.gc();
    assert_eq!(removed, 1, "the non-lichen slot is cleaned");
    assert_eq!(
        fs::read_dir(cache.join("artifacts")).unwrap().count(),
        1,
        "the .lichen slot survives"
    );

    // A fresh store reuses the cleaned key.
    let c_path = write(&dir, "c.lichen", "9\n");
    let mut store2 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let h = store2.load_package(&c_path).unwrap();
    assert_eq!(h.key, junk_key, "the cleaned key is reused");
    drop(keep);
}

#[test]
fn non_lichen_package_is_rejected_at_load() {
    // Only `.lichen` files are packages: a non-lichen path is rejected up
    // front, so the cache invariant (file ID is a `.lichen` or `virtual:` path)
    // holds by construction.
    let dir = temp_dir("extension");
    let txt_path = write(&dir, "data.txt", "7\n");
    let mut store = PackageStore::<LangProgram>::with_cache_dir(dir.join("cache"));
    let err = store.load_package(&txt_path).unwrap_err();
    assert!(
        err.iter().any(|d| d.message.contains("only .lichen files")),
        "got: {}",
        err.iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    );
}

#[test]
fn a_missing_package_is_an_io_diagnostic_not_a_line_one_syntax_error() {
    // A path that does not exist is a filesystem failure, so it carries no
    // source position: it must not be reported as a source problem at line 1,
    // column 1 (a caret at the first character of a file that is not there).
    let dir = temp_dir("missing");
    let missing = dir.join("absent.lichen");
    let mut store = PackageStore::<LangProgram>::with_cache_dir(dir.join("cache"));
    let err = store.load_package(&missing).unwrap_err();
    let diagnostic = err.first().expect("a diagnostic");
    assert_eq!(
        diagnostic.stage,
        Stage::Io,
        "a filesystem failure is its own stage: {diagnostic:?}"
    );
    assert!(
        diagnostic.span.is_none(),
        "an I/O failure is not grounded in the source: {diagnostic:?}"
    );
    assert!(
        diagnostic.message.contains("absent.lichen"),
        "the path must survive: {}",
        diagnostic.message
    );
    let rendered = lichen_language::render::render("", diagnostic);
    assert!(
        !rendered.contains('^') && !rendered.contains("-->"),
        "an I/O failure renders without a caret:\n{rendered}"
    );
}

#[test]
fn remove_drops_one_package_and_recompiles_on_demand() {
    let dir = temp_dir("remove");
    let pkg_path = write(&dir, "pkg.lichen", "42\n");
    let cache = dir.join("cache");
    let mut store = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let h = store.load_package(&pkg_path).unwrap();
    let artifacts_before = fs::read_dir(cache.join("artifacts")).unwrap().count();

    assert!(store.remove(&pkg_path));
    assert_eq!(
        fs::read_dir(cache.join("artifacts")).unwrap().count(),
        artifacts_before - 1,
        "the artifact file is removed"
    );

    let mut store2 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let h2 = store2.load_package(&pkg_path).unwrap();
    assert_eq!(store2.compiled, 1, "a removed package recompiles");
    assert_eq!(h.key, h2.key, "its key is reclaimed and reused");
}

#[test]
fn a_crash_between_alloc_and_publish_recovers() {
    // Allocate a key and drop the registry without publishing (a crash
    // between allocation and the end of the compile).  A fresh store sees
    // the pending record as a miss, recompiles, and completes the same key.
    let dir = temp_dir("pending");
    let pkg_path = write(&dir, "pkg.lichen", "42\n");
    let cache = dir.join("cache");
    let file_id = std::fs::canonicalize(&pkg_path)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut device = DeviceRegistry::open(cache.clone());
    let (key, is_new) = device.alloc(&file_id);
    assert!(is_new);
    drop(device); // no publish, no artifact file

    let mut store = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let h = store.load_package(&pkg_path).unwrap();
    assert_eq!(store.compiled, 1, "the pending record recompiles");
    assert_eq!(h.key, key, "the pending key is completed, not reallocated");
}

#[test]
fn two_stores_share_the_device_registry() {
    // Two store instances (two processes) over one cache directory: keys
    // come from the one shared registry and the second store's artifacts
    // are served to the first.
    let dir = temp_dir("twostores");
    write(&dir, "a.lichen", "1\n");
    write(&dir, "b.lichen", "2\n");
    let cache = dir.join("cache");
    let mut store1 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    let mut store2 = PackageStore::<LangProgram>::with_cache_dir(cache.clone());

    let ha = store1.load_package(&dir.join("a.lichen")).unwrap();
    let hb = store2.load_package(&dir.join("b.lichen")).unwrap();
    assert_ne!(ha.key, hb.key, "distinct artifacts get distinct keys");

    let hb_again = store1.load_package(&dir.join("b.lichen")).unwrap();
    assert_eq!(hb.key, hb_again.key, "store1 serves store2's artifact");
    assert_eq!(store1.compiled, 1, "store1 never recompiles b");
}

#[test]
fn a_package_that_imports_an_embedded_source_verifies_across_stores() {
    // The package imports an embedded (native virtual) source instead of a file
    // on disk.  Its bytes are compiled into the compiler binary, so the
    // dependency can never change under this cache root and must not force the
    // package to recompile on every run.
    let dir = temp_dir("embedded-dep");
    let pkg_path = write(
        &dir,
        "pkg.lichen",
        "---p = import \"plug.lichen\"---p + 1\n",
    );
    let cache = dir.join("cache");

    let mut first = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    first
        .register_native("plug.lichen", "42", lichen_highlevel::no_native_ops())
        .unwrap();
    first.load_package(&pkg_path).unwrap();
    assert_eq!(first.loaded_from_cache, 0, "the first load compiles");

    let mut second = PackageStore::<LangProgram>::with_cache_dir(cache.clone());
    second
        .register_native("plug.lichen", "42", lichen_highlevel::no_native_ops())
        .unwrap();
    second.load_package(&pkg_path).unwrap();
    assert_eq!(
        second.loaded_from_cache, 1,
        "an embedded dependency cannot change, so it must not force a recompile every run"
    );
}

#[test]
fn cache_only_when_a_cache_dir_is_configured() {
    // The default store is purely in-memory: no directory, no files.
    let dir = temp_dir("nocache");
    write(&dir, "pkg.lichen", "42\n");
    let mut store = PackageStore::<LangProgram>::new();
    store.load_package(&dir.join("pkg.lichen")).unwrap();
    assert_eq!(store.compiled, 1);
    assert_eq!(store.loaded_from_cache, 0);
    assert!(store.cache_dir().is_none());
}
