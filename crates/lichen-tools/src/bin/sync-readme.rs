//! Regenerate the example section of the top-level README from
//! `examples/`.
//!
//! Run with: `cargo run -p lichen-tools --bin sync-readme`
//!
//! The section lives between the `<!-- begin: examples -->` and
//! `<!-- end: examples -->` markers; only that region is rewritten, so the
//! heading and lead-in around it stay as they are.  Each example's `output =`
//! metadata is also rewritten to its actual output first, so the README
//! embeds the whole file as it stands.  Idempotent: running it twice changes
//! nothing.  `tests/readme.rs` resyncs the README in place on drift, so this
//! command is only needed to commit the result of an example change right
//! away.
//!
//! Run outside the repository — where the compile-time
//! `CARGO_MANIFEST_DIR`-relative `examples/` does not exist — it prints the
//! unreadable path and exits non-zero instead of panicking.

use std::fs;
use std::process::ExitCode;

use lichen_tools::readme;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// The whole command: rewrite the `output =` metadata, render the tree, splice
/// the blob into the README and write it back when it changed.  `Err` is the
/// diagnostic [`main`] prints.
fn run() -> readme::ReadmeResult<()> {
    if readme::sync_output_comments()? {
        println!("updated example output metadata");
    }
    let blob = readme::render_examples()?;
    let path = readme::readme_path();
    let content = readme::read_normalized(&path);
    let updated = readme::replace_examples(&content, &blob)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if updated != content {
        fs::write(&path, updated).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("updated {}", path.display());
    }
    Ok(())
}
