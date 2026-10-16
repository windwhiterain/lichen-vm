//! Probe for the `examples` panic: names the example the binary dies on, and
//! collects the drift verdicts the panic hides. See docs/notes/examples-panic-diagnosis.md.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use lichen_tools::readme;

/// Matches `examples.rs`'s own skip list, so the probe walks the same set.
const WORK_IN_PROGRESS: &[&str] = &["compute_jit.lichen"];

#[test]
fn per_example_survey() {
    let examples: Vec<(String, PathBuf)> = readme::example_files()
        .unwrap_or_else(|e| panic!("{e}"))
        .into_iter()
        .collect();
    println!("PROBE: {} examples discovered", examples.len());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut panicked = Vec::new();
    let mut drifted = Vec::new();
    let mut clean = 0usize;
    for (name, file) in examples {
        if WORK_IN_PROGRESS.contains(&name.as_str()) {
            println!("PROBE: {name}: SKIPPED (work in progress)");
            continue;
        }
        let source = readme::read_normalized(&file);
        let declared = readme::declared_output(&source);
        let run = catch_unwind(AssertUnwindSafe(|| readme::program_output(&file, &source)));
        match (declared, run) {
            (_, Err(_)) => {
                println!("PROBE: {name}: PANICKED");
                panicked.push(name);
            }
            (None, Ok(_)) => {
                println!("PROBE: {name}: NO DECLARATION");
            }
            (Some(declared), Ok(actual)) if declared != actual => {
                println!("PROBE: {name}: DRIFT declared={declared:?} actual={actual:?}");
                drifted.push(name);
            }
            (Some(_), Ok(_)) => {
                clean += 1;
            }
        }
    }
    std::panic::set_hook(previous);
    println!("PROBE SUMMARY: clean={clean} panicked={panicked:?} drifted={drifted:?}");
}
