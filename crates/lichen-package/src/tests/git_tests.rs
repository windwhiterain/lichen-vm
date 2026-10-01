use super::*;

/// The URL and the revision reach git verbatim from the source file, so an
/// option-like value must be refused before any git command runs.  This is the
/// layer that also covers `checkout <rev>`, which `clone --` cannot reach and
/// which needs no minimum git version.
#[test]
fn option_like_values_are_refused() {
    for value in ["--upload-pack=<command>", "-f", "-"] {
        assert!(reject_option_like(value, "url").is_err(), "{value}");
    }
    for value in [
        "https://example.com/x.git",
        "git@github.com:owner/repo.git",
        "../local/repo",
    ] {
        assert!(reject_option_like(value, "url").is_ok(), "{value}");
    }
}
