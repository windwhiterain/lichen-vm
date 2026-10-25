//! The fusion file: the plan it answers, and the pass report it produces.
//!
//! # Invariant
//! **The file steers and the pass still decides.** The probe shows one region refused by a
//! legality check under the plan that names it, refused by the plan's own budget when the
//! budget is tighter, and refused by the plan's own barrier: the gate declines a region, and
//! it never accepts one a legality check refuses.

use lichen_compute::fusion_file::{Budget, FusionPlan, emit_plan, read_fusion_file};
use lichen_compute::placement_pack::read_placement_pack;
use lichen_serve::{PRE_STEP, record, record_with_plan};

/// The class pack the composed step is prepared with: bf16 for the three weight slots.
const CLASS_PACK: &str = r#"{"name":"narrow","requires":{"class":null,"nodes":[2,6,7],"classes":[{"at":1,"class":"bfloat16"}]},"plan":{"staging":[]}}"#;

/// The file the probe reads, beside this example.
const MANIFEST: &str = "fusion_file.json";

fn main() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(MANIFEST);
    let text = std::fs::read_to_string(&path).expect("the fusion file is beside the probe");
    println!("=== the fusion file: {}", path.display());
    println!("{text}");

    let file = read_fusion_file(&text).expect("the fusion file reads");
    let plan = file.plan().expect("the file answers a plan");
    println!("=== the plan");
    println!("strategy: {}", plan.strategy.as_str());
    println!(
        "budget: {} node(s), {} deep, a tolerance of {}, {} byte(s) of staging, {} pass(es)",
        plan.budget.max_region_nodes,
        plan.budget.max_region_depth,
        plan.budget.compute_tolerance,
        plan.budget.staging_bytes,
        plan.budget.passes,
    );
    for pack in &plan.overrides {
        println!("override: the pack \"{}\"", pack.name());
    }
    for barrier in &plan.barriers {
        println!("barrier: {barrier}");
    }

    let class_pack = read_placement_pack(CLASS_PACK).expect("the class pack");
    let mut packs = vec![class_pack.clone()];
    packs.extend(plan.overrides.iter().cloned());

    let recorded = record(PRE_STEP, &packs).expect("the packs record the step");
    println!("=== the packs alone: {} node(s)", recorded.nodes());
    for line in recorded.report() {
        println!("{line}");
    }

    let recorded = record_with_plan(PRE_STEP, &[class_pack.clone()], &plan)
        .expect("the plan records the step");
    println!("=== the plan: {} node(s)", recorded.nodes());
    for line in recorded.report() {
        println!("{line}");
    }

    // The file that report and plan answer: the editable starting point, and the file read back.
    let emitted = emit_plan(recorded.report(), &plan);
    println!("=== the emitted file");
    println!("{emitted}");
    let round = read_fusion_file(&emitted).expect("the emitted file reads");
    let round = round.plan().expect("the emitted file answers a plan");
    println!("=== the round trip");
    println!("strategy: {}", round.strategy.as_str());
    for pack in &round.overrides {
        println!("override: the pack \"{}\"", pack.name());
    }
    for barrier in &round.barriers {
        println!("barrier: {barrier}");
    }

    let tight = FusionPlan {
        budget: Budget {
            max_region_nodes: 1,
            max_region_depth: 1,
            ..plan.budget
        },
        ..plan.clone()
    };
    let recorded = record_with_plan(PRE_STEP, &[class_pack.clone()], &tight)
        .expect("the tight plan records the step");
    println!(
        "=== the plan with a budget of one node: {} node(s)",
        recorded.nodes()
    );
    for line in recorded.report() {
        println!("{line}");
    }

    let region = plan
        .overrides
        .first()
        .and_then(|pack| pack.fuse())
        .and_then(|directive| directive.region())
        .cloned()
        .expect("the override names a region");
    let barred = FusionPlan {
        barriers: vec![region],
        ..plan.clone()
    };
    let recorded = record_with_plan(PRE_STEP, &[class_pack], &barred)
        .expect("the barred plan records the step");
    println!(
        "=== the plan with a barrier on the region: {} node(s)",
        recorded.nodes()
    );
    for line in recorded.report() {
        println!("{line}");
    }
}
