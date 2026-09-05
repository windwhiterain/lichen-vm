//! Parse every sample `.lichen` program and assert the grammar produces no
//! ERROR nodes.  This exercises `src/parser.c` through the `tree-sitter`
//! runtime and the `tree-sitter-lichen` Rust binding.

use std::fs;
use std::path::{Path, PathBuf};

use tree_sitter::{Language, Parser};

fn language() -> Language {
    Language::from(tree_sitter_lichen::language())
}

fn walk(dir: &Path, acc: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, acc);
            } else if path.extension().is_some_and(|e| e == "lichen") {
                acc.push(path);
            }
        }
    }
}

fn sample_dirs() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vec![
        root.join("../crates/lichen-language/examples/programs"),
        root.join("../crates/lichen-language/tests/fixtures/readme"),
    ]
}

#[test]
fn samples_parse_without_error_nodes() {
    let mut parser = Parser::new();
    parser
        .set_language(&language())
        .expect("set lichen grammar");

    let mut files = Vec::new();
    for dir in sample_dirs() {
        walk(&dir, &mut files);
    }
    assert!(!files.is_empty(), "no sample .lichen files found");

    let mut failures = Vec::new();
    for file in &files {
        let src = fs::read_to_string(file).expect("read sample");
        let tree = parser.parse(&src, None).expect("parse");
        if tree.root_node().has_error() {
            failures.push(file.display().to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "these samples had ERROR nodes:\n{}",
        failures.join("\n")
    );
}

#[test]
fn edge_cases_parse_without_error_nodes() {
    let mut parser = Parser::new();
    parser
        .set_language(&language())
        .expect("set lichen grammar");

    let cases = [
        "",                               // empty file
        "5",                              // bare expression
        "a = 1\nb = a + 2\nb",            // no preprocess block, bindings + expr
        "f = x => x\nf 1",                // lambda + application
        "if x then a else b",             // conditional
        "let a = 1\na",                   // restrictive binding
        "T = struct<.x Int, .y Type>\nT", // named struct fields
        "t = table{}\nt",                 // empty constant table
        "f = type_of\ng = x => type_of x\nf 1", // `type_of` first-class function
        "v = 5 # 8 ? doc\n{ return v }\nv",     // `?` doc annotation + `return` block tail
        "b = { pub a = 1; a }\nb",               // `pub`-marked block statement
    ];

    for (idx, src) in cases.iter().enumerate() {
        let tree = parser.parse(src, None).expect("parse");
        assert!(
            !tree.root_node().has_error(),
            "edge case #{idx} should parse cleanly:\n---\n{src}\n---\n{}",
            tree.root_node().to_sexp()
        );
    }
}
