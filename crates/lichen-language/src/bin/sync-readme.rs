//! Regenerate the example section of the top-level README from
//! `examples/programs/`.
//!
//! Run with: `cargo run -p lichen-language --bin sync-readme`
//!
//! The section lives between the `<!-- begin: examples -->` and
//! `<!-- end: examples -->` markers; only that region is rewritten, so the
//! heading and lead-in around it stay as they are.  Idempotent: running it
//! twice changes nothing.  `tests/readme.rs` resyncs the README in place on
//! drift, so this command is only needed to commit the result of an example
//! change right away.

use std::fs;
use std::process::ExitCode;

use lichen_language::readme;

fn main() -> ExitCode {
    if readme::sync_output_comments() {
        println!("updated example output comments");
    }
    let blob = readme::render_examples();
    let path = readme::readme_path();
    let content = readme::read_normalized(&path);
    match readme::replace_examples(&content, &blob) {
        Ok(updated) if updated != content => match fs::write(&path, updated) {
            Ok(()) => {
                println!("updated {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                ExitCode::FAILURE
            }
        },
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}
