//! Rebuilding the compiler for a native plugin.
//!
//! A *native plugin* (see [`docs/notes/plugin-taxonomy.md`]) extends the
//! compiler's value/operator vocabulary at **compile time**: it contributes
//! enum leaves to the `Program` marker, so a compiler that knows a plugin
//! must be built with that plugin composed into its vocabulary.  That is what
//! this module does — "when a native plugin is imported, rebuild the
//! compiler."
//!
//! The mechanism: generate a compiler crate under a caller-chosen directory
//! (the package manager's compiler cache under the lichen home — see
//! [`crate::compiler_cache`]) that depends on the plugin (from git or a local
//! path) and composes its vocabulary with the shipping leaves via
//! `liche_language::lang_compose_vocabulary!`, then run `cargo build` and
//! report the produced `lichen-compiler` binary.
//!
//! **Status:** the *composition* scaffold is real — the generated crate
//! `cargo check`s once the plugin's leaves exist.  The *tooling* of a
//! generated compiler (its package store, persist codec, CLI, and `run`
//! path) is currently monomorphic over the shipped `LangProgram`, so a
//! compiler built with an *additional* plugin cannot yet route through the
//! language crate's store/run machinery; that generalization — turning the
//! language layer's tooling generic over the `Program` marker — is the
//! tracked follow-up in [`docs/notes/plugin-taxonomy.md`].  A rebuild over the
//! shipping plugin set produces a fully working compiler.

use std::path::{Path, PathBuf};
use std::process::Command;

use lichen_language::preprocess::Depend;

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
                ("liche_lowlevel::LowValue", "LowValue"),
                ("liche_highlevel::program::TypeValue", "TypeValue"),
                ("liche_compute::ComputeValue", "ComputeValue"),
            ],
            operators: vec![
                ("liche_lowlevel::LowOperator", "LowOperator"),
                ("liche_highlevel::program::TypeOperator", "TypeOperator"),
                ("liche_perspective::GcdOp", "GcdOp"),
                ("liche_compute::ComputeOperator", "ComputeOperator"),
            ],
            attrs: vec![
                ("liche_perspective::Perspective", "Perspective"),
                ("liche_doc::Doc", "Doc"),
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
    write_cargo_toml(dir, name, core_repo, plugins)?;
    write_lib_rs(dir, plugins, leaves)?;
    write_main_rs(dir)?;

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

/// The generated crate's `Cargo.toml`: depends on the language crate and the
/// plugin dependencies (from a local path when `git` is a path that exists).
fn write_cargo_toml(
    dir: &Path,
    name: &str,
    core_repo: &str,
    plugins: &[Depend],
) -> Result<(), String> {
    let mut plugin_lines = String::new();
    for dep in plugins {
        let crate_name = git::crate_name(dep);
        if std::path::Path::new(&dep.url).exists() {
            plugin_lines.push_str(&format!("{crate_name} = {{ path = \"{}\" }}\n", dep.url));
        } else {
            let rev = git::checkout(dep)
                .map(|r| format!(", rev = \"{r}\""))
                .unwrap_or_default();
            plugin_lines.push_str(&format!(
                "{crate_name} = {{ git = \"{}\"{rev} }}\n",
                dep.url
            ));
        }
    }
    // Core crates come from a local checkout (path deps) when `core_repo` is a
    // directory here, else from git.
    let core_is_path = std::path::Path::new(core_repo).is_dir();
    let core_dep = |crate_name: &str| -> String {
        if core_is_path {
            let rel = format!("{core_repo}/crates/{crate_name}");
            format!("{crate_name} = {{ path = \"{rel}\" }}")
        } else {
            format!("{crate_name} = {{ git = \"{core_repo}\" }}")
        }
    };
    let toml = format!(
        r#"[package]
name = "lichen-compiler-{name}"
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
{plugin_lines}"#,
        core_language = core_dep("liche-language"),
        core_lowlevel = core_dep("liche-lowlevel"),
        core_highlevel = core_dep("liche-highlevel"),
        core_compute = core_dep("liche-compute"),
        core_perspective = core_dep("liche-perspective"),
        core_doc = core_dep("liche-doc"),
        core_utils = core_dep("liche-utils"),
    );
    std::fs::write(dir.join("Cargo.toml"), toml).map_err(|e| format!("write Cargo.toml: {e}"))
}

/// The generated crate's `src/lib.rs`: compose the program marker over the
/// shipping leaves plus the plugin's contributed leaves.  The shipping leaves
/// are spelled inline (with the `plugins = [...]` arm composing each native
/// plugin's own leaves via its `liche_leaves!` macro — no config file).
fn write_lib_rs(dir: &Path, plugins: &[Depend], leaves: &Leaves) -> Result<(), String> {
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
        plugin_line.push_str(&format!("    {};\n", git::crate_name(dep)));
    }
    let lines = format!(
        r#"//! A compiler built over the project's plugin set.  Generated by
//! `lichen rebuild-plugin`; re-run it whenever the native-plugin set changes.

// The value/operator/attribute vocabulary and the program marker, composed
// from the shipping leaves plus each plugin's own `liche_leaves!` (the
// `plugins = [...]` arm stitches their leaves in — no config file).
liche_language::lang_compose_vocabulary! {{
    attrs = [
{attrs}    ]
    [ P::Operator: From<liche_perspective::GcdOp> ];
    values = [
{values}    ];
    operators = [
{operators}    ];
    plugins = [
{plugin_line}    ];
}}

/// The composed program marker for this compiler build.
pub type Program = LangProgram;
"#,
        attrs = attrs,
        values = values,
        operators = operators,
        plugin_line = plugin_line,
    );
    std::fs::write(dir.join("src/lib.rs"), lines).map_err(|e| format!("write src/lib.rs: {e}"))
}

/// The generated crate's `src/main.rs`: a thin compiler CLI over the language
/// crate's library.  It shares [`liche_language::cli`], so a plugin-built
/// compiler speaks the same dialect as the shipped `lichen-compiler` and is
/// already depend-aware (resolving `depend` directives against the source
/// cache).  A compiler built with an *additional* plugin keeps this shell; the
/// plugin's leaves are declared in `lib.rs` and the tooling generalization is
/// the tracked follow-up.
fn write_main_rs(dir: &Path) -> Result<(), String> {
    let lines = r#"fn main() -> std::process::ExitCode {
    liche_language::cli::main()
}
"#;
    std::fs::write(dir.join("src/main.rs"), lines).map_err(|e| format!("write src/main.rs: {e}"))
}
