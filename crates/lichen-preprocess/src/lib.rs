//! The preprocessor for the `@{...@}` directive block.
//!
//! A file may open with a single `@{...@}` block (once, before any code; a
//! non-`@` prefix is allowed and ignored).  Inside the block is a set of
//! statements, Separator-separated: `name = import "path"` loads a package
//! bound to `name`, `name = depend "url"` declares a git dependency bound to
//! `name`, and `name = "value"` defines a string metadata entry.
//! The language lexer/parser never sees the block: it is cut out by a pure
//! byte scan (independent of the lexer), the interior is parsed by this
//! module's own little frontend (see [lex] and [parse]), and the caller
//! compiles only the code that follows the block.
//!
//! The leading non-`@` bytes before the block are ignored; `@` is reserved
//! for the block delimiters, so it cannot appear in the surrounding code or
//! inside a block string.  A file with no `@` is ordinary code (code = the
//! whole source, base = 0).
//!
//! The preprocessor is allocation-light: `code` is a borrowed suffix of the
//! source (no copy), and only the metadata values (tiny) are owned.
//!
//! This crate is **isolated**: it depends only on the shared source-position
//! protocol ([`liche_span`]) and its own block lexer/parser (`logos`).  It
//! never names a compile vocabulary or a package store.  Resolving an
//! `import "path"` against a package store is delegated to a caller through
//! the [`ImportResolver`] trait, and the only vocabulary-bound data it
//! carries is generic over the export handle `E` (see [`ResolvedImport`]),
//! which the language layer pins to its own package handle.
//!
//! The preprocessor also **owns the preprocessor import path**: the lichen
//! home ([`lichendir`]) and its git source cache ([`sources_root`]), the root
//! the package manager fetches each `depend` into and the compiler reads the
//! vendored aliases from (see [`Depend::vendored_dir`]).

use std::path::{Path, PathBuf};

use lichen_span::{Span, line_col, line_starts};

mod lex;
mod parse;

pub use parse::Directive;

/// A preprocessor diagnostic: a message plus the source position it is
/// grounded in.  Check-free — the preprocessor never touches a checker or a
/// program marker — so it is not typed over a vocabulary.  A caller (the
/// language layer) widens it into its own diagnostic at `Stage::Preprocess`.
#[derive(Clone, Debug)]
pub struct PreprocessDiag {
    pub span: Option<Span>,
    pub message: String,
}

impl PreprocessDiag {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        PreprocessDiag {
            span: Some(span),
            message: message.into(),
        }
    }

    pub fn at_zero(message: impl Into<String>) -> Self {
        PreprocessDiag {
            span: Some((0, 0)),
            message: message.into(),
        }
    }
}

/// A resolved package: what an `import "path"` binding resolves to, before it
/// is wrapped with the binding's own `name` and `span`.  The export handle `E`
/// is vocabulary-bound (the language layer pins it to its static node id), so
/// this crate needs no dependency on the lowlevel/VM stack.
#[derive(Clone, Debug)]
pub struct ResolvedPackage<E> {
    pub export: E,
    pub path: PathBuf,
    /// Extra `(name, export)` bindings the package exposes directly, so
    /// `import` can bind them as names (the compute package's
    /// `jit`/`launch`/`Kernel`).
    pub direct: Vec<(String, E)>,
}

/// The import-resolution seam.  A caller provides a `PackageStore`-like object
/// implementing this: it resolves an `import "path"` (relative to `base`), and
/// registers a vendored dependency alias the package manager staged into the
/// source cache.  Generic over the export handle `E` so the preprocessor crate
/// stays free of any compile vocabulary.
pub trait ImportResolver<E> {
    /// Resolve an import path relative to the current source file's directory
    /// (`base` is the file or directory the import is resolved against).
    fn resolve_import(
        &mut self,
        base: Option<&Path>,
        import_path: &str,
    ) -> Result<ResolvedPackage<E>, PreprocessDiag>;

    /// Register a vendored dependency directory under an import alias, so the
    /// compiler resolves `import "alias"` / `import "alias/rest"` into it.
    fn register_vendored(&mut self, alias: String, dir: PathBuf);
}

/// One resolved `name = import "path"` binding.
#[derive(Clone, Debug)]
pub struct ResolvedImport<E> {
    /// The binding name the import is available under.
    pub name: String,
    /// The (line, column) of the import statement in the original file.
    pub span: Span,
    /// The package's exported final [value, type] pair node.
    pub export: E,
    /// The canonical path of the imported package.
    pub path: PathBuf,
    /// Extra `(name, export)` bindings the package exposes directly (the
    /// compute package's `jit`/`launch`/`Kernel`), bound as names alongside
    /// the import's own `name`.
    pub direct: Vec<(String, E)>,
}

/// A git dependency declared by a `name = depend "url"` directive in the block.
/// The package manager fetches it (into the lichen-home source cache) and
/// stages it as a vendored alias before resolving the block's `import`
/// bindings.  `name` is the left-hand side binding, the alias the dependency
/// is staged under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Depend {
    /// The git repository URL.
    pub url: String,
    /// The import alias the dependency is staged under (`name = depend`).
    pub name: String,
    /// A pinned revision, branch, or tag.
    pub rev: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    /// The Rust crate package name (a native-plugin dependency).
    pub package: Option<String>,
    /// A subdirectory of the fetched repository holding the package
    /// (a monorepo dependency): the vendored alias resolves to
    /// `<clone>/<sub>` instead of the clone root.
    pub sub: Option<String>,
    /// A native plugin: importing it requires the compiler to be rebuilt.
    pub plugin: bool,
}

impl Depend {
    /// The import alias the dependency is staged under: its binding name
    /// (`name = depend`).  This is the key both the package manager (when it
    /// fetches into the source cache) and the compiler (when it resolves the
    /// vendored alias) use, so the two agree by construction.
    pub fn alias(&self) -> String {
        self.name.clone()
    }

    /// The git clone root in the source cache: `sources_root()/<alias>`.
    /// The package manager clones (or fetches) a dependency here; the compiler
    /// reads the same path when it resolves the vendored alias.
    pub fn sources_dir(&self) -> PathBuf {
        sources_root().join(sanitize_alias(&self.alias()))
    }

    /// The vendored directory the alias resolves to: the clone root, or its
    /// `sub` subdirectory (a monorepo dependency).
    pub fn vendored_dir(&self) -> PathBuf {
        match &self.sub {
            Some(sub) => self.sources_dir().join(sub),
            None => self.sources_dir(),
        }
    }
}

/// Sanitize an alias into a filesystem-safe directory name.
fn sanitize_alias(alias: &str) -> String {
    let mut out = String::new();
    for ch in alias.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => out.push(ch),
            _ => out.push('_'),
        }
    }
    if out.is_empty() {
        out.push_str("dep");
    }
    out
}

/// The preprocessor output: the borrowed code (a suffix of the source), the
/// byte offset where it starts (so spans map back to the original file),
/// resolved imports, the block's string metadata, and its git dependency set.
#[derive(Clone, Debug)]
pub struct Preprocessed<'a, E> {
    /// The code to compile: the source after the `@{...@}` block (or the
    /// whole source when there is no block).  Borrowed, never copied.
    pub code: &'a str,
    /// The byte offset of `code` within the original source.
    pub code_base: u32,
    /// Resolved import bindings, in source order.
    pub imports: Vec<ResolvedImport<E>>,
    /// String metadata entries `(name, value)` from the block.
    pub metadata: Vec<(String, String)>,
    /// The `depend "url"` directives from the block (git dependencies).
    pub depends: Vec<Depend>,
}

/// Preprocess: cut out the leading `@{...@}` block (if any), resolve its
/// import bindings through `resolver`, and collect its metadata.  The code to
/// compile is the source after the block.  Diagnostics (lex/parse/resolve)
/// are reported with spans against the original file.
pub fn preprocess<'a, E, R>(
    raw: &'a str,
    base: Option<&Path>,
    resolver: &mut R,
) -> (Preprocessed<'a, E>, Vec<PreprocessDiag>)
where
    R: ImportResolver<E>,
{
    let mut diags = Vec::new();

    let Some((interior_start, interior_end, code_start)) = scan_block(raw) else {
        // No block: the whole source is ordinary code.
        return (
            Preprocessed {
                code: raw,
                code_base: 0,
                imports: Vec::new(),
                metadata: Vec::new(),
                depends: Vec::new(),
            },
            diags,
        );
    };

    let interior = &raw[interior_start..interior_end];
    let code = &raw[code_start..];
    let starts = line_starts(raw);

    let mut imports = Vec::new();
    let mut metadata = Vec::new();
    let mut depends = Vec::new();

    let lexed = lex::tokenize(interior);
    for err in &lexed.errors {
        let global = interior_start as u32 + err.offset;
        diags.push(PreprocessDiag::new(
            line_col(&starts, global),
            err.message.clone(),
        ));
    }
    if lexed.errors.is_empty() {
        match parse::parse(&lexed.tokens) {
            Ok(dirs) => {
                for (dir, off) in dirs {
                    let global = interior_start as u32 + off;
                    let span = line_col(&starts, global);
                    match dir {
                        Directive::Import { name, path } => {
                            match resolver.resolve_import(base, &path) {
                                Ok(pkg) => imports.push(ResolvedImport {
                                    name,
                                    span,
                                    export: pkg.export,
                                    path: pkg.path,
                                    direct: pkg.direct,
                                }),
                                Err(mut diag) => {
                                    diag.span = Some(span);
                                    diag.message =
                                        format!("cannot load package '{}': {}", path, diag.message);
                                    diags.push(diag);
                                }
                            }
                        }
                        Directive::Metadata { name, value } => metadata.push((name, value)),
                        Directive::Depend {
                            url,
                            name,
                            rev,
                            branch,
                            tag,
                            package,
                            sub,
                            plugin,
                        } => depends.push(Depend {
                            url,
                            name,
                            rev,
                            branch,
                            tag,
                            package,
                            sub,
                            plugin,
                        }),
                        Directive::Plug {
                            url,
                            name,
                            rev,
                            branch,
                            tag,
                            package,
                            sub,
                        } => depends.push(Depend {
                            url,
                            name,
                            rev,
                            branch,
                            tag,
                            package,
                            sub,
                            plugin: true,
                        }),
                    }
                }
            }
            Err(errors) => {
                for (off, message) in errors {
                    let global = interior_start as u32 + off;
                    diags.push(PreprocessDiag::new(line_col(&starts, global), message));
                }
            }
        }
    }

    (
        Preprocessed {
            code,
            code_base: code_start as u32,
            imports,
            metadata,
            depends,
        },
        diags,
    )
}

/// Split a source into its leading `@{...@}` interior (if any) and the code
/// after it.  Never resolves imports -- for tooling (readme, sync) that reads
/// a block's metadata without a package store.
pub fn split_block(source: &str) -> (Option<&str>, &str) {
    if let Some((is, ie, cs)) = scan_block(source) {
        (Some(&source[is..ie]), &source[cs..])
    } else {
        (None, source)
    }
}

/// Parse a block interior into its directives (imports + metadata), in order.
/// Empty when the interior fails to lex/parse (the block is unusable).
pub fn block_directives(interior: &str) -> Vec<Directive> {
    let lexed = lex::tokenize(interior);
    if !lexed.errors.is_empty() {
        return Vec::new();
    }
    parse::parse(&lexed.tokens)
        .unwrap_or_default()
        .into_iter()
        .map(|(d, _)| d)
        .collect()
}

/// The string metadata `(name, value)` entries of a block interior (
/// import bindings excluded), in order.
pub fn block_metadata(interior: &str) -> Vec<(String, String)> {
    block_directives(interior)
        .into_iter()
        .filter_map(|d| match d {
            Directive::Metadata { name, value } => Some((name, value)),
            _ => None,
        })
        .collect()
}

/// The `Depend`s in a block interior: `name = depend "url"` git bindings and
/// `name = plug "url"` native-plugin bindings, both normalized to a [`Depend`]
/// (a `plug` is a plugin dep bound to the statement's name).
pub fn block_depends(interior: &str) -> Vec<Depend> {
    block_directives(interior)
        .into_iter()
        .filter_map(depend_of)
        .collect()
}

/// Normalize a [`Directive`] to a [`Depend`]: a `depend` passes through, a
/// `plug` becomes a plugin dep (`plugin: true`) bound to the statement's name.
pub fn depend_of(dir: Directive) -> Option<Depend> {
    match dir {
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
        Directive::Plug {
            url,
            name,
            rev,
            branch,
            tag,
            package,
            sub,
        } => Some(Depend {
            url,
            name,
            rev,
            branch,
            tag,
            package,
            sub,
            plugin: true,
        }),
        _ => None,
    }
}

/// Stage a source's `depend "url"` / `name = plug "url"` directives onto
/// `resolver` as vendored aliases, resolving each against the lichen-home
/// source cache (see [`Depend::vendored_dir`]).  A dependency that has not
/// been fetched by the package manager (`lichen fetch`) is reported as a
/// preprocess diagnostic naming the missing dir — the compiler never fetches
/// git sources itself, it only reads what the package manager put in the
/// cache.
pub fn stage_depends<E, R>(resolver: &mut R, source: &str) -> Vec<PreprocessDiag>
where
    R: ImportResolver<E>,
{
    let mut diags = Vec::new();
    let (interior, _) = split_block(source);
    let Some(interior) = interior else {
        return diags;
    };
    for dep in block_depends(interior) {
        let alias = dep.alias();
        let dir = dep.vendored_dir();
        if dir.is_dir() {
            resolver.register_vendored(alias, dir);
        } else {
            diags.push(PreprocessDiag::at_zero(format!(
                "dependency '{alias}' is not fetched (expected {}) — run `lichen fetch` first",
                dir.display()
            )));
        }
    }
    diags
}

/// Locate the leading `@{...@}` block in `raw` by a pure byte scan.  Returns
/// the byte ranges of the interior and the start of the code that follows the
/// block.  `None` when there is no `@` (no block).  `@` is reserved, so the
/// first `@` is the block open and the first `@}` is its close -- it cannot
/// appear inside a string or in code.
fn scan_block(raw: &str) -> Option<(usize, usize, usize)> {
    let at = raw.find('@')?;
    if !raw[at..].starts_with("@{") {
        return None;
    }
    let rest = &raw[at + 2..];
    let close = rest.find("@}")?;
    let interior_start = at + 2;
    let interior_end = at + 2 + close;
    let mut code_start = at + 2 + close + 2;
    // Skip the newline (or CRLF) that terminates the block line, so the code
    // begins at its first real character (a leading Separator would be
    // harmless, but this keeps the code text and rendered output tidy).
    let bytes = raw.as_bytes();
    if bytes.get(code_start) == Some(&b'\n') {
        code_start += 1;
    } else if bytes.get(code_start) == Some(&b'\r') && bytes.get(code_start + 1) == Some(&b'\n') {
        code_start += 2;
    }
    Some((interior_start, interior_end, code_start))
}

/// The device's cache directory: `$LICHEN_HOME` when set, otherwise
/// `~/.lichen`.
pub fn lichendir() -> PathBuf {
    if let Some(home) = std::env::var_os("LICHEN_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home {
        Some(home) => PathBuf::from(home).join(".lichen"),
        None => PathBuf::from(".lichen"),
    }
}

/// The source-cache subdir name, under the lichen home.  The package manager
/// keeps each fetched git dependency under `sources/<alias>`; the compiler
/// resolves a file's `depend "url"` directives against this same root.
pub const SOURCES_DIR: &str = "sources";

/// The root of the git source cache: the lichen home's `sources/` directory.
pub fn sources_root() -> PathBuf {
    lichendir().join(SOURCES_DIR)
}

#[cfg(test)]
mod tests {
    use super::{block_depends, depend_of, scan_block};
    use crate::Directive;

    #[test]
    fn no_block_is_the_whole_source() {
        assert_eq!(scan_block("a = 1\na"), None);
    }

    #[test]
    fn a_leading_block_is_located() {
        assert_eq!(scan_block("@{order = \"3\"@}\na = 1"), Some((2, 13, 16)));
    }

    #[test]
    fn a_non_at_prefix_is_ignored() {
        // The prefix may be any non-@ bytes; the block is still located.
        assert_eq!(scan_block("# header\n@{x = \"1\"@}\na"), Some((11, 18, 21)));
    }

    #[test]
    fn a_multiline_interior_is_cut_correctly() {
        let (s, e, c) = scan_block("@{a = \"1\"\nb = \"2\"@}\na").expect("block");
        let raw = "@{a = \"1\"\nb = \"2\"@}\na";
        assert_eq!(&raw[s..e], "a = \"1\"\nb = \"2\"");
        assert_eq!(&raw[c..], "a");
    }

    #[test]
    fn block_depends_collects_depend_and_plug() {
        let interior = "foo = depend \"https://example.com/foo.git\"\ngpu = plug \"https://example.com/gpu.git\"";
        let deps = block_depends(interior);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "foo");
        assert!(!deps[0].plugin);
        assert_eq!(deps[1].name, "gpu");
        assert!(deps[1].plugin);
    }

    #[test]
    fn depend_of_marks_a_plug_as_a_plugin() {
        let dir = Directive::Plug {
            url: "https://example.com/gpu.git".to_string(),
            name: "gpu".to_string(),
            rev: None,
            branch: None,
            tag: None,
            package: Some("gpu-crate".to_string()),
            sub: None,
        };
        let dep = depend_of(dir).expect("a plug is a depend");
        assert!(dep.plugin);
        assert_eq!(dep.name, "gpu");
        assert_eq!(dep.package.as_deref(), Some("gpu-crate"));
    }
}
