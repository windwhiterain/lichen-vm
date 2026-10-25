//! The interface claim a kernel declares and the fragment its marks live in.
//! See docs/notes/graph-fusion.md §3 and §4.
//!
//! # Invariant
//! **Both facts come from the registry, neither from the value.** The claim was read when the
//! kernel compiled and stored beside its fragment; the body marks were published into that same
//! fragment by the same walk. A kernel *value* is a `Copy` id, and only the fragment the
//! **dispatcher** compiled is the one a launch runs — so a reader here needs no `Module`.

use lichen_kernel_ir::{ClaimRefusal, KernelClaim, KernelFragment, KernelId, ValueId};

use crate::kernel_registry::{kernel_claim, kernel_fragment};

mod schema;

pub use schema::{
    BRIDGED, Between, DeclaredFold, ELEMENTWISE, Instance, OPAQUE, Port, PortKind, REDUCTION,
    Region, RegionSchema, RegionShape, RoleProjection, SchemaHash, SchemaNode, SchemaRefusal,
    derive_regions, ordinary_regions, region_positions, sibling_regions,
};
pub(crate) use schema::{offsets, producer_ordinal};

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
    kernel_fragment(id).ok_or(ClaimRefusal::UnknownKernel { id })
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

/// The claim a merged fragment is registered with: the opaque interface, and no roles.
///
/// # Invariant
/// **A merged fragment is registered with this claim and no other**: the passes name it and
/// refuse it by name, and they read no algebra from it — so no pass can take a merged node for
/// a reduction to compose again.
pub fn merged_claim() -> KernelClaim {
    KernelClaim {
        interface: OPAQUE.to_string(),
        roles: Vec::new(),
    }
}
