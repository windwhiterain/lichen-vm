//! Regenerate the README example section and each example's `output =` value,
//! on demand.  See docs/notes/readme-sync.md.

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

/// Rewrite the `output =` metadata, render the tree, and splice the blob into
/// the README.
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
