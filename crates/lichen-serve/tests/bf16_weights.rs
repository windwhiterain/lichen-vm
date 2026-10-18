//! A projection whose weights a pack stores in bf16, compared against the same projection in f32.

mod common;

use common::read;
use lichen_compute::automatic_tiling::install_tiling_with_packs;
use lichen_compute::compute::clear_graph_pass;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_compute::placement_pack::read_placement_pack;
use lichen_kernel_ir::ScalarClass;
use lichen_serve::{MATVEC_STEP, Session};

/// The pack a human writes: the schema to target, and the class that input is stored in.
fn pack_text(schema: &str) -> String {
    format!(
        r#"{{
  "schema": "{schema}",
  "name": "narrow",
  "requires": {{ "class": "bfloat16" }},
  "plan": {{ "staging": [ {{ "name": "tile", "space": "shared", "group_width": 64 }} ] }}
}}"#
    )
}

/// The `sha256:<hex>` a node's line prints.
fn reported_schema(lines: &[String]) -> Option<String> {
    let line = lines.iter().find(|line| line.contains("sha256:"))?;
    let start = line.find("sha256:")?;
    Some(line[start..start + "sha256:".len() + 64].to_string())
}

/// The program with a tail that **runs** the graph, which is what makes the pass report.
///
/// # Invariant
/// The kernel half is `MATVEC_STEP`'s own text, so the schema the probe reports is the schema the
/// session's copy has: the pass matches on that hash, and a probe over different text would teach a
/// hash nothing would answer to.
fn probe_source() -> String {
    let whole = MATVEC_STEP;
    let tail = "\nbuilt\n";
    let kernel = whole
        .strip_suffix(tail)
        .unwrap_or_else(|| panic!("the step's tail is its last two lines"));
    format!(
        r#"{kernel}
xs = (compute.buffer ([1.0, 2.0, 3.0, 4.0] : array{{Float, 4}}) : (compute.Buf _))
ws = (compute.buffer ([1.0, 0.0, 0.0, 1.0] : array{{Float, 4}}) : (compute.Buf _))
out = compute.collect (compute.graphrun built (2, 2, xs, ws))
out
"#
    )
}

/// The bf16 bytes of `values`, which is the top half of each f32.
fn halves(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::BFloat16,
        data: values
            .iter()
            .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
            .collect(),
    }
}

/// A f32 buffer argument of `values`.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

/// The projection answers the same numbers when a pack narrows its storage.
///
/// # Invariant
/// The two runs differ only in the class the pack states, so the pair of answers is the whole
/// measurement: a bf16 element keeps eight mantissa bits, so the two agree to a few parts in a
/// thousand and to nothing at all if the bytes were reinterpreted the wrong way round.
// Ignored: the emitter reads a mixed node's second input through the first's stride.
#[ignore = "a mixed-class node answers wrongly: the emitter uses one stride for the whole fragment"]
#[test]
fn a_narrowed_projection_answers_the_same_numbers() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_narrowed_projection_answers_the_same_numbers: no device");
        return;
    };
    let values = [1.0_f32, 2.0, 3.0, 4.0];
    let weights = [1.0_f32, 0.0, 0.0, 1.0];
    let plain = session
        .prepare(MATVEC_STEP)
        .unwrap_or_else(|refusal| panic!("the projection prepares: {refusal}"));
    let reference = run(&session, &plain, floats(&values), floats(&weights));
    println!("in f32: {reference:?}");

    // The schema, learned from a run: only a dispatch makes the pass report.
    clear_graph_pass();
    let placeholder = read_placement_pack(&pack_text(&format!("sha256:{}", "0".repeat(64))))
        .expect("the placeholder reads");
    let mut store = lichen_compute::language::new_store(None).expect("a store");
    let (first, _value, _root) =
        lichen_language::testing::run_at(&probe_source(), None, &mut store);
    install_tiling_with_packs(&first, &[placeholder]);
    let (module, _value, _root) =
        lichen_language::testing::run_at(&probe_source(), None, &mut store);
    let lines: Vec<String> = module
        .extension_diagnostics
        .iter()
        .map(|entry| entry.message.clone())
        .collect();
    let Some(schema) = reported_schema(&lines) else {
        panic!("a running graph prints its node's schema: {lines:?}");
    };
    println!("the projection's schema is {schema}");
    clear_graph_pass();

    let pack = read_placement_pack(&pack_text(&schema)).expect("the real pack reads");
    let narrowed = session
        .prepare_with_packs(MATVEC_STEP, &[pack])
        .unwrap_or_else(|refusal| panic!("the narrowed projection prepares: {refusal}"));
    // The pack narrows the node's **first** input, which is the vector; the weights stay f32.
    let narrowed_answer = run(&session, &narrowed, halves(&values), floats(&weights));
    println!("in bf16: {narrowed_answer:?}");
    for (index, (wide, narrow)) in reference.iter().zip(&narrowed_answer).enumerate() {
        assert!(
            (wide - narrow).abs() < 1.0e-1,
            "row {index} answered {narrow} in bf16 where f32 answered {wide}"
        );
    }
}

/// Run the projection over one 2x2 matrix and read its two rows.
fn run(
    session: &Session,
    prepared: &lichen_serve::Prepared<'_>,
    vector: RunArgument,
    weight: RunArgument,
) -> Vec<f32> {
    let stepped = prepared
        .step(vec![
            RunArgument::Count(2),
            RunArgument::Count(2),
            vector,
            weight,
        ])
        .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("the projection answers with one buffer, and this one answered otherwise");
    };
    let values = read(session, *resident);
    session.release(*resident);
    values
}
