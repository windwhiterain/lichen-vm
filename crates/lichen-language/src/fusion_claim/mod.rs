//! The interface claim a kernel carries, as a host reads it.
//! See docs/notes/graph-fusion.md §3 and §9.
//!
//! # Invariant
//! A claim rides the `?` label slot of the expression whose value is the kernel, and it
//! names the interface and the roles that interface requires. The projections are **not**
//! in the claim: each is a mark on an expression inside the kernel's own body, so a claim
//! cannot diverge from what the kernel computes.

mod payload;
mod refusal;

pub use refusal::ClaimRefusal;

use lichen_compute::{
    ComputeValue, KernelId, compile_fragment, compile_parallel_fragment, kernel_fragment,
};
use lichen_highlevel::shape::{TypeRef, field_names};
use lichen_kernel_ir::{KernelFragment, ValueId};
use lichen_lowlevel::{AnyFunctionId, AnyNodeId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

use crate::program::LangProgram;

/// The interface a kernel implements, and the roles its body marks for it.
#[derive(Debug, Clone)]
pub struct Claim {
    /// The interface's name, as the claim spells it in its `.name` field.
    pub interface: String,
    /// The roles the interface requires, as the claim's `.roles` field lists them.
    pub roles: Vec<String>,
}

/// The claim the value `value` carries.
///
/// # Invariant
/// The label is read through [`Module::label_of`], because the pair it sits on is not
/// reachable from the value: the checker publishes it against both.
pub fn read_claim(module: &Module<LangProgram>, value: NodeId) -> Result<Claim, ClaimRefusal> {
    let slot = module.label_of(value).ok_or(ClaimRefusal::NoLabel)?;
    let fields = payload::record(module, slot)?;
    Ok(Claim {
        interface: payload::text(module, &fields, "name")?,
        roles: payload::texts(module, &fields, "roles")?,
    })
}

/// The fragment a claimed value's projections live in.
///
/// # Invariant
/// A **kernel body** is compiled here, and its in-body marks are published. A **kernel
/// value** the dispatcher built is read from the registry instead, and carries no marks
/// today: `compute.parallel` compiles a per-call clone whose nodes the checker's stamps do
/// not reach (docs/notes/graph-fusion.md §3). Which one the value is is a fact, not a guess.
pub fn claim_fragment(
    module: &mut Module<LangProgram>,
    value: NodeId,
) -> Result<KernelFragment, ClaimRefusal> {
    let held = held_value(module, value);
    if let Some(LowValue::Function(function)) = held {
        return compile_body(module, function);
    }
    if let Some(kernel) = kernel_of(module, value) {
        return kernel_fragment(kernel).ok_or(ClaimRefusal::UnknownKernel { id: kernel });
    }
    Err(ClaimRefusal::NotAKernel {
        found: payload::describe_compute(compute_of(module, value)),
    })
}

/// Compile a kernel body through the entry its own parameter's type states.
///
/// # Invariant
/// A **named-struct** parameter is a parallel index function and a scalar or tuple one is a
/// scalar kernel: which entry compiles the body is a fact of the type the source wrote, so
/// neither entry is tried and neither refusal stands in for a decision.
fn compile_body(
    module: &mut Module<LangProgram>,
    function: AnyFunctionId,
) -> Result<KernelFragment, ClaimRefusal> {
    let parallel = struct_parameter(module, function);
    let compiled = if parallel {
        compile_parallel_fragment(module, function)
    } else {
        compile_fragment(module, function)
    };
    compiled.map_err(|reason| ClaimRefusal::NotAKernel {
        found: format!("a body the kernel compiler refused: {reason}"),
    })
}

/// Whether a function's parameter is the named struct a parallel index function takes.
fn struct_parameter(module: &Module<LangProgram>, function: AnyFunctionId) -> bool {
    let AnyFunctionId::Dynamic(function) = function else {
        return false;
    };
    let Some(parameter) = module.functions.get(function).map(|it| it.parameter) else {
        return false;
    };
    // SAFETY: `parameter` is a live node of `module`; nothing here releases a block.
    let Some(pair) = (unsafe { module.array_items(parameter) }) else {
        return false;
    };
    let ty = pair.get(lichen_highlevel::shape::PAIR_TYPE_SLOT);
    matches!(
        ty.map(|item| item.node),
        Some(node) if field_names(module, TypeRef::Term(node)).is_some()
    )
}

/// The body value the fragment's `role` mark names.
///
/// # Invariant
/// A role mark is read by the role it names — `? "combine"`, or a record whose `as` field
/// holds it — and the interface comes from the claim the mark rides under, so a body never
/// restates which interface it belongs to. A role the body does not mark is refused by name.
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

/// The kernel id a claimed value names, if the value is a kernel the dispatcher built.
fn kernel_of(module: &Module<LangProgram>, value: NodeId) -> Option<KernelId> {
    if let Some(ComputeValue::Kernel(id) | ComputeValue::ParKernel(id, _)) =
        compute_of(module, value)
    {
        return Some(id);
    }
    // The wrapper's own fields, where `.native` is.
    // SAFETY: `value` is live, and no block is dropped here.
    for item in unsafe { module.array_items(value) }.unwrap_or_default() {
        if let AnyNodeId::Dynamic(node) = item.node
            && let Some(ComputeValue::Kernel(id) | ComputeValue::ParKernel(id, _)) =
                compute_of(module, node)
        {
            return Some(id);
        }
    }
    None
}

/// The compute value a node's class holds, or `None` when it holds none.
fn compute_of(module: &Module<LangProgram>, value: NodeId) -> Option<ComputeValue> {
    module
        .class_value(value)
        .or_else(|| module.node_value(AnyNodeId::Dynamic(value)))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
}

/// The lowlevel value a node's class holds, or `None` when it holds none.
fn held_value(module: &Module<LangProgram>, value: NodeId) -> Option<LowValue> {
    module
        .class_value(value)
        .or_else(|| module.node_value(AnyNodeId::Dynamic(value)))
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
}

/// The function a claimed value names, when it is a kernel body rather than a kernel.
pub fn body_of(module: &Module<LangProgram>, value: NodeId) -> Option<AnyFunctionId> {
    match held_value(module, value) {
        Some(LowValue::Function(function)) => Some(function),
        _ => None,
    }
}
