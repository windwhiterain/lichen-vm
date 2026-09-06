//! Rebuilding the compiler / language server for a native plugin.
//!
//! A *native plugin* (see [`docs/notes/plugin-taxonomy.md`]) extends the
//! compiler's value/operator vocabulary at **compile time**: it contributes
//! enum leaves to the `Program` marker, so a compiler (or the language server)
//! that knows a plugin must be built with that plugin composed into its
//! vocabulary.  That is what this module does — "when a native plugin is
//! imported, rebuild the compiler (or the LSP server)."
//!
//! The mechanism: generate a crate under a caller-chosen directory (the package
//! manager's compiler cache under the lichen home — see
//! [`crate::compiler_cache`]) that depends on the plugin (from git or a local
//! path) and composes its vocabulary with the shipping leaves via
//! `lichen_language::lang_compose_vocabulary!`, then run `cargo build` and
//! report the produced binary.  [`rebuild`] builds the *compiler*
//! (`lichen-compiler-<name>`); [`rebuild_lsp`] builds the *language server*
//! (`lichen-language-server-<name>`), which drives the shared generic
//! [`liche_language_server::server::main`] over the composed program.
//!
//! **Structure of the generated crate.**  Both the compiler crate and the
//! language-server crate are generated **bin-only**: `src/main.rs` holds the
//! composition *and* the `main`.  In a lib+bin package `crate::` refers to the
//! *binary* crate (which has no composition), so `crate::LangProgram` — used by
//! both `cli::main::<crate::LangProgram>()` (compiler) and
//! `server::main::<crate::LangProgram>()` (LSP) — would not resolve there.  The
//! bin-only shape makes the composed `LangProgram` a root item of the binary
//! crate, so both entry points resolve it.
//!
//! **Status:** the *composition* scaffold is real — the generated crate
//! `cargo check`s once the plugin's leaves exist.  The language layer's
//! tooling (package store, run, render, CLI, server) is generic over a
//! program's value/operator vocabularies (see `lichen_language::LangProgramShape`),
//! so a generated compiler routes through the shared [`lichen_language::cli`]
//! and a generated server through the shared
//! [`liche_language_server::server`] over its own composed vocabulary.  The
//! composition macro emits a **per-leaf [`ProgramCodec`]** (persistent — see
//! `liche_language::persist`), so a built compiler writes a real device cache;
//! a generated compiler scopes that artifact cache to its **own plugin-set
//! slot** (`<lichendir>/compilers/<key>`, [`write_compiler_main_rs`]), so its
//! compile artifacts are isolated per vocabulary and never collide with (or
//! reuse) another plugin set's artifacts for the same file ID.

use std::path::{Path, PathBuf};
use std::process::Command;

use lichen_preprocess::Depend;

use crate::git;

/// The value/operator/attr leaves a plugin contributes to the vocabulary.
/// A plugin's leaves are its own item names, spelled at the call site; the
/// generator cannot infer them from the crate.  The shipping set is composed
/// by default.
#[derive(Debug, Clone, Default)]
pub struct Leaves {
    pub values: Vec<(&'static str, &'static str)>,
    pub operators: Vec<(&'static str, &'static str)>,
    pub attrs: Vec<(&'static str, &'static str)>,
}

impl Leaves {
    /// The vocabulary contribution of the shipping plugin set.
    pub fn shipping() -> Self {
        Leaves {
            values: vec![
                ("lichen_lowlevel::LowValue", "LowValue"),
                ("lichen_highlevel::program::TypeValue", "TypeValue"),
                ("lichen_compute::ComputeValue", "ComputeValue"),
            ],
            operators: vec![
                ("lichen_lowlevel::LowOperator", "LowOperator"),
                ("lichen_highlevel::program::TypeOperator", "TypeOperator"),
                ("lichen_perspective::GcdOp", "GcdOp"),
                ("lichen_compute::ComputeOperator", "ComputeOperator"),
            ],
            attrs: vec![
                ("lichen_perspective::Perspective", "Perspective"),
                ("lichen_doc::Doc", "Doc"),
            ],
        }
    }
}

/// The program generated for a compiler build.
pub struct CompilerBuild {
    /// The generated crate directory.
    pub dir: PathBuf,
    /// The produced binary path (after `build`).
    pub bin: PathBuf,
}

/// The program generated for a language-server build.
pub struct ServerBuild {
    /// The generated crate directory.
    pub dir: PathBuf,
    /// The produced binary path (after `build`).
    pub bin: PathBuf,
}

/// Rebuild the compiler: generate a compiler crate at `dir` (the cache slot)
/// composing `leaves` with the plugin dependencies (if any), then `cargo build`
/// it.  Returns the produced binary path.
pub fn rebuild(
    dir: &Path,
    name: &str,
    core_repo: &str,
    plugins: &[Depend],
    leaves: &Leaves,
) -> Result<CompilerBuild, String> {
    if !cargo_available() {
        return Err("`cargo` is required to rebuild the compiler, but it is not on $PATH".into());
    }
    std::fs::create_dir_all(dir.join("src"))
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    write_cargo_toml(
        dir,
        &format!("lichen-compiler-{name}"),
        core_repo,
        plugins,
        "",
    )?;
    write_compiler_main_rs(dir, plugins, leaves)?;

    let out = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run cargo build: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "rebuild failed:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let bin = dir.join("target").join("release").join(bin_name(name));
    Ok(CompilerBuild {
        dir: dir.to_path_buf(),
        bin,
    })
}

/// Rebuild the language server: generate a **bin-only** crate at `dir` (the
/// cache slot) composing `leaves` with the plugin dependencies (if any), then
/// `cargo build` it.  The generated `main` drives the shared generic server over
/// the composed program (`liche_language_server::server::main::<crate::LangProgram>()`),
/// so the produced server understands the plugin's leaves for
/// diagnostics / hover / go-to-definition.
pub fn rebuild_lsp(
    dir: &Path,
    name: &str,
    core_repo: &str,
    plugins: &[Depend],
    leaves: &Leaves,
) -> Result<ServerBuild, String> {
    if !cargo_available() {
        return Err(
            "`cargo` is required to rebuild the language server, but it is not on $PATH".into(),
        );
    }
    std::fs::create_dir_all(dir.join("src"))
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    write_cargo_toml(
        dir,
        &format!("lichen-language-server-{name}"),
        core_repo,
        plugins,
        &format!("\n{}", server_dep(core_repo)),
    )?;
    write_server_main_rs(dir, plugins, leaves)?;

    let out = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run cargo build: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "rebuild failed:\n{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let bin = dir
        .join("target")
        .join("release")
        .join(server_bin_name(name));
    Ok(ServerBuild {
        dir: dir.to_path_buf(),
        bin,
    })
}

/// Whether `cargo` is on `$PATH`.
pub fn cargo_available() -> bool {
    Command::new("cargo")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// The compiled binary name for a compiler named `name`.
pub fn bin_name(name: &str) -> String {
    let n = format!("lichen-compiler-{name}");
    if cfg!(windows) { format!("{n}.exe") } else { n }
}

/// The compiled binary name for a language server named `name`.
pub fn server_bin_name(name: &str) -> String {
    let n = format!("lichen-language-server-{name}");
    if cfg!(windows) { format!("{n}.exe") } else { n }
}

/// The `liche-language-server` dependency line for a generated crate: a local
/// path dep (in `core_repo/crates/liche-language-server`) when `core_repo` is a
/// directory here, else a git dep, both without the default `server` feature
/// (so the server's own default is not double-enlisted) and with `server`
/// enabled explicitly.
fn server_dep(core_repo: &str) -> String {
    if std::path::Path::new(core_repo).is_dir() {
        let rel = format!("{core_repo}/crates/liche-language-server");
        format!(
            "liche-language-server = {{ path = \"{rel}\", default-features = false, features = [\"server\"] }}"
        )
    } else {
        format!(
            "liche-language-server = {{ git = \"{core_repo}\", default-features = false, features = [\"server\"] }}"
        )
    }
}

/// The generated crate's `Cargo.toml`: depends on the language crate, the
/// plugin dependencies (from a local path when `git` is a path that exists),
/// core crates from `core_repo` (a local checkout path or a git URL), and any
/// `extra_deps` (the language-server dependency for an LSP crate).
///
/// `package_name` is the generated package's name (e.g.
/// `lichen-compiler-{name}` or `lichen-language-server-{name}`); the binary
/// target is auto-detected from `src/main.rs` (bin-only) or both `src/lib.rs`
/// and `src/main.rs` (lib+bin).
fn write_cargo_toml(
    dir: &Path,
    package_name: &str,
    core_repo: &str,
    plugins: &[Depend],
    extra_deps: &str,
) -> Result<(), String> {
    let toml = format!(
        r#"[package]
name = "{package_name}"
version = "0.1.0"
edition = "2024"

[dependencies]
{core_language}
{core_lowlevel}
{core_highlevel}
{core_compute}
{core_perspective}
{core_doc}
{core_utils}
{plugin_lines}{extra_deps}{core_patch}"#,
        package_name = package_name,
        core_language = core_dep_line(core_repo, "lichen-language"),
        core_lowlevel = core_dep_line(core_repo, "lichen-lowlevel"),
        core_highlevel = core_dep_line(core_repo, "lichen-highlevel"),
        core_compute = core_dep_line(core_repo, "lichen-compute"),
        core_perspective = core_dep_line(core_repo, "lichen-perspective"),
        core_doc = core_dep_line(core_repo, "lichen-doc"),
        core_utils = core_dep_line(core_repo, "lichen-utils"),
        plugin_lines = plugin_lines(plugins),
        extra_deps = extra_deps,
        core_patch = core_patch(core_repo),
    );
    std::fs::write(dir.join("Cargo.toml"), toml).map_err(|e| format!("write Cargo.toml: {e}"))
}

/// A single core-crate dependency line for the generated crate: a local path
/// dep (`{core_repo}/crates/{crate_name}`) when `core_repo` is a directory
/// here, else a git dep.  A git `core_repo` names the workspace member via
/// `package = <crate_name>` (the repo root is a virtual workspace, so without
/// it cargo looks for a package there and fails).
fn core_dep_line(core_repo: &str, crate_name: &str) -> String {
    if std::path::Path::new(core_repo).is_dir() {
        let rel = format!("{core_repo}/crates/{crate_name}");
        format!("{crate_name} = {{ path = \"{rel}\" }}")
    } else {
        format!("{crate_name} = {{ git = \"{core_repo}\", package = \"{crate_name}\" }}")
    }
}

/// The `[patch]` section emitted when the generated crate is built against a
/// **non-default** `core_repo`.  A native plugin (e.g. `liche-std-native`)
/// declares its own core crates as **git** deps to the canonical repo
/// ([`crate::toolchain::DEFAULT_REPO`]), so a local `core_repo` (a `file://`
/// checkout of the same repo) must add a `[patch]` redirecting those deps to
/// the local source — otherwise cargo reaches out to the canonical repo (the
/// network dependency the harness is avoiding) and, worse, the plugin's git
/// core crates would be *distinct* crate instances from the compositor's own
/// and the plugins' trait impls would not unify (the type-unification break
/// the workspace root's `[patch]` already solves for the monorepo's build).
///
/// Only the core crates the plugin set links against are patched (the ones a
/// native plugin git-deps), not every core crate, to keep the redirect minimal.
/// When `core_repo` is the canonical repo itself, no patch is needed (the
/// plugin's git deps already point at the same source).
fn core_patch(core_repo: &str) -> String {
    if core_repo == crate::toolchain::DEFAULT_REPO {
        return String::new();
    }
    let mut out = String::from("\n[patch.\"");
    out.push_str(crate::toolchain::DEFAULT_REPO);
    out.push_str("\"]\n");
    for crate_name in ["lichen-utils", "lichen-lowlevel", "lichen-highlevel"] {
        out.push_str(&core_dep_line(core_repo, crate_name));
        out.push('\n');
    }
    out
}

/// The plugin dependency lines for a generated crate: each plugin from a local
/// path when its `url` is a path that exists, else from git (with the pinned
/// `rev`/`branch`/`tag` if any).
fn plugin_lines(plugins: &[Depend]) -> String {
    let mut plugin_lines = String::new();
    for dep in plugins {
        let crate_name = git::crate_name(dep);
        if std::path::Path::new(&dep.url).exists() {
            plugin_lines.push_str(&format!("{crate_name} = {{ path = \"{}\" }}\n", dep.url));
        } else {
            let rev = git::checkout(dep)
                .map(|r| format!(", rev = \"{r}\""))
                .unwrap_or_default();
            // A remote plugin is a **git** dep on a crate inside the plugin
            // repo's workspace (e.g. `liche-std-native` inside the lichen-vm
            // monorepo).  `package = <crate_name>` names the workspace member
            // so cargo finds it in the repo (without it, cargo looks for a
            // package at the repo root and fails).  The plugin's own core deps
            // are git too, so cargo resolves the whole subtree from git — a
            // path dep into the workspace is what it cannot fresh-resolve.
            plugin_lines.push_str(&format!(
                "{crate_name} = {{ git = \"{}\", package = \"{crate_name}\"{rev}}}\n",
                dep.url
            ));
        }
    }
    plugin_lines
}

/// The Rust crate identifier for a plugin dependency — its crate name with
/// hyphens replaced by underscores (the extern-prelude / macro-preferred
/// spelling).  The Cargo.toml dependency key keeps the hyphenated package
/// name; the Rust source uses this ident for `<crate>` and derives the leaf
/// macro name `<crate>_leaves`.
fn crate_ident(dep: &Depend) -> String {
    git::crate_name(dep).replace('-', "_")
}

/// The composed-vocabulary body shared by the compiler (`src/lib.rs`) and the
/// language server (`src/main.rs`): the `lang_compose_vocabulary!` invocation
/// over the shipping plus plugin leaves, closing with the composed `LangProgram`
/// and its `Program` alias.  Both generated crates put this at the crate root,
/// so `LangProgram` is available as `<crate>::LangProgram` (the bin-only server
/// crate resolves it from `main`).
fn compose_source(plugins: &[Depend], leaves: &Leaves) -> String {
    let mut attrs = String::new();
    for (ty, variant) in &leaves.attrs {
        attrs.push_str(&format!("        {ty} as {variant};\n"));
    }
    let mut values = String::new();
    for (ty, variant) in &leaves.values {
        values.push_str(&format!("        {ty} as {variant};\n"));
    }
    let mut operators = String::new();
    for (ty, variant) in &leaves.operators {
        operators.push_str(&format!("        {ty} as {variant};\n"));
    }
    let mut plugin_line = String::new();
    for dep in plugins {
        let ident = crate_ident(dep);
        plugin_line.push_str(&format!("    {ident} as {ident}_leaves;\n"));
    }
    format!(
        r#"// The value/operator/attribute vocabulary and the program marker, composed
// from the shipping leaves plus each plugin's own leaf macro (the
// `plugins = [<crate> as <crate>_leaves; ...]` arm stitches their leaves in —
// no config file).
lichen_language::lang_compose_vocabulary! {{
    attrs = [
{attrs}    ]
    [ P::Operator: From<lichen_perspective::GcdOp> ];
    values = [
{values}    ];
    operators = [
{operators}    ];
    plugins = [
{plugin_line}    ];
}}

/// The composed program marker for this build.
pub type Program = LangProgram;
"#,
        attrs = attrs,
        values = values,
        operators = operators,
        plugin_line = plugin_line,
    )
}

/// The `register_native` slots a plugin's embedded wrapper against its private
/// native-op registry, served as a native virtual package at `<alias>.lichen`.
/// A plugin-built compiler's `main` hands one `(virtual_path, wrapper, ops)`
/// tuple per plugin to `cli::main_with_native_packages`, so the plugin's
/// `$sort` (etc.) resolves privately — and the wrapper is compiled on the same
/// store the program evaluates against, exactly as the reference
/// `std_native` test's `register_native` plug.
///
/// The wrapper is `<crate_ident>::WRAPPER_SOURCE` and the ops registry is
/// `<crate_ident>::<crate_ident>_ops!(crate::LangProgram)` — the plugin's
/// `WRAPPER_SOURCE` const and the `<crate>_ops!` macro, both named from the
/// plugin's crate ident (the Cargo.toml dependency key with `-`→`_`).
fn native_package_lines(plugins: &[Depend]) -> String {
    let mut out = String::new();
    for dep in plugins {
        let ident = crate_ident(dep);
        let alias = dep.alias();
        out.push_str(&format!(
            "            (\"{alias}.lichen\", {ident}::WRAPPER_SOURCE, {ident}::{ident}_ops!(crate::LangProgram)),\n"
        ));
    }
    out
}

/// The generated crate's `src/main.rs` (the **bin-only** compiler): the
/// composition at the crate root, then the shared [`lichen_language::cli`]
/// over the composed program.  A compiler must be **bin-only** (not lib+bin)
/// so `<crate>::LangProgram` resolves from the binary crate's root — in a
/// lib+bin package `crate::` refers to the binary crate, which would have no
/// composition, so `cli::main::<crate::LangProgram>()` would not compile.
///
/// The generated `main` runs the compiler with its **own plugin-set cache
/// slot** as the artifact-cache root, so the compiled-artifact store is scoped
/// per vocabulary: a rebuilt compiler never shares (or reuses) another plugin
/// set's artifacts for the same file ID (see `liche_language::cli` and
/// `docs/notes/artifact-cache.md`).
fn write_compiler_main_rs(dir: &Path, plugins: &[Depend], leaves: &Leaves) -> Result<(), String> {
    // The slot directory base name IS the plugin-set cache key — the same key
    // [compiler_cache::key] used to place this build in `compilers/<key>/`.
    let key = dir
        .file_name()
        .map(|k| k.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lines = format!(
        r#"//! A compiler built over the project's plugin set.  Generated by
//! `lichen rebuild-plugin`; re-run it whenever the native-plugin set changes.

{compose}

fn main() -> std::process::ExitCode {{
    // Scope the compiled-artifact cache to this plugin-set slot, so a compiler
    // for a different plugin set never collides with (or reuses) this
    // vocabulary's artifacts for the same file ID.
    let cache_root = lichen_language::persist::lichendir()
        .join("compilers")
        .join("{key}");
    lichen_language::cli::main_with_native_packages::<crate::LangProgram>(
        &cache_root,
        &[
{native}        ],
    )
}}
"#,
        compose = compose_source(plugins, leaves),
        key = key,
        native = native_package_lines(plugins),
    );
    std::fs::write(dir.join("src/main.rs"), lines).map_err(|e| format!("write src/main.rs: {e}"))
}

/// The generated crate's `src/main.rs` (the **bin-only** language server): the
/// composition at the crate root, then a `main` that drives the shared generic
/// server over the composed program.  Because the composition lives in the
/// binary crate's root, `<crate>::LangProgram` (used by `server::main`) is the
/// composed program — see the module docs for why a bin-only crate, not
/// lib+bin.
fn write_server_main_rs(dir: &Path, plugins: &[Depend], leaves: &Leaves) -> Result<(), String> {
    let lines = format!(
        r#"//! A language server composed over the project's plugin set.  Generated by
//! `liche path language-server --project`; re-run it whenever the
//! native-plugin set changes.

{compose}

fn main() {{
    liche_language_server::server::main::<crate::LangProgram>()
}}
"#,
        compose = compose_source(plugins, leaves),
    );
    std::fs::write(dir.join("src/main.rs"), lines).map_err(|e| format!("write src/main.rs: {e}"))
}

#[cfg(test)]
mod generated_main_tests {
    use super::*;

    #[test]
    fn compiler_main_scopes_the_artifact_cache_to_its_plugin_set_slot() {
        let dir = std::env::temp_dir().join(format!("lichen-plugin-main-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A compiler is built into `compilers/<key>/`; its generated `main`
        // must scope the compile-artifact cache to that slot, not the shared
        // lichen home (so a different plugin set never reuses this vocabulary's
        // artifacts for the same file ID).
        let key = "deadbeef";
        let slot = dir.join(key);
        std::fs::create_dir_all(slot.join("src")).unwrap();
        write_compiler_main_rs(&slot, &[], &Leaves::shipping()).expect("write the generated main");
        let main = std::fs::read_to_string(slot.join("src/main.rs")).unwrap();
        assert!(
            main.contains("main_with_native_packages"),
            "the generated compiler must register the plugin's native packages:\n{main}"
        );
        assert!(
            main.contains(&format!(".join(\"{key}\")")),
            "the generated compiler must root its cache at its own slot:\n{main}"
        );
        assert!(
            main.contains("lichen_language::persist::lichendir()"),
            "the generated compiler must root the cache under the lichen home:\n{main}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_package_lines_register_each_plugins_wrapper() {
        // A generated compiler must register each plugin's native package so its
        // wrapper (`$sort` etc.) resolves against the plugin's own registry —
        // the tuple shape `cli::main_with_native_packages` expects.
        let dep = Depend {
            url: "file:///C:/work/lichen-vm".into(),
            name: "std".into(),
            rev: None,
            branch: None,
            tag: None,
            package: Some("lichen-std-native".into()),
            sub: Some("lichen-std-native/src".into()),
            plugin: true,
        };
        let lines = native_package_lines(&[dep]);
        assert!(
            lines.contains(
                "(\"std.lichen\", lichen_std_native::WRAPPER_SOURCE, \
                 lichen_std_native::lichen_std_native_ops!(crate::LangProgram)),"
            ),
            "must register the plugin's wrapper at its alias:\n{lines}"
        );
        assert!(native_package_lines(&[]).is_empty());
    }

    #[test]
    fn core_patch_redirects_plugin_core_deps_to_a_local_repo() {
        // A local (non-default) core_repo must add a `[patch]` redirecting the
        // crates a native plugin git-deps against the canonical repo to the
        // local source, so the generated compositor resolves offline.
        let patch = core_patch("file:///C:/work/lichen-vm");
        assert!(
            patch.contains("[patch.\"https://github.com/windwhiterain/lichen-vm\"]"),
            "must patch the canonical repo:\n{patch}"
        );
        for crate_name in ["lichen-utils", "lichen-lowlevel", "lichen-highlevel"] {
            assert!(
                patch.contains(&format!(
                    "git = \"file:///C:/work/lichen-vm\", package = \"{crate_name}\""
                )),
                "must redirect {crate_name} to the local repo:\n{patch}"
            );
        }
        // The default (canonical) core_repo needs no patch — its git deps already
        // point at the same source.
        assert_eq!(core_patch("https://github.com/windwhiterain/lichen-vm"), "");
    }
}
