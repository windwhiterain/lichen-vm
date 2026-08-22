//! The example programs in `examples/programs/` are the living spec: each
//! must compile and run.  The top-level README embeds each program and its
//! *actual* output — computed by the same runner used here (see
//! `src/readme.rs`) — so the `-- output:` comments in the files are
//! documentation, not the source of truth.

use std::fs;
use std::path::PathBuf;

#[test]
fn every_example_runs() {
    let mut files: Vec<PathBuf> = fs::read_dir("examples/programs")
        .expect("examples/programs")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lichen"))
        .collect();
    files.sort();
    // A sanity floor, not an exact count: examples may be merged (e.g.
    // struct_instance.lichen folded into structs.lichen) as long as the
    // set stays a reasonable living spec.
    assert!(
        files.len() >= 9,
        "expected at least 9 example programs, found {}",
        files.len()
    );
    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        lichen_language::run::evaluate(&source)
            .unwrap_or_else(|diags| panic!("{} failed: {diags:?}", file.display()));
    }
}
