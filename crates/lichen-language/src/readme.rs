//! Keeping the README example section in sync with `examples/programs/`.
//!
//! The example programs are the single source of truth for the top-level
//! README's example section: [`render_examples`] walks `examples/programs/`
//! as a tree, and every directory under it renders as one unit — opened by
//! the directory's `_.lichen` program, followed by the files it contains,
//! with nested directories rendering the same way to any depth, one heading
//! level deeper each time.  Every program is rendered as its whole source
//! file, `@{...@}` block included; the block's `output = "..."` metadata is
//! the file's actual output, kept current by [`sync_output_comments`], so
//! the README never relies on a hand-written promise in the file.
//! Placement is declared in each file's opening `@{...@}` block as
//! `order = "N"`: a file's order places it among its siblings, and a
//! directory's is the one in its `_.lichen`, whose program also always shows
//! first inside the directory; undeclared entries sort last, ties by name.
//! Programs run standalone wherever they sit — the block's
//! `name = import "path"` entries resolve relative to their own file, so a
//! directory of packages is just a group of ordinary programs.
//! [`sync_output_comments`] rewrites each file's `output = "..."` entry (in
//! the same block) to that same actual output, so the file and the README
//! agree.
//! [`replace_examples`] splices the rendered blob into the region between
//! the `<!-- begin: examples -->` / `<!-- end: examples -->` markers, and
//! `cargo run -p lichen-language --bin sync-readme` writes it back.
//! `tests/readme.rs` resyncs the README and the output metadata in place
//! whenever they drift, so they cannot go stale — `cargo test` self-heals a
//! stale README or stale output metadata (the sync binary does the same, for
//! committing on demand).

use std::fs;
use std::path::{Path, PathBuf};

use crate::preprocess::Directive;
use crate::preprocess::{block_directives, split_block};

/// The marker that opens the generated region in the READMEs.
pub const BEGIN_MARKER: &str = "<!-- begin: examples -->";
/// The marker that closes the generated region in the READMEs.
pub const END_MARKER: &str = "<!-- end: examples -->";

/// A directory's own program: its `order =` places the whole directory
/// among its siblings, and its code opens the directory's section.
const DIR_FACE: &str = "_.lichen";

/// The crate directory, embedded at compile time so it is independent of the
/// current working directory (tests run from the crate dir, the sync binary
/// from wherever the user invokes it).
pub fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The directory holding the example programs — a tree of files and
/// directories, where each directory renders as one unit.
pub fn example_dir() -> PathBuf {
    crate_dir().join("examples").join("programs")
}

/// The top-level README that carries the generated section.
pub fn readme_path() -> PathBuf {
    crate_dir().join("..").join("..").join("README.md")
}

/// Read a file with `\r\n` line endings normalized to `\n`, so a README
/// checked out with CRLF on Windows still compares equal to the rendered blob.
pub fn read_normalized(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// Every example program: every `.lichen` file anywhere under the example
/// directory, including each directory's `_.lichen` face.  Order within the
/// list is irrelevant — [`render_examples`] re-sorts the tree.
pub fn example_files() -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
        for entry in fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .flatten()
        {
            let path = entry.path();
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if path.is_dir() {
                walk(&path, &format!("{name}/"), files);
            } else if path.extension().is_some_and(|e| e == "lichen") {
                files.push((name, path));
            }
        }
    }
    let mut files = Vec::new();
    walk(&example_dir(), "", &mut files);
    files
}

/// The order an unnumbered entry sorts after — after every numbered entry.
const DEFAULT_ORDER: usize = usize::MAX;

/// Read the `order = "N"` metadata from a program's source, if it has one.
///
/// The directive can sit on any line; a value that is not a number panics
/// with the file's name, so a typo is caught by the sync command instead of
/// silently mis-ordering the section.
fn declared_order(file: &Path, source: &str) -> Option<usize> {
    let (interior, _) = split_block(source);
    let interior = interior?;
    let value = crate::preprocess::block_metadata(interior)
        .into_iter()
        .find(|(name, _)| name == "order")
        .map(|(_, value)| value)?;
    Some(value.parse().unwrap_or_else(|_| {
        panic!(
            "{}: expected a number after `order =`, found {value:?}",
            file.display()
        )
    }))
}

/// One entry of a directory: a `.lichen` program file, or a subdirectory
/// (which renders as one unit, opened by its `_.lichen`).
struct Entry {
    /// The path relative to the example directory, `/`-separated — the name
    /// the entry shows under in the README.
    name: String,
    path: PathBuf,
    is_dir: bool,
}

impl Entry {
    /// The `order =` the entry sorts by: a file's own directive, or a
    /// directory's the one in its `_.lichen`.  Undeclared sorts last either
    /// way — including a directory without an `_.lichen` at all.
    fn order(&self) -> usize {
        let path = if self.is_dir {
            self.path.join(DIR_FACE)
        } else {
            self.path.clone()
        };
        let source = if self.is_dir {
            fs::read_to_string(&path).ok()
        } else {
            Some(read_normalized(&path))
        };
        source
            .and_then(|source| declared_order(&path, &source))
            .unwrap_or(DEFAULT_ORDER)
    }
}

/// The program's actual output, or a panic naming the file and showing its
/// diagnostics — the same rendering the CLI prints for a failing file.
/// Programs run through a package store with their own path as the base, so
/// `@import` lines resolve relative to the file (import-free programs are
/// unaffected).
fn program_output(file: &Path, source: &str) -> String {
    let mut store = crate::package::PackageStore::new();
    crate::run::evaluate_raw(source, Some(file), &mut store).unwrap_or_else(|diags| {
        panic!(
            "{}: failed\n{}",
            file.display(),
            crate::render::render_all(source, &diags)
        )
    })
}

/// One program's markdown body: the whole source file, `@{...@}` block
/// included, shown as-is.  The block's `output = "..."` metadata is the
/// file's actual output (kept current by [`sync_output_comments`]), so the
/// README shows the file exactly as it is in the repo.
fn render_program_body(path: &Path) -> String {
    let source = read_normalized(path);
    let text = source.trim_end_matches('\n');
    format!("```text\n{text}\n```")
}

/// Render one directory's entries — its `.lichen` files and subdirectories,
/// each already ordered by [`Entry::order`] (ties by name) — as markdown
/// blocks at the given heading level.  The `_.lichen` face is not an entry:
/// [`render_entry`] opens the directory with it.
fn render_dir(dir: &Path, prefix: &str, level: usize) -> Vec<String> {
    let mut entries: Vec<(usize, Entry)> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
        .filter_map(|item| {
            let path = item.path();
            let is_dir = path.is_dir();
            if !is_dir
                && (!path.extension().is_some_and(|e| e == "lichen")
                    || path.file_name().is_some_and(|f| f == DIR_FACE))
            {
                return None;
            }
            let name = format!("{prefix}{}", path.file_name().unwrap().to_string_lossy());
            let entry = Entry { name, path, is_dir };
            Some((entry.order(), entry))
        })
        .collect();
    entries.sort_by(|(order_a, entry_a), (order_b, entry_b)| {
        (*order_a, &entry_a.name).cmp(&(*order_b, &entry_b.name))
    });
    entries
        .into_iter()
        .map(|(_, entry)| render_entry(&entry, level))
        .collect()
}

/// Render one entry at the given heading level: a program file becomes a
/// heading over its whole file; a directory becomes a heading — opened
/// by its `_.lichen` when it has one — over its entries, one level deeper.
/// Headings start at `###` for the top level and deepen per directory,
/// capped at `######`, so nesting of any depth still renders as markdown.
fn render_entry(entry: &Entry, level: usize) -> String {
    let hashes = "#".repeat(level.min(6));
    if !entry.is_dir {
        return format!(
            "{hashes} `{}`\n\n{}",
            entry.name,
            render_program_body(&entry.path)
        );
    }
    let mut blocks = vec![format!("{hashes} `{}`", entry.name)];
    let face = entry.path.join(DIR_FACE);
    if face.is_file() {
        blocks.push(render_program_body(&face));
    }
    blocks.extend(render_dir(
        &entry.path,
        &format!("{}/", entry.name),
        level + 1,
    ));
    blocks.join("\n\n")
}

/// Render every example program as the markdown section between the markers.
///
/// `examples/programs/` is walked as a tree: each directory renders as one
/// unit — a heading named by its path relative to the example directory,
/// opened by its `_.lichen` program when it has one, then its files and
/// nested directories, ordered by their `order =` metadata (a directory's
/// is its `_.lichen`'s), undeclared last, ties by name.  Each program is
/// shown as its whole file, so its `output =` metadata must be current —
/// [`sync_output_comments`] keeps it that way (and `tests/readme.rs` runs
/// it before rendering).
pub fn render_examples() -> String {
    render_examples_in(&example_dir())
}

/// Render every example program under `dir` as the markdown section between
/// the markers.
///
/// [`render_examples`] renders the live `examples/programs/` tree; this takes
/// a base directory so the unit tests drive the same rendering logic from a
/// controlled fixture instead of the live example set (which is a moving spec,
/// so asserting it in a unit test would force a test edit per add/rename/
/// reorder).
fn render_examples_in(dir: &Path) -> String {
    let root = dir;
    let mut blocks = Vec::new();
    // A `_.lichen` directly in the example directory has no directory to
    // introduce (the section itself is the root's unit) — it renders as an
    // ordinary program.
    let face = root.join(DIR_FACE);
    if face.is_file() {
        blocks.push(format!(
            "### `{DIR_FACE}`\n\n{}",
            render_program_body(&face)
        ));
    }
    blocks.extend(render_dir(&root, "", 3));
    blocks.join("\n\n")
}

/// Rewrite every example program's `output = "..."` metadata entry to its
/// actual output, so each file shows what the language really prints — which
/// is exactly what the README then embeds.  An existing `output =` entry is
/// replaced in place; a file without one gets it appended.  A multi-line
/// output becomes a multi-line string.  Returns true when any file was
/// rewritten.
pub fn sync_output_comments() -> bool {
    let mut changed = false;
    for (_, file) in example_files() {
        let source = read_normalized(&file);
        let output = program_output(&file, &source);
        let updated = replace_output_comment(&source, &output);
        if updated != source {
            fs::write(&file, updated).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            changed = true;
        }
    }
    changed
}

/// Replace the `output = "..."` metadata entry in `source` with `comment`;
/// append it (inside the block) when there is none.  The result always ends
/// with a newline; the block is normalized to the `@{` … `@}` form, one
/// directive per line, two-space indented.
fn replace_output_comment(source: &str, output: &str) -> String {
    let (interior, code) = split_block(source);
    let mut out = String::with_capacity(source.len() + output.len() + 16);
    out.push_str("@{\n");
    let mut has_output = false;
    if let Some(interior) = interior {
        for dir in block_directives(interior) {
            match dir {
                Directive::Import { name, path } => {
                    out.push_str("  ");
                    out.push_str(&format!("{name} = import \"{path}\""));
                    out.push('\n');
                }
                Directive::Metadata { name, value: _ } if name == "output" => {
                    out.push_str("  ");
                    out.push_str(&format!("output = \"{output}\""));
                    out.push('\n');
                    has_output = true;
                }
                Directive::Metadata { name, value } => {
                    out.push_str("  ");
                    out.push_str(&format!("{name} = \"{value}\""));
                    out.push('\n');
                }
                Directive::Depend {
                    url,
                    name,
                    rev,
                    branch,
                    tag,
                    package,
                    sub,
                    plugin,
                } => {
                    out.push_str("  ");
                    out.push_str(&format!("depend \"{url}\""));
                    if let Some(name) = name {
                        out.push_str(&format!(" as {name}"));
                    }
                    if let Some(rev) = rev {
                        out.push_str(&format!(" rev = \"{rev}\""));
                    }
                    if let Some(branch) = branch {
                        out.push_str(&format!(" branch = \"{branch}\""));
                    }
                    if let Some(tag) = tag {
                        out.push_str(&format!(" tag = \"{tag}\""));
                    }
                    if let Some(package) = package {
                        out.push_str(&format!(" package = \"{package}\""));
                    }
                    if let Some(sub) = sub {
                        out.push_str(&format!(" sub = \"{sub}\""));
                    }
                    if plugin {
                        out.push_str(" plugin");
                    }
                    out.push('\n');
                }
            }
        }
    }
    if !has_output {
        out.push_str("  ");
        out.push_str(&format!("output = \"{output}\""));
        out.push('\n');
    }
    out.push_str("@}\n");
    out.push_str(code.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Replace the region between the markers in `content` with `blob`.
///
/// The markers themselves stay, with one blank line around the region.
/// Errors with a message naming the marker when one is missing (or the `end`
/// marker appears before the `begin` marker).
pub fn replace_examples(content: &str, blob: &str) -> Result<String, String> {
    let begin = content
        .find(BEGIN_MARKER)
        .ok_or_else(|| format!("missing {BEGIN_MARKER}"))?;
    let end = content
        .find(END_MARKER)
        .ok_or_else(|| format!("missing {END_MARKER}"))?;
    if end < begin {
        return Err(format!("{END_MARKER} appears before {BEGIN_MARKER}"));
    }
    let mut out = String::with_capacity(content.len() + blob.len() + 8);
    out.push_str(&content[..begin]);
    out.push_str(BEGIN_MARKER);
    out.push_str("\n\n");
    out.push_str(blob);
    out.push_str("\n\n");
    out.push_str(END_MARKER);
    out.push_str(&content[end + END_MARKER.len()..]);
    Ok(out)
}

#[cfg(test)]
#[path = "tests/readme_tests.rs"]
mod tests;
