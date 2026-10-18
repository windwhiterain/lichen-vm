//! A projection the model can run at half width, which is how its weights are meant to travel.

use lichen_compute::compute::clear_graph_pass;
use lichen_compute::placement_pack::read_placement_pack;

use crate::{MATVEC_STEP, Prepared, QwenRefusal, Session};

/// The pack a human writes for `schema`: the class and a stage, which no projection is wide enough for.
pub fn pack_text(schema: &str) -> String {
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
pub fn reported_schema(lines: &[String]) -> Option<String> {
    let line = lines.iter().find(|line| line.contains("sha256:"))?;
    let start = line.find("sha256:")?;
    Some(line[start..start + "sha256:".len() + 64].to_string())
}

/// The projection's text with a tail that **runs** the graph, which is what makes the pass report.
///
/// # Invariant
/// The kernel half is `MATVEC_STEP`'s own text, so the schema the probe reports is the schema the
/// projection is prepared under: a pack matches on that hash and nothing else.
fn probe_source() -> String {
    let tail = "\nbuilt\n";
    let kernel = MATVEC_STEP
        .strip_suffix(tail)
        .unwrap_or_else(|| panic!("the step's tail is its last line"));
    format!(
        r#"{kernel}
xs = (compute.buffer ([1.0, 2.0, 3.0, 4.0] : array{{Float, 4}}) : (compute.Buf _))
ws = (compute.buffer ([1.0, 0.0, 0.0, 1.0] : array{{Float, 4}}) : (compute.Buf _))
out = compute.collect (compute.graphrun built (2, 2, xs, ws))
out
"#
    )
}

/// The projection, prepared so a pack states `BFloat16` for its buffers.
pub fn narrowed_projection(session: &Session) -> Result<Prepared<'_>, QwenRefusal> {
    narrowed(session, MATVEC_STEP)
}

/// Prepare `source` with a pack stating `BFloat16` for the classes its first node reads.
///
/// # Invariant
/// The schema is learned from a **preparation**, not from a run: the pass is asked while the graph is
/// recorded, so the step a placeholder pack returns already carries the lines that name every node's
/// schema. A hash learned any other way would target a node this build does not have.
pub fn narrowed<'session>(
    session: &'session Session,
    source: &str,
) -> Result<Prepared<'session>, QwenRefusal> {
    clear_graph_pass();
    let placeholder = read_placement_pack(&pack_text(&format!("sha256:{}", "0".repeat(64))))
        .map_err(|refusal| QwenRefusal::Weights(format!("the placeholder pack: {refusal}")))?;
    let probe = session
        .prepare_with_packs(source, &[placeholder])
        .map_err(QwenRefusal::Step)?;
    let Some(schema) = reported_schema(probe.report()) else {
        return Err(QwenRefusal::Weights(format!(
            "a preparation names its nodes' schemas: {:?}",
            probe.report()
        )));
    };
    drop(probe);
    clear_graph_pass();
    let pack = read_placement_pack(&pack_text(&schema))
        .map_err(|refusal| QwenRefusal::Weights(format!("the pack for {schema}: {refusal}")))?;
    session
        .prepare_with_packs(source, &[pack])
        .map_err(QwenRefusal::Step)
}
