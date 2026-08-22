//! The example programs in `examples/programs/` are the living spec: each
//! must compile, run, and print exactly the output its `-- output:` comment
//! line promises.

use std::fs;
use std::path::PathBuf;

#[test]
fn every_example_runs_and_prints_its_output() {
    let mut files: Vec<PathBuf> = fs::read_dir("examples/programs")
        .expect("examples/programs")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lang"))
        .collect();
    files.sort();
    assert!(
        files.len() >= 10,
        "expected at least 10 example programs, found {}",
        files.len()
    );
    for file in files {
        let source = fs::read_to_string(&file).unwrap();
        let expected = source
            .lines()
            .find_map(|line| line.strip_prefix("-- output:"))
            .expect("an `-- output:` line")
            .trim();
        let actual = language::run::evaluate(&source)
            .unwrap_or_else(|diags| panic!("{} failed: {diags:?}", file.display()));
        assert_eq!(actual, expected, "{} prints the wrong output", file.display());
    }
}
