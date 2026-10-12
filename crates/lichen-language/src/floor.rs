//! The floor: both automatic transforms, as the one installed pass.
//! See docs/notes/graph-fusion.md §5.

use lichen_compute::compute::{GraphPass, pass_chain, set_graph_pass};
use lichen_lowlevel::Module;

use crate::automatic_tiling::tiling_pass_with_packs;
use crate::placement_pack::PlacementPack;
use crate::program::LangProgram;
use crate::region_merge::region_merge_pass_with_packs;

/// Install the floor for `module`, with no pack installed.
pub fn install_floor(module: &Module<LangProgram>) {
    install_floor_with_packs(module, &[]);
}

/// Install the floor for `module`, with the packs a runner installed there.
///
/// # Invariant
/// **The region merge, then the automatic tiling**, as one `set_graph_pass`: a merge leaves
/// fewer nodes for the tiling pass to look at, and a second installer would replace the first
/// rather than follow it, so a caller would get one transform and not both.
pub fn install_floor_with_packs(module: &Module<LangProgram>, packs: &[PlacementPack]) {
    set_graph_pass(floor_pass_with_packs(module, packs));
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

/// The pass `install_floor_with_packs` installs, for a caller that wants to wrap it.
///
/// # Invariant
/// One spelling of the order: the installer and any caller that wraps the chain read the same
/// two stages, so a test that wraps it wraps what a run dispatches.
pub fn floor_pass_with_packs(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
) -> Box<dyn GraphPass> {
    pass_chain(vec![
        region_merge_pass_with_packs(module, packs),
        tiling_pass_with_packs(module, packs),
    ])
}
