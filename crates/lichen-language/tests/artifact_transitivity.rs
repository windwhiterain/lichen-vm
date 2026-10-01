//! The artifact cache must be transitive: an importer's frozen artifact is only
//! safe to serve while every dependency it was compiled against still holds the
//! same content.
//!
//! The hazard is that a **recompile reuses the device key** (`DeviceRegistry::alloc`
//! — the key names the cache slot, not the content behind it), so a dependency's
//! key survives its own change.  An importer's identity therefore has to fold in
//! each dependency's *identity*, not its key: its frozen artifact is full of
//! cross-module node references written as `(dependency key, index)`, and serving
//! it after a dependency changed resolves those against the dependency's new node
//! layout.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram as P;

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lichen-artifact-{name}-{}-{nonce}",
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

/// Load the package `path` through a fresh store backed by `cache` and report
/// `(compiled, loaded from the artifact cache)`.
///
/// The pair is what identifies one package: loading an importer also loads its
/// dependency, so a bare "was anything cached" answer is the wrong question.
/// `geometry` importing `math` makes the two counts distinguishable — the
/// dependency is a hit on every line below, so `compiled == 1` is `geometry`
/// itself.
fn load(cache: &Path, path: &Path) -> (usize, usize) {
    let mut store = PackageStore::<P>::with_cache_dir(cache.to_path_buf());
    store.load_package(path).expect("the package loads");
    (store.compiled, store.loaded_from_cache)
}

/// The importer's frozen artifact holds node references *into its dependency's
/// module*, written as `(dependency key, index)`.  When the dependency's content
/// changes but its key is reused, those indices name different nodes — the
/// importer's own bytes never changed, so only the identity can catch it.
#[test]
fn a_dependency_change_rekeys_its_importer() {
    let root = temp_dir("rekey");
    let pkg = root.join("pkg");
    let cache = root.join("cache");
    fs::create_dir_all(&pkg).unwrap();

    // `geometry` imports `math` and calls one of its functions, so its frozen
    // module carries references into `math`'s.
    let math = write(
        &pkg,
        "math.lichen",
        "succ = x => x + 1\nadd = x => y => x + y\n",
    );
    let geometry = write(
        &pkg,
        "geometry.lichen",
        "@{\n  math = import \"math.lichen\"\n@}\ndouble = x => math.add x x\n",
    );

    // First compile: everything is fresh.
    let mut store = PackageStore::<P>::with_cache_dir(cache.clone());
    store.load_package(&math).expect("math compiles");
    store.load_package(&geometry).expect("geometry compiles");
    assert_eq!(store.compiled, 2, "a cold cache compiles both packages");
    drop(store);

    // Nothing changed: both are served from their frozen artifacts.
    assert_eq!(load(&cache, &math), (0, 1), "math is a cache hit");
    assert_eq!(
        load(&cache, &geometry),
        (0, 2),
        "the importer and its dependency are both cache hits"
    );

    // The dependency's content changes.  Its own bytes differ, so it recompiles
    // — under the *same* key, because a recompile overwrites one slot.
    write(
        &pkg,
        "math.lichen",
        "succ = x => x + 1\nadd = x => y => x + y\nextra = 7\n",
    );
    assert_eq!(
        load(&cache, &math),
        (1, 0),
        "the changed dependency recompiles"
    );
    assert_eq!(
        load(&cache, &geometry),
        (1, 1),
        "the importer's own bytes never changed, but its dependency's did — \
         serving its frozen artifact would resolve cross-module node references \
         against the dependency's new node layout"
    );

    // And it converges: one recompile, then both are cache hits again.
    assert_eq!(load(&cache, &math), (0, 1), "math settles");
    assert_eq!(
        load(&cache, &geometry),
        (0, 2),
        "geometry settles after its one recompile"
    );
}
