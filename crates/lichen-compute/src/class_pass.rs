//! The class pass, installed for a module.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! Every entry point here takes a `Module<LangProgram>`, so this is the language side: it reads
//! the claimed kernels a module's own values name and hands the pass the instances it finds.
//! The pass itself is `lichen_runtime::class_pass`, which takes no `Module`.

use lichen_lowlevel::Module;

use crate::compute::{GraphPass, set_graph_pass};
use crate::fusion_claim::instances;
use crate::language::LangProgram;
use crate::placement_pack::PlacementPack;

/// Install the class pass for `module`, with no pack installed.
///
/// # Invariant
/// The pass states the classes the packs name and nothing else, so a caller that installs it
/// alone changes no node's algebra and no region's legality.
pub fn install_class_pass(module: &Module<LangProgram>) {
    set_graph_pass(class_pass_with_packs(module, &[]));
}

/// The pass the floor installs at its front, for a caller that wants to wrap it.
///
/// # Invariant
/// **One spelling of the construction**: the floor installs exactly this pass, so a caller that
/// wraps the chain wraps what a run dispatches.
pub fn class_pass_with_packs(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
) -> Box<dyn GraphPass> {
    lichen_runtime::class_pass::class_pass(instances(module), packs)
}
