//! A node's schema, learned from a run, and the pack that class comes back through.

use lichen_compute::automatic_tiling::install_tiling_with_packs;
use lichen_compute::compute::clear_graph_pass;
use lichen_compute::placement_pack::read_placement_pack;
use lichen_serve::Session;

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

/// A program that runs its graph, which is what makes the pass report at all.
const RUNNING: &str = r#"
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
xs = (compute.buffer ([1.0, 2.0, 3.0, 4.0] : array{Float, 4}) : (compute.Buf _))
ws = (compute.buffer ([1.0, 0.0, 0.0, 1.0] : array{Float, 4}) : (compute.Buf _))
out = compute.collect (compute.graphrun built (2, 2, xs, ws))
out
"#;

/// The report names the schema once a graph runs, and a pack for it is then readable.
#[test]
fn a_running_graph_reports_the_schema_a_pack_must_target() {
    let Ok(_session) = Session::open() else {
        eprintln!("SKIPPED a_running_graph_reports_the_schema_a_pack_must_target: no device");
        return;
    };
    clear_graph_pass();
    let placeholder = read_placement_pack(&pack_text(
        &format!("sha256:{}", "0".repeat(64)),
        "bfloat16",
    ))
    .expect("the placeholder reads");
    let mut store = lichen_compute::language::new_store(None).expect("a store");
    let (first, _value, _root) = lichen_language::testing::run_at(RUNNING, None, &mut store);
    install_tiling_with_packs(&first, &[placeholder]);
    let (module, _value, _root) = lichen_language::testing::run_at(RUNNING, None, &mut store);
    let lines: Vec<String> = module
        .extension_diagnostics
        .iter()
        .map(|entry| format!("{}: {}", entry.category, entry.message))
        .collect();
    println!("the module holds {} diagnostic(s)", lines.len());
    for line in &lines {
        println!("{line}");
    }
    let Some(schema) = reported_schema(&lines) else {
        panic!("a running graph prints its node's schema: {lines:?}");
    };
    println!("the node's schema is {schema}");

    // The real pack for that schema: the run under it shows the class reaching the node.
    clear_graph_pass();
    let pack = read_placement_pack(&pack_text(&schema, "bfloat16"))
        .unwrap_or_else(|refusal| panic!("the real pack reads: {refusal}"));
    let mut store = lichen_compute::language::new_store(None).expect("a store");
    let (first, _value, _root) = lichen_language::testing::run_at(RUNNING, None, &mut store);
    install_tiling_with_packs(&first, &[pack]);
    let (module, _value, _root) = lichen_language::testing::run_at(RUNNING, None, &mut store);
    let packed: Vec<String> = module
        .extension_diagnostics
        .iter()
        .map(|entry| format!("{}: {}", entry.category, entry.message))
        .collect();
    for line in &packed {
        println!("packed: {line}");
    }
    assert!(
        packed.iter().any(|line| line.contains("BFloat16")),
        "the pack's class reaches the node: {packed:?}"
    );
}
