//! The lichen package manager CLI.
//!
//! `lichen` manages a project: its git dependencies (add / rm / fetch / list),
//! its toolchain binaries (`install`), its builds (`run` / `build`), its
//! native-plugin compiler rebuild (`rebuild-plugin`), and the device cache
//! (`cache gc`).  The language compiler binary is `lichen-compiler`
//! (in `crates/lichen-language`); this package manager drives it.  Commands
//! operate on the project in the current directory.

use std::env::Args;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lichen_package::{DEFAULT_REPO, Dependency, Project, plugin, toolchain};

const USAGE: &str = "\
usage: lichen <command> [args]

commands:
  add <git-url> [--name <alias>] [--rev <rev>] [--branch <b>] [--tag <t>]
                [--package <crate>] [--plugin]     add a git dependency
  rm <alias>                                        remove a dependency
  list                                              list dependencies
  fetch                                             clone/update all git deps
  run <file|dir>                                    fetch, then compile & run
  build <file>                                      fetch, then load & print type
  install <compiler|language-server|all>            fetch a toolchain binary
  rebuild-plugin [--name <n>] [--repo <url>]        rebuild the compiler for
                                                    the project's native plugins
  cache gc                                          reclaim device-cache artifacts
  --help, -h                                        print this help
  --version, -V                                     print the version
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
        "add" => cmd_add(&mut args),
        "rm" => cmd_rm(&mut args),
        "list" => cmd_list(&mut args),
        "fetch" => cmd_fetch(&mut args),
        "run" => cmd_run(&mut args),
        "build" => cmd_build(&mut args),
        "install" => cmd_install(&mut args),
        "rebuild-plugin" => cmd_rebuild_plugin(&mut args),
        "cache" => cmd_cache(&mut args),
        other => {
            eprintln!("unknown command: {other}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// Load the project in the current directory (an empty manifest is allowed).
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

fn cmd_add(args: &mut Args) -> ExitCode {
    let Some(url) = take(args) else {
        eprintln!("usage: lichen add <git-url> [--name <alias>] [options]");
        return ExitCode::FAILURE;
    };
    let mut alias = None;
    let mut rev = None;
    let mut branch = None;
    let mut tag = None;
    let mut package = None;
    let mut is_plugin = false;
    while let Some(flag) = take(args) {
        match flag.as_str() {
            "--name" | "-n" => alias = take(args),
            "--rev" => rev = take(args),
            "--branch" => branch = take(args),
            "--tag" => tag = take(args),
            "--package" => package = take(args),
            "--plugin" => is_plugin = true,
            other => {
                eprintln!("unknown flag: {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let alias = alias.unwrap_or_else(|| {
        let name = url
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("dep");
        name.strip_suffix(".git").unwrap_or(name).to_string()
    });
    let dep = Dependency {
        git: url,
        rev,
        branch,
        tag,
        package,
        plugin: is_plugin,
    };
    let project = match load_current() {
        Ok(p) => p,
        Err(code) => return code,
    };
    let mut manifest = project.manifest.clone();
    if manifest.deps.contains_key(&alias) {
        eprintln!("dependency '{alias}' already exists");
        return ExitCode::FAILURE;
    }
    manifest.deps.insert(alias.clone(), dep);
    let path = match manifest.save(&project.dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("added {alias} -> {}", path.display());
    ExitCode::SUCCESS
}

fn cmd_rm(args: &mut Args) -> ExitCode {
    let Some(alias) = take(args) else {
        eprintln!("usage: lichen rm <alias>");
        return ExitCode::FAILURE;
    };
    let project = match load_current() {
        Ok(p) => p,
        Err(code) => return code,
    };
    let mut manifest = project.manifest.clone();
    if manifest.deps.remove(&alias).is_none() {
        eprintln!("dependency '{alias}' not found");
        return ExitCode::FAILURE;
    }
    match manifest.save(&project.dir) {
        Ok(_) => {
            println!("removed {alias}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_list(_args: &mut Args) -> ExitCode {
    let project = match load_current() {
        Ok(p) => p,
        Err(code) => return code,
    };
    if project.deps().is_empty() {
        println!("no dependencies");
        return ExitCode::SUCCESS;
    }
    for (alias, dep) in project.deps() {
        let rev = dep
            .checkout()
            .map(|r| format!(" @ {r}"))
            .unwrap_or_default();
        let plugin = if dep.is_plugin() { " (plugin)" } else { "" };
        println!("{alias} = {}{rev}{plugin}", dep.git);
    }
    ExitCode::SUCCESS
}

fn cmd_fetch(_args: &mut Args) -> ExitCode {
    let project = match load_current() {
        Ok(p) => p,
        Err(code) => return code,
    };
    match project.fetch_all() {
        Ok(out) => {
            if out.is_empty() {
                println!("nothing to fetch");
            }
            for (alias, path) in out {
                println!("fetched {alias} -> {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
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
    if target.is_dir() {
        run_directory(&project, &target)
    } else {
        match project.run(&target) {
            Ok(output) => {
                println!("{output}");
                ExitCode::SUCCESS
            }
            Err(diags) => {
                let source = std::fs::read_to_string(&target).unwrap_or_default();
                print!("{}", lichen_language::render::render_all(&source, &diags));
                ExitCode::FAILURE
            }
        }
    }
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
    match project.build(&target) {
        Ok((path, ty)) => {
            println!("built {}", path.display());
            println!("type: {ty}");
            ExitCode::SUCCESS
        }
        Err(diags) => {
            let source = std::fs::read_to_string(&target).unwrap_or_default();
            print!("{}", lichen_language::render::render_all(&source, &diags));
            ExitCode::FAILURE
        }
    }
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
    while let Some(flag) = take(args) {
        match flag.as_str() {
            "--name" | "-n" => name = take(args).unwrap_or(name),
            "--repo" => repo = take(args).unwrap_or(repo),
            other => {
                eprintln!("unknown flag: {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    let plugins: Vec<(&str, &Dependency)> = project
        .plugin_deps()
        .map(|(alias, dep)| (alias.as_str(), dep))
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

fn cmd_cache(args: &mut Args) -> ExitCode {
    let Some(sub) = take(args) else {
        eprintln!("usage: lichen cache gc");
        return ExitCode::FAILURE;
    };
    if sub != "gc" || take(args).is_some() {
        eprintln!("usage: lichen cache gc");
        return ExitCode::FAILURE;
    }
    let dir = lichen_language::persist::lichendir();
    let mut store = lichen_language::package::PackageStore::with_cache_dir(dir.clone());
    let removed = store.gc();
    println!(
        "reclaimed {removed} cached artifact(s) from {}",
        dir.display()
    );
    ExitCode::SUCCESS
}

/// Run every `.lichen` file in a directory, printing `file: output` per file.
fn run_directory(project: &Project, dir: &Path) -> ExitCode {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "lichen"))
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
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: cannot read: {e}", file.display());
                failed += 1;
                continue;
            }
        };
        match project.evaluate(&source, Some(&file)) {
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
