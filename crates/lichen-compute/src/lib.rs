//! The `lichen-compute` native plugin and the sub-language it composes.
//! See docs/notes/plugin-taxonomy.md §5.

pub mod automatic_tiling;
pub mod compute;
pub mod floor;
pub mod fusion_claim;
pub mod language;
pub mod packs;
pub mod placement_pack;
pub mod region_merge;

pub use lichen_highlevel::native::{NativeOp, NativeOps};

pub use compute::{
    BufferCollectOp, BufferOp, BufferPayload, CallOp, ColOp, ComputeOperator, ComputePlugin,
    ComputeValue, FusedKernel, FusedRefusal, GraphId, GraphOp, GraphPass, GraphRunOp, GraphValueId,
    JitOp, KernelEntry, KernelId, LANDING_DIAGNOSTIC, LandingRefusal, LaunchExtent, LaunchOp,
    MmaOp, ParLaunchOp, ParallelOp, RangeOp, ReadOp, RoleUnread, RowOp, WRAPPER_SOURCE,
    WriteAtomicOp, WriteOp, clear_graph_pass, compile_fragment, compile_parallel_fragment,
    consult_graph_pass, kernel_claim, kernel_fragment, land_fused_kernel, module_cache_disk_hits,
    module_cache_disk_writes, module_cache_misses, read_fused_kernel, set_graph_pass,
    set_graph_policy, take_graph_pass_lines, write_fused_kernel,
};

/// Contribute this plugin's vocabulary leaves into a host's composition.
///
/// # Invariant
/// The macro name is `<crate_ident>_leaves`, not a shared name: two
/// `#[macro_export]` macros named identically in one dependency graph collide in the
/// extern prelude. The accumulator and the remaining plugin list are threaded through
/// verbatim so the composition continues past this plugin.
#[macro_export]
macro_rules! lichen_compute_leaves {
    ($next:path, [ $($oa:tt)* ][ $($va:tt)* ][ $($aa:tt)* ][ $($b:tt)* ][ $($rn:tt)* ] ; [ $($rest:tt)* ] ;) => {
        $next! {
            @absorb (
                operators: [ lichen_compute::ComputeOperator as ComputeOperator; ];
                values: [ lichen_compute::ComputeValue as ComputeValue; ];
                attrs: [ ];
                [ $($oa)* ][ $($va)* ][ $($aa)* ][ $($b)* ][ $($rn)* ] ; [ $($rest)* ] ;
            )
        }
    };
}
