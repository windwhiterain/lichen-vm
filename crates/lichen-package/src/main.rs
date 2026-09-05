//! The lichen package manager CLI.
//!
//! `lichen` manages a project whose dependencies are declared per-file as
//! `name = depend "url"` directives in each `@{…@}` meta block: it fetches them
//! (`fetch`), fetches the toolchain binaries (`install`), rebuilds the
//! compiler for a native plugin (`rebuild-plugin`), and reclaims the device
//! cache (`clean` / `cache gc`).
//!
//! The `run` and `build` commands orchestrate the workflow but **delegate the
//! actual compilation to the compiler binary** — they fetch the dependencies
//! into the source cache, then spawn `lichen-compiler` (or the plugin-built
//! `lichen-compiler-<name>`) as a subprocess.  The package manager never
//! compiles a program in-process, so a compiler rebuilt with a native plugin
//! is the one that actually runs the program.

use std::env::Args;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lichen_language::preprocess::{Directive, block_directives, split_block};
use lichen_package::{DEFAULT_REPO, Depend, Project, git, plugin, toolchain};

const USAGE: &str = "\
usage: lichen <command> [args]

commands:
  fetch <file|dir>                                      fetch the git deps declared
                                                        by the file(s)' `depend` block
  run <file|dir>                                        fetch, then compile & run via
                                                        the compiler binary
  build <file>                                          fetch, then compile & print the
                                                        exported type via the compiler
                                                        binary
  clean                                                 reclaim device-cache artifacts
  install <compiler|language-server|all>                fetch a toolchain binary
  rebuild-plugin [<file|dir>] [--name <n>] [--repo <u>] rebuild the compiler for the
                                                        native plugins declared in the
                                                        file(s)' `depend` blocks
  cache gc                                              reclaim device-cache artifacts
  --help, -h                                            print this help
  --version, -V                                         print the version
";

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
        "fetch" => cmd_fetch(&mut args),
        "run" => cmd_run(&mut args),
        "build" => cmd_build(&mut args),
        "clean" => cmd_clean(&mut args),
        "install" => cmd_install(&mut args),
        "rebuild-plugin" => cmd_rebuild_plugin(&mut args),
        "cache" => cmd_cache(&mut args),
        other => {
            eprintln!("unknown command: {other}\n{USAGE}");
            ExitCode::FAILURE
        }
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

fn take(args: &mut Args) -> Option<String> {
    args.next()
}

/// The dependencies declared by a source's `@{…@}` block.
fn depends_of(source: &str) -> Vec<Depend> {
    let (interior, _) = split_block(source);
    let Some(interior) = interior else {
        return Vec::new();
    };
    block_directives(interior)
        .into_iter()
        .filter_map(|dir| match dir {
            Directive::Depend {
                url,
                name,
                rev,
                branch,
                tag,
                package,
                sub,
                plugin,
            } => Some(Depend {
                url,
                name,
                rev,
                branch,
                tag,
                package,
                sub,
                plugin,
            }),
            _ => None,
        })
        .collect()
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

/// Fetch the `depend` directives of every source under `target` into the
/// lichen-home source cache.
fn cmd_fetch(args: &mut Args) -> ExitCode {
    let Some(target) = take(args).map(PathBuf::from) else {
        eprintln!("usage: lichen fetch <file|dir>");
        return ExitCode::FAILURE;
    };
    if !target.exists() {
        eprintln!("cannot fetch: {} does not exist", target.display());
        return ExitCode::FAILURE;
    }
    let depends = collect_depends(&target);
    let mut fetched = 0;
    for dep in &depends {
        let alias = git::alias_of(dep);
        match git::fetch(dep) {
            Ok(dir) => {
                println!("fetched {alias} -> {}", dir.display());
                fetched += 1;
            }
            Err(e) => {
                eprintln!("failed to fetch {alias}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if fetched == 0 {
        println!(
            "nothing to fetch (no `depend` directives in {}",
            target.display()
        );
    }
    ExitCode::SUCCESS
}

fn cmd_run(args: &mut Args) -> ExitCode {
    let project = match load_current() {
        Ok(p) => p,
        Err(code) => return code,
    };
    let Some(target) = take(args).map(PathBuf::from) else {
        eprintln!("usage: lichen run <file|dir>");
        return ExitCode::FAILURE;
    };
    delegate(&project, &target, "run")
}

fn cmd_build(args: &mut Args) -> ExitCode {
    let project = match load_current() {
        Ok(p) => p,
        Err(code) => return code,
    };
    let Some(target) = take(args).map(PathBuf::from) else {
        eprintln!("usage: lichen build <file>");
        return ExitCode::FAILURE;
    };
    if target.is_dir() {
        eprintln!("usage: lichen build <file> (a directory is only valid for `run`)");
        return ExitCode::FAILURE;
    }
    delegate(&project, &target, "build")
}

/// The shared `run`/`build` workflow: fetch the target's dependencies into the
/// source cache, select the compiler binary (the plugin-built one when the
/// project imports a native plugin, else the shipped compiler), and delegate
/// the actual compilation to it as a subprocess.
fn delegate(project: &Project, target: &Path, sub: &str) -> ExitCode {
    if !target.exists() {
        eprintln!("cannot {sub}: {} does not exist", target.display());
        return ExitCode::FAILURE;
    }
    let depends = collect_depends(target);
    for dep in &depends {
        let alias = git::alias_of(dep);
        match git::fetch(dep) {
            Ok(dir) => println!("fetched {alias} -> {}", dir.display()),
            Err(e) => {
                eprintln!("failed to fetch {alias}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    let bin = match select_compiler(project, &depends) {
        Ok(bin) => bin,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("compiler: {}", bin.display());
    spawn_compiler(&bin, sub, target)
}

/// The compiler binary to drive: the plugin-built one when the project's deps
/// include a native plugin (`depend … plugin`), else the installed/shipping
/// `lichen-compiler`.
fn select_compiler(project: &Project, depends: &[Depend]) -> Result<PathBuf, String> {
    if depends.iter().any(|dep| dep.plugin) {
        plugin::built_bin(&project.dir).ok_or_else(|| {
            "the project imports a native plugin; run `lichen rebuild-plugin` first".to_string()
        })
    } else {
        stock_compiler().ok_or_else(|| {
            "no `lichen-compiler` on $PATH (or next to `lichen`); run `lichen install compiler`"
                .to_string()
        })
    }
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
fn spawn_compiler(bin: &Path, sub: &str, target: &Path) -> ExitCode {
    let status = match std::process::Command::new(bin)
        .arg(sub)
        .arg(target)
        .status()
    {
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
/// live `.lichen` (or `virtual:`) source slot.
fn cmd_clean(_args: &mut Args) -> ExitCode {
    let dir = lichen_language::persist::lichendir();
    let mut store = lichen_language::package::PackageStore::with_cache_dir(dir.clone());
    let removed = store.gc();
    println!(
        "reclaimed {removed} cached artifact(s) from {}",
        dir.display()
    );
    ExitCode::SUCCESS
}

fn cmd_cache(args: &mut Args) -> ExitCode {
    let Some(sub) = take(args) else {
        eprintln!("usage: lichen cache gc");
        return ExitCode::FAILURE;
    };
    if sub != "gc" || take(args).is_some() {
        eprintln!("usage: lichen cache gc");
        return ExitCode::FAILURE;
    }
    cmd_clean(args)
}

fn cmd_install(args: &mut Args) -> ExitCode {
    let Some(tool) = take(args) else {
        eprintln!("usage: lichen install <compiler|language-server|all>");
        return ExitCode::FAILURE;
    };
    let mut repo = DEFAULT_REPO.to_string();
    while let Some(flag) = take(args) {
        if flag == "--repo" {
            repo = take(args).unwrap_or(repo);
        } else {
            eprintln!("unknown flag: {flag}");
            return ExitCode::FAILURE;
        }
    }
    let tools: Vec<toolchain::Tool> = match tool.as_str() {
        "all" => vec![toolchain::Tool::Compiler, toolchain::Tool::LanguageServer],
        _ => {
            let Some(t) = toolchain::Tool::from_name(&tool) else {
                eprintln!("unknown tool: {tool}");
                return ExitCode::FAILURE;
            };
            vec![t]
        }
    };
    for t in tools {
        match toolchain::install(t, &repo, None) {
            Ok(()) => println!("installed {}", t.bin_name()),
            Err(e) => {
                eprintln!("failed to install {}: {e}", t.bin_name());
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

fn cmd_rebuild_plugin(args: &mut Args) -> ExitCode {
    let project = match load_current() {
        Ok(p) => p,
        Err(code) => return code,
    };
    let mut name = "project".to_string();
    let mut repo = DEFAULT_REPO.to_string();
    let mut target: Option<PathBuf> = None;
    while let Some(v) = take(args) {
        if v == "--name" || v == "-n" {
            name = take(args).unwrap_or(name);
        } else if v == "--repo" {
            repo = take(args).unwrap_or(repo);
        } else if v.starts_with('-') {
            eprintln!("unknown flag: {v}");
            return ExitCode::FAILURE;
        } else {
            target = Some(PathBuf::from(v));
        }
    }
    let target = target.unwrap_or_else(|| project.dir.clone());
    let plugins: Vec<Depend> = collect_depends(&target)
        .into_iter()
        .filter(|dep| dep.plugin)
        .collect();
    if plugins.is_empty() {
        println!("no native-plugin dependencies; rebuilding over the shipping plugin set");
    }
    let leaves = plugin::Leaves::shipping();
    match plugin::rebuild(&project.dir, &name, &repo, &plugins, &leaves) {
        Ok(build) => {
            println!("rebuilt compiler: {}", build.bin.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
