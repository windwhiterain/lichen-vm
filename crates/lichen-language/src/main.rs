//! The language CLI. `lichen <program.lichen>` compiles and runs one program,
//! printing its output; a directory path runs every `.lichen` file in it,
//! printing `file: output` per program.
//!
//! Install it with `cargo install --path crates/lichen-language` (from a
//! checkout of this repo) or `cargo install --git <repo-url> lichen-language`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: lichen <program.lichen | directory>";

fn main() -> ExitCode {
    let mut args = std::env::args();
    let _program = args.next();
    let Some(arg) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    match arg.as_str() {
        "-h" | "--help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "-V" | "--version" => {
            println!("lichen {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        path_arg => {
            if args.next().is_some() {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            let path = PathBuf::from(path_arg);
            if path.is_dir() {
                run_directory(&path)
            } else {
                run_file(&path)
            }
        }
    }
}

fn run_file(path: &Path) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match lichen_language::run::evaluate(&source) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(diags) => {
            print!("{}", lichen_language::render::render_all(&source, &diags));
            ExitCode::FAILURE
        }
    }
}

fn run_directory(dir: &Path) -> ExitCode {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension() == Some(OsStr::new("lichen")))
            .collect(),
        Err(e) => {
            eprintln!("cannot read {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    files.sort();
    let mut failed = 0;
    for file in files {
        let source = match std::fs::read_to_string(&file) {
            Ok(source) => source,
            Err(e) => {
                eprintln!("{}: cannot read: {e}", file.display());
                failed += 1;
                continue;
            }
        };
        match lichen_language::run::evaluate(&source) {
            Ok(output) => {
                println!("{}: {output}", file.file_name().unwrap().to_string_lossy())
            }
            Err(diags) => {
                failed += 1;
                eprintln!("{}: failed", file.display());
                print!("{}", lichen_language::render::render_all(&source, &diags));
            }
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
