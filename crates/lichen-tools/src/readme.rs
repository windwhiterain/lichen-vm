//! The README example section, rendered from `examples/`, the living spec.  See
//! docs/notes/readme-sync.md.

use std::fs;
use std::path::{Path, PathBuf};

use lichen_language::package::PackageStore;
use lichen_language::preprocess::{Directive, block_directives, block_metadata, split_block};
use lichen_language::program::LangProgram;
use lichen_language::render::render_all;
use lichen_language::run::evaluate_raw;

/// A fallible drive of the checkout; the message names the path and the cause.
pub type ReadmeResult<T> = std::result::Result<T, String>;

/// The deepest directory nesting either tree walk descends; exceeding it is
/// reported, never a silent truncation.
pub const MAX_DEPTH: usize = 32;

/// The marker that opens the generated region.
pub const BEGIN_MARKER: &str = "<!-- begin: examples -->";
/// The marker that closes the generated region.
pub const END_MARKER: &str = "<!-- end: examples -->";

/// A directory's own program; its `order =` places the whole directory.
const DIR_FACE: &str = "_.lichen";

/// Read a directory, or report why it cannot be read.
fn read_dir(dir: &Path) -> ReadmeResult<fs::ReadDir> {
    fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))
}

/// The crate directory, from the compile-time `CARGO_MANIFEST_DIR`, not the
/// current directory.
pub fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The directory holding the example programs.
pub fn example_dir() -> PathBuf {
    crate_dir().join("..").join("..").join("examples")
}

/// The top-level README that carries the generated section.
pub fn readme_path() -> PathBuf {
    crate_dir().join("..").join("..").join("README.md")
}

/// Read a file with `\r\n` normalized to `\n`, so a CRLF checkout still compares
/// equal to the rendered blob.
pub fn read_normalized(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// Every `.lichen` file under the example directory, each directory's
/// `_.lichen` face included.  Order is irrelevant.
pub fn example_files() -> ReadmeResult<Vec<(String, PathBuf)>> {
    fn walk(
        dir: &Path,
        prefix: &str,
        depth: usize,
        files: &mut Vec<(String, PathBuf)>,
    ) -> ReadmeResult<()> {
        if depth > MAX_DEPTH {
            return Err(format!(
                "{}: directory nesting is deeper than {MAX_DEPTH} levels",
                dir.display()
            ));
        }
        for entry in read_dir(dir)?.flatten() {
            let path = entry.path();
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if path.is_dir() {
                walk(&path, &format!("{name}/"), depth + 1, files)?;
            } else if path.extension().is_some_and(|e| e == "lichen") {
                files.push((name, path));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(&example_dir(), "", 0, &mut files)?;
    Ok(files)
}

/// The order an unnumbered entry sorts after — after every numbered entry.
const DEFAULT_ORDER: usize = usize::MAX;

/// Read a program's `order = "N"` metadata, if it has one; a value that is not
/// a number panics.
fn declared_order(file: &Path, source: &str) -> Option<usize> {
    let (interior, _) = split_block(source);
    let interior = interior?;
    let value = block_metadata(interior)
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

/// One entry of a directory: a `.lichen` file, or a subdirectory.
struct Entry {
    /// The entry's path under the example directory, `/`-separated: the name it
    /// shows under in the README.
    name: String,
    path: PathBuf,
    is_dir: bool,
}

impl Entry {
    /// The `order =` the entry sorts by: a file's own, or a directory's from
    /// its `_.lichen`.  Undeclared sorts last.
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

/// The `output = "..."` metadata a program declares; with [`program_output`] it
/// forms the suite's behaviour guard.
pub fn declared_output(source: &str) -> Option<String> {
    let (interior, _) = split_block(source);
    block_metadata(interior?)
        .into_iter()
        .find(|(name, _)| name == "output")
        .map(|(_, value)| value)
}

/// The program's actual output, or a panic showing its diagnostics.  The file's
/// own path is the import base.
pub fn program_output(file: &Path, source: &str) -> String {
    let mut store = PackageStore::<LangProgram>::new();
    evaluate_raw(source, Some(file), &mut store).unwrap_or_else(|diags| {
        panic!("{}: failed\n{}", file.display(), render_all(source, &diags))
    })
}

/// The program's markdown body: its whole source, `---...---` block included.
fn render_program_body(path: &Path) -> String {
    let source = read_normalized(path);
    let text = source.trim_end_matches('\n');
    format!("```text\n{text}\n```")
}

/// Render one directory's entries, already ordered by [`Entry::order`], as
/// markdown blocks at the given heading level.
fn render_dir(dir: &Path, prefix: &str, level: usize, depth: usize) -> ReadmeResult<Vec<String>> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "{}: directory nesting is deeper than {MAX_DEPTH} levels",
            dir.display()
        ));
    }
    let mut entries: Vec<(usize, Entry)> = Vec::new();
    for item in read_dir(dir)?.flatten() {
        let path = item.path();
        let is_dir = path.is_dir();
        if !is_dir
            && (!path.extension().is_some_and(|e| e == "lichen")
                || path.file_name().is_some_and(|f| f == DIR_FACE))
        {
            continue;
        }
        let name = format!("{prefix}{}", path.file_name().unwrap().to_string_lossy());
        let entry = Entry { name, path, is_dir };
        entries.push((entry.order(), entry));
    }
    entries.sort_by(|(order_a, entry_a), (order_b, entry_b)| {
        (*order_a, &entry_a.name).cmp(&(*order_b, &entry_b.name))
    });
    entries
        .into_iter()
        .map(|(_, entry)| render_entry(&entry, level, depth))
        .collect()
}

/// Render one entry: a file becomes a heading over its file, a directory one
/// over its `_.lichen` and its entries.
fn render_entry(entry: &Entry, level: usize, depth: usize) -> ReadmeResult<String> {
    let hashes = "#".repeat(level.min(6));
    if !entry.is_dir {
        return Ok(format!(
            "{hashes} `{}`\n\n{}",
            entry.name,
            render_program_body(&entry.path)
        ));
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
        depth + 1,
    )?);
    Ok(blocks.join("\n\n"))
}

/// Render `examples/` as the markdown section between the markers.
pub fn render_examples() -> ReadmeResult<String> {
    render_examples_in(&example_dir())
}

/// Render the tree under `dir`, so the tests can drive a controlled fixture.
fn render_examples_in(dir: &Path) -> ReadmeResult<String> {
    let root = dir;
    let mut blocks = Vec::new();
    // The root's own `_.lichen` has no directory to introduce, so it renders
    // as an ordinary program.
    let face = root.join(DIR_FACE);
    if face.is_file() {
        blocks.push(format!(
            "### `{DIR_FACE}`\n\n{}",
            render_program_body(&face)
        ));
    }
    blocks.extend(render_dir(root, "", 3, 0)?);
    Ok(blocks.join("\n\n"))
}

/// Rewrite every example's `output =` metadata to its actual output.
///
/// # Invariant
///
/// A maintenance operation, run by the `sync-readme` binary on demand, never by
/// the test suite: the suite asserts a declared output against its actual one
/// and fails on a difference, so rewriting it away would hide the change.
pub fn sync_output_comments() -> ReadmeResult<bool> {
    let mut changed = false;
    for (_, file) in example_files()? {
        let source = read_normalized(&file);
        let output = program_output(&file, &source);
        let updated = replace_output_comment(&source, &output);
        if updated != source {
            fs::write(&file, updated).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            changed = true;
        }
    }
    Ok(changed)
}

/// Replace the `output =` metadata in `source` with `output`, appending it
/// inside the block when there is none.
///
/// # Invariant
///
/// The result re-emits the block as `---`, one two-space-indented directive per
/// line, and ends with a newline.
fn replace_output_comment(source: &str, output: &str) -> String {
    let (interior, code) = split_block(source);
    let mut out = String::with_capacity(source.len() + output.len() + 16);
    out.push_str("---\n");
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
                    out.push_str(&format!("{name} = depend \"{url}\""));
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
                Directive::Plug {
                    url,
                    name,
                    rev,
                    branch,
                    tag,
                    package,
                    sub,
                } => {
                    out.push_str("  ");
                    out.push_str(&format!("{name} = plug \"{url}\""));
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
    out.push_str("---\n");
    out.push_str(code.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Replace the region between the markers in `content` with `blob`; the markers
/// stay.
///
/// # Invariant
///
/// The region keeps one blank line on each side, and a missing marker or an
/// `end` before a `begin` is an error naming it.
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
