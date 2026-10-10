//! Rebuilding the compiler / language server for a native plugin.
//! See docs/notes/plugin-taxonomy.md.
//!
//! # Invariant
//!
//! The generated crate is bin-only: in a lib+bin package `crate::` is the binary crate, which
//! would carry no composition, so `<crate>::LangProgram` has to be a root item of the binary.
//! The generated `main` roots the artifact cache at its own plugin-set slot.

use std::path::{Path, PathBuf};
use std::process::Command;

use lichen_preprocess::Depend;

use crate::git;

/// The value/operator/attr leaves a plugin contributes to the vocabulary.
///
/// # Invariant
///
/// A plugin's leaves are its own item names, spelled at the call site — the generator cannot
/// infer them from the crate.
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

/// Build a compiler crate at `dir` (the cache slot) over `leaves` and the plugins.
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
        // Only the compiler build needs the CLI crate, so it is passed per call.
        &format!("\n{}", core_dep_line(core_repo, "lichen-compiler")),
    )?;
    write_compiler_main_rs(dir, plugins, leaves)?;

    cargo_build(dir)?;
    let bin = dir.join("target").join("release").join(bin_name(name));
    Ok(CompilerBuild {
        dir: dir.to_path_buf(),
        bin,
    })
}

/// Build the language-server crate at `dir` (the cache slot) over `leaves` and the
/// plugins.
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

    cargo_build(dir)?;
    let bin = dir
        .join("target")
        .join("release")
        .join(server_bin_name(name));
    Ok(ServerBuild {
        dir: dir.to_path_buf(),
        bin,
    })
}

/// `cargo build --release` the generated crate at `dir`, surfacing its stderr on
/// failure.
fn cargo_build(dir: &Path) -> Result<(), String> {
    let out = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("cannot run cargo build: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!(
        "rebuild failed:\n{}",
        String::from_utf8_lossy(&out.stderr).trim()
    ))
}

/// Whether `cargo` is on `$PATH`.
pub fn cargo_available() -> bool {
    crate::tool_available("cargo")
}

/// The compiled binary name for a compiler named `name`.
pub fn bin_name(name: &str) -> String {
    format!("lichen-compiler-{name}{}", crate::toolchain::exe_suffix())
}

/// The compiled binary name for a language server named `name`.
pub fn server_bin_name(name: &str) -> String {
    format!(
        "lichen-language-server-{name}{}",
        crate::toolchain::exe_suffix()
    )
}

/// The TOML basic-string literal for `value`: the only way a value reaches the
/// generated manifest.
///
/// # Invariant
///
/// Every interpolated value (a dependency key, a path, a URL, a revision, a package name) goes
/// through here, so a `"`, a backslash, or a control character cannot end the literal or become
/// an escape of its own.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            // The remaining control characters (and DEL) have no short escape.
            ch if (ch as u32) < 0x20 || ch == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// The `lichen-language-server` dependency line: a path dep for a local
/// `core_repo`, else a git dep, with `server` on.
fn server_dep(core_repo: &str) -> String {
    let key = toml_string("lichen-language-server");
    if std::path::Path::new(core_repo).is_dir() {
        let rel = format!("{core_repo}/crates/lichen-language-server");
        format!(
            "{key} = {{ path = {}, default-features = false, features = [\"server\"] }}",
            toml_string(&rel)
        )
    } else {
        format!(
            "{key} = {{ git = {}, default-features = false, features = [\"server\"] }}",
            toml_string(core_repo)
        )
    }
}

/// Write the generated crate's `Cargo.toml` from `core_repo`, the plugins, and
/// `extra_deps`.
///
/// # Invariant
///
/// Every value in the document is written through [`toml_string`], so no value can end the
/// literal it sits in. `extra_deps` holds the dependency lines only one generated crate needs:
/// [`server_dep`] for the LSP crate, [`core_dep_line`] with `"lichen-compiler"` for the compiler.
fn write_cargo_toml(
    dir: &Path,
    package_name: &str,
    core_repo: &str,
    plugins: &[Depend],
    extra_deps: &str,
) -> Result<(), String> {
    let toml = format!(
        r#"[package]
name = {package_name}
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
        package_name = toml_string(package_name),
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

/// A core-crate dependency line: a path dep for a local `core_repo`, else a git dep
/// naming the workspace member.
fn core_dep_line(core_repo: &str, crate_name: &str) -> String {
    let key = toml_string(crate_name);
    if std::path::Path::new(core_repo).is_dir() {
        let rel = format!("{core_repo}/crates/{crate_name}");
        format!("{key} = {{ path = {} }}", toml_string(&rel))
    } else {
        format!(
            "{key} = {{ git = {}, package = {} }}",
            toml_string(core_repo),
            toml_string(crate_name)
        )
    }
}

/// The `[patch]` section for a non-default `core_repo`: it redirects the crates a
/// plugin git-deps to the local source.
///
/// # Invariant
///
/// Without it, cargo reaches the canonical repo over the network and the plugin's git core crates
/// are *distinct instances* from the compositor's, so the plugins' trait impls do not unify. Only
/// the crates a native plugin git-deps are patched; the canonical repo needs no patch.
fn core_patch(core_repo: &str) -> String {
    if core_repo == crate::toolchain::DEFAULT_REPO {
        return String::new();
    }
    let mut out = format!(
        "\n[patch.{}]\n",
        toml_string(crate::toolchain::DEFAULT_REPO)
    );
    for crate_name in ["lichen-utils", "lichen-lowlevel", "lichen-highlevel"] {
        out.push_str(&core_dep_line(core_repo, crate_name));
        out.push('\n');
    }
    out
}

/// The plugin dependency lines: a path dep when the plugin's `url` exists here,
/// else a git dep at the pinned revision.
fn plugin_lines(plugins: &[Depend]) -> String {
    let mut plugin_lines = String::new();
    for dep in plugins {
        let crate_name = git::crate_name(dep);
        let key = toml_string(&crate_name);
        if std::path::Path::new(&dep.url).exists() {
            plugin_lines.push_str(&format!("{key} = {{ path = {} }}\n", toml_string(&dep.url)));
        } else {
            let rev = git::checkout(dep)
                .map(|r| format!(", rev = {}", toml_string(r)))
                .unwrap_or_default();
            // A remote plugin is a git dep on a crate in the plugin's workspace;
            // `package` names the member so cargo finds it there.
            plugin_lines.push_str(&format!(
                "{key} = {{ git = {}, package = {}{rev} }}\n",
                toml_string(&dep.url),
                toml_string(&crate_name)
            ));
        }
    }
    plugin_lines
}

/// The Rust keywords, reserved words included, refused as crate identifiers.
const RUST_KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl", "in",
    "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "try", "type",
    "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Whether `value` is spelled in the identifier alphabet the generated source can carry.
///
/// # Invariant
///
/// `[A-Za-z_][A-Za-z0-9_]*` is the alphabet the preprocessor lexes a binding name from, so a name
/// from a source file passes and a hand-composed `Depend` is refused rather than written into code
/// or a string literal.
fn is_identifier_name(value: &str) -> bool {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// The Rust identifier for a plugin dependency: its crate name with `-` as `_`.
///
/// # Invariant
///
/// The identifier is written into generated Rust source, and `package` is lexed with `"[^"@]*"` (no
/// escapes, may be multiline), so it is refused unless it is a valid identifier: a newline or a `}`
/// would close the generated item and inject code that `cargo build` then compiles.
fn crate_ident(dep: &Depend) -> Result<String, String> {
    let name = git::crate_name(dep);
    let ident = name.replace('-', "_");
    if !is_identifier_name(&ident) || RUST_KEYWORDS.contains(&ident.as_str()) {
        return Err(format!(
            "plugin '{}' has package name '{name}', which is not a Rust crate identifier",
            dep.alias()
        ));
    }
    Ok(ident)
}

/// The composed-vocabulary body shared by both generated crates: the
/// `lang_compose_vocabulary!` call and `Program` alias.
fn compose_source(plugins: &[Depend], leaves: &Leaves) -> Result<String, String> {
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
        let ident = crate_ident(dep)?;
        plugin_line.push_str(&format!("    {ident} as {ident}_leaves;\n"));
    }
    Ok(format!(
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
    ))
}

/// The native-package tuples a generated compiler registers, one per plugin, at
/// `<alias>.lichen`.
///
/// # Invariant
///
/// The alias is written into a Rust string literal, so it is refused unless it is spelled in the
/// same identifier alphabet: a hand-composed `Depend` carrying a `"` cannot end the literal and
/// inject code after it.
fn native_package_lines(plugins: &[Depend]) -> Result<String, String> {
    let mut out = String::new();
    for dep in plugins {
        let ident = crate_ident(dep)?;
        let alias = dep.alias();
        if !is_identifier_name(&alias) {
            return Err(format!(
                "plugin alias '{alias}' is not a name the generated source can spell"
            ));
        }
        out.push_str(&format!(
            "            (\"{alias}.lichen\", {ident}::WRAPPER_SOURCE, {ident}::{ident}_ops!(crate::LangProgram)),\n"
        ));
    }
    Ok(out)
}

/// The generated compiler crate's `src/main.rs`: the composition, then the shared
/// CLI over the composed program.
///
/// # Invariant
///
/// The crate is bin-only, so `<crate>::LangProgram` is a root item and
/// `lichen_compiler::cli::main::<crate::LangProgram>()` resolves; in a lib+bin package `crate::`
/// is the binary crate, which would carry no composition. The cache root is the plugin-set slot.
fn write_compiler_main_rs(dir: &Path, plugins: &[Depend], leaves: &Leaves) -> Result<(), String> {
    // The slot directory's base name is the plugin-set cache key that placed it here
    // (`compiler_cache::key`).
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
    lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>(
        &cache_root,
        &[
{native}        ],
    )
}}
"#,
        compose = compose_source(plugins, leaves)?,
        key = key,
        native = native_package_lines(plugins)?,
    );
    std::fs::write(dir.join("src/main.rs"), lines).map_err(|e| format!("write src/main.rs: {e}"))
}

/// The generated language-server crate's `src/main.rs`: the composition, then a
/// `main` driving the shared server.
fn write_server_main_rs(dir: &Path, plugins: &[Depend], leaves: &Leaves) -> Result<(), String> {
    // The slot directory's base name is the plugin-set cache key that placed it here
    // (`compiler_cache::key`).
    let key = dir
        .file_name()
        .map(|k| k.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lines = format!(
        r#"//! A language server composed over the project's plugin set.  Generated by
//! `lichen path language-server --project`; re-run it whenever the
//! native-plugin set changes.

{compose}

fn main() {{
    // Scope the compiled-artifact cache to this plugin-set slot, consistent with
    // the compiler (`write_compiler_main_rs`), so a server over a different
    // plugin set never collides with (or reuses) this vocabulary's artifacts for
    // the same file ID.
    let cache_root = lichen_language::persist::lichendir()
        .join("compilers")
        .join("{key}");
    lichen_language_server::server::main::<crate::LangProgram>(&cache_root);
}}
"#,
        compose = compose_source(plugins, leaves)?,
        key = key,
    );
    std::fs::write(dir.join("src/main.rs"), lines).map_err(|e| format!("write src/main.rs: {e}"))
}

#[cfg(test)]
#[path = "tests/plugin_manifest_tests.rs"]
mod plugin_manifest_tests;

#[cfg(test)]
#[path = "tests/plugin_source_tests.rs"]
mod plugin_source_tests;

#[cfg(test)]
mod generated_main_tests {
    use super::*;

    #[test]
    fn compiler_main_scopes_the_artifact_cache_to_its_plugin_set_slot() {
        let dir = std::env::temp_dir().join(format!("lichen-plugin-main-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The generated `main` must root the cache at its own slot: another plugin
        // set must not reuse these artifacts.
        let key = "deadbeef";
        let slot = dir.join(key);
        std::fs::create_dir_all(slot.join("src")).unwrap();
        write_compiler_main_rs(&slot, &[], &Leaves::shipping()).expect("write the generated main");
        let main = std::fs::read_to_string(slot.join("src/main.rs")).unwrap();
        assert!(
            main.contains("lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>("),
            "the generated compiler must drive the CLI crate's entry point over its composed \
             program:\n{main}"
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
    fn language_server_main_scopes_the_artifact_cache_to_its_plugin_set_slot() {
        let dir = std::env::temp_dir().join(format!("lichen-server-main-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The generated LSP `main` must root the cache at its own plugin-set slot,
        // not the shared lichen home.
        let key = "cafebabe";
        let slot = dir.join(key);
        std::fs::create_dir_all(slot.join("src")).unwrap();
        write_server_main_rs(&slot, &[], &Leaves::shipping()).expect("write the generated main");
        let main = std::fs::read_to_string(slot.join("src/main.rs")).unwrap();
        assert!(
            main.contains(&format!(".join(\"{key}\")")),
            "the generated server must root its cache at its own slot:\n{main}"
        );
        assert!(
            main.contains("server::main::<crate::LangProgram>(&cache_root)"),
            "the generated server must pass its cache root to server::main:\n{main}"
        );
        assert!(
            main.contains("lichen_language::persist::lichendir()"),
            "the generated server must root the cache under the lichen home:\n{main}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_package_lines_register_each_plugins_wrapper() {
        // Each plugin's native package must be registered so its wrapper resolves
        // against the plugin's own registry.
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
        let lines =
            native_package_lines(&[dep]).expect("a well-formed plugin registers its wrapper");
        assert!(
            lines.contains(
                "(\"std.lichen\", lichen_std_native::WRAPPER_SOURCE, \
                 lichen_std_native::lichen_std_native_ops!(crate::LangProgram)),"
            ),
            "must register the plugin's wrapper at its alias:\n{lines}"
        );
        assert!(
            native_package_lines(&[])
                .expect("no plugins, no lines")
                .is_empty()
        );
    }

    #[test]
    fn core_patch_redirects_plugin_core_deps_to_a_local_repo() {
        // A local `core_repo` must patch a plugin's core deps to the local source,
        // so the build resolves offline.
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
