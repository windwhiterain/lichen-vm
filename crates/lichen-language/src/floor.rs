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
