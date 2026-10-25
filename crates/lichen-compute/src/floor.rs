//! The floor: both automatic transforms, as the one installed pass.
//! See docs/notes/graph-fusion.md §5.

use crate::compute::{GraphPass, pass_chain, set_graph_pass};
use lichen_lowlevel::Module;

use crate::automatic_tiling::tiling_pass_with_packs;
use crate::class_pass::class_pass_with_packs;
use crate::fusion_file::FusionPlan;
use crate::language::LangProgram;
use crate::placement_pack::PlacementPack;
use crate::region_merge::region_merge_pass_with_packs;

/// Install the floor for `module`, with no pack installed.
pub fn install_floor(module: &Module<LangProgram>) {
    install_floor_with_packs(module, &[]);
}

/// Install the floor for `module`, with the packs a runner installed there.
///
/// # Invariant
/// **The classes, then the region merge, then the automatic tiling**, as one `set_graph_pass`: a
/// merge leaves fewer nodes for the tiling pass to look at, the classes are what a merged node's
/// ports inherit, and a second installer would replace the first rather than follow it, so a
/// caller would get one transform and not both.
pub fn install_floor_with_packs(module: &Module<LangProgram>, packs: &[PlacementPack]) {
    set_graph_pass(floor_pass_with_packs(module, packs));
}

/// Install the floor for `module`, with the packs and the plan a runner installed there.
///
/// # Invariant
/// The same three stages as [`install_floor_with_packs`], and the plan rides the **region merge**
/// alone: a plan states which regions are attempted and how large they may be, and the tiling
/// pass tiles what the merge left.
pub fn install_floor_with_plan(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
    plan: &FusionPlan,
) {
    set_graph_pass(floor_pass_with_plan(module, packs, plan));
}

/// Install the floor for a run, before that run's program is compiled.
///
/// # Invariant
/// **The seed module carries no kernel, and that is the whole of it**: a module's own join names
/// only the kernels it has already evaluated, so one that is not compiled yet has none, and both
/// stages answer from the kernel registry the compile is about to fill — the same join, from the
/// same registry, that the module itself would have named once it ran.
pub fn install_floor_for_run(packs: &[PlacementPack]) {
    install_floor_with_packs(&Module::new(), packs);
}

/// Install the floor for a run, with the packs and the plan a runner installed there.
///
/// # Invariant
/// The same seed module as [`install_floor_for_run`], and the plan rides the **region merge**
/// alone: a plan states which regions are attempted and how large they may be, and the tiling
/// pass tiles what the merge left.
pub fn install_floor_for_run_with_plan(packs: &[PlacementPack], plan: &FusionPlan) {
    install_floor_with_plan(&Module::new(), packs, plan);
}

/// The pass `install_floor_with_packs` installs, for a caller that wants to wrap it.
///
/// # Invariant
/// One spelling of the order: the installer and any caller that wraps the chain read the same
/// three stages, so a test that wraps it wraps what a run dispatches.
pub fn floor_pass_with_packs(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
) -> Box<dyn GraphPass> {
    pass_chain(vec![
        class_pass_with_packs(module, packs),
        region_merge_pass_with_packs(module, packs),
        tiling_pass_with_packs(module, packs),
    ])
}

/// The pass `install_floor_with_plan` installs, for a caller that wants to wrap it.
///
/// # Invariant
/// The same three stages in the same order, with the plan's policy on the region merge: a
/// caller that wraps this chain wraps what a run dispatches.
pub fn floor_pass_with_plan(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
    plan: &FusionPlan,
) -> Box<dyn GraphPass> {
    pass_chain(vec![
        class_pass_with_packs(module, packs),
        crate::region_merge::region_merge_pass_with_plan(module, packs, plan),
        tiling_pass_with_packs(module, packs),
    ])
}
