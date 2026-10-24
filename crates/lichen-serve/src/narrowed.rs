//! A projection the model can run at half width, which is how its weights are meant to travel.

use lichen_compute::compute::clear_graph_pass;
use lichen_compute::placement_pack::read_placement_pack;

use crate::{MATVEC_BATCH_STEP, Prepared, QwenRefusal, Session};

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

/// The pack a graph with no listed nodes takes: the class applies to every node.
pub fn wildcard_pack_text() -> String {
    r#"{
  "name": "narrow",
  "requires": { "class": "bfloat16" },
  "plan": { "staging": [] }
}"#
    .to_string()
}

/// Prepare `source` with one class per listed node and input slot, and nothing else.
///
/// # Invariant
/// `nodes` are graph positions and `classes` are `(input slot, class)` pairs: a composed graph
/// has no schema to name, so its positions are the contract between this call and the program.
pub fn narrowed_composed<'session>(
    session: &'session Session,
    source: &str,
    nodes: &[usize],
    classes: &[(usize, &str)],
) -> Result<Prepared<'session>, QwenRefusal> {
    let listed: Vec<String> = classes
        .iter()
        .map(|(at, class)| format!(r#"{{"at":{at},"class":"{class}"}}"#))
        .collect();
    let positions: Vec<String> = nodes.iter().map(|node| node.to_string()).collect();
    let text = format!(
        r#"{{"name":"narrow","requires":{{"class":null,"nodes":[{}],"classes":[{}]}},"plan":{{"staging":[]}}}}"#,
        positions.join(","),
        listed.join(",")
    );
    clear_graph_pass();
    let pack = read_placement_pack(&text)
        .map_err(|refusal| QwenRefusal::Weights(format!("the composed pack: {refusal}")))?;
    session
        .prepare_with_packs(source, &[pack])
        .map_err(QwenRefusal::Step)
}

/// The `sha256:<hex>` a node's line prints.
pub fn reported_schema(lines: &[String]) -> Option<String> {
    let line = lines.iter().find(|line| line.contains("sha256:"))?;
    let start = line.find("sha256:")?;
    Some(line[start..start + "sha256:".len() + 64].to_string())
}

/// The projection, prepared so a pack states `BFloat16` for its buffers.
pub fn narrowed_projection(session: &Session) -> Result<Prepared<'_>, QwenRefusal> {
    narrowed(session, MATVEC_BATCH_STEP)
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
    let schema = reported_schema(probe.report());
    drop(probe);
    clear_graph_pass();
    // A composed graph names no node, so the wildcard states the class for every node.
    let text = match &schema {
        Some(schema) => pack_text(schema),
        None => wildcard_pack_text(),
    };
    let pack = read_placement_pack(&text)
        .map_err(|refusal| QwenRefusal::Weights(format!("the pack for {schema:?}: {refusal}")))?;
    session
        .prepare_with_packs(source, &[pack])
        .map_err(QwenRefusal::Step)
}
