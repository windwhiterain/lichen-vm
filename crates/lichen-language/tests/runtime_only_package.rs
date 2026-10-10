//! A package holding a live kernel must still run: the refusal is the cache, not
//! the compile. See artifact-cache.md.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

mod common;

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("lichen-{name}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn an_imported_package_that_jits_at_its_top_level_still_runs() {
    let dir = temp_dir("runtime-only");
    // The package that cannot be cached: its top level names a live kernel.
    write(
        &dir,
        "kernels.lichen",
        "---\n  order = \"0\"\n  compute = import \"compute.lichen\"\n  output = \"Function\"\n---\n\
         k_double = compute.jit (y : Int => y + y)\n",
    );
    let main = write(
        &dir,
        "_.lichen",
        "---\n  order = \"1\"\n  compute = import \"compute.lichen\"\n  \
         kernels = import \"kernels.lichen\"\n  output = \"6: Int\"\n---\n\
         compute.launch kernels.k_double 3\n",
    );

    // A cache directory puts the codec on the path: nothing is ever serialized.

    // ...so the refusal under test is never reached.
    let mut store = PackageStore::<LangProgram>::with_cache_dir(dir.join("cache"));
    let source = fs::read_to_string(&main).unwrap();
    let (_, value, _) = common::run_at(&source, Some(main.as_path()), &mut store);
    assert_eq!(
        common::usize_of(&value),
        6,
        "the program must still evaluate to its declared output"
    );

    // Any cached artifact means the refusal went unexercised: `kernels.lichen` is
    // the program's only package.
    let artifacts = dir.join("cache").join("artifacts");
    let cached: Vec<PathBuf> = match fs::read_dir(&artifacts) {
        Ok(entries) => entries.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => Vec::new(),
    };
    assert!(
        cached.is_empty(),
        "the package holding a live kernel must be left uncached; found {cached:?}"
    );
}
