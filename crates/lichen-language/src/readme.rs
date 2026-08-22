//! Keeping the README example section in sync with `examples/programs/`.
//!
//! The example programs are the single source of truth for the top-level
//! README's example section: [`render_examples`] renders each file (its `--`
//! comment header and code) into a markdown blob and appends the program's
//! *actual* output, computed by running it — the README never relies on a
//! hand-written promise in the file.  [`replace_examples`] splices that blob
//! into the region between the `<!-- begin: examples -->` /
//! `<!-- end: examples -->` markers, and
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

/// Render every example program as the markdown section between the markers.
///
/// Each program becomes a `### \`name.lichen\`` heading, a `text` code block
/// with the file's own comments and code, and the program's actual output —
/// computed by running it with the language runner — in a second `text`
/// block.  Any `-- output:` promise in the file is dropped, so the section
/// shows what the language really prints, not what the file claims; a
/// program that fails to run panics with its diagnostics.
pub fn render_examples() -> String {
    let mut files: Vec<PathBuf> = fs::read_dir(example_dir())
        .expect("examples/programs")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lichen"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|file| {
            let name = file.file_name().unwrap().to_string_lossy();
            let source = read_normalized(&file);
            let text = source
                .lines()
                .filter(|line| !line.starts_with("-- output:"))
                .collect::<Vec<_>>()
                .join("\n");
            let output = crate::run::evaluate(&source)
                .unwrap_or_else(|diags| panic!("{} failed to run: {diags:?}", file.display()));
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
    fn renders_every_program_sorted_by_name() {
        let blob = render_examples();
        let array = blob.find("### `array.lichen`").unwrap();
        let dependent = blob.find("### `dependent.lichen`").unwrap();
        assert!(array < dependent, "entries are sorted by file name");
        // The output is computed by running the program, not read from the
        // file: `array.lichen` runs to `[1, 2, 3]: Int<3>`, and no promise
        // remains.
        assert!(
            blob.contains("output:\n```text\n[1, 2, 3]: Int<3>\n```"),
            "the runner's output is embedded"
        );
        assert!(!blob.contains("-- output:"), "file promises are not shown");
    }
}
