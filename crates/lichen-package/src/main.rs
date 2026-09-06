//! The lichen package manager CLI.
//!
//! `lichen` manages a project whose dependencies are declared per-file as
//! `name = depend "url"` directives in each `@{…@}` meta block: it fetches them
//! (`fetch`), fetches the toolchain binaries (`install`), rebuilds the
//! compiler for a native plugin (`rebuild-plugin`), and reclaims the device
//! cache (`clean`).
//!
//! The `run` and `build` commands orchestrate the workflow but **delegate the
//! actual compilation to the compiler binary** — they fetch the dependencies
//! into the source cache, then spawn `lichen-compiler` (or the plugin-built
//! `lichen-compiler-<name>`) as a subprocess.  The package manager never
//! compiles a program in-process, so a compiler rebuilt with a native plugin
//! is the one that actually runs the program.
//!
//! `clean` is the exception: the package manager reclaims the plugin-set
//! compiler cache slots itself, opening each slot's [`lichen_registry::DeviceRegistry`]
//! and calling `gc()` — no compiler subprocess, and no language/VM dependency
//! (the registry layer is type-independent, in `lichen-registry`).
//!
//! The command surface is declared with clap (derive); each command's runtime
//! work stays in the `cmd_*` functions below.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use lichen_package::{DEFAULT_REPO, Depend, Project, compiler_cache, git, plugin, toolchain};
use lichen_preprocess::{block_depends, lichendir, split_block};
use lichen_registry::DeviceRegistry;

#[derive(Parser)]
#[command(
    name = "lichen",
    bin_name = "lichen",
    version,
    about = "The lichen package manager: resolve git dependencies, fetch the toolchain binaries, and own the preprocessor import path."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch the git deps declared by the file(s)' `depend` block.
    Fetch {
        /// A `.lichen` file, or a directory scanned for `.lichen` files.
        target: PathBuf,
    },

    /// Fetch, then compile & run via the compiler binary.
    Run {
        /// A `.lichen` file, or a directory scanned for `.lichen` files.
        target: PathBuf,
        /// The repository (or local checkout / git URL) the core crates come from.
        #[arg(long, default_value = DEFAULT_REPO)]
        repo: String,
    },

    /// Fetch, then compile & print the exported type via the compiler binary.
    Build {
        /// A `.lichen` file.
        target: PathBuf,
        /// The repository (or local checkout / git URL) the core crates come from.
        #[arg(long, default_value = DEFAULT_REPO)]
        repo: String,
    },

    /// Reclaim device-cache artifacts.
    Clean,

    /// Install a prebuilt toolchain binary into Lichen Home.
    Install {
        /// The tool to install: `compiler`, `language-server`, or `all`.
        tool: String,
        /// The repository the release asset is fetched from.
        #[arg(long, default_value = DEFAULT_REPO)]
        repo: String,
    },

    /// Update the package manager itself to the repository's latest release.
    Update {
        /// The repository to update from.
        #[arg(long, default_value = DEFAULT_REPO)]
        repo: String,
    },

    /// Print the resolved toolchain binary path (installing it if absent).
    Path {
        /// The tool: `compiler` or `language-server`.
        tool: String,
        /// The repository the release asset is fetched from.
        #[arg(long, default_value = DEFAULT_REPO)]
        repo: String,
        /// For `language-server`: compose a server over the project's native
        /// plugins rooted at this directory.
        #[arg(long)]
        project: Option<PathBuf>,
    },

    /// Build (or reuse) a cached compiler over the project's native plugins.
    RebuildPlugin {
        /// A `.lichen` file or directory to collect plugins from (default: the
        /// current project).
        target: Option<PathBuf>,
        /// The repository (or local checkout / git URL) the core crates come from.
        #[arg(long, default_value = DEFAULT_REPO)]
        repo: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Fetch { target } => cmd_fetch(&target),
        Command::Run { target, repo } => cmd_run(target, &repo),
        Command::Build { target, repo } => cmd_build(target, &repo),
        Command::Clean => cmd_clean(),
        Command::Install { tool, repo } => cmd_install(&tool, &repo),
        Command::Update { repo } => cmd_update(&repo),
        Command::Path {
            tool,
            repo,
            project,
        } => cmd_path(&tool, &repo, project),
        Command::RebuildPlugin { target, repo } => cmd_rebuild_plugin(target, &repo),
    }
}

/// Load the project rooted at the current directory.
fn load_current() -> Result<Project, ExitCode> {
    let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Project::load(&dir).map_err(|e| {
        eprintln!("{e}");
        ExitCode::FAILURE
    })
}

/// The dependencies declared by a source's `@{…@}` block (`depend` and `plug`
/// bindings).
fn depends_of(source: &str) -> Vec<Depend> {
    let (interior, _) = split_block(source);
    block_depends(interior.unwrap_or_default())
}

/// Every `.lichen` source under `target` (a file or a directory), as
/// `(path, source)`.
fn each_source(target: &Path) -> Vec<(PathBuf, String)> {
    if target.is_dir() {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(target) {
            Ok(entries) => entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "lichen"))
                .collect(),
            Err(_) => Vec::new(),
        };
        files.sort();
        files
            .into_iter()
            .filter_map(|p| std::fs::read_to_string(&p).ok().map(|s| (p, s)))
            .collect()
    } else {
        std::fs::read_to_string(target)
            .ok()
            .map(|s| vec![(target.to_path_buf(), s)])
            .unwrap_or_default()
    }
}

/// The union of the `depend` directives across all sources under `target`.
fn collect_depends(target: &Path) -> Vec<Depend> {
    let mut out = Vec::new();
    for (_, source) in each_source(target) {
        out.extend(depends_of(&source));
    }
    out
}

/// Fetch every dependency into the lichen-home source cache, printing each
/// fetched `alias -> dir`.  Returns an error (with the failing alias) on the
/// first failure, so callers can abort instead of half-fetching.
fn fetch_depends(depends: &[Depend]) -> Result<(), String> {
    for dep in depends {
        let alias = git::alias_of(dep);
        match git::fetch(dep) {
            Ok(dir) => println!("fetched {alias} -> {}", dir.display()),
            Err(e) => return Err(format!("failed to fetch {alias}: {e}")),
        }
    }
    Ok(())
}

/// Fetch the `depend` directives of every source under `target` into the
/// lichen-home source cache.
fn cmd_fetch(target: &Path) -> ExitCode {
    if !target.exists() {
        eprintln!("cannot fetch: {} does not exist", target.display());
        return ExitCode::FAILURE;
    }
    let depends = collect_depends(target);
    if depends.is_empty() {
        println!(
            "nothing to fetch (no `depend` directives in {})",
            target.display()
        );
        return ExitCode::SUCCESS;
    }
    match fetch_depends(&depends) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_run(target: PathBuf, repo: &str) -> ExitCode {
    delegate(&target, "run", repo)
}

fn cmd_build(target: PathBuf, repo: &str) -> ExitCode {
    if target.is_dir() {
        eprintln!("usage: lichen build <file> (a directory is only valid for `run`)");
        return ExitCode::FAILURE;
    }
    delegate(&target, "build", repo)
}

/// The shared `run`/`build` workflow: fetch the target's dependencies into the
/// source cache, select the compiler binary (the plugin-built one, from the
/// lichen-home compiler cache, when the target imports a native plugin, else
/// the shipped compiler), and delegate the actual compilation to it as a
/// subprocess.
///
/// `core_repo` is the repository (or local checkout path / git URL) the
/// compositor's core crates come from; a local (non-default) `core_repo` lets
/// the generated compositor build offline (see [`plugin::core_patch`]).
fn delegate(target: &Path, sub: &str, core_repo: &str) -> ExitCode {
    if !target.exists() {
        eprintln!("cannot {sub}: {} does not exist", target.display());
        return ExitCode::FAILURE;
    }
    let depends = collect_depends(target);
    if let Err(e) = fetch_depends(&depends) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    let bin = match select_compiler(&depends, core_repo) {
        Ok(bin) => bin,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("compiler: {}", bin.display());
    let target_str = target.to_string_lossy().into_owned();
    spawn_compiler(&bin, &[sub, target_str.as_str()])
}

/// The compiler binary to drive: the plugin-built one (from the lichen-home
/// compiler cache, built or reused) when the target's deps include a native
/// plugin (`name = plug …` or `depend … plugin`), else the installed/shipping
/// `lichen-compiler`.
///
/// The plugin set must already be fetched ([`crate::git::fetch`]) so its
/// resolved version can key the cache.  `core_repo` is the repository (or
/// local checkout path / git URL) the compositor's core crates come from.
fn select_compiler(depends: &[Depend], core_repo: &str) -> Result<PathBuf, String> {
    let plugins: Vec<Depend> = depends.iter().filter(|dep| dep.plugin).cloned().collect();
    if plugins.is_empty() {
        return stock_compiler().ok_or_else(|| {
            "no `lichen-compiler` on $PATH (or next to `lichen`); run `lichen install compiler`"
                .to_string()
        });
    }
    compiler_cache::ensure(core_repo, &plugins, &plugin::Leaves::shipping())
}

/// The shipped compiler binary: an installed one, else a sibling
/// `lichen-compiler` next to the running `lichen` (a dev workspace build).
fn stock_compiler() -> Option<PathBuf> {
    if let Some(found) = toolchain::find(toolchain::Tool::Compiler) {
        return Some(found);
    }
    let exe_name = if cfg!(windows) {
        "lichen-compiler.exe"
    } else {
        "lichen-compiler"
    };
    std::env::current_exe()
        .ok()
        .and_then(|self_exe| self_exe.parent().map(|p| p.join(exe_name)))
        .filter(|p| p.is_file())
}

/// Spawn the compiler binary as a subprocess, relaying its exit status.
fn spawn_compiler(bin: &Path, args: &[&str]) -> ExitCode {
    let status = match std::process::Command::new(bin).args(args).status() {
        Ok(status) => status,
        Err(e) => {
            eprintln!("cannot run {}: {e}", bin.display());
            return ExitCode::FAILURE;
        }
    };
    if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `lichen clean`: reclaim every device-cache artifact that is no longer a
/// live `.lichen` (or `virtual:`) source slot, in **every plugin-composed
/// compiler cache slot** under the lichen home (`compilers/<key>`).
///
/// The package manager owns the clean now: it opens each plugin-set slot's
/// [`lichen_registry::DeviceRegistry`] and calls `gc()` directly, so it never
/// needs to spawn (or even install) the compiler binary.  The registry layer
/// is type-independent (`lichen-registry`), so this pulls no language/VM
/// stack.  The shipping compiler's own base cache root (`lichendir()`) is
/// untouched — `clean` reclaims only the per-plugin-set slots.
fn cmd_clean() -> ExitCode {
    let compilers_dir = lichendir().join("compilers");
    match std::fs::read_dir(&compilers_dir) {
        Ok(entries) => {
            let mut slots: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            slots.sort();
            for slot in slots {
                let mut registry = DeviceRegistry::open(slot.clone());
                let removed = registry.gc();
                println!(
                    "reclaimed {removed} cached artifact(s) from {}",
                    slot.display()
                );
            }
        }
        Err(_) => {
            println!(
                "no plugin compiler caches under {}",
                compilers_dir.display()
            );
        }
    }
    ExitCode::SUCCESS
}

/// `lichen install <tool>`: install a prebuilt toolchain binary into Lichen Home
/// from the GitHub release at this binary's own commit (so the package manager and
/// the toolchain are always the same revision).
fn cmd_install(tool: &str, repo: &str) -> ExitCode {
    let tools: Vec<toolchain::Tool> = match tool {
        "all" => toolchain::Tool::ALL_PLUGIN_SENSITIVE.to_vec(),
        _ => {
            let Some(t) = toolchain::Tool::from_name(tool) else {
                eprintln!("unknown tool: {tool}");
                return ExitCode::FAILURE;
            };
            vec![t]
        }
    };
    for t in tools {
        match toolchain::install(t, repo) {
            Ok(path) => println!("installed {} -> {}", t.bin_name(), path.display()),
            Err(e) => {
                eprintln!("failed to install {}: {e}", t.bin_name());
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// `lichen update`: update the package manager itself to the repo's latest release,
/// written to `$LICHEN_HOME/tools/lichen`.
fn cmd_update(repo: &str) -> ExitCode {
    match toolchain::update(repo) {
        Ok(None) => {
            println!("lichen is already at the latest release");
            ExitCode::SUCCESS
        }
        Ok(Some(commit)) => {
            println!(
                "updated lichen to {commit} ({})",
                toolchain::tools_dir()
                    .join(toolchain::local_name(toolchain::PACKAGE_MANAGER_BIN))
                    .display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("failed to update lichen: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `lichen path <tool>`: print the resolved toolchain binary path, installing it
/// into Lichen Home first if it is absent.  For `language-server`, an optional
/// `--project <dir>` gathers the project's native-plugin set and composes a
/// server over it (built + cached into the plugin-set LSP slot).
fn cmd_path(tool: &str, repo: &str, project: Option<PathBuf>) -> ExitCode {
    let Some(t) = toolchain::Tool::from_name(tool) else {
        eprintln!("unknown tool: {tool}");
        return ExitCode::FAILURE;
    };
    // A plugin-set language server: when a project directory is given, gather its
    // native plugins and resolve the composed server for them.  The empty plugin
    // set falls through to the standard shipping resolution below.
    if t == toolchain::Tool::LanguageServer {
        if let Some(dir) = project {
            return cmd_path_lsp_project(&dir, repo);
        }
    }
    resolve_and_print(t, repo)
}

/// `lichen path language-server --project <dir>`: gather the project's native
/// plugin set from the `.lichen` sources under `dir`, fetch each plugin, and
/// resolve the language server for the set.  With plugins, a composed server is
/// ensured (built + cached into the plugin-set LSP slot) and its path printed;
/// with no plugins, the standard shipping server resolution/install runs.
fn cmd_path_lsp_project(dir: &Path, repo: &str) -> ExitCode {
    if !dir.exists() {
        eprintln!(
            "cannot resolve language server: {} does not exist",
            dir.display()
        );
        return ExitCode::FAILURE;
    }
    let plugins: Vec<Depend> = collect_depends(dir)
        .into_iter()
        .filter(|dep| dep.plugin)
        .collect();
    // Fetch each plugin so its resolved version can key the LSP cache, then
    // ensure a composed server over the plugin set (or fall through to the
    // shipping server when there are no native plugins).
    if let Err(e) = fetch_depends(&plugins) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    if plugins.is_empty() {
        return resolve_and_print(toolchain::Tool::LanguageServer, repo);
    }
    match toolchain::resolve_lsp_for(&plugins) {
        Ok(Some(path)) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!("could not resolve a language server for the project's plugins");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve `tool` (installing it into Lichen Home from the release at `repo` if
/// absent) and print its path.
fn resolve_and_print(t: toolchain::Tool, repo: &str) -> ExitCode {
    if !toolchain::resolve(t).is_some() {
        if let Err(e) = toolchain::install(t, repo) {
            eprintln!("failed to install {}: {e}", t.bin_name());
            return ExitCode::FAILURE;
        }
    }
    match toolchain::resolve(t) {
        Some(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("could not resolve {}", t.bin_name());
            ExitCode::FAILURE
        }
    }
}

fn cmd_rebuild_plugin(target: Option<PathBuf>, repo: &str) -> ExitCode {
    let project = match load_current() {
        Ok(p) => p,
        Err(code) => return code,
    };
    let target = target.unwrap_or_else(|| project.dir.clone());
    let plugins: Vec<Depend> = collect_depends(&target)
        .into_iter()
        .filter(|dep| dep.plugin)
        .collect();
    if plugins.is_empty() {
        println!("no native-plugin dependencies; rebuilding over the shipping plugin set");
    }
    // Fetch the plugins so their resolved versions can key the cache, then
    // build (or reuse) a plugin-composed compiler into the lichen-home cache.
    if let Err(e) = fetch_depends(&plugins) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    let leaves = plugin::Leaves::shipping();
    match compiler_cache::ensure(repo, &plugins, &leaves) {
        Ok(bin) => {
            println!("rebuilt compiler: {}", bin.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
