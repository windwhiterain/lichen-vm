//! Every example must compile, run, and print the `output = "..."` it declares.
//! See readme-sync.md.

use std::path::PathBuf;

use lichen_tools::readme;

/// `compute_jit` is parked: `Launch` states no type, so the launch result's type
/// stays an open cell.
const WORK_IN_PROGRESS: &[&str] = &["compute_jit.lichen"];

#[test]
fn every_example_runs_and_prints_what_it_declares() {
    // The name is the path under `examples/`, so a skip names one directory's
    // `_.lichen` without catching the others'.
    let examples: Vec<(String, PathBuf)> = readme::example_files()
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .collect();
    // A sanity floor, not an exact count: examples may be merged as long as the
    // set stays a reasonable living spec.
    assert!(
        examples.len() >= 9,
        "expected at least 9 example programs, found {}",
        examples.len()
    );
    let mut drifted = Vec::new();
    for (name, file) in examples {
        let skip = WORK_IN_PROGRESS.contains(&name.as_str());
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
