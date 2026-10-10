//! The `lichen-compute` native plugin. See docs/notes/lichen-compute.md.

pub mod compute;

#[cfg(test)]
mod waffle_spike;

pub use lichen_highlevel::native::{NativeOp, NativeOps};

pub use compute::{
    BufferCollectOp, BufferPayload, CallOp, ComputeOperator, ComputePlugin, ComputeValue, GraphId,
    GraphOp, GraphRunOp, GraphValueId, JitOp, KernelId, LaunchOp, ParLaunchOp, ParallelOp, RangeOp,
    ReadOp, WRAPPER_SOURCE, WriteOp, module_cache_misses, set_graph_policy,
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
    ($next:path, [ $($oa:tt)* ][ $($va:tt)* ][ $($aa:tt)* ][ $($b:tt)* ] ; [ $($rest:tt)* ] ;) => {
        $next! {
            @absorb (
                operators: [ lichen_compute::ComputeOperator as ComputeOperator; ];
                values: [ lichen_compute::ComputeValue as ComputeValue; ];
                attrs: [ ];
                [ $($oa)* ][ $($va)* ][ $($aa)* ][ $($b)* ] ; [ $($rest)* ] ;
            )
        }
    };
}
