//! The compiler CLI, shared by the real `lichen-compiler` binary and the
//! plugin-built compiler crate (its generated `main.rs` calls [`main`]) so
//! every compiler speaks the same dialect.
//!
//! `lichen-compiler <program.lichen>` compiles and runs one program, printing
//! its output; a directory path runs every `.lichen` file in it, printing
//! `file: output` per program.  The `run` and `build` subcommands are also
//! accepted.
//!
//! The compiler is **depend-aware**: a file's `depend "url"` directives
//! resolve against the lichen-home source cache (populated by the package
//! manager's `lichen fetch`), so running a file with dependencies needs no
//! git access here — the compiler only *reads* what the package manager put in
//! the cache.  The compiler binary is invoked by the package manager for its
//! `run`/`build` commands, which is how a plugin-built compiler's vocabulary
//! takes effect.
//!
//! The compiler's **artifact cache is scoped per plugin set**.  A compiled
//! package is serialized into the device store keyed by file ID (see
//! [`crate::package::PackageStore`] / [`crate::persist`]), and the artifact
//! encoding depends on the compiler's value/operator vocabulary.  A
//! plugin-built compiler must therefore NOT share the shipping compiler's
//! device cache — the same source file compiled by a different plugin set
//! produces a different artifact, so the cache slot must be isolated per
//! vocabulary.  [`main`] uses the shipping compiler's own
//! `compilers/<toolchain-key>` slot (`persist::shipping_cache_root`);
//! [`main_with_cache_dir`] lets a plugin-built compiler scope its artifacts to
//! its own `compilers/<plugin-set-key>` slot.  Source staging (the git source
//! cache) stays shared; only the compiled-artifact store is scoped.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use lichen_highlevel::native::NativeOps;
use lichen_highlevel::program::{TypeOperator, ValueType};
use lichen_utils::extend::AsEnum;

use crate::LangProgramShape;
use crate::package::PackageStore;
use crate::persist::{self, ArtifactCodec};
use crate::preprocess::stage_depends;
use crate::program::GcdOp;

/// One native plugin package to register on the store a compiler evaluates
/// against: `(virtual_path, embedded_source, private_native_ops)`.  A
/// plugin-built compiler's generated `main` supplies one entry per plugin, so
/// the plugin's wrapper source is compiled against its own private native-op
/// registry and served by name (`<alias>.lichen`) — see
/// [`PackageStore::register_native`](crate::package::PackageStore::register_native).
///
/// `native_ops` is the plugin's per-module registry; the wrapper const and the
/// ops macro are named from the plugin crate (see `crate::plugin`'s generated
/// `main`).  A shipping compiler registers nothing (`&[]`), so its store is
/// exactly as before — the plugin-built path is opt-in.
pub type NativePackage<P> = (&'static str, &'static str, NativeOps<P>);

/// The compiler CLI surface: a single positional program path (the default
/// `run` action) or an explicit subcommand (`run`, `build`).
///
/// The command name is overridden at runtime from `argv[0]` (see
/// [`main_with_native_packages`]) so a plugin-built `lichen-compiler-<name>`
/// reports its own name in usage/help.
#[derive(Parser)]
#[command(name = "lichen-compiler", version)]
struct Cli {
    /// A program to run when no subcommand is given: a `.lichen` file, or a
    /// directory scanned for `.lichen` files.
    #[arg(value_name = "program")]
    program: Option<PathBuf>,
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

/// Run the compiler CLI with the process arguments, using the shipping
/// compiler's `compilers/<toolchain-key>` slot as the device/artifact cache
/// root (so every vocabulary — shipping included — caches under `compilers/`).
/// The program name is read from `argv[0]` so the plugin-built
/// `lichen-compiler-<name>` reports its own name in usage.  Generic over a
/// single program type `P` (the associate-type collector), so the shipped
/// compiler and a plugin-built compiler share one CLI.
pub fn main<P>() -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    main_with_cache_dir::<P>(&persist::shipping_cache_root())
}

/// [`Self::main`] with an explicit **device/artifact cache root**.
///
/// The compile artifacts drive the incremental device store
/// ([`crate::package::PackageStore`]'s `with_cache_dir`).  Every compiler
/// scopes its artifacts to a `compilers/<plugin-set-key>` slot so it never
/// collides with (or reuses) another vocabulary's artifacts — the shipping
/// compiler uses the empty plugin set's slot (see [`persist::shipping_cache_root`]),
/// a **plugin-built** compiler its own slot (`<lichendir>/compilers/<plugin-set-key>`).
pub fn main_with_cache_dir<P>(cache_root: &Path) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    main_with_native_packages::<P>(cache_root, &[])
}

/// [`Self::main`] with an explicit **device/artifact cache root** and a set of
/// **native plugin packages** to register on the store each program is
/// evaluated against.
///
/// This is the plugin-built compiler's entry: a generated `main` passes one
/// `(virtual_path, embedded_source, native_ops)` triple per plugin, so the
/// plugin's wrapper source (e.g. `std.lichen`) is compiled against the
/// plugin's private native-op registry and served by name — the same store the
/// program runs through, exactly as the reference `std_native` test's
/// `register_native` plug.  A shipping compiler calls [`Self::main_with_cache_dir`]
/// with the empty set, keeping its store native-free.
pub fn main_with_native_packages<P>(cache_root: &Path, native: &[NativePackage<P>]) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    // The program name is read from argv[0] so a plugin-built
    // `lichen-compiler-<name>` reports its own name in usage/help.  clap's
    // `Command::name` takes a `'static` string (clap's `Str`), so the name is
    // leaked once — harmless for a short-lived CLI process.
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
    match cli.command {
        Some(Command::Run { path }) => run_path::<P>(cache_root, &path, native),
        Some(Command::Build { path }) => build_file::<P>(cache_root, &path, native),
        None => {
            if let Some(program) = cli.program {
                run_path::<P>(cache_root, &program, native)
            } else {
                eprintln!("{}", Cli::command().name(bin).render_help());
                ExitCode::FAILURE
            }
        }
    }
}

fn run_path<P>(cache_root: &Path, path: &Path, native: &[NativePackage<P>]) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    if path.is_dir() {
        run_directory::<P>(cache_root, path, native)
    } else {
        run_file::<P>(cache_root, path, native)
    }
}

/// A store that stages the file's `depend` directives from the source cache
/// and reports a diagnostic when one has not been fetched.  The device cache
/// root is the caller's artifact/cache root (the lichen home for the shipping
/// compiler, the per-plugin-set slot for a plugin-built compiler).
///
/// Each native plugin package in `native` is registered on the store **after**
/// the file's own dependencies are staged, so the block's `import "<alias>"`
/// resolves the plugin's wrapper as a native virtual package (compiled against
/// the plugin's private native-op registry) on this same store — the same
/// `register_native` plug the reference `std_native` test uses.
fn staged_store<P>(
    source: &str,
    cache_root: &Path,
    native: &[NativePackage<P>],
) -> (PackageStore<P>, Vec<crate::diag::Diag<P>>)
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    // A persistent codec drives a cache directory; an in-memory-only one
    // (`NoPersist`) never serializes, so the store is in-memory and no
    // artifact is written (avoids the `NoPersist` unreachable path).
    let mut store: PackageStore<P> = if P::Codec::PERSISTENT {
        PackageStore::with_cache_dir(cache_root.to_path_buf())
    } else {
        PackageStore::new()
    };
    let mut diags = stage_depends::<P>(&mut store, source);
    for &(virtual_path, wrapper, native_ops) in native {
        if let Err(e) = store.register_native(virtual_path, wrapper, native_ops) {
            diags.push(crate::diag::Diag::new(
                crate::diag::Stage::Preprocess,
                (0, 0),
                format!("cannot register native package {virtual_path}: {e}"),
            ));
        }
    }
    (store, diags)
}

fn run_file<P>(cache_root: &Path, path: &Path, native: &[NativePackage<P>]) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
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
        print!("{}", crate::render::render_all(&source, &diags));
        return ExitCode::FAILURE;
    }
    match crate::run::evaluate_raw::<P>(&source, Some(path), &mut store) {
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

fn run_directory<P>(cache_root: &Path, dir: &Path, native: &[NativePackage<P>]) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
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
            print!("{}", crate::render::render_all(&source, &diags));
            continue;
        }
        match crate::run::evaluate_raw::<P>(&source, Some(&file), &mut store) {
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

fn build_file<P>(cache_root: &Path, path: &Path, native: &[NativePackage<P>]) -> ExitCode
where
    P: LangProgramShape,
    P::Value: ValueType
        + AsEnum<lichen_compute::ComputeValue>
        + From<lichen_compute::ComputeValue>
        + 'static,
    P::Operator: From<GcdOp> + From<TypeOperator> + From<lichen_compute::ComputeOperator> + 'static,
{
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let (mut store, diags) = staged_store::<P>(&source, cache_root, native);
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
            match crate::run::evaluate_raw::<P>(&source, Some(path), &mut store) {
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
