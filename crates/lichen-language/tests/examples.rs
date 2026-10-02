//! The example programs in `examples/` — at any depth, including
//! each directory's `_.lichen` — are the living spec: each must compile and
//! run, and each must **print what it declares**.  The `output = "..."`
//! metadata in every file is a claim about observable behaviour, and this
//! suite checks it: a program whose output drifts from its declaration fails
//! here rather than having the declaration quietly rewritten.  Programs run
//! through a package store with their own path as the base, so `@import`
//! lines resolve relative to the file.
//!
//! Deliberately *not* self-healing, unlike the README's embedded section (see
//! `tests/readme.rs`, which rewrites the generated blob because that is
//! documentation).  The difference is the point: the blob is derived
//! documentation, while a program's output is the language's behaviour, and a
//! behaviour change belongs in a reviewable diff — update the `output =`
//! entry in the same commit, or run the `sync-readme` binary on demand.

use std::path::PathBuf;

use lichen_tools::readme;

#[test]
fn every_example_runs_and_prints_what_it_declares() {
    let files: Vec<PathBuf> = readme::example_files()
        .unwrap_or_else(|e| panic!("{e}"))
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
    let mut drifted = Vec::new();
    for file in files {
        let source = readme::read_normalized(&file);
        let declared = readme::declared_output(&source).unwrap_or_else(|| {
            panic!(
                "{}: declares no `output = \"...\"` in its ---...--- block, so nothing \
                 states what it prints",
                file.display()
            )
        });
        // Runs the program, panicking with its rendered diagnostics on failure.
        let actual = readme::program_output(&file, &source);
        if declared != actual {
            drifted.push(format!(
                "{}:\n  declared: {declared:?}\n  actual:   {actual:?}",
                file.display()
            ));
        }
    }
    assert!(
        drifted.is_empty(),
        "an example's output no longer matches its `output =` declaration — the \
         language's observable behaviour changed.  If that was intended, update \
         the declaration in the same commit (or run the sync-readme binary); \
         otherwise it is a regression.\n{}",
        drifted.join("\n")
    );
}
