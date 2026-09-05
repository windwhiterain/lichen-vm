//! Keeps the grammar `rev` in `extension.toml` honest, so it can't silently go stale.
//!
//! Zed builds the Lichen grammar from the `[grammars.lichen] rev` commit. If that commit's
//! grammar doesn't contain a node that a `.scm` query references, loading the language fails
//! with `Invalid node type "…"` (e.g. after `return` was added to the grammar but `rev` was
//! not bumped — the `0773b18` commit). This guard fails the suite instead of letting Zed break:
//!
//! 1. every `.scm` query in `languages/lichen/` and `tree-sitter-lichen/queries/` compiles
//!    against the current grammar (so the grammar has every node the queries reference), and
//! 2. the pinned `rev` is not behind the grammar/query source — a `git diff` between `rev`
//!    and `HEAD` over the grammar-defining + query paths is empty. When it is not, the
//!    message prints the `rev` to set.
//!
//! Both checks run in CI and locally via `cargo test -p lichen-language-zed`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tree_sitter::{Language, Query};

/// Files that define the grammar surface / queries, so a change to any of them needs a
/// `rev` bump. Deliberately excludes the grammar's `tests/` (test-only churn, e.g. formatting).
const GRAMMAR_PATHS: &[&str] = &[
    "tree-sitter-lichen/src",
    "tree-sitter-lichen/grammar.js",
    "tree-sitter-lichen/queries",
    "crates/lichen-language-zed/languages/lichen",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned()
}

fn scm_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "scm") {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn every_query_compiles_against_the_current_grammar() {
    let language = Language::from(tree_sitter_lichen::language());
    let root = repo_root();

    let mut files = scm_files(&root.join("crates/lichen-language-zed/languages/lichen"));
    files.extend(scm_files(&root.join("tree-sitter-lichen/queries")));
    assert!(!files.is_empty(), "no .scm query files found");

    let mut failures = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).expect("read query");
        if let Err(err) = Query::new(&language, &source) {
            failures.push(format!("{}: {}", file.display(), err));
        }
    }
    assert!(
        failures.is_empty(),
        "queries reference a node the grammar lacks:\n{}",
        failures.join("\n")
    );
}

/// The active `rev` under `[grammars.lichen]` — the first non-comment line that assigns `rev`.
fn pinned_rev() -> String {
    let manifest = repo_root().join("crates/lichen-language-zed/extension.toml");
    let text = fs::read_to_string(&manifest).expect("read extension.toml");
    text.lines()
        .find_map(|line| {
            let line = line.trim_start();
            if line.starts_with('#') {
                return None;
            }
            line.strip_prefix("rev")?
                .trim_start()
                .strip_prefix('=')?
                .trim()
                .strip_prefix('"')?
                .strip_suffix('"')
                .map(str::to_string)
        })
        .unwrap_or_else(|| panic!("no active `rev = \"…\"` under [grammars.lichen] in extension.toml"))
}

fn git(args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(args)
        .output()
        .expect("run git")
}

#[test]
fn grammar_rev_is_not_stale() {
    let rev = pinned_rev();
    assert_eq!(rev.len(), 40, "rev should be a full git SHA, got `{rev}`");

    let mut args = vec!["diff", "--quiet", &rev, "HEAD", "--"];
    args.extend_from_slice(GRAMMAR_PATHS);
    if git(&args).status.success() {
        return;
    }

    let mut hinted = vec!["log", "-1", "--format=%H", "HEAD", "--"];
    hinted.extend_from_slice(GRAMMAR_PATHS);
    let correct = String::from_utf8_lossy(&git(&hinted).stdout).trim().to_string();

    panic!(
        "the grammar `rev` in extension.toml is stale: `{rev}` is behind the current \
         grammar/query source (the last commit touching those paths is `{correct}`).\n\
         Bump it, e.g. `rev = \"{correct}\"`, then re-run Install Dev Extension."
    );
}
