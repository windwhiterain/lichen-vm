//! The reported unclosed-delimiter case, on the example it came from: line 174.

use super::*;
use lichen_language_lex::lex;
use std::path::PathBuf;

/// The example, with one `)` removed from `.at (s(0) - 1)`.
fn broken_example() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
        .join("decode_loop.lichen");
    let source = std::fs::read_to_string(&path).expect("the example is readable");
    let needle = ".at (s(0) - 1)";
    let at = source
        .find(needle)
        .expect("the example writes .at (s(0) - 1)");
    let mut broken = String::with_capacity(source.len() - 1);
    broken.push_str(&source[..at + ".at (s(0) - 1".len()]);
    broken.push_str(&source[at + needle.len()..]);
    broken
}

/// Every diagnostic, numbered, with the line it points at.
fn print_diagnostics(label: &str, source: &str) {
    println!("--- {label} ---");
    let tokens = lex(source).tokens;
    for (index, diag) in parse(&tokens).errors.iter().enumerate() {
        println!("[{index}] {:?}", diag.span);
        println!("     {}", diag.message);
    }
}

#[test]
fn unclosed_delimiter_probe() {
    print_diagnostics("after the fix", &broken_example());

    // The same shape in miniature, so the probe does not depend on the
    // example's other contents.
    print_diagnostics("miniature", "x = .at (s(0) - 1\n");
}
