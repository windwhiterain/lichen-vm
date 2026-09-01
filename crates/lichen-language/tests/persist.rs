//! Device-persistence tests: the `~/.lichen` cache — cross-store round
//! trips, incremental recompilation, stable/reclaimed device keys, content
//! dedup, explicit GC, crash recovery, and corrupt-artifact self-healing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lichen_language::package::PackageStore;
use lichen_language::persist::DeviceRegistry;
use lichen_language::run::evaluate_raw;

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
fn handle_of(store: &PackageStore, name: &str) -> lichen_language::package::PackageHandle {
    store
        .packages
        .values()
        .find(|handle| handle.path.file_name().is_some_and(|n| n == name))
        .unwrap_or_else(|| panic!("{name} was not loaded"))
        .clone()
}

#[test]
fn cache_round_trip_across_stores() {
    // A transitive chain compiles once, then a fresh store over the same
    // cache directory loads the whole chain from disk — same output, same
    // device keys, zero compiles.
    let dir = temp_dir("roundtrip");
    write(&dir, "inner.lichen", "x => x + 1\n");
    write(&dir, "middle.lichen", "@{inc = import \"inner.lichen\"@}x => inc x\n");
    let main_path = write(&dir, "main.lichen", "@{f = import \"middle.lichen\"@}f 41\n");
    let cache = dir.join("cache");

    let mut store1 = PackageStore::with_cache_dir(cache.clone());
    let source = fs::read_to_string(&main_path).unwrap();
    let out1 = evaluate_raw(&source, Some(&dir), &mut store1).unwrap();
    assert_eq!(out1, "42: Int");
    assert_eq!(
        store1.compiled, 2,
        "the first load compiles the two packages of the chain"
    );
    assert_eq!(store1.loaded_from_cache, 0);

    let mut store2 = PackageStore::with_cache_dir(cache.clone());
    let out2 = evaluate_raw(&source, Some(&dir), &mut store2).unwrap();
    assert_eq!(out2, "42: Int");
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
    write(&dir, "b.lichen", "@{c = import \"c.lichen\"@}c + 1\n");
    let a_path = write(&dir, "a.lichen", "@{b = import \"b.lichen\"@}b + 1\n");
    let cache = dir.join("cache");

    let mut store1 = PackageStore::with_cache_dir(cache.clone());
    let a1 = store1.load_package(&a_path).unwrap();
    assert_eq!(store1.compiled, 3);
    let c_key = handle_of(&store1, "c.lichen").key;

    write(&dir, "b.lichen", "@{c = import \"c.lichen\"@}c + 2\n");
    let mut store2 = PackageStore::with_cache_dir(cache.clone());
    let a2 = store2.load_package(&a_path).unwrap();
    assert_eq!(store2.compiled, 2, "only B and A recompile");
    assert_eq!(store2.loaded_from_cache, 1, "C loads from the cache");
    assert_ne!(a2.key, a1.key, "A's transitive content changed → a new key");
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
    let mut store1 = PackageStore::with_cache_dir(cache.clone());
    let h1 = store1.load_package(&dir.join("pkg.lichen")).unwrap();

    for entry in fs::read_dir(cache.join("artifacts")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let mut store2 = PackageStore::with_cache_dir(cache.clone());
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
    let mut store1 = PackageStore::with_cache_dir(cache.clone());
    let h1 = store1.load_package(&dir.join("pkg.lichen")).unwrap();

    for entry in fs::read_dir(cache.join("artifacts")).unwrap() {
        fs::write(entry.unwrap().path(), b"LCHN\x00\x00\x00\x01garbage").unwrap();
    }
    let mut store2 = PackageStore::with_cache_dir(cache.clone());
    let h2 = store2.load_package(&dir.join("pkg.lichen")).unwrap();
    assert_eq!(store2.compiled, 1, "the corrupt artifact recompiles");
    assert_eq!(h1.key, h2.key);

    // The chain is usable again: a third store loads the healed cache.
    let mut store3 = PackageStore::with_cache_dir(cache.clone());
    store3.load_package(&dir.join("pkg.lichen")).unwrap();
    assert_eq!(store3.compiled, 0, "the healed artifact loads from cache");
}

#[test]
fn identical_content_shares_one_device_key() {
    // Two paths with identical content are one artifact: one compile, one
    // key, two aliases.
    let dir = temp_dir("dedupe");
    write(&dir, "a.lichen", "7\n");
    write(&dir, "b.lichen", "7\n");
    let cache = dir.join("cache");
    let mut store = PackageStore::with_cache_dir(cache.clone());
    let a = store.load_package(&dir.join("a.lichen")).unwrap();
    let b = store.load_package(&dir.join("b.lichen")).unwrap();
    assert_eq!(store.compiled, 1, "the second path reuses the first artifact");
    assert_eq!(a.key, b.key);
}

#[test]
fn gc_reclaims_orphans_and_reuses_keys() {
    // A → B.  Changing B orphans the old B and old A entries; the explicit
    // `gc` reclaims exactly those two (keys and artifact files), and the
    // next fresh compile reuses the smallest reclaimed key.
    let dir = temp_dir("gc");
    write(&dir, "b.lichen", "1\n");
    let a_path = write(&dir, "a.lichen", "@{b = import \"b.lichen\"@}b\n");
    let cache = dir.join("cache");
    let mut store = PackageStore::with_cache_dir(cache.clone());
    store.load_package(&a_path).unwrap();
    let artifacts_before = fs::read_dir(cache.join("artifacts")).unwrap().count();
    assert_eq!(artifacts_before, 2);

    write(&dir, "b.lichen", "2\n");
    let mut store2 = PackageStore::with_cache_dir(cache.clone());
    store2.load_package(&a_path).unwrap();
    let artifacts_now = fs::read_dir(cache.join("artifacts")).unwrap().count();
    assert_eq!(artifacts_now, 4, "the new chain compiles alongside the old");

    let removed = store2.gc();
    assert_eq!(removed, 2, "the two orphaned artifacts are reclaimed");
    assert_eq!(
        fs::read_dir(cache.join("artifacts")).unwrap().count(),
        2,
        "their files are deleted"
    );

    // The reclaimed keys are reused: the fresh package takes key 0 (the
    // smallest freed index — B compiled first, so B was key 0).
    write(&dir, "c.lichen", "9\n");
    let h = store2.load_package(&dir.join("c.lichen")).unwrap();
    assert_eq!(h.key.as_raw(), 0, "a reclaimed key is reused");
}

#[test]
fn remove_drops_one_package_and_recompiles_on_demand() {
    let dir = temp_dir("remove");
    let pkg_path = write(&dir, "pkg.lichen", "42\n");
    let cache = dir.join("cache");
    let mut store = PackageStore::with_cache_dir(cache.clone());
    let h = store.load_package(&pkg_path).unwrap();
    let artifacts_before = fs::read_dir(cache.join("artifacts")).unwrap().count();

    assert!(store.remove(&pkg_path));
    assert_eq!(
        fs::read_dir(cache.join("artifacts")).unwrap().count(),
        artifacts_before - 1,
        "the artifact file is removed"
    );

    let mut store2 = PackageStore::with_cache_dir(cache.clone());
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
    write(&dir, "pkg.lichen", "42\n");
    let cache = dir.join("cache");
    let hash = lichen_language::persist::artifact_hash(b"42\n", &[]);
    let mut device = DeviceRegistry::open(cache.clone());
    let (key, is_new) = device.alloc(hash);
    assert!(is_new);
    drop(device); // no publish, no artifact file

    let mut store = PackageStore::with_cache_dir(cache.clone());
    let h = store.load_package(&dir.join("pkg.lichen")).unwrap();
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
    let mut store1 = PackageStore::with_cache_dir(cache.clone());
    let mut store2 = PackageStore::with_cache_dir(cache.clone());

    let ha = store1.load_package(&dir.join("a.lichen")).unwrap();
    let hb = store2.load_package(&dir.join("b.lichen")).unwrap();
    assert_ne!(ha.key, hb.key, "distinct artifacts get distinct keys");

    let hb_again = store1.load_package(&dir.join("b.lichen")).unwrap();
    assert_eq!(hb.key, hb_again.key, "store1 serves store2's artifact");
    assert_eq!(store1.compiled, 1, "store1 never recompiles b");
}

#[test]
fn cache_only_when_a_cache_dir_is_configured() {
    // The default store is purely in-memory: no directory, no files.
    let dir = temp_dir("nocache");
    write(&dir, "pkg.lichen", "42\n");
    let mut store = PackageStore::new();
    store.load_package(&dir.join("pkg.lichen")).unwrap();
    assert_eq!(store.compiled, 1);
    assert_eq!(store.loaded_from_cache, 0);
    assert!(store.cache_dir().is_none());
}