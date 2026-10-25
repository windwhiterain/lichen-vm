//! The kernel id a module's own values name.
//! See docs/notes/graph-fusion.md §3 and §4.
//!
//! # Invariant
//! **The claim, the fragment and the schema come from the registry, which is the runtime's.**
//! What is left here is the one reader that needs the language: walking a `Module` to find
//! which node holds which kernel id.

use std::collections::HashSet;

use crate::compute::{ComputeValue, KernelId};
use lichen_kernel_ir::ClaimRefusal;
use lichen_lowlevel::{AnyNodeId, Module, NodeId};
use lichen_utils::extend::AsEnum;

use crate::language::LangProgram;

pub use lichen_runtime::fusion_claim::{
    Between, ELEMENTWISE, Instance, Port, PortKind, REDUCTION, RegionSchema, RoleProjection,
    SchemaHash, SchemaNode, SchemaRefusal, claim_of, derive_regions, fragment_of, marked_value,
    region_positions,
};

/// The claimed kernels a module's own nodes name, as the passes read them.
///
/// # Invariant
/// The id is read from a node whose own value names a kernel, never re-derived from the source: a
/// kernel *value* is a `Copy` id, so the fragment the registry holds beside it is the only
/// compiled body there is.
pub fn instances(module: &Module<LangProgram>) -> Vec<Instance> {
    let mut instances = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for (id, _node) in module.nodes.iter() {
        let Ok(kernel) = kernel_id(module, id) else {
            continue;
        };
        if !seen.insert(kernel) {
            continue;
        }
        let (Some(claim), Ok(fragment)) = (claim_of(kernel), fragment_of(kernel)) else {
            continue;
        };
        instances.push(Instance {
            kernel,
            claim,
            fragment,
        });
    }
    instances
}

/// The kernel id a value names, or the refusal that names what the value is.
///
/// # Invariant
/// A `compute.parallel` result is the wrapper struct whose `.native` field holds the id,
/// and a `compute.jit` result holds it directly — one walk for both shapes, because the
/// host that reads claims should not have to know which spelling a program used.
pub fn kernel_id(module: &Module<LangProgram>, value: NodeId) -> Result<KernelId, ClaimRefusal> {
    if let Some(id) = kernel_id_below(module, value, 0) {
        return Ok(id);
    }
    Err(ClaimRefusal::NotAKernel {
        found: describe(module, value),
    })
}

/// The kernel id a node's value holds, or one an array it nests holds, to a bounded depth.
///
/// # Invariant
/// A compound's element is a slot and a wrapper nests its `.native`, so the walk goes one
/// level per compound; the bound is [`KERNEL_DEPTH`], past which a value names itself.
fn kernel_id_below(module: &Module<LangProgram>, value: NodeId, depth: usize) -> Option<KernelId> {
    if depth > KERNEL_DEPTH {
        return None;
    }
    if let Some(id) = kernel_id_of(module, value) {
        return Some(id);
    }
    // SAFETY: `value` is live, and no block is dropped here.
    for item in unsafe { module.array_items(value) }.unwrap_or_default() {
        if let AnyNodeId::Dynamic(node) = item.node
            && let Some(id) = kernel_id_below(module, node, depth + 1)
        {
            return Some(id);
        }
    }
    None
}

/// The deepest array nesting a kernel lookup walks.
const KERNEL_DEPTH: usize = 4;

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
        .node_value(AnyNodeId::Dynamic(value))
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
