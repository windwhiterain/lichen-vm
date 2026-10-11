//! The interface claim a kernel declares and the fragment its marks live in.
//! See docs/notes/graph-fusion.md §3 and §4.
//!
//! # Invariant
//! **Both facts come from the registry, neither from the value.** The claim was read when
//! `compute.parallel`/`compute.jit` compiled the kernel and stored beside its fragment; the
//! body marks were published into that same fragment by the same walk. A kernel *value* is a
//! `Copy` id, and only the fragment the **dispatcher** compiled is the one a launch runs.

use lichen_compute::{ComputeValue, KernelId, kernel_claim};
use lichen_kernel_ir::{ClaimRefusal, KernelClaim, KernelFragment, ValueId};
use lichen_lowlevel::{AnyNodeId, Module, NodeId};
use lichen_utils::extend::AsEnum;

use crate::program::LangProgram;

mod schema;

pub use schema::{
    Between, Instance, Port, PortKind, RegionSchema, RoleProjection, SchemaHash, SchemaNode,
    SchemaRefusal, derive_regions,
};

/// The kernel id a value names, or the refusal that names what the value is.
///
/// # Invariant
/// A `compute.parallel` result is the wrapper struct whose `.native` field holds the id,
/// and a `compute.jit` result holds it directly — one walk for both shapes, because the
/// host that reads claims should not have to know which spelling a program used.
pub fn kernel_id(module: &Module<LangProgram>, value: NodeId) -> Result<KernelId, ClaimRefusal> {
    if let Some(id) = kernel_id_of(module, value) {
        return Ok(id);
    }
    // The wrapper's own fields, where `.native` is.
    // SAFETY: `value` is live, and no block is dropped here.
    for item in unsafe { module.array_items(value) }.unwrap_or_default() {
        if let AnyNodeId::Dynamic(node) = item.node
            && let Some(id) = kernel_id_of(module, node)
        {
            return Ok(id);
        }
    }
    Err(ClaimRefusal::NotAKernel {
        found: describe(module, value),
    })
}

/// The interface a registered kernel claims, or `None` when it claims none.
///
/// # Invariant
/// Read out of the registry the compile filled, so a host never reads a claim off a runtime
/// value and never re-derives one from the source.
pub fn claim_of(id: KernelId) -> Option<KernelClaim> {
    kernel_claim(id)
}

/// The fragment a kernel's body's marks live in.
///
/// # Invariant
/// The registry's own read: the marks are already published in it, so a reader carves a
/// projection out of the fragment the **dispatcher** compiled — the one whose nodes are a
/// per-call clone — rather than out of a body compiled a second time from the value the host
/// happens to hold.
pub fn fragment_of(id: KernelId) -> Result<KernelFragment, ClaimRefusal> {
    lichen_compute::kernel_fragment(id).ok_or(ClaimRefusal::UnknownKernel { id })
}

/// The body value the fragment's `role` mark names.
///
/// # Invariant
/// A role mark is read by the role it names, and the interface comes from the claim the
/// instance was assembled with, so a body never restates which interface it belongs to. A
/// role the body does not mark is refused by name.
pub fn marked_value(
    fragment: &KernelFragment,
    interface: &str,
    role: &str,
) -> Result<ValueId, ClaimRefusal> {
    fragment
        .marks
        .iter()
        .find(|mark| mark.role() == Some(role))
        .map(|mark| mark.value)
        .ok_or_else(|| ClaimRefusal::RoleNotMarked {
            interface: interface.to_string(),
            role: role.to_string(),
        })
}

/// The kernel id a value's own class holds, when it holds one.
fn kernel_id_of(module: &Module<LangProgram>, value: NodeId) -> Option<KernelId> {
    match compute_value(module, value) {
        Some(ComputeValue::Kernel(id) | ComputeValue::ParKernel(id, _)) => Some(id),
        _ => None,
    }
}

/// The compute value a node's class holds, or `None` when it holds none.
fn compute_value(module: &Module<LangProgram>, value: NodeId) -> Option<ComputeValue> {
    module
        .class_value(value)
        .or_else(|| module.node_value(AnyNodeId::Dynamic(value)))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
}

/// What a value is, for a refusal that names it.
fn describe(module: &Module<LangProgram>, value: NodeId) -> String {
    match compute_value(module, value) {
        Some(ComputeValue::Kernel(_)) | Some(ComputeValue::ParKernel(..)) => "a kernel".to_string(),
        Some(ComputeValue::Buffer(..)) | Some(ComputeValue::DeviceBuffer(_)) => {
            "a buffer".to_string()
        }
        Some(_) => "a compute value".to_string(),
        None => "an undecided value".to_string(),
    }
}
