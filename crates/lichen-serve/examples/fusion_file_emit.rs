//! The fusion file a report and a plan answer: the emitted text, and the file read back.
//!
//! # Invariant
//! **The emitted text is a file the reader reads**: the example records a step under a plan,
//! emits the file that report and plan answer, and reads it back — so the round trip is shown
//! rather than asserted, and a human sees the file a run would leave behind.

use lichen_compute::fusion_file::{emit_plan, read_fusion_file};
use lichen_compute::placement_pack::read_placement_pack;
use lichen_serve::{PRE_STEP, record_with_plan};

/// The class pack the composed step is prepared with: bf16 for the three weight slots.
const CLASS_PACK: &str = r#"{"name":"narrow","requires":{"class":null,"nodes":[2,6,7],"classes":[{"at":1,"class":"bfloat16"}]},"plan":{"staging":[]}}"#;

/// The file the example reads, beside this example.
const MANIFEST: &str = "fusion_file.json";

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(MANIFEST);
    let text = std::fs::read_to_string(&path).expect("the fusion file is beside the example");
    let file = read_fusion_file(&text).expect("the fusion file reads");
    let plan = file.plan().expect("the file answers a plan");

    let class_pack = read_placement_pack(CLASS_PACK).expect("the class pack");
    let recorded =
        record_with_plan(PRE_STEP, &[class_pack], &plan).expect("the plan records the step");
    println!("=== the report: {} node(s)", recorded.nodes());
    for line in recorded.report() {
        println!("{line}");
    }

    let emitted = emit_plan(recorded.report(), &plan);
    println!("=== the emitted file");
    println!("{emitted}");

    let read = read_fusion_file(&emitted).expect("the emitted file reads");
    let round = read.plan().expect("the emitted file answers a plan");
    println!("=== the round trip");
    println!("strategy: {}", round.strategy.as_str());
    println!(
        "budget: {} node(s), {} deep, a tolerance of {}, {} byte(s) of staging, {} pass(es)",
        round.budget.max_region_nodes,
        round.budget.max_region_depth,
        round.budget.compute_tolerance,
        round.budget.staging_bytes,
        round.budget.passes,
    );
    for pack in &round.overrides {
        println!("override: the pack \"{}\"", pack.name());
    }
    for barrier in &round.barriers {
        println!("barrier: {barrier}");
    }
}
