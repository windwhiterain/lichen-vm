//! The compiler CLI, shared by the real `lichen-compiler` binary and the
//! plugin-built compiler crate (its generated `main.rs` calls [`main`]) so
//! every compiler speaks the same dialect.
//!
//! `lichen-compiler <program.lichen>` compiles and runs one program, printing
//! its output; a directory path runs every `.lichen` file in it, printing
//! `file: output` per program.  The `run`, `build`, and `cache gc` subcommands
//! are also accepted.
//!
//! The compiler is **depend-aware**: a file's `depend "url"` directives
//! resolve against the lichen-home source cache (populated by the package
//! manager's `lichen fetch`), so running a file with dependencies needs no
//! git access here — the compiler only *reads* what the package manager put in
//! the cache.  The compiler binary is invoked by the package manager for its
//! `run`/`build` commands, which is how a plugin-built compiler's vocabulary
//! takes effect.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::package::PackageStore;
use crate::persist;
use crate::preprocess::stage_depends;

/// Run the compiler CLI with the process arguments.  The program name is read
/// from `argv[0]` so the plugin-built `lichen-compiler-<name>` reports its own
/// name in usage.
pub fn main() -> ExitCode {
    let mut args = std::env::args();
    let bin = args
        .next()
        .map(|p| {
            PathBuf::from(&p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "lichen-compiler".to_string())
        })
        .unwrap_or_else(|| "lichen-compiler".to_string());
    let usage = format!("usage: {bin} [run|build|cache gc] <program.lichen | directory>");

    let Some(arg) = args.next() else {
        eprintln!("{usage}");
        return ExitCode::FAILURE;
    };
    match arg.as_str() {
        "-h" | "--help" => {
            println!("{usage}");
            ExitCode::SUCCESS
        }
        "-V" | "--version" => {
            println!("lichen-compiler {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "cache" => {
            if args.next().as_deref() != Some("gc") || args.next().is_some() {
                eprintln!("{usage}");
                return ExitCode::FAILURE;
            }
            cache_gc()
        }
        "run" => {
            let Some(path) = args.next() else {
                eprintln!("{usage}");
                return ExitCode::FAILURE;
            };
            if args.next().is_some() {
                eprintln!("{usage}");
                return ExitCode::FAILURE;
            }
            run_path(&PathBuf::from(path))
        }
        "build" => {
            let Some(path) = args.next() else {
                eprintln!("{usage}");
                return ExitCode::FAILURE;
            };
            if args.next().is_some() {
                eprintln!("{usage}");
                return ExitCode::FAILURE;
            }
            build_file(&PathBuf::from(path))
        }
        path_arg => {
            if args.next().is_some() {
                eprintln!("{usage}");
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

/// `cache gc`: explicitly reclaim every artifact in the device cache that no
/// live source chain references.
fn cache_gc() -> ExitCode {
    let dir = persist::lichendir();
    let mut store = PackageStore::with_cache_dir(dir.clone());
    let removed = store.gc();
    println!(
        "reclaimed {removed} cached artifact(s) from {}",
        dir.display()
    );
    ExitCode::SUCCESS
}

/// A store that stages the file's `depend` directives from the source cache
/// and reports a diagnostic when one has not been fetched.
fn staged_store(source: &str) -> (PackageStore, Vec<crate::diag::Diag>) {
    let mut store = PackageStore::with_cache_dir(persist::lichendir());
    let diags = stage_depends(&mut store, source);
    (store, diags)
}

fn run_file(path: &Path) -> ExitCode {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let (mut store, diags) = staged_store(&source);
    if !diags.is_empty() {
        print!("{}", crate::render::render_all(&source, &diags));
        return ExitCode::FAILURE;
    }
    match crate::run::evaluate_raw(&source, Some(path), &mut store) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(diags) => {
            print!("{}", crate::render::render_all(&source, &diags));
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
        let (mut store, diags) = staged_store(&source);
        if !diags.is_empty() {
            failed += 1;
            eprintln!("{}: failed to stage dependencies", file.display());
            print!("{}", crate::render::render_all(&source, &diags));
            continue;
        }
        match crate::run::evaluate_raw(&source, Some(&file), &mut store) {
            Ok(output) => {
                println!("{}: {output}", file.file_name().unwrap().to_string_lossy())
            }
            Err(diags) => {
                failed += 1;
                eprintln!("{}: failed", file.display());
                print!("{}", crate::render::render_all(&source, &diags));
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
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let (mut store, diags) = staged_store(&source);
    if !diags.is_empty() {
        print!("{}", crate::render::render_all(&source, &diags));
        return ExitCode::FAILURE;
    }
    match store.load_package(path) {
        Ok(handle) => {
            println!("built {}", handle.path.display());
            // The build command is a prototyping command: the package was
            // loaded/frozen.  Its value's type is rendered via a tiny import
            // of the same file, which exercises the real importer path and
            // prints the exported type.  The import names the file itself,
            // resolved against the file's directory.
            let name = path.file_name().unwrap().to_string_lossy();
            let source = format!("@{{\n  _pkg = import \"{name}\"\n@}}\n_pkg\n");
            match crate::run::evaluate_raw(&source, Some(path), &mut store) {
                Ok(output) => println!("type: {}", output.split(": ").nth(1).unwrap_or(&output)),
                Err(diags) => {
                    print!("{}", crate::render::render_all(&source, &diags));
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(diags) => {
            print!("{}", crate::render::render_all(&source, &diags));
            ExitCode::FAILURE
        }
    }
}
