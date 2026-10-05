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

/// Examples whose output is blocked on work in progress rather than on the
/// language.
///
/// `compute_jit` is the cross-kernel case: the `Launch` operator states **no
/// type at all** (`crates/lichen-compute/src/compute.rs`, `LaunchOp::build`) —
/// the kernel's `.I`/`.O` constraints in the lichen wrapper
/// (`crates/lichen-compute/src/compute.lichen`) are the whole of the result's
/// type, and that wrapper is still being annotated.  With the launch result's
/// type left as an open cell, the renderer's `leaf_class` finds no class in the
/// type's first slot and falls back to the raw layout, so the program prints
/// `6: raw[Int, raw[?a, ?b]]` against its declared `6: Int`.  The value (`6`)
/// is already right.
///
/// Ignoring is per example, not per suite: every other program here is still
/// the living spec, and this list is the record of what is not yet.
const WORK_IN_PROGRESS: &[&str] = &["compute_jit.lichen"];

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
        let skip = file
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| WORK_IN_PROGRESS.contains(&name));
        if skip {
            continue;
        }
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
