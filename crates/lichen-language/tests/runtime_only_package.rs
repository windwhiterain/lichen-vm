//! A package whose top level holds a **runtime-only** value must still run.
//!
//! This is the `P1-29` reproduction, and it is the reason the fix is "refuse the
//! cache" rather than "refuse the compile": a kernel is a process-local registry
//! handle with no on-disk form, so a package that `jit`s at its top level cannot
//! be *cached* — but the program is ordinary, valid lichen and must evaluate.
//!
//! The shape is deliberately the imported one. A top-level `jit` in a *single*
//! file never reaches the artifact codec, because only packages are frozen and
//! serialized while the main program is not; the codec only meets a kernel when
//! an importer freezes the package that holds one. An in-memory store therefore
//! exercises nothing here, which is why `readme::program_output` — and so the
//! whole examples suite — never caught the panic: the test has to go through a
//! **cache-backed** store, exactly as the CLI does.

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
         k_double = compute.jit (y => y + y)\n",
    );
    let main = write(
        &dir,
        "_.lichen",
        "---\n  order = \"1\"\n  compute = import \"compute.lichen\"\n  \
         kernels = import \"kernels.lichen\"\n  output = \"6: Int\"\n---\n\
         compute.launch kernels.k_double 3\n",
    );

    // A cache directory is what puts the artifact codec on the path: with an
    // in-memory store nothing is ever serialized, and the refusal under test is
    // never reached.
    let mut store = PackageStore::<LangProgram>::with_cache_dir(dir.join("cache"));
    let source = fs::read_to_string(&main).unwrap();
    let (_, value, _) = common::run_at(&source, Some(main.as_path()), &mut store);
    assert_eq!(
        common::usize_of(&value),
        6,
        "the program must still evaluate to its declared output"
    );

    // ...and it ran *because the refusal was taken*, not because serialization
    // happened to succeed. `kernels.lichen` is the only package in the program,
    // so a cache that holds any artifact at all means this test stopped
    // exercising the refusal.
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
