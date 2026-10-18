//! Temporary: a compute pipeline under an edit, with cached buffer stages.
//! See heavy-value-cache.md §9.7.
//!
//! # Invariant
//!
//! The value is compared element for element against a fresh session's, so a fast row that
//! answered differently is reported rather than counted as a win.

use std::time::Instant;

use lichen_compute::ComputeValue;
use lichen_compute::language::{LangProgram, LangValue, new_store};
use lichen_compute_gpu::install_default;
use lichen_language::preprocess::preprocess;
use lichen_language::session::BufferSession;
use lichen_lowlevel::AnyNodeId;
use lichen_utils::extend::AsEnum;

const PIPELINE: &str = r#"---
  compute = import "compute.lichen"
---
InA  = struct{.a Int}
OutA = struct{.z (compute.Buf Int)}
ParA = compute.P (compute.KT _)(.I InA, .O OutA)
adder = compute.parallel ((k : ParA) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 3))
}) "cpu" OutA
InB  = struct{.b (compute.Buf Int)}
OutB = struct{.w (compute.Buf Int)}
ParB = compute.P (compute.KT _)(.I InB, .O OutB)
doubler = compute.parallel ((k : ParB) => {
  i = compute.range k.n
  j = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : Int)
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value j + j))
}) "cpu" OutB
raw = (compute.plrun adder ((compute.A InA)(.n 4, .I InA(.a 0))) : OutA)
@cache data = raw.z
raw2 = (compute.plrun doubler ((compute.A InB)(.n 4, .I InB(.b data))) : OutB)
@cache result = raw2.w.native
result
"#;

/// The buffer's elements, decoded at the class the value carries.
fn items(value: &LangValue) -> Option<Vec<i64>> {
    let compute = AsEnum::<ComputeValue>::as_enum(value)?;
    let ComputeValue::Buffer(handle, class) = compute else {
        return None;
    };
    // SAFETY: the handle names a payload in a live arena, dynamic or artifact.
    let bytes = unsafe { std::slice::from_raw_parts(handle.as_ptr(), handle.len()) };
    Some(
        bytes
            .chunks_exact(class.byte_width())
            .map(|element| match class {
                lichen_kernel_ir::ScalarClass::Int => {
                    i64::from_le_bytes(element.try_into().unwrap_or_default())
                }
                lichen_kernel_ir::ScalarClass::Float => {
                    i64::from(u32::from_le_bytes(element.try_into().unwrap_or_default()))
                }
                // The half is the float's high bits, so the word is its pattern
                // lifted back into place.
                lichen_kernel_ir::ScalarClass::BFloat16 => {
                    i64::from(u16::from_le_bytes(element.try_into().unwrap_or_default())) << 16
                }
            })
            .collect(),
    )
}

struct Row {
    label: &'static str,
    fresh_ms: f64,
    session_ms: f64,
    reused: usize,
    frozen: usize,
    value: Option<Vec<i64>>,
    same: bool,
    build_reused: bool,
    key_len: usize,
    /// Whether a full parse and this compile's splice produced the same content key.
    key_matches_fresh: bool,
}

fn main() {
    if let Err(reason) = install_default() {
        println!("SKIPPED: no device: {reason}");
        return;
    }
    let mut store = new_store(None).expect("the embedded compute package registers");
    let (preprocessed, diags) = preprocess(PIPELINE, None, &mut store);
    if !diags.is_empty() {
        println!("the pipeline does not preprocess: {diags:?}");
        return;
    }
    let starts = lichen_language::lex::line_starts(PIPELINE);
    // Its own root: the vocabulary scopes the directory, and a probe must start cold.
    let _ = std::fs::remove_dir_all("target/compute-cells");
    let mut session = BufferSession::<LangProgram>::with_registry(
        preprocessed.code,
        "pipeline.lichen",
        store.registry(),
    )
    .with_cell_cache("target/compute-cells");
    session.set_view(
        preprocessed.code,
        preprocessed.code_base,
        &starts,
        &preprocessed.imports,
    );
    let start = Instant::now();
    let first = session.compile();
    println!(
        "== compute pipeline: 2 cached buffer stages, first compile {:.2} ms, \
         frozen {}, ok {} ==",
        start.elapsed().as_secs_f64() * 1000.0,
        first.cells.frozen,
        first.build.as_ref().is_some_and(|build| build.ok),
    );
    for diagnostic in first.diagnostics.iter().take(4) {
        println!("  diagnostic: {diagnostic:?}");
    }

    let mut rows: Vec<Row> = Vec::new();
    // A whitespace-only edit on the first build: no resolved structure moves.
    let first_spaced = PIPELINE.replace("@cache data", "@cache  data");
    rows.push(measure(
        &mut store,
        &mut session,
        &first_spaced,
        &preprocessed.imports,
        "whitespace, first",
    ));
    // Editing the first kernel's constant dirties both stages; editing the tail dirties none.
    let steeper = PIPELINE.replace("i + 3", "i + 4");
    rows.push(measure(
        &mut store,
        &mut session,
        &steeper,
        &preprocessed.imports,
        "kernel constant",
    ));
    // A whitespace edit changes no resolved structure: the whole build is reused.
    let spaced = steeper.replace("@cache data", "@cache  data");
    rows.push(measure(
        &mut store,
        &mut session,
        &spaced,
        &preprocessed.imports,
        "whitespace only",
    ));

    // The same source again: a stable key reuses the build with nothing changed at all.
    rows.push(measure(
        &mut store,
        &mut session,
        &spaced,
        &preprocessed.imports,
        "same source again",
    ));

    println!(
        "\n{:<18} {:>9} {:>10} {:>8} {:>7} {:>7} {:>8} {:>6} {:>7}",
        "edit", "fresh ms", "session ms", "speedup", "build", "reused", "frozen", "value", "key="
    );
    println!("{}", "-".repeat(80));
    for row in &rows {
        println!(
            "{:<18} {:>9.2} {:>10.2} {:>7.1}x {:>5} {:>7} {:>7} {:>6} {:>7}",
            row.label,
            row.fresh_ms,
            row.session_ms,
            row.fresh_ms / row.session_ms.max(0.001),
            if row.build_reused {
                "reuse".to_string()
            } else {
                format!("new/{}", row.key_len)
            },
            row.reused,
            row.frozen,
            if row.same { "same" } else { "DIFF" },
            if row.key_matches_fresh {
                "fresh"
            } else {
                "DIFFERS"
            },
        );
    }
    for row in &rows {
        println!("{}: value {:?}", row.label, row.value);
    }

    // Two full parses, no session: is the content key whitespace-sensitive for this program?
    let plain = fresh_key(PIPELINE);
    let spaced = fresh_key(&first_spaced);
    println!(
        "content key over two full parses: {} entries, equal {}, first difference {:?}",
        plain.len(),
        plain == spaced,
        plain
            .iter()
            .zip(&spaced)
            .position(|(left, right)| left != right),
    );
}

/// The content key of `source` from a full parse in a fresh store.
fn fresh_key(source: &str) -> Vec<u64> {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let (preprocessed, _) = preprocess(source, None, &mut store);
    let starts = lichen_language::lex::line_starts(source);
    let mut session =
        BufferSession::<LangProgram>::with_registry(preprocessed.code, "key", store.registry());
    session.set_view(
        preprocessed.code,
        preprocessed.code_base,
        &starts,
        &preprocessed.imports,
    );
    session.compile().key
}

fn measure(
    store: &mut lichen_language::package::PackageStore<LangProgram>,
    session: &mut BufferSession<LangProgram>,
    source: &str,
    imports: &[lichen_language::preprocess::ResolvedImport],
    label: &'static str,
) -> Row {
    // The fresh arm is its own session over the same store, so nothing is retained.
    let (code, base, diags) = {
        let (preprocessed, diags) = preprocess(source, None, store);
        (preprocessed.code.to_string(), preprocessed.code_base, diags)
    };
    if !diags.is_empty() {
        println!("fresh arm: {label} does not preprocess: {diags:?}");
    }
    // The line starts must describe *this* source: an edit that adds a line moves them.
    let starts = lichen_language::lex::line_starts(source);
    let start = Instant::now();
    let mut fresh =
        BufferSession::<LangProgram>::with_registry(code.clone(), "fresh", store.registry());
    fresh.set_view(code.clone(), base, &starts, imports);
    let fresh_report = fresh.compile();
    let fresh_ms = start.elapsed().as_secs_f64() * 1000.0;
    let fresh_value = fresh_report
        .build
        .as_ref()
        .and_then(|build| build.module.node_value(AnyNodeId::Dynamic(build.root_val)))
        .and_then(|value| items(&value));

    session.set_view(code, base, &starts, imports);
    let start = Instant::now();
    let report = session.compile();
    let session_ms = start.elapsed().as_secs_f64() * 1000.0;
    let value = report
        .build
        .as_ref()
        .and_then(|build| build.module.node_value(AnyNodeId::Dynamic(build.root_val)))
        .and_then(|value| items(&value));

    Row {
        label,
        fresh_ms,
        session_ms,
        reused: report.cells.reused,
        frozen: report.cells.frozen,
        same: fresh_value == value,
        value,
        build_reused: report.reused,
        key_len: report.key.len(),
        key_matches_fresh: fresh_report.key == report.key,
    }
}
