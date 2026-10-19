//! A buffer's warp-uniformity is a requirement of its readers and writers.
//!
//! # Invariant
//! The requirement is the field's (`Read.from (Buf T) # warp`), and the argument's
//! provided perspective is the meet of what it is made of: a plain buffer provides the
//! top, which any requirement holds; a lane-varying claim is refused. The positive test
//! compiles only — its run's value class is the vocabulary's business, not this check's.

use lichen_compute::language::{LangProgram, new_store};
use lichen_language::Report;

/// The program: one buffer read at an index.
const PROGRAM: &str = "---
  order = \"30\"
  compute = import \"compute.lichen\"
---
b = (compute.buffer ([1, 2, 3] : array{Int, 3}))
r = compute.read ((compute.Read _)(.from BUFFER, .at 1))
r
";

/// `source` compiled and checked, never run.
fn compile(source: &str) -> Report<LangProgram> {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    assert!(diags.is_empty(), "the program's imports resolve: {diags:?}");
    lichen_language::compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &lichen_language::lex::line_starts(source),
        lichen_highlevel::no_native_ops(),
    )
}

#[test]
fn a_uniform_buffer_is_read() {
    let report = compile(&PROGRAM.replace("BUFFER", "b"));
    assert!(report.ok(), "{:?}", report.diagnostics);
}

#[test]
fn a_lane_varying_buffer_is_refused() {
    let report = compile(&PROGRAM.replace("BUFFER", "(b # 1)"));
    assert!(!report.ok(), "a lane-varying buffer must not be read");
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected 32, found 1")),
        "{messages:?}"
    );
}
