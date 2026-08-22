//! Regenerate the example section of the top-level README from
//! `examples/programs/`.
//!
//! Run with: `cargo run -p lichen-language --bin sync-readme`
//!
//! The section lives between the `<!-- begin: examples -->` and
//! `<!-- end: examples -->` markers; only that region is rewritten, so the
//! heading and lead-in around it stay as they are.  Idempotent: running it
//! twice changes nothing.  `tests/readme.rs` enforces it stays in sync, so
//! commit the result of this command together with any example change.

use std::fs;
use std::process::ExitCode;

use lichen_language::readme;

fn main() -> ExitCode {
    let blob = readme::render_examples();
    let path = readme::readme_path();
    let content = readme::read_normalized(&path);
    match readme::replace_examples(&content, &blob) {
        Ok(updated) => match fs::write(&path, updated) {
            Ok(()) => {
                println!("updated {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}
