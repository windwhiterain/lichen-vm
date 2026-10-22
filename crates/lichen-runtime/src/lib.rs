//! `lichen-runtime`: an artifact, the passes and the packs, read on IR alone.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! **Nothing here takes a `Module`.** `Module` is `lichen-lowlevel`'s, so it is the
//! line between this crate and `lichen-compute`: an IR type belongs here, and anything
//! that also wants a `Module` belongs to the language side. `cargo tree -p
//! lichen-runtime` naming no language crate *is* the check.

pub mod automatic_tiling;
pub mod device;
pub mod fused;
pub mod fusion_claim;
pub mod graph_pass;
pub mod kernel_registry;
pub mod load;
pub mod module_cache;
pub mod packs;
pub mod placement_pack;
pub mod region_merge;
#[cfg(feature = "wasm")]
pub mod wasm;

pub use automatic_tiling::tiling_pass;
pub use device::{DeviceFacts, DeviceFactsSource, DeviceRefusal};
pub use fused::schema::FORMAT_VERSION as ARTIFACT_FORMAT_VERSION;
pub use fused::{
    FusedKernel, FusedRefusal, LandingRefusal, land_fused_kernel, malformed_at, read_fused_kernel,
    write_fused_kernel,
};
pub use graph_pass::{GraphPass, LaunchExtent, pass_chain};
pub use kernel_registry::{KernelEntry, claimed_kernels, kernel_claim, kernel_fragment};
pub use load::{LoadRefusal, LoadedKernel, load_fused_kernel};
#[cfg(feature = "wasm")]
pub use module_cache::cached_module;
pub use module_cache::{
    configure_kernel_disk_cache, kernel_disk_cache, kernel_disk_cache_dir, module_cache_disk_hits,
    module_cache_disk_writes, module_cache_misses,
};
pub use placement_pack::{PlacementPack, read_placement_pack};
pub use region_merge::{MergeRefusal, region_merge_pass};
