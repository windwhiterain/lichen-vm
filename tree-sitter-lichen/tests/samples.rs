//! Every `.lichen` sample must parse without ERROR nodes; run this crate by
//! its own manifest (not a workspace member).

use std::fs;
use std::path::{Path, PathBuf};

use tree_sitter::{Language, Parser};

fn language() -> Language {
    Language::from(tree_sitter_lichen::language())
}

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&language())
        .expect("set lichen grammar");
    parser
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
        root.join("../examples"),
        root.join("../crates/lichen-tools/tests/fixtures/readme"),
    ]
}

#[test]
fn samples_parse_without_error_nodes() {
    let mut parser = parser();

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
    // One case per current surface form; see `docs/language-spec.md` §2.
    let mut parser = parser();

    let cases = [
        "",
        "5",
        "a = 1\nb = a + 2\nb",
        "f = x => x\nf 1",
        "if x then a else b",
        "let a = 1\na",
        "t = tuple{e, f}\nt",
        "s = struct{.name e, .other f}\ns",
        "a = array{T, 3}\na",
        "k = set{E}\nk",
        "b = table{K, V}\nb",
        "c = A -> B\nc",
        "p = (a, b)\np",
        "q = [a, b]\nq",
        "r = S(.a 1, .b 2)\nr",
        "u = set(a, b)\nu",
        "v = table(k1 ==> v1)\nv",
        "w = { return 1 }\nw",
        "x = @any\nx",
        "y = : T # a\ny",
        "z = [1, @shallow 2, @shallow(3) 4]\nz",
        "n = tuple{Int, string}{0}\nn",
        "m = S::a\nm",
        "o = t <== k\no",
        "g = v @in set(Int, Float)\ng",
        "h = v ! (p => p > 3)\nh",
        "i = v ~ \"payload\"\ni",
        "j = v ? doc\nj",
        "k2 = v # 4\nk2",
        "type_of = x => {t = _; x: t; t}\ntype_of 1",
        "b2 = { pub a = 1; a }\nb2",
        "@loop f = 1\nf",
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

#[test]
fn retired_spellings_have_error_nodes() {
    // The retired angle spellings must not parse; see `docs/notes/operators.md` §10.
    let mut parser = parser();

    let cases = [
        "s = struct<.x Int>",
        "a = array<Int, 3>",
        "b = x : <Int, Int>",
        "c = f<A, B>",
        "d = a <Int, string>",
    ];

    for (idx, src) in cases.iter().enumerate() {
        let tree = parser.parse(src, None).expect("parse");
        assert!(
            tree.root_node().has_error(),
            "retired spelling #{idx} must not parse:\n---\n{src}\n---\n{}",
            tree.root_node().to_sexp()
        );
    }
}
