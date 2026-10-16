//! The span a `depend`/`plug` diagnostic points at: its own directive line.

use std::path::{Path, PathBuf};

use lichen_preprocess::{ImportResolver, PreprocessDiag, ResolvedPackage, stage_depends};

struct NoImports;

impl ImportResolver<()> for NoImports {
    fn resolve_import(
        &mut self,
        _base: Option<&Path>,
        _import_path: &str,
    ) -> Result<ResolvedPackage<()>, PreprocessDiag> {
        unreachable!("a depend-only block resolves no import")
    }

    fn register_vendored(&mut self, _alias: String, _dir: PathBuf) {}
}

#[test]
fn an_unfetched_depend_is_reported_at_its_own_line() {
    // The header pushes the block off line 1, so a span on the directive is
    // distinguishable from no span at all.
    let source = "# a project header\n---\nprobe_dep_span = depend \
                  \"https://example.invalid/probe.git\"\n---\n42\n";
    let diagnostics = stage_depends(&mut NoImports, source);
    let diagnostic = diagnostics
        .first()
        .expect("an unfetched depend is reported");
    assert_eq!(
        diagnostic.span,
        Some((3, 1)),
        "the directive's own line is the earliest actionable place: {diagnostics:?}"
    );
}
