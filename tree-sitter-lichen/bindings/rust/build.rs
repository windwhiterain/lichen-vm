use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    // The generated parser is a build output of `grammar.js`, not committed.
    regenerate_if_stale(&manifest_dir);

    let src_dir = manifest_dir.join("src");
    let mut config = cc::Build::new();
    config.include(&src_dir);
    config
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-but-set-variable")
        .flag_if_supported("-Wno-unused-variable");
    config.file(src_dir.join("parser.c"));
    config.compile("tree-sitter-lichen");
}

/// (Re)generate the parser from `grammar.js` when it is missing or stale.
fn regenerate_if_stale(manifest_dir: &Path) {
    // Watch the grammar and `parser.c`; watching all of `src/` would
    // rebuild after every regeneration.
    println!("cargo:rerun-if-changed=grammar.js");
    println!("cargo:rerun-if-changed=src/parser.c");

    let grammar = manifest_dir.join("grammar.js");
    let parser = manifest_dir.join("src").join("parser.c");

    let outdated = match (grammar.metadata(), parser.metadata()) {
        (Ok(g), Ok(p)) => match (g.modified().ok(), p.modified().ok()) {
            (Some(gm), Some(pm)) => gm > pm,
            _ => true,
        },
        _ => true, // either file absent -> generate
    };

    if !outdated {
        return;
    }

    let cli = find_tree_sitter_cli(manifest_dir).unwrap_or_else(|| {
        panic!(
            "\nThe Tree-sitter CLI was not found. The generated parser is not committed, so it \
             must be regenerated from `grammar.js` before building.\n\
             Install it (e.g. `cargo install tree-sitter-cli --version 0.25`) or put `tree-sitter` \
             on PATH; keep it on the same major line as the `tree-sitter` runtime crate, i.e. 0.25.\n"
        )
    });

    let output = run_generate(&cli, manifest_dir).expect("spawn tree-sitter generate");
    if !output.status.success() {
        panic!(
            "\n`tree-sitter generate` failed (running `{}`):\n{}\n",
            cli.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Invoke `tree-sitter generate` in `manifest_dir`, handling the Windows `.cmd`
/// shim that npm installs for the CLI.
fn run_generate(cli: &Path, manifest_dir: &Path) -> std::io::Result<Output> {
    #[cfg(windows)]
    {
        if cli
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd"))
        {
            return Command::new("cmd")
                .arg("/c")
                .arg(cli)
                .arg("generate")
                .current_dir(manifest_dir)
                .output();
        }
    }
    Command::new(cli)
        .arg("generate")
        .current_dir(manifest_dir)
        .output()
}

/// Locate a runnable `tree-sitter` CLI: on `$PATH`, else under
/// `node_modules/.bin` (an npm-installed grammar).
fn find_tree_sitter_cli(manifest_dir: &Path) -> Option<PathBuf> {
    // Prefer a `tree-sitter` on PATH (cargo-installed or otherwise).
    if Command::new("tree-sitter")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some()
    {
        return Some(PathBuf::from("tree-sitter"));
    }
    // Fallback: an npm-installed bin (Windows `.cmd` shim or native exe).
    let bin = manifest_dir.join("node_modules").join(".bin");
    for name in ["tree-sitter.cmd", "tree-sitter.exe", "tree-sitter"] {
        let candidate = bin.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
