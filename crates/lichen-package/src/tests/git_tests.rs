use super::*;

/// The URL and the revision reach git verbatim from source, so an option-like value
/// is refused before any git command.
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
