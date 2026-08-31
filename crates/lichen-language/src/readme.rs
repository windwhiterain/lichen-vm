//! Keeping the README example section in sync with `examples/programs/`.
//!
//! The example programs are the single source of truth for the top-level
//! README's example section: [`render_examples`] walks `examples/programs/`
//! as a tree, and every directory under it renders as one unit — opened by
//! the directory's `_.lichen` program, followed by the files it contains,
//! with nested directories rendering the same way to any depth, one heading
//! level deeper each time.  Every program is rendered with its code and its
//! *actual* output, computed by running it — the README never relies on a
//! hand-written promise in the file.  Placement is declared with a
//! `-- order: N` comment: a file's own orders it among its siblings, and a
//! directory's is the one in its `_.lichen`, whose program also always
//! shows first inside the directory; undeclared entries sort last, ties by
//! name.  Programs run standalone wherever they sit — `@import` lines
//! resolve relative to their own file, so a directory of packages is just a
//! group of ordinary programs.  [`sync_output_comments`] rewrites each
//! file's `-- output:` comment to that same actual output (appending it
//! when the file has none), so the file and the README agree.
//! [`replace_examples`] splices the rendered blob into the region between
//! the `<!-- begin: examples -->` / `<!-- end: examples -->` markers, and
//! `cargo run -p lichen-language --bin sync-readme` writes it back.
//! `tests/readme.rs` resyncs the README and the output comments in place
//! whenever they drift, so they cannot go stale — `cargo test` self-heals a
//! stale README or stale comment (the sync binary does the same, for
//! committing on demand).

use std::fs;
use std::path::{Path, PathBuf};

/// The marker that opens the generated region in the READMEs.
pub const BEGIN_MARKER: &str = "<!-- begin: examples -->";
/// The marker that closes the generated region in the READMEs.
pub const END_MARKER: &str = "<!-- end: examples -->";

/// A directory's own program: its `-- order:` places the whole directory
/// among its siblings, and its code and output open the directory's section.
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

/// Read the `-- order: N` comment from a program's source, if it has one.
///
/// The directive can sit on any line; a value that is not a number panics
/// with the file's name, so a typo is caught by the sync command instead of
/// silently mis-ordering the section.
fn declared_order(file: &Path, source: &str) -> Option<usize> {
    let line = source.lines().find(|l| l.starts_with("-- order:"))?;
    let value = line.strip_prefix("-- order:").unwrap().trim();
    Some(value.parse().unwrap_or_else(|_| {
        panic!(
            "{}: expected a number after `-- order:`, found {value:?}",
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
    /// The `-- order:` the entry sorts by: a file's own directive, or a
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

/// One program's markdown body: its code — with the `-- output:` and
/// `-- order:` directives dropped, so the section shows what the language
/// really prints, not what the file claims — and its actual output,
/// computed by running it.
fn render_program_body(path: &Path) -> String {
    let source = read_normalized(path);
    let text = source
        .lines()
        .filter(|line| !line.starts_with("-- output:") && !line.starts_with("-- order:"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = program_output(path, &source);
    format!("```text\n{text}\n```\n\noutput:\n```text\n{output}\n```")
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
/// heading over its code and output; a directory becomes a heading — opened
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
    blocks.extend(render_dir(&entry.path, &format!("{}/", entry.name), level + 1));
    blocks.join("\n\n")
}

/// Render every example program as the markdown section between the markers.
///
/// `examples/programs/` is walked as a tree: each directory renders as one
/// unit — a heading named by its path relative to the example directory,
/// opened by its `_.lichen` program when it has one, then its files and
/// nested directories, ordered by their `-- order:` comments (a directory's
/// is its `_.lichen`'s), undeclared last, ties by name.  A program that
/// fails to run panics with its diagnostics.
pub fn render_examples() -> String {
    let root = example_dir();
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

/// Rewrite every example program's `-- output:` comment to its actual
/// output, so each file shows what the language really prints.  An existing
/// `-- output:` line is replaced in place; a file without one gets the
/// comment appended.  A multi-line output becomes one `-- output:` line per
/// line, so [`render_examples`]'s filter drops them all.  Returns true when
/// any file was rewritten.
pub fn sync_output_comments() -> bool {
    let mut changed = false;
    for (_, file) in example_files() {
        let source = read_normalized(&file);
        let output = program_output(&file, &source);
        let comment = output
            .lines()
            .map(|line| format!("-- output: {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let updated = replace_output_comment(&source, &comment);
        if updated != source {
            fs::write(&file, updated).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            changed = true;
        }
    }
    changed
}

/// Replace the first run of `-- output:` lines in `source` with `comment`;
/// append `comment` (on a fresh line) when there is no such line.  The
/// result always ends with a newline.
fn replace_output_comment(source: &str, comment: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let start = lines.iter().position(|line| line.starts_with("-- output:"));
    let mut out = String::with_capacity(source.len() + comment.len() + 2);
    match start {
        Some(start) => {
            let mut end = start;
            while end < lines.len() && lines[end].starts_with("-- output:") {
                end += 1;
            }
            for line in &lines[..start] {
                out.push_str(line);
                out.push('\n');
            }
            out.push_str(comment);
            out.push('\n');
            for line in &lines[end..] {
                out.push_str(line);
                out.push('\n');
            }
        }
        None => {
            out.push_str(source);
            if !source.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(comment);
            out.push('\n');
        }
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
mod tests {
    use super::*;

    #[test]
    fn replaces_the_region_between_markers() {
        let content = "before\n<!-- begin: examples -->\nstale\n<!-- end: examples -->\nafter";
        let blob = "### `x.lichen`\n\n```text\n1\n```";
        let expected = "before\n<!-- begin: examples -->\n\n### `x.lichen`\n\n```text\n1\n```\n\n\
             <!-- end: examples -->\nafter";
        assert_eq!(replace_examples(content, blob).unwrap(), expected);
    }

    #[test]
    fn missing_markers_are_errors() {
        assert!(replace_examples("no markers", "blob").is_err());
        assert!(replace_examples("<!-- begin: examples -->\nno end", "blob").is_err());
        assert!(
            replace_examples("<!-- end: examples -->\n<!-- begin: examples -->", "blob").is_err()
        );
    }

    #[test]
    fn a_synced_region_is_a_noop() {
        let blob = "### `x.lichen`\n\n```text\n1\n```";
        let content = format!("<!-- begin: examples -->\n\n{blob}\n\n<!-- end: examples -->");
        assert_eq!(replace_examples(&content, blob).unwrap(), content);
    }

    #[test]
    fn renders_the_tree_grouped_and_ordered() {
        let blob = render_examples();
        let headings: Vec<(usize, String)> = blob
            .lines()
            .filter_map(|line| {
                let level = line.chars().take_while(|&c| c == '#').count();
                if level < 3 {
                    return None;
                }
                let rest = &line[level..];
                let name = rest.strip_prefix(" `")?.strip_suffix('`')?;
                (rest.len() == name.len() + 3).then(|| (level, name.to_owned()))
            })
            .collect();
        assert_eq!(
            headings,
            [
                "array.lichen",
                "tuple.lichen",
                "index.lichen",
                "closure.lichen",
                "dependent_type.lichen",
                "lazy_infinite.lichen",
                "let_polymorphism.lichen",
                "mutual_recursion.lichen",
                "nested_function.lichen",
                "recursion.lichen",
                "placeholder.lichen",
                "struct.lichen",
                "struct_recursion.lichen",
                "struct_generic.lichen",
                "table.lichen",
            ]
            .into_iter()
            .map(|name| (3, name.to_owned()))
            .chain([
                // The `import` directory is one unit: `_.lichen`'s order (7)
                // places it after every top-level file, the face opens the
                // group, and its files follow by their own orders.
                (3, "import".to_owned()),
                (4, "import/math.lichen".to_owned()),
                (4, "import/geometry.lichen".to_owned()),
                // `perspective.lichen`'s order (6) places it after the
                // `import` directory (order 5).
                (3, "perspective.lichen".to_owned()),
            ])
            .collect::<Vec<_>>(),
            "directories render as units ordered by their `_.lichen`, files by their `-- order:`"
        );
        // The face opens the directory: `_.lichen`'s program sits directly
        // under the directory heading.
        assert!(
            blob.contains("### `import`\n\n```text\n@import \"math.lichen\" as math"),
            "the directory's `_.lichen` is shown first inside the directory"
        );
        // The output is computed by running the program, not read from the
        // file: `array.lichen` runs to `[1, 2, 3]: Int<3>`, and no promise
        // or directive remains.
        assert!(
            blob.contains("output:\n```text\n[1, 2, 3]: Int<3>\n```"),
            "the runner's output is embedded"
        );
        assert!(!blob.contains("-- output:"), "file promises are not shown");
        assert!(!blob.contains("-- order:"), "order directives are not shown");
    }

    #[test]
    fn declared_order_reads_the_comment_from_any_line() {
        assert_eq!(
            declared_order(Path::new("a.lichen"), "-- order: 2\nx"),
            Some(2)
        );
        assert_eq!(
            declared_order(Path::new("a.lichen"), "x\n-- order: 42"),
            Some(42)
        );
        assert_eq!(declared_order(Path::new("a.lichen"), "no order here"), None);
    }

    #[test]
    fn an_output_comment_is_replaced_in_place() {
        let source = "-- order: 2\nrec f = x => x\nf 5\n-- output: stale\n";
        assert_eq!(
            replace_output_comment(source, "-- output: 5: Int"),
            "-- order: 2\nrec f = x => x\nf 5\n-- output: 5: Int\n"
        );
        // A multi-line output becomes one `-- output:` line per line.
        let source = "a\n-- output: x\n-- output: y\nb";
        assert_eq!(
            replace_output_comment(source, "-- output: 1\n-- output: 2"),
            "a\n-- output: 1\n-- output: 2\nb\n"
        );
    }

    #[test]
    fn a_missing_output_comment_is_appended() {
        let source = "rec f = x => x\nf 5\n";
        assert_eq!(
            replace_output_comment(source, "-- output: 5: Int"),
            "rec f = x => x\nf 5\n-- output: 5: Int\n"
        );
        // A file without a trailing newline still ends up clean.
        let source = "rec f = x => x\nf 5";
        assert_eq!(
            replace_output_comment(source, "-- output: 5: Int"),
            "rec f = x => x\nf 5\n-- output: 5: Int\n"
        );
    }

    #[test]
    #[should_panic]
    fn a_bad_order_value_panics() {
        declared_order(Path::new("a.lichen"), "-- order: two");
    }

    #[test]
    fn declared_order_breaks_ties_by_name() {
        let files = [
            (
                "c.lichen".to_string(),
                PathBuf::from("c.lichen"),
                "-- order: 2".to_string(),
            ),
            (
                "a.lichen".to_string(),
                PathBuf::from("a.lichen"),
                "-- order: 1".to_string(),
            ),
            (
                "b.lichen".to_string(),
                PathBuf::from("b.lichen"),
                "-- order: 1".to_string(),
            ),
            (
                "d.lichen".to_string(),
                PathBuf::from("d.lichen"),
                "no order".to_string(),
            ),
        ];
        let mut files = files.to_vec();
        files.sort_by(|(name_a, file_a, source_a), (name_b, file_b, source_b)| {
            let order_a = declared_order(file_a, source_a).unwrap_or(DEFAULT_ORDER);
            let order_b = declared_order(file_b, source_b).unwrap_or(DEFAULT_ORDER);
            (order_a, name_a).cmp(&(order_b, name_b))
        });
        let names: Vec<&str> = files.iter().map(|(name, _, _)| name.as_str()).collect();
        assert_eq!(names, ["a.lichen", "b.lichen", "c.lichen", "d.lichen"]);
    }
}
