//! A node's schema, learned from a recording, and the pack that class comes back through.

use lichen_compute::compute::clear_graph_pass;
use lichen_compute::placement_pack::read_placement_pack;
use lichen_serve::record;

/// The pack a human writes: the schema to target, and the class that input is stored in.
fn pack_text(schema: &str, class: &str) -> String {
    format!(
        r#"{{
  "schema": "{schema}",
  "name": "narrow",
  "requires": {{ "class": "{class}" }},
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

/// A program whose tail is the recorded graph, which is what a recording is asked for.
const RECORDED: &str = r#"
---
  compute = import "compute.lichen"
---
In   = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
Rows = struct{.y (compute.Buf Float)}
Par  = struct{.n Int, .dim Int, .in In, .out Rows}
Arg  = struct{.n Int, .dim Int, .I In}
inner = (k : Par) => {
  row = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (row * k.dim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at ((s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at row, .value (fold (k.dim, 0.0 ~ "identity"))))
}
kernel = compute.parallel (inner ~ "claim:Reduction(combine,identity)") "gpu" Rows
step = (s : Par) => {
  compute.plrun kernel ((Arg)(.n s.n, .dim s.dim, .I In(.x s.in.x, .w s.in.w))) : Rows
}
built = compute.graph step
built
"#;

/// The report names the schema once a graph is recorded, and a pack for it is then readable.
///
/// # Invariant
/// The schema is learned from a **recording** and not from a run: the pass is asked while the graph
/// is recorded, which dispatches nothing, so the step a placeholder pack returns already carries the
/// lines that name every node's schema. A hash learned from a run would target a node this build
/// does not have.
#[test]
fn a_recorded_graph_reports_the_schema_a_pack_must_target() {
    clear_graph_pass();
    let placeholder = read_placement_pack(&pack_text(
        &format!("sha256:{}", "0".repeat(64)),
        "bfloat16",
    ))
    .expect("the placeholder reads");
    let recorded = record(RECORDED, &[placeholder])
        .unwrap_or_else(|refusal| panic!("the graph is recorded: {refusal}"));
    let lines: Vec<String> = recorded.report().to_vec();
    println!("the recording holds {} line(s)", lines.len());
    for line in &lines {
        println!("{line}");
    }
    let Some(schema) = reported_schema(&lines) else {
        panic!("a recorded graph prints its node's schema: {lines:?}");
    };
    println!("the node's schema is {schema}");

    // The real pack for that schema: the recording under it shows the class reaching the node.
    clear_graph_pass();
    let pack = read_placement_pack(&pack_text(&schema, "bfloat16"))
        .unwrap_or_else(|refusal| panic!("the real pack reads: {refusal}"));
    let recorded = record(RECORDED, &[pack])
        .unwrap_or_else(|refusal| panic!("the packed graph records: {refusal}"));
    let packed: Vec<String> = recorded.report().to_vec();
    for line in &packed {
        println!("packed: {line}");
    }
    assert!(
        packed.iter().any(|line| line.contains("BFloat16")),
        "the pack's class reaches the node: {packed:?}"
    );
}
