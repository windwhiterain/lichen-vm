//! Keeping the README example section in sync with `examples/programs/`.
//!
//! The example programs are the single source of truth for the top-level
//! README's example section: [`render_examples`] renders each file (its `--`
//! comment header and code) into a markdown blob and appends the program's
//! *actual* output, computed by running it — the README never relies on a
//! hand-written promise in the file.  Each file may declare its place in the
//! section with a `-- order: N` comment; entries sort by that number, with a
//! file without one sorting last and then by name.  [`replace_examples`]
//! splices the blob into the region between the
//! `<!-- begin: examples -->` / `<!-- end: examples -->` markers, and
//! `cargo run -p lichen-language --bin sync-readme` writes it back.
//! `tests/readme.rs` fails the suite if the README ever drifts from the
//! example files, so the two cannot go stale.

use std::fs;
use std::path::{Path, PathBuf};

/// The marker that opens the generated region in the READMEs.
pub const BEGIN_MARKER: &str = "<!-- begin: examples -->";
/// The marker that closes the generated region in the READMEs.
pub const END_MARKER: &str = "<!-- end: examples -->";

/// The crate directory, embedded at compile time so it is independent of the
/// current working directory (tests run from the crate dir, the sync binary
/// from wherever the user invokes it).
pub fn crate_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The directory holding the example programs.
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

/// The order an unnumbered file sorts after — after every numbered file.
const DEFAULT_ORDER: usize = usize::MAX;

/// Read the `-- order: N` comment from a program's source, if it has one.
///
/// The directive can sit on any line; a value that is not a number panics
/// with the file's name, so a typo is caught by the sync command instead of
/// silently mis-ordering the section.
fn declared_order(file: &Path, source: &str) -> Option<usize> {
    let line = source.lines().find(|l| l.starts_with("-- order:"))?;
    let value = line.strip_prefix("-- order:").unwrap().trim();
    Some(
        value
            .parse()
            .unwrap_or_else(|_| panic!("{}: expected a number after `-- order:`, found {value:?}", file.display())),
    )
}

/// Render every example program as the markdown section between the markers.
///
/// Each program becomes a `### \`name.lichen\`` heading, a `text` code block
/// with the file's own comments and code, and the program's actual output —
/// computed by running it with the language runner — in a second `text`
/// block.  The entries are ordered by each file's `-- order: N` comment
/// (ties, and files without one, sort by name).  Any `-- output:` promise or
/// `-- order:` directive in the file is dropped, so the section shows what
/// the language really prints, not what the file claims; a program that
/// fails to run panics with its diagnostics.
pub fn render_examples() -> String {
    let mut programs: Vec<(String, PathBuf, String)> = fs::read_dir(example_dir())
        .expect("examples/programs")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lichen"))
        .map(|file| {
            let source = read_normalized(&file);
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            (name, file, source)
        })
        .collect();
    programs.sort_by(|(name_a, file_a, source_a), (name_b, file_b, source_b)| {
        let order_a = declared_order(file_a, source_a).unwrap_or(DEFAULT_ORDER);
        let order_b = declared_order(file_b, source_b).unwrap_or(DEFAULT_ORDER);
        (order_a, name_a).cmp(&(order_b, name_b))
    });
    programs
        .into_iter()
        .map(|(name, file, source)| {
            let text = source
                .lines()
                .filter(|line| !line.starts_with("-- output:") && !line.starts_with("-- order:"))
                .collect::<Vec<_>>()
                .join("\n");
            let output = crate::run::evaluate(&source)
                .unwrap_or_else(|diags| {
                    // The same rendering the CLI prints for a failing file,
                    // so the panic names the file and shows the caret
                    // diagnostics instead of a Debug dump.
                    panic!(
                        "{}: failed\n{}",
                        file.display(),
                        crate::render::render_all(&source, &diags)
                    )
                });
            format!("### `{name}`\n\n```text\n{text}\n```\n\noutput:\n```text\n{output}\n```")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
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
        let expected =
            "before\n<!-- begin: examples -->\n\n### `x.lichen`\n\n```text\n1\n```\n\n\
             <!-- end: examples -->\nafter";
        assert_eq!(replace_examples(content, blob).unwrap(), expected);
    }

    #[test]
    fn missing_markers_are_errors() {
        assert!(replace_examples("no markers", "blob").is_err());
        assert!(replace_examples("<!-- begin: examples -->\nno end", "blob").is_err());
        assert!(replace_examples("<!-- end: examples -->\n<!-- begin: examples -->", "blob").is_err());
    }

    #[test]
    fn a_synced_region_is_a_noop() {
        let blob = "### `x.lichen`\n\n```text\n1\n```";
        let content = format!("<!-- begin: examples -->\n\n{blob}\n\n<!-- end: examples -->");
        assert_eq!(replace_examples(&content, blob).unwrap(), content);
    }

    #[test]
    fn renders_every_program_ordered_by_its_comment() {
        let blob = render_examples();
        let headings: Vec<String> = blob
            .lines()
            .filter_map(|line| line.strip_prefix("### `").and_then(|rest| rest.strip_suffix('`')))
            .map(str::to_owned)
            .collect();
        let mut expected: Vec<(usize, String)> = fs::read_dir(example_dir())
            .expect("examples/programs")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "lichen"))
            .map(|file| {
                let source = read_normalized(&file);
                let order = declared_order(&file, &source).unwrap_or(DEFAULT_ORDER);
                (order, file.file_name().unwrap().to_string_lossy().into_owned())
            })
            .collect();
        expected.sort();
        assert_eq!(
            headings,
            expected.into_iter().map(|(_, name)| name).collect::<Vec<_>>(),
            "entries follow the `-- order:` comments"
        );
        // The output is computed by running the program, not read from the
        // file: `array.lichen` runs to `[1, 2, 3]: Int<3>`, and no promise
        // remains.
        assert!(
            blob.contains("output:\n```text\n[1, 2, 3]: Int<3>\n```"),
            "the runner's output is embedded"
        );
        assert!(!blob.contains("-- output:"), "file promises are not shown");
        assert!(!blob.contains("-- order:"), "order directives are not shown");
    }

    #[test]
    fn declared_order_reads_the_comment_from_any_line() {
        assert_eq!(declared_order(Path::new("a.lichen"), "-- order: 2\nx"), Some(2));
        assert_eq!(declared_order(Path::new("a.lichen"), "x\n-- order: 42"), Some(42));
        assert_eq!(declared_order(Path::new("a.lichen"), "no order here"), None);
    }

    #[test]
    #[should_panic]
    fn a_bad_order_value_panics() {
        declared_order(Path::new("a.lichen"), "-- order: two");
    }

    #[test]
    fn declared_order_breaks_ties_by_name() {
        let files = [
            ("c.lichen".to_string(), PathBuf::from("c.lichen"), "-- order: 2".to_string()),
            ("a.lichen".to_string(), PathBuf::from("a.lichen"), "-- order: 1".to_string()),
            ("b.lichen".to_string(), PathBuf::from("b.lichen"), "-- order: 1".to_string()),
            ("d.lichen".to_string(), PathBuf::from("d.lichen"), "no order".to_string()),
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
