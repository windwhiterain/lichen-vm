//! The region merge pass, installed for a module.
//! See docs/notes/graph-fusion.md §5 and §8.
//!
//! # Invariant
//! Every entry point here takes a `Module<LangProgram>`, so this is the language side: it reads
//! the claimed kernels a module's own values name and hands the pass the instances it finds.
//! The pass itself is `lichen_runtime::region_merge`, which takes no `Module`.

use lichen_lowlevel::Module;

use crate::compute::{GraphPass, set_graph_pass};
use crate::fusion_claim::instances;
use crate::fusion_file::FusionPlan;
use crate::language::LangProgram;
use crate::placement_pack::PlacementPack;

pub use lichen_runtime::region_merge::MergeRefusal;

/// Install the region merge pass for `module`, with no pack installed.
pub fn install_region_merge(module: &Module<LangProgram>) {
    install_region_merge_with_packs(module, &[]);
}

/// Install the region merge pass for `module`, with the packs a runner installed there.
///
/// # Invariant
/// The packs are consulted in order and the first whose `fuse` names a region's schema and whose
/// constraints hold licenses that region; a region no pack names is left to the floor, which
/// merges only what a declaration lets it prove (`docs/notes/graph-fusion.md` §5).
pub fn install_region_merge_with_packs(module: &Module<LangProgram>, packs: &[PlacementPack]) {
    set_graph_pass(region_merge_pass_with_packs(module, packs));
}

/// Install the region merge pass for `module`, with the packs and the plan a runner installed.
///
/// # Invariant
/// The plan's budget and its barriers are the policy the pass reads, and the plan's own
/// force-merge overrides are installed beside the packs: a plan alone steers the merges it
/// names, and every legality check stands whatever the plan says.
pub fn install_region_merge_with_plan(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
    plan: &FusionPlan,
) {
    set_graph_pass(region_merge_pass_with_plan(module, packs, plan));
}

/// The pass `install_region_merge` installs, for a caller that wants to wrap it.
pub fn region_merge_pass(module: &Module<LangProgram>) -> Box<dyn GraphPass> {
    region_merge_pass_with_packs(module, &[])
}

/// The pass `install_region_merge_with_packs` installs, for a caller that wants to wrap it.
pub fn region_merge_pass_with_packs(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
) -> Box<dyn GraphPass> {
    lichen_runtime::region_merge::region_merge_pass(instances(module), packs)
}

/// The pass `install_region_merge_with_plan` installs, for a caller that wants to wrap it.
pub fn region_merge_pass_with_plan(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
    plan: &FusionPlan,
) -> Box<dyn GraphPass> {
    lichen_runtime::region_merge::region_merge_pass_with_plan(instances(module), packs, plan)
}
