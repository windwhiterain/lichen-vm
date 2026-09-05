//! The example programs in `examples/programs/` — at any depth, including
//! each directory's `_.lichen` — are the living spec: each must compile and
//! run.  The top-level README embeds each program's whole source file, whose
//! `output =` metadata is rewritten to its *actual* output (computed by the
//! same runner used here, see `src/readme.rs`), so the metadata in the files
//! is documentation kept current, not a hand-written promise.  Programs run
//! through a package store with their own path as the base, so `@import`
//! lines resolve relative to the file.

use std::fs;
use std::path::PathBuf;

use lichen_language::package::PackageStore;
use lichen_language::program::{LangOperator, LangValue};
use lichen_language::readme;
use lichen_language::run::evaluate_raw;

#[test]
fn every_example_runs() {
    let files: Vec<PathBuf> = readme::example_files()
        .into_iter()
        .map(|(_, file)| file)
        .collect();
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
        let mut store = PackageStore::<LangValue, LangOperator>::new();
        evaluate_raw(&source, Some(&file), &mut store)
            .unwrap_or_else(|diags| panic!("{} failed: {diags:?}", file.display()));
    }
}
