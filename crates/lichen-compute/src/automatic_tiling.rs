//! The automatic tiling pass, installed for a module.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! Every entry point here takes a `Module<LangProgram>`, so this is the language side: it reads
//! the kernel identities a module's own values name and hands the pass the kernels it finds. The
//! pass itself is `lichen_runtime::automatic_tiling`, which takes no `Module`.

use lichen_kernel_ir::{KernelClaim, KernelFragment, KernelId};
use lichen_lowlevel::Module;

use crate::compute::{GraphPass, set_graph_pass};
use crate::fusion_claim::instances;
use crate::language::LangProgram;
use crate::placement_pack::PlacementPack;

/// Install the automatic tiling pass for `module`, with no pack installed.
pub fn install_tiling(module: &Module<LangProgram>) {
    install_tiling_with_packs(module, &[]);
}

/// Install the tiling pass for `module`, with the packs a runner installed there.
///
/// # Invariant
/// The join is built from the module's own kernel identities: a node holds a fragment clone and
/// no kernel id, so the digest is their only key. The packs are consulted in order, and the first
/// whose hash is the node's and whose constraints hold fires; a node no pack covers is tiled by
/// the engine's own choice, the floor `docs/notes/graph-fusion.md` §5 puts under every pack.
pub fn install_tiling_with_packs(module: &Module<LangProgram>, packs: &[PlacementPack]) {
    set_graph_pass(tiling_pass_with_packs(module, packs));
}

/// The pass `install_tiling` installs, for a caller that wants to wrap it.
pub fn tiling_pass(module: &Module<LangProgram>) -> Box<dyn GraphPass> {
    tiling_pass_with_packs(module, &[])
}

/// The pass `install_tiling_with_packs` installs, for a caller that wants to wrap it.
///
/// # Invariant
/// The one spelling of the construction: `install_tiling_with_packs` installs exactly this pass,
/// so a caller that wraps it — in a chain, or in a test — wraps what the installer runs.
pub fn tiling_pass_with_packs(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
) -> Box<dyn GraphPass> {
    lichen_runtime::automatic_tiling::tiling_pass(kernels(module), packs)
}

/// The kernels a module's own nodes name, as the derivation reads them.
///
/// # Invariant
/// The id is read from a node whose own value names a kernel, never re-derived from the source:
/// a kernel *value* is a `Copy` id, so the fragment the registry holds beside it is the only
/// compiled body there is.
fn kernels(module: &Module<LangProgram>) -> Vec<(KernelId, KernelClaim, KernelFragment)> {
    instances(module)
        .into_iter()
        .map(|instance| (instance.kernel, instance.claim, instance.fragment))
        .collect()
}
