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

/// The program: one read, its buffer and index spelled by the source.
const READ: &str = "---
  order = \"30\"
  compute = import \"compute.lichen\"
---
b = ((compute.buffer ([1, 2, 3] : array{Int, 3})) # BUFFER)
r = compute.read ((compute.Read _)(.from b, .at INDEX))
(r # DEMAND)
";

/// The program: one write, its buffer, index and value spelled by the source.
const WRITE: &str = "---
  order = \"30\"
  compute = import \"compute.lichen\"
---
b = ((compute.buffer ([1, 2, 3] : array{Int, 3})) # BUFFER)
w = compute.write ((compute.Write _)(.to b, .at INDEX, .value (0 # 1)))
(w # DEMAND)
";

/// `program` with its three spellings replaced.
fn spelled(program: &str, buffer: &str, index: &str, demand: &str) -> String {
    program
        .replace("BUFFER", buffer)
        .replace("INDEX", index)
        .replace("DEMAND", demand)
}

#[test]
fn a_read_takes_its_perspective_from_the_buffer_and_the_index() {
    // `divisor(from, at)`: a uniform index keeps the buffer's perspective, a
    // lane-varying one does not.
    assert!(compile(&spelled(READ, "compute.warp", "0", "compute.warp")).ok());
    let report = compile(&spelled(READ, "compute.warp", "(0 # 1)", "compute.warp"));
    assert!(
        !report.ok(),
        "a lane-varying index must not stay warp-uniform"
    );
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

#[test]
fn a_write_returns_the_input_buffer_perspective() {
    // The output buffer is the input's segment, so its perspective is the
    // input's whatever the written index and value are.
    assert!(compile(&spelled(WRITE, "0", "(0 # 1)", "0")).ok());
    let report = compile(&spelled(WRITE, "compute.warp", "(0 # 1)", "0"));
    assert!(!report.ok(), "a warp segment is not zero-uniform");
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected 0, found 32")),
        "{messages:?}"
    );
}

/// The body's requirement is judged against what the caller provides.
///
/// # Invariant
/// The requirement a body states is recorded where its read consumes it and replayed
/// at the apply, so a caller whose attribute is below it is refused
/// (`docs/notes/array-attribute-view.md`).
#[test]
fn a_bodys_requirement_refuses_a_caller_whose_attribute_is_below_it() {
    let source = "---
  order = \"30\"
  compute = import \"compute.lichen\"
---
use = (b : (compute.Buf Int)) => compute.read ((compute.Read _)(.from b, .at 1))
b = (compute.buffer ([1, 2, 3] : array{Int, 3}))
use (b # 1)
";
    let report = compile(source);
    assert!(!report.ok(), "the body's warp requirement sees the caller's `1`");
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
