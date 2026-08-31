//! The language CLI. `lichen <program.lichen>` compiles and runs one program,
//! printing its output; a directory path runs every `.lichen` file in it,
//! printing `file: output` per program.  Newer-style `lichen run`/`build`
//! subcommands are also accepted.
//!
//! Install it with `cargo install --path crates/lichen-language` (from a
//! checkout of this repo) or `cargo install --git <repo-url> lichen-language`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: lichen [run|build] <program.lichen | directory>";

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
        "cache" => {
            if args.next().as_deref() != Some("gc") || args.next().is_some() {
                eprintln!("usage: lichen cache gc");
                return ExitCode::FAILURE;
            }
            cache_gc()
        }
        "run" => {
            let Some(path) = args.next() else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            if args.next().is_some() {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            run_path(&PathBuf::from(path))
        }
        "build" => {
            let Some(path) = args.next() else {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            };
            if args.next().is_some() {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            build_file(&PathBuf::from(path))
        }
        path_arg => {
            if args.next().is_some() {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            run_path(&PathBuf::from(path_arg))
        }
    }
}

fn run_path(path: &Path) -> ExitCode {
    if path.is_dir() {
        run_directory(path)
    } else {
        run_file(path)
    }
}

/// `lichen cache gc`: explicitly reclaim every artifact in the device cache
/// that no live source chain references.
fn cache_gc() -> ExitCode {
    let dir = lichen_language::persist::lichendir();
    let mut store = lichen_language::package::PackageStore::with_cache_dir(dir.clone());
    let removed = store.gc();
    println!(
        "reclaimed {removed} cached artifact(s) from {}",
        dir.display()
    );
    ExitCode::SUCCESS
}

fn run_file(path: &Path) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let mut store = lichen_language::package::PackageStore::with_cache_dir(
        lichen_language::persist::lichendir(),
    );
    match lichen_language::run::evaluate_raw(&source, Some(path), &mut store) {
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
        let mut store = lichen_language::package::PackageStore::with_cache_dir(
            lichen_language::persist::lichendir(),
        );
        match lichen_language::run::evaluate_raw(&source, Some(&file), &mut store) {
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

fn build_file(path: &Path) -> ExitCode {
    let mut store = lichen_language::package::PackageStore::with_cache_dir(
        lichen_language::persist::lichendir(),
    );
    match store.load_package(path) {
        Ok(handle) => {
            println!("built {}", handle.path.display());
            // The plan's build is a prototyping command: the package was
            // loaded/frozen.  Its value's type is rendered via a tiny import
            // of the same file, which exercises the real importer path and
            // prints the exported type.  The import names the file itself,
            // resolved against the file's directory.
            let name = path.file_name().unwrap().to_string_lossy();
            let source = format!("@import \"{name}\" as _pkg;\n_pkg\n");
            match lichen_language::run::evaluate_raw(&source, Some(path), &mut store) {
                Ok(output) => println!("type: {}", output.split(": ").nth(1).unwrap_or(&output)),
                Err(diags) => {
                    print!("{}", lichen_language::render::render_all(&source, &diags));
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(diags) => {
            let source = std::fs::read_to_string(path).unwrap_or_default();
            print!("{}", lichen_language::render::render_all(&source, &diags));
            ExitCode::FAILURE
        }
    }
}
