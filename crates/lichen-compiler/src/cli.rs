//! The compiler CLI, shared by the shipping and plugin-built compilers.  See
//! docs/notes/language-toolchain.md.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_language::LangProgramShape;
use lichen_language::package::{PackageStore, Wiring};
use lichen_language::persist::{self, ArtifactCodec};
use lichen_language::preprocess::stage_depends;
use lichen_language::program::GcdOp;
use lichen_language::run::{PassReport, RunOutput};

pub use lichen_language::package::NativePackage;

/// The compiler CLI surface: a positional program path, or an explicit
/// `run`/`build` subcommand.
#[derive(Parser)]
#[command(name = "lichen-compiler", version)]
struct Cli {
    /// A program to run when no subcommand is given: a `.lichen` file, or a
    /// directory scanned for `.lichen` files.
    #[arg(value_name = "program")]
    program: Option<PathBuf>,
    /// A placement pack to install for this run; name it once per pack.
    #[arg(long = "pack", value_name = "path", global = true)]
    pack: Vec<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Compile & run a program (a file, or every `.lichen` file in a directory).
    Run {
        /// The program to compile & run.
        path: PathBuf,
    },
    /// Compile a program and print its exported type.
    Build {
        /// The program file to build.
        path: PathBuf,
    },
}

/// Run the compiler CLI, reading the program name from `argv[0]`, with the
/// shipping compiler's cache slot.
pub fn main<P>() -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    main_with_wiring::<P>(&persist::shipping_cache_root(), &[], Wiring::default())
}

/// [`Self::main`] with an explicit artifact cache root, selecting the plugin
/// set's cache slot.
pub fn main_with_cache_dir<P>(cache_root: &Path) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    main_with_native_packages::<P>(cache_root, &[], Wiring::default())
}

/// [`Self::main_with_cache_dir`] plus native plugin packages for the store each
/// program is evaluated against.
pub fn main_with_native_packages<P>(
    cache_root: &Path,
    native: &[NativePackage<P>],
    wiring: Wiring<'_>,
) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    main_with_wiring::<P>(cache_root, native, wiring)
}

/// The one entry every other reaches: the CLI over a composed program,
/// its native packages, and the host's pack wiring.
pub fn main_with_wiring<P>(
    cache_root: &Path,
    native: &[NativePackage<P>],
    wiring: Wiring<'_>,
) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    // clap's `Command::name` wants a `'static` string, so the name is leaked;
    // the short-lived process makes that harmless.
    let bin: &'static str = Box::leak(
        std::env::args()
            .next()
            .map(|p| {
                PathBuf::from(&p)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "lichen-compiler".to_string())
            })
            .unwrap_or_else(|| "lichen-compiler".to_string())
            .into_boxed_str(),
    );
    let matches = Cli::command().name(bin).get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };
    // A pack that is not one stops the run here, before anything is installed or run.
    if !cli.pack.is_empty()
        && let Err(refusal) = (wiring.install)(&cli.pack)
    {
        eprintln!("{refusal}");
        return ExitCode::FAILURE;
    }
    // The one value every run below reads: this invocation named packs, or it named none.
    let report = if cli.pack.is_empty() {
        PassReport::Silent
    } else {
        PassReport::Reported(wiring.pass_categories)
    };
    match cli.command {
        Some(Command::Run { path }) => run_path::<P>(cache_root, &path, native, report),
        Some(Command::Build { path }) => build_file::<P>(cache_root, &path, native, report),
        None => {
            if let Some(program) = cli.program {
                run_path::<P>(cache_root, &program, native, report)
            } else {
                eprintln!("{}", Cli::command().name(bin).render_help());
                ExitCode::FAILURE
            }
        }
    }
}

/// The run's recorded reasons and stage lines, on **stderr**.
///
/// # Invariant
/// **stderr, because stdout is the program's own output**: a host reads a value off stdout, and a
/// report written there would be read as part of the value. Each line is the pass's own sentence,
/// verbatim and unprefixed — it already names its node and its pack — so what a machine parses is
/// what the pass wrote. A refusal line explains an undecided value and precedes the value.
fn report_passes(run: &RunOutput) {
    for line in &run.refusals {
        eprintln!("{line}");
    }
    for line in &run.pass_lines {
        eprintln!("{line}");
    }
}

fn run_path<P>(
    cache_root: &Path,
    path: &Path,
    native: &[NativePackage<P>],
    report: PassReport,
) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    if path.is_dir() {
        run_directory::<P>(cache_root, path, native, report)
    } else {
        run_file::<P>(cache_root, path, native, report)
    }
}

/// A store holding the file's staged `depend` directives, with native packages
/// registered on it after staging.
fn staged_store<P>(
    source: &str,
    cache_root: &Path,
    native: &[NativePackage<P>],
) -> (PackageStore<P>, Vec<lichen_language::diag::Diag<P>>)
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    // A non-persistent codec never serializes, so the store stays in memory
    // and no artifact is written.
    let mut store: PackageStore<P> = if P::Codec::PERSISTENT {
        PackageStore::with_cache_dir(cache_root.to_path_buf())
    } else {
        PackageStore::new()
    };
    let mut diags = stage_depends::<P>(&mut store, source);
    for &(virtual_path, wrapper, native_ops) in native {
        // Deferred: an unimported package is never compiled, and a broken wrapper is
        // reported once, at the import that names it.
        store.register_native_deferred(virtual_path, wrapper, native_ops);
    }
    (store, diags)
}

fn run_file<P>(
    cache_root: &Path,
    path: &Path,
    native: &[NativePackage<P>],
    report: PassReport,
) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let (mut store, diags) = staged_store::<P>(&source, cache_root, native);
    if !diags.is_empty() {
        print!("{}", lichen_language::render::render_all(&source, &diags));
        return ExitCode::FAILURE;
    }
    match lichen_language::run::evaluate_raw_reporting::<P>(&source, Some(path), &mut store, report)
    {
        Ok(run) => {
            report_passes(&run);
            println!("{}", run.output);
            ExitCode::SUCCESS
        }
        Err(diags) => {
            print!("{}", lichen_language::render::render_all(&source, &diags));
            ExitCode::FAILURE
        }
    }
}

fn run_directory<P>(
    cache_root: &Path,
    dir: &Path,
    native: &[NativePackage<P>],
    report: PassReport,
) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
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
        let (mut store, diags) = staged_store::<P>(&source, cache_root, native);
        if !diags.is_empty() {
            failed += 1;
            eprintln!("{}: failed to stage dependencies", file.display());
            print!("{}", lichen_language::render::render_all(&source, &diags));
            continue;
        }
        match lichen_language::run::evaluate_raw_reporting::<P>(
            &source,
            Some(&file),
            &mut store,
            report,
        ) {
            Ok(run) => {
                report_passes(&run);
                println!(
                    "{}: {}",
                    file.file_name().unwrap().to_string_lossy(),
                    run.output
                );
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

fn build_file<P>(
    cache_root: &Path,
    path: &Path,
    native: &[NativePackage<P>],
    report: PassReport,
) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + 'static,
{
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let (mut store, diags) = staged_store::<P>(&source, cache_root, native);
    if !diags.is_empty() {
        print!("{}", lichen_language::render::render_all(&source, &diags));
        return ExitCode::FAILURE;
    }
    match store.load_package(path) {
        Ok(handle) => {
            println!("built {}", handle.path.display());
            // The package was loaded and frozen.  The tiny import below is the
            // real importer path, and it prints the exported type.
            let name = path.file_name().unwrap().to_string_lossy();
            let source = format!("---\n  _pkg = import \"{name}\"\n---\n_pkg\n");
            match lichen_language::run::evaluate_raw_reporting::<P>(
                &source,
                Some(path),
                &mut store,
                report,
            ) {
                Ok(run) => {
                    report_passes(&run);
                    println!(
                        "type: {}",
                        run.output.split(": ").nth(1).unwrap_or(&run.output)
                    )
                }
                Err(diags) => {
                    print!("{}", lichen_language::render::render_all(&source, &diags));
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(diags) => {
            print!("{}", lichen_language::render::render_all(&source, &diags));
            ExitCode::FAILURE
        }
    }
}
