//! The `lichen-compute` native plugin.
//! See docs/notes/lichen-compute.md and compute-kernel-struct.md.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use lichen_graph_ir::{Count, Node, Policy};
use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp};
use lichen_highlevel::program::{
    Ctx, HighProgram, LeafKindMarkers, TypeOperator, TypeValue, ValueType,
};
use lichen_highlevel::shape;
use lichen_highlevel::shape::{expr_type, expr_value, low_type_of_slot};
use lichen_kernel_ir::{
    BufferSlot, CLAIM_PREFIX, IntWidth, KernelBin, KernelClaim, KernelFragment, KernelInstr,
    KernelMark, KernelRoles, KernelShape, LaneHeight, LaneWidth, ResidentId, ScalarClass,
    ScalarData, ScalarLeaf, TargetFacts,
};
use lichen_lowlevel::codec::{OperatorCodec, Reader, ValueCodec, Writer};
use lichen_lowlevel::{
    AnyFunctionId, AnyHandle, AnyNodeId, ArrayItem, ArrayValue, BlockId, ExtensionDiagnostic,
    FunctionId, LowOperator, LowShape, LowValue, Module, ModuleKey, NodeId, Operation, OperatorExt,
    Program, Release, StaticModule, TableItem, ValueExt,
};
use lichen_utils::extend::AsEnum;

mod body;
use body::{Lower, MAX_KERNEL_BODY_DEPTH};
pub mod fused;
pub mod graph;
pub mod graph_pass;

use graph::{Extent, Placed, RunArgument, RunResult};
use lichen_runtime::wasm::{
    HostWords, SEQUENTIAL_PARALLEL_ELEMENTS, assemble_module, buffer_import_name, param_classes,
    run_kernel as run_on_engine, run_parallel,
};

pub use fused::{
    FusedKernel, FusedRefusal, LandingRefusal, land_fused_kernel, malformed_at, read_fused_kernel,
    write_fused_kernel,
};
pub use graph::GraphId;
pub use graph_pass::{
    GraphPass, LaunchExtent, clear_graph_pass, consult_graph_pass, pass_chain, set_graph_pass,
    take_graph_pass_lines,
};

// The emitted-module cache is `lichen-runtime`'s: it takes a `KernelId`, never a `Module`.
pub use lichen_runtime::{
    cached_module, module_cache_disk_hits, module_cache_disk_writes, module_cache_misses,
};

// The kernel registry is `lichen-runtime`'s too, and it is keyed by a `KernelId`.
pub use lichen_runtime::kernel_registry::{
    KernelEntry, claimed_kernels, kernel_claim, kernel_fragment,
};
use lichen_runtime::kernel_registry::{intern_kernel, kernels};

/// The diagnostic a graph's own refusals are recorded under.
///
/// # Invariant
/// Every graph refusal is about the recording, never about a run, so filing one
/// under `compute.parallel` would blame the launch the author did not write.
pub const GRAPH_DIAGNOSTIC: &str = "compute.graph";

/// The diagnostic a graph pass's own lines are recorded under.
///
/// # Invariant
/// A pass is guidance rather than a launch, so a line it reports — a tiling it applied, or a
/// node it declined to tile and why — is filed under the pass and never under a kernel the
/// author wrote.
pub const LANDING_DIAGNOSTIC: &str = "compute.graph.pass";

/// The schedule a graph run uses.
///
/// # Invariant
/// Rust-side only, and never in the language: a graph states what precedes
/// what, never when the host may notice a finish — a graph that named its own
/// synchronisation would make correctness depend on where a keyword was put.
/// `Policy::Async` is the default; `Batch` needs a host that asks for it.
pub fn set_graph_policy(policy: Policy) {
    GRAPH_POLICY.with(|current| current.set(policy));
}

thread_local! {
    static GRAPH_POLICY: Cell<Policy> = const { Cell::new(Policy::Async) };
}

/// A compiled kernel artifact's identity: a compact index into the process
/// kernel registry.
///
/// # Invariant
/// A kernel value is host-owned (a small `Copy` scalar), so it is never an arena
/// payload and needs no GC re-homing; the id lives in the lowered-kernel IR
/// because a cross-kernel call there must name a callee.
pub use lichen_kernel_ir::KernelId;

/// A parallel-buffer artifact's payload: `n` collected elements, packed (`D15`).
/// See [`ComputeValue::Buffer`].
///
/// # Invariant
/// A `Copy` handle like an array/table payload: the block's lifetime *is* the
/// ownership, and the length is in *bytes*, never elements.
pub type BufferPayload = AnyHandle<[u8]>;

#[cfg(test)]
mod kernel_intern_tests {
    use super::*;
    use lichen_kernel_ir::{Br, FlatOp, KernelBody, Terminator};

    fn fragment(body: KernelBody) -> KernelFragment {
        KernelFragment {
            param_shape: KernelShape::Scalar(ScalarClass::Int),
            roles: KernelRoles::default(),
            body,
            inputs: 0,
            outputs: 0,
            input_classes: Vec::new(),
            output_classes: Vec::new(),
            result_classes: vec![ScalarClass::Int],
            target: TargetFacts {
                int_width: IntWidth::I64,
                lane_width: LaneWidth::new(64),
                lane_height: LaneHeight::new(1),
            },
            slices: Vec::new(),
            marks: Vec::new(),
        }
    }

    /// A counting loop, in the shape the IR can express.
    ///
    /// # Invariant
    /// The loop's state is the header block's `params`, so a body's read of it is an
    /// ordinary read of a named value — a header that receives the carried state, a body
    /// computing the next one, and an exit taking what the header has.
    fn counting_loop(backedge: bool) -> KernelBody {
        let mut body = KernelBody::new();
        let entry = body.add_block();
        let one = body.add_const(entry, ScalarClass::Int, 1);
        // **A branch hands the target its parameters**, so entering the header hands it
        // the state it starts from.
        body.set_terminator(
            entry,
            Terminator::Br(Br {
                target: 1,
                args: vec![one],
            }),
        );

        let header = body.add_block();
        let carried = body.add_param(header);
        let next = body.add_op(
            header,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![carried, one],
            vec![ScalarClass::Int],
        );
        // **A backedge *is* a branch back to the header**, handing over the next
        // iteration's state.
        body.set_terminator(
            header,
            if backedge {
                Terminator::Br(Br {
                    target: header,
                    args: vec![next],
                })
            } else {
                Terminator::Br(Br {
                    target: 2,
                    args: vec![carried],
                })
            },
        );

        let exit = body.add_block();
        let result = body.add_param(exit);
        body.set_terminator(
            exit,
            Terminator::Return {
                values: vec![result],
            },
        );
        body
    }

    /// A backedge is not a special form: it is a `Br` naming the current block.
    #[test]
    fn a_backedge_is_a_branch_to_the_header_and_a_branch_nowhere_is_refused() {
        counting_loop(true)
            .validate()
            .unwrap_or_else(|broken| panic!("a loop with a backedge is well formed: {broken}"));

        let mut body = counting_loop(true);
        body.set_terminator(
            body.blocks.len() - 1,
            Terminator::Br(Br {
                target: 99,
                args: Vec::new(),
            }),
        );
        let refusal = body
            .validate()
            .expect_err("a branch to a block this body does not have has nowhere to go");
        assert!(
            refusal.contains("branch"),
            "the refusal must name the branch, and said: {refusal}"
        );
    }

    /// A straight-line body is still the form a lowering produces.
    #[test]
    fn a_straight_line_body_is_still_straight_line() {
        let body = KernelBody::from_flat(
            1,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
                FlatOp::Read(0),
            ],
        );
        assert!(body.is_straight_line());
        assert_eq!(body.instrs().len(), 1);
        body.validate()
            .expect("a straight-line body is well formed");
    }

    /// The same function compiled twice must intern to one id: every cache is
    /// keyed on it.
    #[test]
    fn the_same_fragment_interns_to_one_id() {
        let one = KernelBody::from_flat(1, &[FlatOp::Read(0)]);
        let first = intern_kernel(fragment(one.clone()), None);
        let second = intern_kernel(fragment(one), None);
        assert_eq!(
            first, second,
            "a recompiled identical fragment must reuse its id"
        );
    }

    /// ...and different fragments must not collide onto one id, or the cache
    /// would serve one kernel's module for another's.
    #[test]
    fn a_different_fragment_gets_a_different_id() {
        let a = intern_kernel(fragment(KernelBody::from_flat(1, &[FlatOp::Read(0)])), None);
        let b = intern_kernel(fragment(KernelBody::from_flat(2, &[FlatOp::Read(1)])), None);
        assert_ne!(a, b, "different content must not share an id");
        // Both stay registered under their own ids.
        let registry = kernels().lock().unwrap();
        assert!(registry.contains_key(&a) && registry.contains_key(&b));
    }

    /// A branch handing a block more values than it takes is refused before a backend
    /// reads it.
    #[test]
    fn a_branch_handing_over_more_than_its_target_takes_is_refused() {
        let mut body = KernelBody::new();
        let entry = body.add_block();
        let one = body.add_const(entry, ScalarClass::Int, 1);
        let two = body.add_const(entry, ScalarClass::Int, 2);
        body.set_terminator(
            entry,
            Terminator::Br(Br {
                target: 1,
                args: vec![],
            }),
        );

        let exit = body.add_block();
        for _ in 0..2 {
            body.add_param(exit);
        }
        body.set_terminator(exit, Terminator::Return { values: vec![] });

        // Well formed when the handover matches.
        body.set_terminator(
            entry,
            Terminator::Br(Br {
                target: 1,
                args: vec![one, two],
            }),
        );
        body.validate()
            .expect("a branch handing over exactly the target's parameters is well formed");

        // Refused when it hands over one more than the target takes.
        body.set_terminator(
            entry,
            Terminator::Br(Br {
                target: 1,
                args: vec![one, two, one],
            }),
        );
        let refusal = body
            .validate()
            .expect_err("a branch cannot hand over more values than its target takes");
        assert!(
            refusal.contains("value"),
            "the refusal must name the arity, and said: {refusal}"
        );
    }
}

/// The scalar classes a parameter slot's shape flattens to, in order.
pub(crate) fn scalar_classes_of(shape: &LowShape) -> Vec<ScalarClass> {
    let mut classes = Vec::new();
    flatten_classes_into(shape, &mut classes);
    classes
}

/// Append one shape's leaves' classes, recursing for a tuple.
fn flatten_classes_into(shape: &LowShape, classes: &mut Vec<ScalarClass>) {
    match shape {
        LowShape::Float => classes.push(ScalarClass::Float),
        LowShape::Tuple(items) => {
            for item in items.iter().flatten() {
                flatten_classes_into(item, classes);
            }
        }
        _ => classes.push(ScalarClass::Int),
    }
}

/// The element results of a buffer payload — a borrowed view into the block
/// arena the value lives in.
///
/// # Safety
///
/// The caller must hold the value the handle came from on a borrow of its
/// module, so the payload's home block is alive — the same obligation every
/// reader of an arena payload carries (`lichen_lowlevel::Handle::from_raw`).
fn buffer_items(payload: &BufferPayload) -> Option<&[u8]> {
    match payload {
        // SAFETY: the value is held on a module borrow, so its payload's home
        // block — and this slice — is alive.
        AnyHandle::Dynamic(handle) => Some(unsafe { &*handle.as_ptr() }),
        // SAFETY: the artifact that filed these bytes is pinned by its registry entry.
        AnyHandle::Static(handle) => Some(unsafe { &*handle.as_ptr() }),
    }
}

/// How many elements a packed payload of `bytes` bytes holds at `class`'s width.
fn element_count(class: ScalarClass, bytes: usize) -> usize {
    bytes / class.byte_width()
}

/// The bytes of element `index` in a packed payload, or `None` past the end.
fn element_bytes(class: ScalarClass, payload: &[u8], index: usize) -> Option<&[u8]> {
    let width = class.byte_width();
    payload.get(index * width..(index + 1) * width)
}

/// One buffer element as the class says it is, from the packed bytes it occupies.
///
/// # Invariant
/// The class travels beside the payload because it is what says how many bytes
/// an element is; the bytes alone cannot answer that.
fn element_value(class: ScalarClass, bytes: &[u8]) -> Option<LowValue> {
    match class {
        ScalarClass::Int => {
            let word = i64::from_le_bytes(bytes.try_into().ok()?);
            Some(LowValue::USize(word as usize))
        }
        ScalarClass::Float => {
            let bits = u32::from_le_bytes(bytes.try_into().ok()?);
            Some(LowValue::Float(f32::from_bits(bits)))
        }
        // The half is the float's high bits, so the pattern is lifted back into place.
        ScalarClass::BFloat16 => {
            let half = u16::from_le_bytes(bytes.try_into().ok()?);
            Some(LowValue::Float(f32::from_bits(u32::from(half) << 16)))
        }
    }
}

/// The packed payload of `words` at `class`'s width — encoding for [`element_value`].
///
/// # Invariant
/// Each element is written as exactly its own width, so a float buffer's payload
/// is `count * 4` bytes — what a device module's declared `ArrayStride` reads.
fn pack_elements(class: ScalarClass, words: &[i64]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(words.len() * class.byte_width());
    for word in words {
        match class {
            ScalarClass::Int => payload.extend_from_slice(&word.to_le_bytes()),
            ScalarClass::Float => payload.extend_from_slice(&(*word as u32).to_le_bytes()),
            // A bf16 is the top half of the float it rounds to, so the word keeps its high twice.
            ScalarClass::BFloat16 => {
                payload.extend_from_slice(&((*word as u32) >> 16).to_le_bytes()[..2])
            }
        }
    }
    payload
}

/// The words an interpreter's state holds for `class`, from a packed payload.
///
/// # Invariant
/// The interpreter's state is word-per-element for both classes (an `f32` rides
/// its bits in the low 32), which is a fact about that interpreter and not about
/// the ABI; [`pack_elements`] is the conversion back.
fn unpack_elements(class: ScalarClass, payload: &[u8]) -> Vec<i64> {
    (0..element_count(class, payload.len()))
        .map(|index| match class {
            ScalarClass::Int => i64::from_le_bytes(
                payload[index * 8..index * 8 + 8]
                    .try_into()
                    .unwrap_or_default(),
            ),
            ScalarClass::Float => i64::from(u32::from_le_bytes(
                payload[index * 4..index * 4 + 4]
                    .try_into()
                    .unwrap_or_default(),
            )),
            ScalarClass::BFloat16 => {
                i64::from(u16::from_le_bytes(
                    payload[index * 2..index * 2 + 2]
                        .try_into()
                        .unwrap_or_default(),
                )) << 16
            }
        })
        .collect()
}

/// The bits of one `f32`, as the `i64` word the kernel IR's constants carry.
fn float_bits(value: f32) -> i64 {
    i64::from(value.to_bits())
}

// The lowered-kernel IR lives in `lichen-kernel-ir`, not here, so another
// backend reads a fragment without `wasmi`.

/// Whether a value is a graph placeholder, **or a `Buf` wrapper around one**.
///
/// # Invariant
/// An operator that reads a buffer names the **wrapper**, and the recorder puts the
/// placeholder in `.native`, so asking only whether an operand *is* a placeholder
/// answers `false` for the case the rule exists to catch. The wrapper is recognised by
/// [`buf_value`]'s own construction — two items, the second an element **type**.
fn is_a_graph_placeholder<P>(module: &Module<P>, node: AnyNodeId) -> bool
where
    P: Program,
    P::Value: ValueType + AsEnum<LowValue> + AsEnum<ComputeValue>,
{
    let Some(value) = module.node_value(node) else {
        return false;
    };
    if matches!(
        AsEnum::<ComputeValue>::as_enum(&value),
        Some(ComputeValue::GraphInput(_) | ComputeValue::GraphValue(_))
    ) {
        return true;
    }
    let Some(LowValue::Array(wrapper)) = AsEnum::<LowValue>::as_enum(&value) else {
        return false;
    };
    // SAFETY: `wrapper` is a live array of `module`, and this reads only the `Buf`
    // wrapper's two slots.
    let wrapper = unsafe { wrapper.items() };
    let [payload, element] = wrapper else {
        return false;
    };
    let is_element_type = module
        .node_value(element.node)
        .is_some_and(|value| value == P::Value::int_marker() || value == P::Value::float_marker());
    is_element_type
        && matches!(
            module
                .node_value(payload.node)
                .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value)),
            Some(ComputeValue::GraphInput(_) | ComputeValue::GraphValue(_))
        )
}

/// Whether any operand of `operator` is one of a graph's placeholders.
///
/// # Invariant
/// One level, and one level is enough: every operator that reads a dispatch's output takes
/// it as a direct item of its operand array, so a graph's value is never buried in a
/// structure the scan would walk. A `plrun`'s argument *is* the placeholder structure, which
/// is why this is asked about operators other than a parallel launch; the one level it looks
/// through is the `Buf` wrapper.
fn handed_a_placeholder<P>(module: &Module<P>, operand: &P::Value) -> bool
where
    P: Program,
    P::Value: ValueType + AsEnum<LowValue> + AsEnum<ComputeValue>,
{
    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(operand) else {
        return false;
    };
    // SAFETY: the operand array is the operator's own, and this reads only, for
    // the length of the call.
    let items = unsafe { operands.items() };
    items
        .iter()
        .any(|item| is_a_graph_placeholder(module, item.node))
}

/// Why this operator cannot appear in a body that is being recorded.
///
/// # Invariant
/// A graph has nowhere to put a scalar value: the one node kind is a parallel
/// dispatch with buffers bound to it. A host read or a collect is refused only
/// when it was handed a placeholder — a read of a closed-over host buffer is
/// ordinary host arithmetic. Both used to answer `None` silently, which is an
/// absent answer rather than an undecided one.
fn unrecordable<P>(
    operator: &ComputeOperator,
    module: &Module<P>,
    operand: &P::Value,
) -> Option<String>
where
    P: Program,
    P::Value: ValueType + AsEnum<LowValue> + AsEnum<ComputeValue>,
{
    match operator {
        ComputeOperator::Launch | ComputeOperator::Call => Some(
            "a scalar kernel is called here, and a graph has no node for one. The only node kind \
             is a parallel dispatch, with its buffers bound to it, while a scalar kernel works on \
             its own operand stack and names no buffer — so there is nowhere for what it computes \
             to travel. Run this body un-graphed, or write the work as a parallel kernel."
                .into(),
        ),
        ComputeOperator::Read if handed_a_placeholder(module, operand) => Some(
            "a host read is asked here for a value one of this graph's own dispatches produced, \
             and a graph run cannot do that: its values are edges into a run, and what a run \
             produces is on the device until the run ends. Derive the number from the function's \
             own argument instead, or read it after the graph has run."
                .into(),
        ),
        ComputeOperator::BufferCollect if handed_a_placeholder(module, operand) => Some(
            "a collect is asked here for a value one of this graph's own dispatches produced, and \
             a graph run cannot do that: its values are edges into a run, and what a run produces \
             is on the device until the run ends. Collect after the graph has run — that is the \
             one point at which a \"gpu\" chain's results cross the bus."
                .into(),
        ),
        _ => None,
    }
}

/// The diagnostic categories this plugin records through the extension channel.
const JIT_DIAGNOSTIC: &str = "compute.jit";
const PARALLEL_DIAGNOSTIC: &str = "compute.parallel";
/// Making a buffer out of a program array: the `Buffer` arm.
const BUFFER_DIAGNOSTIC: &str = "compute.buffer";
/// Running an already-compiled kernel: the `Launch` and `Call` arms.
///
/// # Invariant
/// Named for the path, so one category catches a `compute.call` refusal too. The
/// two arms refuse on the same three facts: an argument that is not a parameter
/// vector, a tuple element that is not a concrete `Int`, and a run that failed.
const KERNEL_LAUNCH_DIAGNOSTIC: &str = "compute.kernel_launch";

/// Where a parallel kernel's runs are dispatched.
///
/// # Invariant
/// A named, required choice: `parallel` takes the backend as an argument, so a
/// program always says where its kernels run, and a backend that declines is a
/// named failure. The choice rides on the value ([`ComputeValue::ParKernel`]),
/// not on the fragment, so two programs compiling one function for different
/// backends share an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// This crate's wasm backend: one interpreter per worker thread.
    Cpu,
    /// A compute shader on a device, through an installed
    /// [`lichen_kernel_ir::ParallelBackend`].
    Gpu,
}

impl Backend {
    /// The name this backend is spelled with in source.
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Cpu => "cpu",
            Backend::Gpu => "gpu",
        }
    }
}

/// A backend name that is neither `cpu` nor `gpu`.
///
/// # Invariant
/// The message quotes what was written and what is accepted: a string parameter
/// has no enum to check against, so a typo must be reported rather than
/// defaulted.
fn unknown_backend(written: &str) -> String {
    format!("{written:?} is not a compute backend; name one of \"cpu\" or \"gpu\"")
}

/// The backend a `"cpu"` / `"gpu"` argument names.
fn parse_backend(written: &str) -> Result<Backend, String> {
    match written {
        "cpu" => Ok(Backend::Cpu),
        "gpu" => Ok(Backend::Gpu),
        other => Err(unknown_backend(other)),
    }
}

/// The backend named by an operand, recording a refusal by name.
///
/// # Invariant
/// A non-string backend argument is as much a refusal as the wrong string, and
/// is reported the same way.
fn backend_argument<P>(
    module: &mut Module<P>,
    node: Option<NodeId>,
    category: &'static str,
) -> Option<Backend>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(node) = node else {
        return None;
    };
    let written = module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value));
    let parsed = match written {
        Some(LowValue::Str(text)) => parse_backend(text),
        _ => Err(format!(
            "a compute backend must be the string \"cpu\" or \"gpu\", not {written:?}"
        )),
    };
    match parsed {
        Ok(backend) => Some(backend),
        Err(reason) => {
            module
                .record_extension_diagnostic(ExtensionDiagnostic::failure(category, None, reason));
            None
        }
    }
}

/// One value of a graph's value table.
///
/// # Invariant
/// A `usize`, stable for the graph's life: a node's outputs are consecutive from
/// the first, so nothing has to be looked up to know which is which.
pub type GraphValueId = usize;

/// The compute value vocabulary — a sibling leaf in a host's value union.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComputeValue {
    /// A compiled, runnable kernel artifact.
    ///
    /// # Invariant
    /// No backend, deliberately: a `jit` kernel is launched an invocation at a
    /// time, so the choice belongs to [`Self::ParKernel`].
    Kernel(KernelId),
    /// A compiled **parallel** kernel artifact, and where its runs are dispatched.
    ///
    /// # Invariant
    /// A curried `?a -> USize -> ?b` flattened to `(?a, USize) -> ?b`: the config
    /// is the first group of parameters, the *index* the last scalar.
    ParKernel(KernelId, Backend),
    /// A runtime results **buffer**: `plrun` collected the `n` `?b` results here.
    ///
    /// # Invariant
    /// The payload is a packed byte slice in the block arena — a `Copy` handle
    /// owned by its block (`D15`) — whose elements are [`ScalarClass::byte_width`]
    /// bytes each. **The element class rides on the value**: the payload cannot
    /// say it, so it is read off the producing fragment's
    /// [`KernelFragment::output_classes`] at the ordinal.
    Buffer(BufferPayload, ScalarClass),
    /// A run's results, still **on the device**.
    ///
    /// # Invariant
    /// Not a handle: the id is plain data, so the copy path copies it rather than
    /// relocating an arena pointer — and there is no per-value release, because
    /// the id names device memory reclaimed when the backend drops.
    DeviceBuffer(ResidentBuffer),
    /// A built graph, as its registry slot.
    ///
    /// # Invariant
    /// A `usize` and nothing else: a graph is kernel ids, edge numbers and
    /// counts, so it needs no tracing and states no host obligation. The backend
    /// rides on the value, exactly as it does on [`Self::ParKernel`], because the
    /// graph's shape is content-addressed and the registry stores no answer.
    Graph(GraphId, Backend),
    /// A placeholder for a graph's own `slot`-th input.
    ///
    /// # Invariant
    /// The slot is the number, so `ins(i)` binds the `i`-th argument — and it is
    /// inert: no operation turns a `usize` into device memory, so a closure that
    /// captured one cannot read a buffer.
    GraphInput(usize),
    /// A placeholder for a value this recording has already produced.
    ///
    /// # Invariant
    /// Carrying the value number is the design: the placeholder sits in the node
    /// the next dispatch reads, so the edge follows the operand with no side
    /// table to disagree with.
    GraphValue(GraphValueId),
}

/// A run's results as the language holds them while they are still on a device.
///
/// # Invariant
/// The count travels with the value, not in a side table: fetching needs to know
/// how many elements to ask for. The element class travels here too — a fact
/// about this buffer that the id cannot recover, taken from the producing
/// fragment's [`KernelFragment::output_classes`] at the ordinal — and being a
/// fieldless tag keeps the struct `Eq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentBuffer {
    /// The backend's own name for the buffer: meaningless outside its issuer.
    pub id: ResidentId,
    /// How many elements the run produced — the count, not the padded length.
    pub count: usize,
    /// The class the elements are to be read as.
    pub class: ScalarClass,
}

/// One host buffer's payload as the interpreter holds it: class and words.
///
/// # Invariant
/// The word-per-element shape is the parallel interpreter's own state, not the
/// ABI's ([`BufferSlot`] crosses packed at [`ScalarClass::byte_width`] bytes per
/// element), and [`pack_elements`]/[`unpack_elements`] are the two conversions.
/// A backend reads the class from the fragment's `input_classes`/`output_classes`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BufferWords {
    class: ScalarClass,
    words: Vec<i64>,
}

impl BufferWords {
    /// An integer buffer's payload.
    fn ints(words: Vec<i64>) -> Self {
        BufferWords {
            class: ScalarClass::Int,
            words,
        }
    }

    /// This buffer as the packed bytes the ABI carries.
    fn packed(&self) -> Vec<u8> {
        pack_elements(self.class, &self.words)
    }
}

/// What one parallel run produced: host data, or results left on the device.
///
/// # Invariant
/// A `"cpu"` run always produces the first and a `"gpu"` run the second: a run
/// that produced host data would have paid a download nobody asked for.
#[derive(Debug, PartialEq, Eq)]
enum RunOutcome {
    Host(Vec<BufferWords>),
    Resident(Vec<ResidentBuffer>),
}

/// One input to a run, as the language holds it.
#[derive(Debug)]
enum RunInput {
    /// Data the host already has, with the class its elements are.
    Host(BufferWords),
    /// Results a previous run left on the device.
    Resident(ResidentBuffer),
}

/// The compute leaf has no kind markers: a buffer's type is the struct the type
/// level reads, and a kernel never had one.
impl LeafKindMarkers for ComputeValue {
    fn is_kind_marker(&self) -> bool {
        false
    }
}

/// The compute leaf's payload contract: only [`ComputeValue::Buffer`] is arena data.
///
/// # Invariant
/// A leaf that owns a payload and does not answer `true` here would be copied by
/// reference, leaving a handle pointing into a block that may be released.
impl ValueExt for ComputeValue {
    fn is_handle(&self) -> bool {
        matches!(self, ComputeValue::Buffer(..))
    }

    /// The buffer's payload viewed as bytes: the class's elements packed at
    /// [`ScalarClass::byte_width`] bytes each.
    fn handle(&self) -> AnyHandle<[u8]> {
        match self {
            ComputeValue::Buffer(handle, _) => *handle,
            _ => unreachable!("only Buffer carries a payload"),
        }
    }

    fn set_handle(&mut self, payload: AnyHandle<[u8]>) {
        match self {
            ComputeValue::Buffer(slot, _) => {
                // The same allocation, re-viewed as bytes; the width says the
                // element size, so no class is consulted.
                *slot = payload;
            }
            _ => unreachable!("only Buffer carries a payload"),
        }
    }

    /// A packed element needs no more than `i64`'s alignment to be read.
    ///
    /// # Invariant
    /// Every element access goes through a decoded copy, never a `&[i64]` view;
    /// the composition takes the strictest alignment over its leaves.
    fn alignment() -> usize {
        std::mem::align_of::<i64>()
    }

    /// A value that left its results on the device: freezing it owes that memory back.
    ///
    /// # Invariant
    /// The id alone cannot release: it names a buffer in the backend that issued
    /// it, so the obligation holds the `Arc` the lookup hands back — which keeps
    /// the issuer alive. A backend that is not installed has nothing to release.
    fn release_obligations(&self, out: &mut Vec<Box<dyn Release>>) {
        let ComputeValue::DeviceBuffer(resident) = self else {
            return;
        };
        if let Some(backend) = lichen_kernel_ir::parallel_backend() {
            out.push(Box::new(ReleaseResident {
                backend,
                id: resident.id,
            }));
        }
    }
}

/// One device buffer's release, owed by the artifact that froze the value.
struct ReleaseResident {
    backend: std::sync::Arc<dyn lichen_kernel_ir::ParallelBackend>,
    id: ResidentId,
}

impl Release for ReleaseResident {
    fn release(self: Box<Self>) {
        self.backend.release(self.id);
    }
}

/// The compute operator vocabulary, composed into a host's operator union.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComputeOperator {
    /// Compile a function value to wasm bytecode → a `Kernel` value.
    Jit,
    /// `[kernel, arg]` operand — run the kernel on the arg → the result.
    Launch,
    /// `[kernel, arg]` — a cross-kernel call: run kernel `k` on `arg` → the result.
    Call,
    /// Compile a `compute.P` index function to a parallel kernel → a `ParKernel`.
    Parallel,
    /// `[parallel_kernel, cfg]` — run it over `[0, cfg.n)` → the `.out` structure.
    ParLaunch,
    /// `[n]` — the loop index of the current parallel invocation, `i ∈ [0, n)`.
    Range,
    /// `[cfg]` — the lane's **row**, `i / cols`, over the launch's second extent.
    ///
    /// # Invariant
    /// The extent is the config's **second scalar leaf** and nothing else. `range`'s own
    /// argument is typed and then ignored, and this one is too — a body handed the columns
    /// to divide by would be a body that can take a shape the launch never declared, so
    /// reading the leaf leaves only the configurations that have one able to answer.
    Row,
    /// `[cfg]` — the lane's **column**, `i % cols`, over the launch's second extent.
    ///
    /// # Invariant
    /// [`Self::Row`]'s, and the same read: the two differ in one instruction, and an expr
    /// would compute the half the body did not ask for — the kernel IR has no instruction
    /// that leaves two values, so an expr would be a change to the instruction set instead.
    Col,
    /// `[buffer, index]` — read one buffer element → `?b`.
    Read,
    /// `[length, index, value]` — a pending parallel write; kernel-only.
    ///
    /// # Invariant
    /// The write is **disjoint** by contract: a lane names its own slot, so no two lanes
    /// write one element. A body that scatters needs [`Self::WriteAtomic`] instead.
    Write,
    /// `[length, index, value]` — a pending parallel **atomic add**; kernel-only.
    ///
    /// # Invariant
    /// One read-modify-write per lane, so a scatter accumulates. The order of the adds is
    /// the implementation's, so a `Float` accumulator's answer is a sum whose order is not
    /// the program's — the synchronization this primitive provides is the add's atomicity
    /// and nothing beyond it.
    WriteAtomic,
    /// `[buffer]` operand — collect the whole buffer into a lichen array `[?b]`.
    BufferCollect,
    /// `[length, element]` — declare a **slice**: an undecided value of buffer type.
    ///
    /// # Invariant
    /// **Undecided on purpose**: a slice is a name and a length, not an allocation, so the
    /// operation never evaluates to a `Buffer` payload — the graph keeps the node and the
    /// kernel's own walk decides which output it is (`docs/notes/kernel-buffer-declarations.md`).
    /// A decided value here would be a physical buffer the checker could not trace.
    Buf,
    /// `[type, length]` — the native buffer **type** `(T, L)`: a decided type term.
    BufType,
    /// `[function]` — record the dispatches that function performs, and hand back
    /// a `Graph`.
    ///
    /// # Invariant
    /// The operand **is** deep-evaluated first, by `run_deferred`'s default: this
    /// operator overrides nothing. That is enough because a graph is a bare
    /// function, so the evaluated operand still names it, and `build_graph` reaches
    /// the body through an `Apply` it builds and runs itself.
    Graph,
    /// `[graph, arguments]` — run a graph over its source function's values.
    GraphRun,
    /// `[backend, a, b, c]` — the cooperative matrix product `c' = a·b + c`.
    ///
    /// # Invariant
    /// `a`/`b` hold signed bytes and `c` signed words, row-major, over the shape
    /// [`lichen_kernel_ir::COOPERATIVE_MATRIX_M`] states. The backend is a **required**
    /// argument, as `parallel`'s is: a program says where its product runs, and the two
    /// paths answer the same two's-complement words.
    Mma,
}

// A buffer, a kernel and a parallel kernel persist; every other arm names a process handle.

/// The artifact tag of [`ComputeValue::Buffer`], private to this leaf's value encoding.
const BUFFER_TAG: u8 = 9;

/// The artifact tag of [`ComputeValue::Kernel`].
///
/// # Invariant
/// **A kernel id is a content digest**: `KernelId` is `fragment_digest` of the whole fragment, so
/// the id names the same fragment in every process — the id is the whole handle that travels.
const KERNEL_TAG: u8 = 10;

/// The artifact tag of [`ComputeValue::ParKernel`]: the same id, beside where it runs.
const PAR_KERNEL_TAG: u8 = 11;

/// A collected buffer's element class on the wire: the payload cannot say it, so it travels.
fn write_class(w: &mut Writer, class: ScalarClass) -> Result<(), String> {
    let tag = match class {
        ScalarClass::Int => 0,
        ScalarClass::Float => 1,
        ScalarClass::BFloat16 => 2,
    };
    w.u8(tag);
    Ok(())
}

fn read_class(r: &mut Reader<'_>) -> Result<ScalarClass, String> {
    match r.u8()? {
        0 => Ok(ScalarClass::Int),
        1 => Ok(ScalarClass::Float),
        2 => Ok(ScalarClass::BFloat16),
        other => Err(format!("unknown buffer element class {other}")),
    }
}

/// A kernel's content digest on the wire.
///
/// # Invariant
/// `KernelId` is `fragment_digest` — a `u64` carried in a `usize` — so the wire form is the
/// `u64`. An id that does not fit the host's `usize` is refused by name rather than truncated
/// into another kernel's identity.
fn write_kernel_id(w: &mut Writer, id: KernelId) {
    w.u64(id as u64);
}

fn read_kernel_id(r: &mut Reader<'_>) -> Result<KernelId, String> {
    let bits = r.u64()?;
    usize::try_from(bits).map_err(|_| format!("kernel id {bits} does not fit this host's usize"))
}

/// Where a kernel runs, on the wire: the two backends the language names.
fn write_backend(w: &mut Writer, backend: Backend) {
    w.u8(match backend {
        Backend::Cpu => 0,
        Backend::Gpu => 1,
    });
}

fn read_backend(r: &mut Reader<'_>) -> Result<Backend, String> {
    match r.u8()? {
        0 => Ok(Backend::Cpu),
        1 => Ok(Backend::Gpu),
        other => Err(format!("unknown compute backend {other}")),
    }
}

impl ValueCodec for ComputeValue {
    fn write_value<P: Program>(
        w: &mut Writer,
        value: Self,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<(), String> {
        match value {
            // Files like an array: the handle relocates and the class tags along.
            ComputeValue::Buffer(AnyHandle::Static(handle), class) => {
                w.u8(BUFFER_TAG);
                write_class(w, class)?;
                lichen_lowlevel::codec::write_relocated_handle(
                    w,
                    modules,
                    handle.module,
                    handle.as_ptr(),
                );
                Ok(())
            }
            ComputeValue::Buffer(AnyHandle::Dynamic(_), _) => {
                Err("cannot serialize a module carrying a dynamic buffer payload".into())
            }
            // The handle is the digest, so the id is all of what travels.
            ComputeValue::Kernel(id) => {
                w.u8(KERNEL_TAG);
                write_kernel_id(w, id);
                Ok(())
            }
            ComputeValue::ParKernel(id, backend) => {
                w.u8(PAR_KERNEL_TAG);
                write_kernel_id(w, id);
                write_backend(w, backend);
                Ok(())
            }
            ComputeValue::DeviceBuffer(_) => {
                return Err(
                    "this package is not cached: it holds a buffer that is still on a device \
                     at its top level, and a device buffer is a runtime value with no on-disk \
                     form — the id would mean nothing without the device that issued it"
                        .into(),
                );
            }
            ComputeValue::Graph(..) => {
                return Err(
                    "this package is not cached: it holds a built graph at its top level, and a \
                     graph is a runtime value with no on-disk form — the id names a process \
                     registry entry, which is empty in another process"
                        .into(),
                );
            }
            // A placeholder is refused: its meaning is a position in this
            // recording's value table, not a number.
            ComputeValue::GraphInput(_) | ComputeValue::GraphValue(_) => {
                return Err(
                    "this package is not cached: it holds a graph's own placeholder at its top \
                     level, and a placeholder means nothing outside the recording that made it — \
                     the number is a position in that recording's value table and nothing else"
                        .into(),
                );
            }
        }
    }

    fn read_value<P: Program>(
        r: &mut Reader<'_>,
        self_key: ModuleKey,
        self_arena: &[u8],
        self_base: *const u8,
        modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<Self, String> {
        match r.u8()? {
            BUFFER_TAG => {
                let class = read_class(r)?;
                let handle = lichen_lowlevel::codec::read_relocated_handle::<u8, P>(
                    r, self_key, self_arena, self_base, modules,
                )?;
                Ok(ComputeValue::Buffer(AnyHandle::Static(handle), class))
            }
            KERNEL_TAG => Ok(ComputeValue::Kernel(read_kernel_id(r)?)),
            PAR_KERNEL_TAG => {
                let id = read_kernel_id(r)?;
                Ok(ComputeValue::ParKernel(id, read_backend(r)?))
            }
            other => Err(format!("unknown compute-value tag {other}")),
        }
    }

    /// A kernel, a graph and a device buffer name a registry entry the writer's process holds.
    ///
    /// # Invariant
    /// A cell's artifact is loaded beside its source, so a nested content-addressed kernel id is
    /// still resolvable; a *package* artifact replaces its source, so any of these makes the
    /// module runtime-only (`runtime_only_package` is that rule's test).
    fn names_process_handle(value: &Self) -> bool {
        matches!(
            value,
            ComputeValue::Kernel(_)
                | ComputeValue::ParKernel(..)
                | ComputeValue::Graph(..)
                | ComputeValue::DeviceBuffer(_)
                | ComputeValue::GraphInput(_)
                | ComputeValue::GraphValue(_)
        )
    }
}

impl OperatorCodec for ComputeOperator {
    /// The operator's wire tag.
    ///
    /// # Invariant
    /// A tag is never reused, and a new operator appends: the operation is what the artifact's
    /// value graph is made of, so a tag that moved between operators would make an old artifact
    /// a different program. Every variant is tagged, because a portable cell keeps its
    /// operations and the value codec now carries the kernel handles those operations name.
    fn write_operator(w: &mut Writer, op: Self) -> Result<(), String> {
        let tag: u8 = match op {
            ComputeOperator::Jit => 0,
            ComputeOperator::Launch => 1,
            ComputeOperator::Call => 2,
            ComputeOperator::Parallel => 3,
            ComputeOperator::ParLaunch => 4,
            ComputeOperator::Range => 5,
            ComputeOperator::Row => 6,
            ComputeOperator::Col => 7,
            ComputeOperator::Read => 8,
            ComputeOperator::Write => 9,
            ComputeOperator::WriteAtomic => 10,
            ComputeOperator::Graph => 11,
            ComputeOperator::GraphRun => 12,
            ComputeOperator::BufType => 13,
            ComputeOperator::Buf => 16,
            ComputeOperator::BufferCollect => 14,
            ComputeOperator::Mma => 15,
        };
        w.u8(tag);
        Ok(())
    }

    fn read_operator(r: &mut Reader<'_>) -> Result<Self, String> {
        match r.u8()? {
            0 => Ok(ComputeOperator::Jit),
            1 => Ok(ComputeOperator::Launch),
            2 => Ok(ComputeOperator::Call),
            3 => Ok(ComputeOperator::Parallel),
            4 => Ok(ComputeOperator::ParLaunch),
            5 => Ok(ComputeOperator::Range),
            6 => Ok(ComputeOperator::Row),
            7 => Ok(ComputeOperator::Col),
            8 => Ok(ComputeOperator::Read),
            9 => Ok(ComputeOperator::Write),
            10 => Ok(ComputeOperator::WriteAtomic),
            11 => Ok(ComputeOperator::Graph),
            12 => Ok(ComputeOperator::GraphRun),
            13 => Ok(ComputeOperator::BufType),
            16 => Ok(ComputeOperator::Buf),
            14 => Ok(ComputeOperator::BufferCollect),
            15 => Ok(ComputeOperator::Mma),
            other => Err(format!("unknown compute-operator tag {other}")),
        }
    }
}

// --- OperatorExt::run (the VM dispatch for the injected operators) ---------

impl<P> OperatorExt<P> for ComputeOperator
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    /// The compute leaf's applicability policy.
    ///
    /// # Invariant
    /// A **kernel** apply is the one lowlevel `Apply` this vocabulary lowers, as
    /// a cross-kernel call, so it stays lazy instead of being refused: both halves
    /// of the answer are the JIT's own predicate — [`kernel_id_of`] and
    /// [`pending_kernel`]. A static node is never a kernel.
    fn is_callable(module: &Module<P>, callee: AnyNodeId) -> bool {
        match callee {
            AnyNodeId::Dynamic(node) => {
                kernel_id_of(module, node).is_some() || pending_kernel(module, node)
            }
            AnyNodeId::Static(_) => false,
        }
    }

    /// A compute operand is gated on its operands' values, not on their verdict
    /// (`docs/notes/compound-element-slots.md`).
    fn run_deferred(
        &self,
        operand: Option<NodeId>,
        block: BlockId,
        module: &mut Module<P>,
    ) -> Option<P::Value> {
        {
            let Some(node) = operand else {
                return self.run(P::Value::from(LowValue::Error), block, module);
            };
            let value = module.evaluate_node_deep(node, Some(block))?;
            // SAFETY: `node` is the operand the VM just evaluated, and no block is dropped
            // in this walk.
            for item in unsafe { module.array_items(node) }.unwrap_or_default() {
                let element = module.element_value_node(item.node).unwrap_or(item.node);
                if module.node_value(element).is_none() {
                    return None;
                }
            }
            return self.run(value, block, module);
        }
        let value = match operand {
            Some(node) => {
                let value = module.evaluate_node_deep(node, Some(block));
                if module.node_is_undecided(node) {
                    return None;
                }
                value?
            }
            None => P::Value::from(LowValue::Error),
        };
        self.run(value, block, module)
    }

    fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> Option<P::Value> {
        // An operator that cannot decide yet answers `None`, the trait's own
        // spelling of "undecided".
        (|| {
            // A recorded body is a sequence of dispatches; anything else is
            // stopped here rather than in each arm.

            // The reason is computed on a borrow, and recorded after it ends.
            if graph::is_recording()
                && let Some(reason) = unrecordable(self, module, &operand)
            {
                module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                    GRAPH_DIAGNOSTIC,
                    None,
                    reason,
                ));
                return None;
            }
            match self {
                ComputeOperator::Jit => {
                    // The operand is `[function]`, like every other kernel operator's:
                    // the argument node is the anchor a claim is read from.
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        // A non-function jit target is a reported type error; stay
                        // lazy rather than panicking.
                        return None;
                    };
                    // SAFETY: `operands` is the operand array the VM just evaluated for
                    // this operation; its home block is alive for the run.
                    let operands = unsafe { operands.items() };
                    let Some(LowValue::Function(function)) = operands
                        .first()
                        .and_then(|item| module.node_value(item.node))
                        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
                    else {
                        return None;
                    };
                    match compile_fragment(module, function) {
                        Ok(fragment) => {
                            // The claim is read **here**, where the kernel is compiled,
                            // and stored beside the fragment: the value carries none.
                            let claim = compile_claim(module, function);
                            // Content-addressed like `jit`'s: the backend is not in
                            // the fragment, so both backends share one id.
                            let id = intern_kernel(fragment, claim);
                            Some(<P::Value as From<ComputeValue>>::from(
                                ComputeValue::Kernel(id),
                            ))
                        }
                        Err(err) => {
                            // Outside the kernel-safe subset, or an undecided
                            // domain: a lazy value plus the reason.
                            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                JIT_DIAGNOSTIC,
                                None,
                                err,
                            ));
                            None
                        }
                    }
                }
                ComputeOperator::Launch => {
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        unreachable!("Launch expects an operand array of [kernel, arg]")
                    };
                    // SAFETY: `operands` is the array the VM just evaluated for
                    // this operation; its home block is alive for the run.
                    let operands = unsafe { operands.items() };
                    let Some(ComputeValue::Kernel(id)) = module
                        .node_value(operands[0].node)
                        .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                    else {
                        return None;
                    };
                    // An undecided argument defers the launch
                    // (`docs/notes/compound-element-slots.md`).
                    if !argument_decided(module, operands[1].node) {
                        return None;
                    }
                    // A scalar for an arity-1 kernel, an `Array` for a tuple-domain
                    // one; anything else stays lazy, and records why.
                    let args = match kernel_arguments(module, operands[1].node) {
                        Ok(args) => args,
                        Err(reason) => {
                            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                KERNEL_LAUNCH_DIAGNOSTIC,
                                None,
                                reason,
                            ));
                            return None;
                        }
                    };
                    match run_kernel(id, &args) {
                        Ok(results) => kernel_results_value(module, block, results),
                        Err(err) => {
                            // Whatever the wasm run said is recorded as it stands,
                            // not replaced by a summary that names none of them.
                            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                KERNEL_LAUNCH_DIAGNOSTIC,
                                None,
                                err,
                            ));
                            None
                        }
                    }
                }
                ComputeOperator::Call => {
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        unreachable!("Call expects an operand array of [kernel, arg]")
                    };
                    // SAFETY: `operands` is the array the VM just evaluated for
                    // this operation; its home block is alive for the run.
                    let operands = unsafe { operands.items() };
                    let Some(ComputeValue::Kernel(id)) = module
                        .node_value(operands[0].node)
                        .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                    else {
                        return None;
                    };
                    // An undecided argument defers the call, as it defers a launch
                    // (`docs/notes/compound-element-slots.md`).
                    if !argument_decided(module, operands[1].node) {
                        return None;
                    }
                    let args = match kernel_arguments(module, operands[1].node) {
                        Ok(collected) => collected,
                        Err(reason) => {
                            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                KERNEL_LAUNCH_DIAGNOSTIC,
                                None,
                                reason,
                            ));
                            return None;
                        }
                    };
                    match run_kernel(id, &args) {
                        Ok(results) => kernel_results_value(module, block, results),
                        Err(err) => {
                            // As in `Launch`: the run's own message is this
                            // refusal's cause, so it is recorded as it stands.
                            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                KERNEL_LAUNCH_DIAGNOSTIC,
                                None,
                                err,
                            ));
                            None
                        }
                    }
                }
                ComputeOperator::Parallel => {
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        unreachable!("Parallel expects an operand array of [function, backend]")
                    };
                    // SAFETY: `operands` is the operand array the VM just evaluated
                    // for this operation; its home block is alive for the run.
                    let operands = unsafe { operands.items() };
                    let Some(backend) = backend_argument(
                        module,
                        operands.get(1).and_then(|o| dyn_node(o.node).ok()),
                        PARALLEL_DIAGNOSTIC,
                    ) else {
                        return None;
                    };
                    let Some(LowValue::Function(function)) = module
                        .node_value(operands[0].node)
                        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                    else {
                        // A non-function parallel target is the checker's
                        // function-ness gate; stay lazy rather than panicking.
                        return None;
                    };
                    match compile_parallel_fragment(module, function) {
                        Ok(fragment) => {
                            // The claim is read **here**, where the kernel is compiled,
                            // and stored beside the fragment: the value carries none.
                            let claim = compile_claim(module, function);
                            // Content-addressed like `jit`'s: the backend is not in
                            // the fragment, so both backends share one id.
                            let id = intern_kernel(fragment, claim);
                            Some(<P::Value as From<ComputeValue>>::from(
                                ComputeValue::ParKernel(id, backend),
                            ))
                        }
                        Err(err) => {
                            // A polymorphic template is not jittable (the superior's
                            // verdict): refuse, never defer.
                            match err {
                                RoleUnread::Undecided => {
                                    module.record_extension_diagnostic(
                                        ExtensionDiagnostic::failure(
                                            PARALLEL_DIAGNOSTIC,
                                            None,
                                            POLYMORPHIC_TEMPLATE.to_string(),
                                        ),
                                    );
                                    return None;
                                }
                                RoleUnread::Refusal(why) => {
                                    module.record_extension_diagnostic(
                                        ExtensionDiagnostic::failure(
                                            PARALLEL_DIAGNOSTIC,
                                            None,
                                            why,
                                        ),
                                    );
                                    None
                                }
                            }
                        }
                    }
                }
                ComputeOperator::ParLaunch => {
                    // A recording intercepts first: a recorded dispatch's arguments
                    // are placeholders, with no buffers or count to read.
                    if graph::is_recording() {
                        return record_launch::<P>(module, block, operand);
                    }
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        unreachable!("ParLaunch expects an operand array of [kernel, cfg]")
                    };
                    // SAFETY: the VM just evaluated `operands`; the arm's `items()`
                    // calls are live nodes of `module`.
                    let operands = unsafe { operands.items() };
                    // The declared codomain, when the caller states one: the result's
                    // layout is the declaration's own.
                    let declared = operands.get(2).map(|item| item.node);
                    let Some(ComputeValue::ParKernel(id, backend)) = module
                        .node_value(operands[0].node)
                        .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                    else {
                        return None;
                    };
                    // The cfg value the kernel is handed: the named parameter
                    // struct, or the retired tuple `(n, (buffer…))`.
                    let Ok(cfg_node) = dyn_node(operands[1].node) else {
                        return None;
                    };
                    // SAFETY: as above — `cfg_node` names a live node of `module`.
                    let Some(cfg_items) = (unsafe { module.array_items(cfg_node) }) else {
                        return None;
                    };
                    // The cfg: the parameter's scalar leaves in field order, then the
                    // input buffers. The fragment says the leaf classes.
                    let leaf_classes = match parallel_leaf_classes(id) {
                        Ok(classes) => classes,
                        Err(err) => {
                            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                PARALLEL_DIAGNOSTIC,
                                None,
                                err,
                            ));
                            return None;
                        }
                    };
                    let facts = fragment_facts(id);
                    let mut leaves: Vec<i64> = Vec::with_capacity(leaf_classes.len());
                    for (position, class) in leaf_classes.iter().enumerate() {
                        // The named form reads a leaf where the walk found it; the
                        // tuple form states no roles and reads the leading positions.
                        let node = match facts.roles.scalars.get(position) {
                            Some(path) => value_at_path::<P>(module, cfg_node, path),
                            None => cfg_items.get(position).map(|item| item.node),
                        };
                        let value = node
                            .and_then(|node| module.node_value(node))
                            .and_then(|v| AsEnum::<LowValue>::as_enum(&v));
                        // A decided wrong-class leaf is refused by name; an undecided
                        // one stays lazy.
                        let word = match (class, value) {
                            (ScalarClass::Int, Some(LowValue::USize(n))) => n as i64,
                            (ScalarClass::Float, Some(LowValue::Float(x))) => float_bits(x),
                            (ScalarClass::Int, Some(LowValue::Float(_))) => {
                                module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                    PARALLEL_DIAGNOSTIC,
                                    None,
                                    if position == 0 {
                                        "the launch count is Float, but a dispatch extent is Int: \
                                     Int and Float do not convert"
                                    } else {
                                        "a parallel parameter's scalar leaf is Int here and the \
                                     launch passes a Float for it: Int and Float do not convert"
                                    },
                                ));
                                return None;
                            }
                            (ScalarClass::Float, Some(LowValue::USize(_))) => {
                                module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                    PARALLEL_DIAGNOSTIC,
                                    None,
                                    "a parallel parameter's scalar leaf is Float here and the \
                                 launch passes an Int for it: Int and Float do not convert",
                                ));
                                return None;
                            }
                            _ => {
                                return None;
                            }
                        };
                        leaves.push(word);
                    }
                    // Input buffers: the named form reads each where the walk found
                    // it; the tuple form reads the position after the leaves.
                    let named_form = !facts.roles.scalars.is_empty()
                        || !facts.roles.inputs.is_empty()
                        || !facts.roles.outputs.is_empty();
                    let mut inputs: Vec<RunInput> = Vec::new();
                    if named_form {
                        for (position, path) in facts.roles.inputs.iter().enumerate() {
                            let Some(node) = value_at_path::<P>(module, cfg_node, path)
                                .and_then(|buf| buf_payload::<P>(module, buf))
                            else {
                                return None;
                            };
                            let Some(input) = run_input::<P>(
                                module,
                                node,
                                "a parallel parameter's input buffer is one of the kernel's own \
                                 `Buf` fields",
                                &format!("position {position} of it"),
                            ) else {
                                return None;
                            };
                            inputs.push(input);
                        }
                    } else if let Some(buf_tuple) = cfg_items.get(leaf_classes.len())
                    && let Ok(buf_tuple_node) = dyn_node(buf_tuple.node)
                    // SAFETY: `buf_tuple_node` names a live node of `module`.
                    && let Some(buf_items) = (unsafe { module.array_items(buf_tuple_node) })
                    {
                        for (position, item) in buf_items.iter().enumerate() {
                            let Some(input) = run_input::<P>(
                                module,
                                item.node,
                                "a parallel launch's `cfg(1)` is the tuple of buffers its \
                                 kernel reads",
                                &format!("position {position} of it"),
                            ) else {
                                return None;
                            };
                            inputs.push(input);
                        }
                    }
                    match run_parallel_kernel(id, backend, leaves, inputs) {
                        Ok(RunOutcome::Host(results)) => {
                            // The named form's result is the kernel's codomain: every
                            // output wrapped as the `Buf` the type level reads.
                            if named_form {
                                let outputs: Vec<(NodeId, ScalarClass)> = results
                                    .iter()
                                    .map(|result| {
                                        let payload = module.add_node(
                                            block,
                                            None,
                                            Some(<P::Value as From<ComputeValue>>::from(
                                                ComputeValue::Buffer(
                                                    module.alloc_payload(&result.packed(), block),
                                                    result.class,
                                                ),
                                            )),
                                        );
                                        (payload, result.class)
                                    })
                                    .collect();
                                return build_outputs::<P>(
                                    module,
                                    block,
                                    &facts.roles,
                                    &outputs,
                                    declared,
                                );
                            }
                            // Several outputs are the tuple of them, each buffer a live
                            // node of this block rather than a detached value.

                            // Each buffer carries the class its producer declared for
                            // the ordinal.
                            if results.len() != 1 {
                                let items: Vec<ArrayItem> = results
                                    .iter()
                                    .map(|result| {
                                        let node = module.add_node(
                                            block,
                                            None,
                                            Some(<P::Value as From<ComputeValue>>::from(
                                                ComputeValue::Buffer(
                                                    module.alloc_payload(&result.packed(), block),
                                                    result.class,
                                                ),
                                            )),
                                        );
                                        ArrayItem::new(AnyNodeId::Dynamic(node))
                                    })
                                    .collect();
                                let handle = module.alloc_array(&items, block);
                                return Some(<P::Value as From<LowValue>>::from(LowValue::Array(
                                    handle.into(),
                                )));
                            }
                            // A single output is a bare `Buffer`; its payload lands in
                            // the arena, so the block owns it (`D15`).
                            Some(<P::Value as From<ComputeValue>>::from(
                                ComputeValue::Buffer(
                                    module.alloc_payload(&results[0].packed(), block),
                                    results[0].class,
                                ),
                            ))
                        }
                        Ok(RunOutcome::Resident(results)) => {
                            // The same shape, results left on the device: a resident
                            // buffer is plain data, so the copy path leaves it alone.
                            let value = |resident: ResidentBuffer| {
                                <P::Value as From<ComputeValue>>::from(ComputeValue::DeviceBuffer(
                                    resident,
                                ))
                            };
                            // The named form's result is the codomain here too; a
                            // resident buffer carries no class of its own.
                            if named_form {
                                let outputs: Vec<(NodeId, ScalarClass)> = results
                                    .iter()
                                    .enumerate()
                                    .map(|(position, resident)| {
                                        let class = facts
                                            .output_classes
                                            .get(position)
                                            .copied()
                                            .unwrap_or(ScalarClass::Int);
                                        let payload =
                                            module.add_node(block, None, Some(value(*resident)));
                                        (payload, class)
                                    })
                                    .collect();
                                return build_outputs::<P>(
                                    module,
                                    block,
                                    &facts.roles,
                                    &outputs,
                                    declared,
                                );
                            }
                            if results.len() != 1 {
                                let items: Vec<ArrayItem> = results
                                    .iter()
                                    .map(|resident| {
                                        let node =
                                            module.add_node(block, None, Some(value(*resident)));
                                        ArrayItem::new(AnyNodeId::Dynamic(node))
                                    })
                                    .collect();
                                let handle = module.alloc_array(&items, block);
                                return Some(<P::Value as From<LowValue>>::from(LowValue::Array(
                                    handle.into(),
                                )));
                            }
                            Some(value(results[0]))
                        }
                        Err(err) => {
                            // The refusal is recorded: `BudgetExhausted`'s renderings
                            // would say "never terminates", which is false here.
                            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                PARALLEL_DIAGNOSTIC,
                                None,
                                err,
                            ));
                            None
                        }
                    }
                }
                ComputeOperator::Read => {
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        unreachable!("Read expects an operand array of [buffer, index]")
                    };
                    // SAFETY: `operands` is the array the VM just evaluated for
                    // this operation; its home block is alive for the run.
                    let operands = unsafe { operands.items() };
                    let index = match module
                        .node_value(operands[1].node)
                        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                    {
                        Some(LowValue::USize(n)) => n,
                        _ => return None,
                    };
                    // The operand is a `Buf` wrapper behind the element's slot
                    // (`docs/notes/compound-element-slots.md`).
                    let element = module
                        .element_value_node(operands[0].node)
                        .unwrap_or(operands[0].node);
                    let buffer = buf_payload::<P>(module, element).unwrap_or(element);
                    let Some(held) = decided_buffer_value(module, buffer) else {
                        return None;
                    };
                    if matches!(held.as_enum(), Some(LowValue::Error)) {
                        return Some(held);
                    }
                    let element = match AsEnum::<ComputeValue>::as_enum(&held) {
                        Some(ComputeValue::Buffer(payload, class)) => {
                            // SAFETY: the buffer value was just read out of `module`, so its
                            // payload's home block is alive for this read.
                            match buffer_items(&payload)
                                .and_then(|items| element_bytes(class, items, index))
                            {
                                // The element becomes the value its class says: the
                                // buffer's class says how many bytes it occupies.
                                Some(bytes) => match element_value(class, bytes) {
                                    Some(element) => <P::Value as From<LowValue>>::from(element),
                                    None => {
                                        return None;
                                    }
                                },
                                None => {
                                    return None;
                                }
                            }
                        }
                        Some(ComputeValue::DeviceBuffer(resident)) => {
                            // A read is where a resident buffer stops being on the
                            // device: the program has asked for host data.
                            if index >= resident.count {
                                return None;
                            }
                            // The fetch returns everything up to the index: the trait
                            // hands over owned data, so a late read transfers the
                            // buffer.
                            match fetch_resident(
                                ResidentBuffer {
                                    id: resident.id,
                                    count: index + 1,
                                    class: resident.class,
                                },
                                0,
                            ) {
                                // The element becomes the value its class says: an
                                // integer stays a scalar, a float becomes `Float`.
                                Ok(ScalarData::Int(elements)) => {
                                    <P::Value as From<LowValue>>::from(LowValue::USize(
                                        elements[index] as usize,
                                    ))
                                }
                                Ok(ScalarData::Float(elements)) => {
                                    <P::Value as From<LowValue>>::from(LowValue::Float(
                                        elements[index],
                                    ))
                                }
                                Err(err) => {
                                    // A failed fetch is a refusal like any other:
                                    // it is recorded and the read stays undecided.
                                    module.record_extension_diagnostic(
                                        ExtensionDiagnostic::failure(
                                            PARALLEL_DIAGNOSTIC,
                                            None,
                                            err,
                                        ),
                                    );
                                    return None;
                                }
                            }
                        }
                        _ => {
                            return not_a_buffer::<P>(
                                module,
                                operands[0].node,
                                "a `compute.read` reads one element of one buffer",
                                "its buffer position",
                            );
                        }
                    };
                    Some(element)
                }
                ComputeOperator::Range
                | ComputeOperator::Row
                | ComputeOperator::Col
                | ComputeOperator::Write
                | ComputeOperator::WriteAtomic
                | ComputeOperator::Buf
                | ComputeOperator::BufType => {
                    // Kernel-only operators stay lazy; a slice has no host value.
                    None
                }
                ComputeOperator::BufferCollect => {
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        unreachable!("BufferCollect expects an operand array of [buffer]")
                    };
                    // SAFETY: `operands` is the array the VM just evaluated for
                    // this operation; its home block is alive for the run.
                    let operands = unsafe { operands.items() };
                    // `collect` says "give me these as host values" — where a `"gpu"`
                    // chain's results cross the bus. The element is a slot.
                    let element = module
                        .element_value_node(operands[0].node)
                        .unwrap_or(operands[0].node);
                    let buffer = buf_payload::<P>(module, element).unwrap_or(element);
                    let Some(held) = decided_buffer_value(module, buffer) else {
                        return None;
                    };
                    if matches!(held.as_enum(), Some(LowValue::Error)) {
                        return Some(held);
                    }

                    let results: ScalarData = match AsEnum::<ComputeValue>::as_enum(&held) {
                        Some(ComputeValue::Buffer(payload, class)) => {
                            // SAFETY: the buffer value was just read out of `module`,
                            // so its payload's block is alive below.
                            let Some(items) = buffer_items(&payload) else {
                                return None;
                            };
                            // The buffer's own class says how each element is read:
                            // an integer value, or an `f32`'s bits.
                            match class {
                                ScalarClass::Int => ScalarData::Int(unpack_elements(class, items)),
                                // A bf16 rides the `f32` the half is the top of, which
                                // is the reading [`unpack_elements`] already lifts it to.
                                ScalarClass::Float | ScalarClass::BFloat16 => ScalarData::Float(
                                    unpack_elements(class, items)
                                        .into_iter()
                                        .map(|word| f32::from_bits(word as u32))
                                        .collect(),
                                ),
                            }
                        }
                        Some(ComputeValue::DeviceBuffer(resident)) => {
                            match fetch_resident(resident, 0) {
                                Ok(data) => data,
                                Err(err) => {
                                    module.record_extension_diagnostic(
                                        ExtensionDiagnostic::failure(
                                            PARALLEL_DIAGNOSTIC,
                                            None,
                                            err,
                                        ),
                                    );
                                    return None;
                                }
                            }
                        }
                        _ => {
                            return not_a_buffer::<P>(
                                module,
                                operands[0].node,
                                "a `compute.collect` reads every element of one buffer",
                                "its buffer position",
                            );
                        }
                    };
                    // Each element becomes a fresh scalar node in a real array value,
                    // of the buffer's own class.
                    let items: Vec<ArrayItem> = match results {
                        ScalarData::Int(values) => values
                            .into_iter()
                            .map(|value| {
                                scalar_item::<P>(module, block, LowValue::USize(value as usize))
                            })
                            .collect::<Option<Vec<_>>>()?,
                        ScalarData::Float(values) => values
                            .into_iter()
                            .map(|value| scalar_item::<P>(module, block, LowValue::Float(value)))
                            .collect::<Option<Vec<_>>>()?,
                    };
                    let handle = module.alloc_array(&items, block);
                    Some(<P::Value as From<LowValue>>::from(LowValue::Array(
                        handle.into(),
                    )))
                }
                ComputeOperator::Mma => {
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        unreachable!("Mma expects an operand array of [backend, a, b, c]")
                    };
                    // SAFETY: `operands` is the array the VM just evaluated for this
                    // operation; its home block is alive for the run.
                    let operands = unsafe { operands.items() };
                    let Some(backend) = backend_argument::<P>(
                        module,
                        operands.first().and_then(|item| match item.node {
                            AnyNodeId::Dynamic(node) => Some(node),
                            AnyNodeId::Static(_) => None,
                        }),
                        MMA_DIAGNOSTIC,
                    ) else {
                        return None;
                    };
                    let Some(a) = mma_words(
                        module,
                        operands.get(1).map(|item| item.node),
                        lichen_kernel_ir::COOPERATIVE_MATRIX_M
                            * lichen_kernel_ir::COOPERATIVE_MATRIX_K,
                        i8::MIN as i64,
                        i8::MAX as i64,
                    ) else {
                        return None;
                    };
                    let Some(b) = mma_words(
                        module,
                        operands.get(2).map(|item| item.node),
                        lichen_kernel_ir::COOPERATIVE_MATRIX_K
                            * lichen_kernel_ir::COOPERATIVE_MATRIX_N,
                        i8::MIN as i64,
                        i8::MAX as i64,
                    ) else {
                        return None;
                    };
                    let Some(c) = mma_words(
                        module,
                        operands.get(3).map(|item| item.node),
                        lichen_kernel_ir::COOPERATIVE_MATRIX_M
                            * lichen_kernel_ir::COOPERATIVE_MATRIX_N,
                        i32::MIN as i64,
                        i32::MAX as i64,
                    ) else {
                        return None;
                    };
                    let product = match backend {
                        Backend::Cpu => mma_product(&a, &b, &c),
                        Backend::Gpu => {
                            let Some(backend) = lichen_kernel_ir::parallel_backend() else {
                                module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                                    MMA_DIAGNOSTIC,
                                    None,
                                    "a \"gpu\" cooperative matrix product needs an installed compute backend, and this host installed none".to_string(),
                                ));
                                return None;
                            };
                            let bytes_a: Vec<i8> = a.iter().map(|word| *word as i8).collect();
                            let bytes_b: Vec<i8> = b.iter().map(|word| *word as i8).collect();
                            let words_c: Vec<i32> = c.iter().map(|word| *word as i32).collect();
                            match backend.cooperative_matrix(&bytes_a, &bytes_b, &words_c) {
                                Ok(product) => product.into_iter().map(i64::from).collect(),
                                Err(reason) => {
                                    module.record_extension_diagnostic(
                                        ExtensionDiagnostic::failure(MMA_DIAGNOSTIC, None, reason),
                                    );
                                    return None;
                                }
                            }
                        }
                    };
                    let payload =
                        module.alloc_payload(&pack_elements(ScalarClass::Int, &product), block);
                    let payload = AnyNodeId::Dynamic(module.add_node(
                        block,
                        None,
                        Some(<P::Value as From<ComputeValue>>::from(
                            ComputeValue::Buffer(payload, ScalarClass::Int),
                        )),
                    ));
                    let wrapper =
                        current_buf_value::<P>(module, block, payload, ScalarClass::Int, None)?;
                    module.node_value(AnyNodeId::Dynamic(wrapper))
                }
                ComputeOperator::Graph => {
                    // The operand is already evaluated here, so the function is reached
                    // through the deep value.

                    // A missing operand stays lazy: a deep pass can come back holding
                    // something else, and that is an answer.
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        return None;
                    };
                    // SAFETY: `operands` is the operand array the VM just evaluated
                    // for this operation; its home block is alive for the run.
                    let operands = unsafe { operands.items() };
                    let Some(function) = operands.first().map(|item| item.node) else {
                        return None;
                    };
                    build_graph::<P>(module, block, function)
                }
                ComputeOperator::GraphRun => {
                    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        return None;
                    };
                    // SAFETY: as above — a live node of `module`.
                    let operands = unsafe { operands.items() };
                    let Some(graph_node) = operands.first().map(|item| item.node) else {
                        return None;
                    };
                    let arguments = operands.get(1).map(|item| item.node);
                    run_graph::<P>(module, block, graph_node, arguments)
                }
            }
        })()
    }

    /// The compute operators' low-type transfer: what a kernel's operators produce.
    ///
    /// # Invariant
    /// Launch and Call are load-bearing: a cross-kernel call sits where no
    /// argument exists yet, so they state honest scalars. `Jit`, `Parallel`,
    /// `Write` and `BufferCollect` decline. The pass does not consult this table
    /// — [`OperatorExt::low_type`]'s default answers — and a read's class comes
    /// from the decided cell beside it ([`shape::prepare_template_low_types`]).
    fn low_type(&self, _arguments: &[Option<LowShape>]) -> Option<LowShape> {
        match self {
            ComputeOperator::Launch
            | ComputeOperator::Call
            | ComputeOperator::Range
            | ComputeOperator::Row
            | ComputeOperator::Col
            | ComputeOperator::Read => Some(LowShape::USize),
            ComputeOperator::Jit
            | ComputeOperator::Parallel
            | ComputeOperator::ParLaunch
            | ComputeOperator::Write
            | ComputeOperator::WriteAtomic
            // A buffer's element class comes from the array it was made of, which the
            // operator answers rather than this table.
            | ComputeOperator::Buf
            | ComputeOperator::BufType
            // `Graph` is host-owned and `GraphRun`'s length is the graph's own
            // return: both decline like `Jit`/`BufferCollect`.
            | ComputeOperator::Graph
            | ComputeOperator::GraphRun
            | ComputeOperator::BufferCollect
            | ComputeOperator::Mma => None,
        }
    }
}

// --- Codegen: lichen graph → a scalar `(i64) -> i64` wasm function body -----

/// Whether `value` is a **struct value**: a compound value that carries the
/// nominal id at its second slot.
///
/// # Invariant
/// The nominal id is what separates a struct value from a tuple's: the two are
/// both compound values, and the id is the one fact only a struct carries
/// (`docs/notes/value-templates.md`).
fn is_struct_value<P>(module: &Module<P>, value: AnyNodeId) -> bool
where
    P: Program,
    P::Value: ValueType,
{
    let Some(items) = shape::value_items(module, value) else {
        return false;
    };
    let Some(id) = items.get(shape::STRUCT_VALUE_ID_SLOT) else {
        return false;
    };
    module
        .node_value(id.node)
        .is_some_and(|value| value.type_id().is_some())
}

/// The layout a compound value carries.
///
/// # Invariant
/// The layout is the value's **type**: the shared `Tuple`, `Array` and `Struct`
/// templates name it, and the value's own offsets follow it
/// (`docs/notes/value-templates.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CompoundLayout {
    /// A tuple value: the elements are the value's own items.
    Tuple,
    /// An array value: the element template holds slot `0`.
    Array,
    /// A struct value: the name table and the nominal id hold the two header
    /// slots.
    Struct,
}

impl CompoundLayout {
    /// The slot the value's own elements start at.
    const fn element_base(self) -> usize {
        match self {
            CompoundLayout::Tuple => shape::TUPLE_VALUE_ELEMENT_BASE,
            CompoundLayout::Array => shape::ARRAY_VALUE_ELEMENT_BASE,
            CompoundLayout::Struct => shape::STRUCT_VALUE_FIELD_BASE,
        }
    }
}

/// The layout the type at `ty` names, read from its **marker**.
///
/// # Invariant
/// A compound's type is the shared template, and the template's value slot holds
/// the marker that names the layout (`docs/notes/value-templates.md`).
fn compound_layout<P>(module: &Module<P>, ty: AnyNodeId) -> Option<CompoundLayout>
where
    P: Program,
    P::Value: ValueType,
{
    let marker = module.node_value(shape::value_item(module, ty, shape::EXPR_VALUE_SLOT)?)?;
    if marker == P::Value::tuple_type_marker() {
        Some(CompoundLayout::Tuple)
    } else if marker == P::Value::array_type_marker() {
        Some(CompoundLayout::Array)
    } else if marker == P::Value::type_struct_marker() {
        Some(CompoundLayout::Struct)
    } else {
        None
    }
}

/// The slot a compound value's own elements start at.
///
/// # Invariant
/// The layout is the value's **type** where the type names one, so an array
/// value's element template is skipped. A value whose type names no layout is
/// read by its own shape: a struct value's header holds the two slots before
/// the first field, and every other compound value's elements are its own items
/// (`docs/notes/value-templates.md`).
fn element_base_of<P>(module: &Module<P>, value: AnyNodeId, ty: Option<AnyNodeId>) -> usize
where
    P: Program,
    P::Value: ValueType,
{
    if let Some(layout) = ty.and_then(|ty| compound_layout::<P>(module, ty)) {
        return layout.element_base();
    }
    if is_struct_value(module, value) {
        shape::STRUCT_VALUE_FIELD_BASE
    } else {
        shape::TUPLE_VALUE_ELEMENT_BASE
    }
}

/// The **elements** of a compound value: its items, past the layout's own slots.
///
/// # Invariant
/// A struct value's name table and nominal id and an array value's element
/// template are the value's own bookkeeping, not elements, so they are skipped;
/// a tuple value's elements are its own items
/// (`docs/notes/value-templates.md`).
fn value_element_items<P>(
    module: &Module<P>,
    value: AnyNodeId,
    ty: Option<AnyNodeId>,
) -> Option<&'static [ArrayItem]>
where
    P: Program,
    P::Value: ValueType,
{
    let items = shape::value_items(module, value)?;
    items.get(element_base_of::<P>(module, value, ty)..)
}

/// The **operands** a compound argument contributes: each call, or value.
///
/// # Invariant
/// A field's value half is the value the field holds, and a call's **compound**
/// result arrives as `Index(call, 0)` — a read of the call's result, not the
/// call. The kernel must lower the **call**: it is the declaration a buffer
/// names and the computation an index is
/// (`docs/notes/value-templates.md`).
pub(super) fn operand_nodes_of<P>(
    module: &Module<P>,
    value: NodeId,
    ty: Option<AnyNodeId>,
) -> Result<Vec<NodeId>, String>
where
    P: Program,
    P::Value: ValueType,
{
    // SAFETY: `value` is a live node of `module`.
    let Some(items) = (unsafe { module.array_items(value) }) else {
        return Ok(vec![value]);
    };
    let base = element_base_of::<P>(module, AnyNodeId::Dynamic(value), ty);
    items
        .iter()
        .skip(base)
        .map(|item| {
            let node = item.node;
            if let AnyNodeId::Dynamic(inner) = node
                && let Some(operation) = module.node_operation(inner)
                && matches!(
                    AsEnum::<LowOperator>::as_enum(&operation.operator),
                    Some(LowOperator::Apply)
                )
            {
                return Ok(inner);
            }
            match node {
                AnyNodeId::Dynamic(node) => module
                    .element_value_node(AnyNodeId::Dynamic(node))
                    .and_then(|node| node.dynamic())
                    .ok_or_else(|| {
                        "a body reads a static reference into a frozen module, which has no node in \
                         this graph"
                            .to_string()
                    }),
                AnyNodeId::Static(_) => Err(
                    "a body reads a static reference into a frozen module, which has no node in this \
                     graph"
                        .to_string(),
                ),
            }
        })
        .collect()
}

/// A value's leaves: itself, or a compound's elements.
///
/// # Invariant
/// A compound's leaves are its **elements**: the layout's own slots — a struct
/// value's header, an array value's template — are the value's bookkeeping, not
/// leaves. Every other element is read through its own value position, so a
/// leaf is the value a slot holds.
pub(super) fn value_leaves_of<P>(
    module: &Module<P>,
    value: NodeId,
    ty: Option<AnyNodeId>,
) -> Result<Vec<NodeId>, String>
where
    P: Program,
    P::Value: ValueType,
{
    // SAFETY: `value` is a live node of `module`.
    let Some(items) = (unsafe { module.array_items(value) }) else {
        return Ok(vec![value]);
    };
    let base = element_base_of::<P>(module, AnyNodeId::Dynamic(value), ty);
    items
        .iter()
        .skip(base)
        .map(|item| match item.node {
            AnyNodeId::Dynamic(node) => module
                .element_value_node(AnyNodeId::Dynamic(node))
                .and_then(|node| node.dynamic())
                .ok_or_else(|| {
                    "a body reads a static reference into a frozen module, which has no node in \
                     this graph"
                        .to_string()
                }),
            AnyNodeId::Static(_) => Err(
                "a body reads a static reference into a frozen module, which has no node in this \
                 graph"
                    .to_string(),
            ),
        })
        .collect()
}

pub(super) fn domain_value_leaves_of<P>(
    module: &Module<P>,
    value: NodeId,
    ty: Option<AnyNodeId>,
    path: &mut Vec<NodeId>,
    leaves: &mut Vec<NodeId>,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    if path.contains(&value) {
        return Err("a parameter domain contains a cyclic tuple".into());
    }
    let elements = value_leaves_of(module, value, ty)?;
    path.push(value);
    for element in elements {
        if is_tuple_value(module, element) {
            domain_value_leaves_of(
                module,
                element,
                shape::expr_type(module, element),
                path,
                leaves,
            )?;
        } else {
            leaves.push(element);
        }
    }
    path.pop();
    Ok(())
}

/// The name table of a struct **value**: the field names and their positions.
///
/// # Invariant
/// The table is the value's first slot, and a name's stored position is its
/// **relative** field position: the fields themselves start after the two header
/// slots, so a name's position is where its field is
/// (`docs/notes/value-templates.md`).
fn struct_name_table<P>(module: &Module<P>, value: AnyNodeId) -> Option<AnyHandle<[TableItem]>>
where
    P: Program,
    P::Value: ValueType,
{
    let names = shape::value_item(module, value, shape::STRUCT_VALUE_NAMES_SLOT)?;
    let held = module.node_value(names)?.as_enum()?.clone();
    match held {
        LowValue::Table(table) => Some(table),
        _ => None,
    }
}

/// The **field names** of a struct **value**, in field order.
///
/// # Invariant
/// The names come from the value's own table and the count from the value's own
/// length, so a name whose position is outside the field list is dropped rather
/// than growing it.
fn struct_field_names<P>(module: &Module<P>, value: AnyNodeId) -> Option<Vec<Option<&'static str>>>
where
    P: Program,
    P::Value: ValueType,
{
    let table = struct_name_table(module, value)?;
    let count = shape::value_len(module, value)?.checked_sub(shape::STRUCT_VALUE_FIELD_BASE)?;
    let mut names = vec![None; count];
    // SAFETY: the table is the payload of a value read from a live node.
    for item in unsafe { table.items() } {
        if let Some((name, index)) = module.name_table_entry(item)
            && index < count
        {
            names[index] = Some(name);
        }
    }
    Some(names)
}

/// The **fields** of a struct **value**: each field's own expression, in field
/// order.
///
/// # Invariant
/// A field is a complete expression — its value, its type and its attributes are
/// its own — so a reader of a field reads the field and not a cell beneath it.
fn struct_field_exprs<P>(module: &Module<P>, value: AnyNodeId) -> Option<Vec<AnyNodeId>>
where
    P: Program,
    P::Value: ValueType,
{
    let items = shape::value_items(module, value)?;
    Some(
        items
            .iter()
            .skip(shape::STRUCT_VALUE_FIELD_BASE)
            .map(|item| item.node)
            .collect(),
    )
}

/// A field expression's **value**: the cell the expression holds.
fn struct_field_value_of_expr<P>(module: &Module<P>, field: AnyNodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: ValueType,
{
    shape::value_item(module, field, shape::EXPR_VALUE_SLOT)
}

/// A field expression's **type**: the type the expression states.
fn struct_field_type_of_expr<P>(module: &Module<P>, field: AnyNodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: ValueType,
{
    shape::value_item(module, field, shape::EXPR_TYPE_SLOT)
}

/// A field's **value**: the cell the field's own expression holds.
///
/// # Invariant
/// The field is the one at the layout's own base, so a struct value's field is
/// past its header and a tuple value's element is its own item.
fn struct_field_value<P>(module: &Module<P>, value: AnyNodeId, index: usize) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: ValueType,
{
    let base = element_base_of::<P>(module, value, None);
    let field = shape::value_item(module, value, base + index)?;
    struct_field_value_of_expr(module, field)
}

/// The **type** the field at `path` states.
///
/// # Invariant
/// The walk follows the field's **value** — the struct value a field holds — and
/// reads the next level off the field's own expression, so a nested group is
/// walked through the value that carries it. The class a leaf has is the one its
/// **type slot** states (`1.0` states `Float`), so the walk ends at that slot.
fn struct_field_type_at_path<P>(
    module: &Module<P>,
    value: AnyNodeId,
    path: &[usize],
) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: ValueType,
{
    let mut current = value;
    let mut field = None;
    for &at in path {
        let expr = shape::value_item(module, current, shape::STRUCT_VALUE_FIELD_BASE + at)?;
        field = Some(expr);
        current = struct_field_value_of_expr(module, expr)?;
    }
    let expr = field?;
    struct_field_type_of_expr(module, expr)
}

/// The class of every scalar leaf of a parallel parameter, in signature order.
///
/// # Invariant
/// The first leaf is the launch extent, read as an `Int` ordinal whatever the
/// parameter declares; a declared class that is not is refused here, where the
/// message can name the field. Every other leaf takes the parameter field's own
/// class — a field whose type is undecided keeps the integer default.
fn scalar_leaf_classes<P>(
    module: &mut Module<P>,
    cfg_expr: NodeId,
    roles: &KernelRoles,
) -> Result<Vec<ScalarClass>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let value = expr_value(module, cfg_expr).map(AnyNodeId::Dynamic);
    // A scalar role is a leaf at its own path: `[field]`, or `[in, g, s]`.
    let class_of_role = |module: &Module<P>, path: &[usize]| -> ScalarClass {
        value
            .and_then(|value| struct_field_type_at_path(module, value, path))
            .and_then(|ty| low_type_of_slot(module, ty))
            .map(|shape| scalar_class_of(&shape))
            .unwrap_or(ScalarClass::Int)
    };
    let classes: Vec<ScalarClass> = roles
        .scalars
        .iter()
        .map(|path| class_of_role(module, path))
        .collect();
    if let Some(first) = classes.first()
        && *first != ScalarClass::Int
    {
        return Err(format!(
            "a parallel parameter's first scalar is the launch extent, and an extent is Int: \
             this one is declared {first:?}. The extent is how many indices the dispatch runs \
             over, so it is a host ordinal in every fragment, and a `Float` one is not a count \
             (`docs/notes/floating-point.md` §4.4)"
        ));
    }
    Ok(classes)
}

/// Why a parallel parameter's roles could not be read.
///
/// # Invariant
/// [`Self::Undecided`] is produced only where the annotation is still a cell with
/// no value: there is nothing to read and nothing to report. [`Self::Refusal`]
/// always carries the sentence its subject was named in. A caller tells them apart
/// by the variant, never by comparing text, so no later edit to a message can turn
/// a refusal into a "not yet".
pub enum RoleUnread {
    /// The parameter's type has not resolved yet: a later pass compiles the kernel
    /// once the annotation states the struct.
    Undecided,
    /// A named refusal: the parameter is not a kernel ABI, or a class it declares
    /// is not decidable, and this is why.
    Refusal(String),
}

impl From<String> for RoleUnread {
    fn from(why: String) -> Self {
        Self::Refusal(why)
    }
}

impl From<&str> for RoleUnread {
    fn from(why: &str) -> Self {
        Self::Refusal(why.to_string())
    }
}

/// The role table of a parallel kernel's parameter struct, decoded from the
/// parameter's **value**.
///
/// # Invariant
/// A struct's layout lives in its value, so the parameter's own value carries
/// the fields: the names from its table, the fields from its own expressions.
/// `Undecided` when the value is not a struct value yet (the retired tuple
/// `cfg`, or an unresolved template match); `Refusal` when it is one and names
/// a field the ABI cannot hold, so a caller cannot mistake a mistake for a
/// "not yet".
fn parallel_roles<P>(module: &mut Module<P>, cfg_expr: NodeId) -> Result<KernelRoles, RoleUnread>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(value) = expr_value(module, cfg_expr).map(AnyNodeId::Dynamic) else {
        return Err(RoleUnread::Undecided);
    };
    // Undecided is not a mistake: a kernel compiled before its annotation
    // resolved waits for a later pass.
    roles_from_value(module, value, RoleSource::Parameter)
}

/// The codomain's roles: its buffer fields, all of them outputs, in field order.
///
/// # Invariant
/// A parameter no longer carries `.out` (`docs/notes/kernel-buffer-declarations.md`), so the
/// return value is where an output's element class and order are stated: the fragment's
/// `roles.outputs` is this walk, and the body's declarations are the positions it fills.
fn codomain_roles<P>(module: &mut Module<P>, return_node: NodeId) -> Result<KernelRoles, RoleUnread>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(value) = expr_value(module, return_node).map(AnyNodeId::Dynamic) else {
        return Err(RoleUnread::Undecided);
    };
    roles_from_value(module, value, RoleSource::Codomain)
}

/// Where a value's leaves take their role from.
#[derive(Clone, Copy, PartialEq)]
enum RoleSource {
    /// The parameter: `.in` is input, `.out` output, anything else a runtime scalar.
    Parameter,
    /// The codomain: every buffer field is an output.
    Codomain,
}

/// Walk a struct **value**'s fields into [`KernelRoles`] under `source`'s rule.
///
/// # Invariant
/// The names and the fields come from the value itself: a template match fills
/// the parameter's own value, so the roles are read off the layout the parameter
/// actually carries.
fn roles_from_value<P>(
    module: &mut Module<P>,
    value: AnyNodeId,
    source: RoleSource,
) -> Result<KernelRoles, RoleUnread>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // A value that is not a struct value states no roles at all.
    if !is_struct_value(module, value) {
        return Err(RoleUnread::Undecided);
    }
    // An anonymous struct has no table: every field is unnamed.
    let names = struct_field_names(module, value).unwrap_or_default();
    let named = |wanted: &str| names.iter().position(|name| *name == Some(wanted));
    // A reserved group may be absent or written empty: the walk treats both
    // alike, so neither needs a filler field.
    let inputs_at = match source {
        RoleSource::Parameter => named("in"),
        RoleSource::Codomain => None,
    };
    let outputs_at = match source {
        RoleSource::Parameter => named("out"),
        RoleSource::Codomain => None,
    };
    let Some(fields) = struct_field_exprs(module, value) else {
        return Err(RoleUnread::Undecided);
    };
    let mut roles = KernelRoles::default();
    for (field, &field_expr) in fields.iter().enumerate() {
        let role = match (source, field) {
            (RoleSource::Codomain, _) => LeafRole::Output,
            (_, at) if inputs_at == Some(at) => LeafRole::Input,
            (_, at) if outputs_at == Some(at) => LeafRole::Output,
            _ => LeafRole::Scalar,
        };
        walk_role(module, field_expr, &[field], role, &mut roles)?;
    }
    Ok(roles)
}

/// The role a field of a parallel kernel's parameter plays.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeafRole {
    /// A leaf outside `.in`/`.out`: a runtime scalar.
    Scalar,
    /// A leaf under `.in`: an input buffer.
    Input,
    /// A leaf under `.out`: an output buffer.
    Output,
}

/// The `.native` field of the `Buf` wrapper — the payload a body's `$read`/`$write` names.
const BUF_NATIVE_FIELD: &str = "native";

/// The `.element` field of the `Buf` wrapper — the element's own type.
const BUF_ELEMENT_FIELD: &str = "element";

/// The nominal id the compute crate gives the `Buf` wrappers it builds itself.
///
/// # Invariant
/// The wrapper is host-owned and carries no written occurrence, so one id names
/// them all; nothing reads a nominal id but the renderer
/// (`docs/notes/value-templates.md`).
const BUF_NOMINAL_ID: usize = 0;

/// Whether a struct's field names are the `Buf` wrapper's.
fn is_buf_shape(names: &[Option<&'static str>]) -> bool {
    // The payload is the first field in both wrappers.
    let fields: Vec<&'static str> = names.iter().filter_map(|name| *name).collect();
    fields.as_slice() == [BUF_NATIVE_FIELD, "element"]
        || fields.as_slice() == [BUF_NATIVE_FIELD, "type", "length"]
}

/// Record the role of one field of a parameter struct, descending into groups.
///
/// # Invariant
/// A buffer is a `Buf`-shaped struct and its path ends at the `.native` slot; any
/// other struct is a group, walked in field order. An unresolvable leaf is
/// refused by name with its path.
///
/// **The role decides what a leaf the field cannot read is, never the shape**:
/// under `.in` it is the buffer the parameter declares, because a body reading
/// none leaves every cell there open.
fn walk_role<P>(
    module: &mut Module<P>,
    field: AnyNodeId,
    path: &[usize],
    role: LeafRole,
    roles: &mut KernelRoles,
) -> Result<(), RoleUnread>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // A field that holds a struct value is a group; a field that holds a number
    // is a leaf.
    module.evaluate_node_deep_ref(field, None);
    let held = struct_field_value_of_expr(module, field);
    if let Some(held) = held
        && is_struct_value(module, held)
    {
        // An anonymous struct has no table: every field is unnamed.
        let names = struct_field_names(module, held).unwrap_or_default();
        if is_buf_shape(&names) {
            // The class is the buffer type's first shape position.
            let fields = struct_field_exprs(module, held);
            let mut element = None;
            if let Some(fields) = &fields {
                // The `.native` field holds the buffer **type**, so the element
                // class is that type's element.
                if let Some(native) = fields.first()
                    && let Some(native) = struct_field_value_of_expr(module, *native)
                    && let Some((element_type, _length)) =
                        shape::buffer_shape(module, shape::TypeRef::Term(native))
                {
                    element =
                        low_type_of_slot(module, element_type).map(|shape| scalar_class_of(&shape));
                }
                if element.is_none() {
                    // The `.type` field holds the element **type**, so its own
                    // value names the class.
                    element = fields
                        .get(1)
                        .copied()
                        .and_then(|field| struct_field_value_of_expr(module, field))
                        .and_then(|value| low_type_of_slot(module, value))
                        .map(|shape| scalar_class_of(&shape));
                }
            }
            // The path is the `Buf` field, as the checker resolves it for the
            // source's read; the payload is one step in.
            return match role {
                LeafRole::Input => {
                    roles.inputs.push(path.to_vec());
                    roles.input_classes.push(element);
                    Ok(())
                }
                LeafRole::Output => {
                    roles.outputs.push(path.to_vec());
                    roles.output_classes.push(element);
                    Ok(())
                }
                LeafRole::Scalar => Err(role_refusal(path, "a buffer outside `.in`")),
            };
        }
        let Some(fields) = struct_field_exprs(module, held) else {
            return Err(role_refusal(
                path,
                "a struct whose field list cannot be read",
            ));
        };
        for (at, field) in fields.into_iter().enumerate() {
            let mut path = path.to_vec();
            path.push(at);
            walk_role(module, field, &path, role, roles)?;
        }
        return Ok(());
    }
    let shape =
        struct_field_type_of_expr(module, field).and_then(|ty| low_type_of_slot(module, ty));
    match (role, &shape) {
        // An output is produced by a write, and a write produces a buffer: any
        // other output type has no mechanism.
        (LeafRole::Output, _) => Err(role_refusal(
            path,
            &format!("an output that is a {}", shape_name(&shape)),
        )),
        // The buffer the parameter declares here; see the invariant above.
        (LeafRole::Input, None) => {
            roles.inputs.push(path.to_vec());
            roles.input_classes.push(None);
            Ok(())
        }
        // A decided number is a scalar wherever it sits: `In1 = struct{.a Int}`.
        (_, Some(LowShape::USize | LowShape::Float)) => {
            roles.scalars.push(path.to_vec());
            Ok(())
        }
        (LeafRole::Scalar, None) => {
            roles.scalars.push(path.to_vec());
            Ok(())
        }
        (_, other) => Err(role_refusal(path, &format!("a {}", shape_name(other)))),
    }
}

/// The name a shape goes by in a refusal, so the refusal says what it met.
fn shape_name(shape: &Option<LowShape>) -> &'static str {
    match shape {
        None => "leaf with no stated type",
        Some(LowShape::USize) => "runtime scalar",
        Some(LowShape::Float) => "runtime float",
        Some(LowShape::Array(..)) => "array",
        Some(LowShape::Tuple(_)) => "tuple",
        Some(LowShape::Function(..)) => "function",
        Some(LowShape::Table(..)) => "table",
    }
}

/// A parameter path as a refusal names it: the positions, dotted.
fn path_text(path: &[usize]) -> String {
    path.iter()
        .map(usize::to_string)
        .collect::<Vec<String>>()
        .join(".")
}

/// The refusal a role walk gives: the path it was walking, and what it found.
fn role_refusal(path: &[usize], what: &str) -> RoleUnread {
    RoleUnread::Refusal(format!(
        "a parallel kernel's parameter holds {what} at position {}, and the ABI has no place for \
         it",
        path_text(path)
    ))
}

/// A parameter slot in a kernel's wasm signature.
///
/// # Invariant
/// A scalar `jit` kernel has one slot; a parallel kernel has the config slot then
/// the index slot. Each slot carries the `[value, type]` expr, the value node, its
/// flattened domain shape, and the wasm local base offset.
#[derive(Clone)]
struct ParamSlot {
    /// The `[value, type]` parameter expr node.
    expr: NodeId,
    /// The parameter's value node (`Index(expr, 0)`), where the shape marker is.
    value: NodeId,
    /// The flattened domain shape this slot reads as.
    shape: LowShape,
    /// The wasm local base offset: `0` for the first slot, the running sum after.
    base: usize,
    /// The parameter struct's role table, for a named struct term; `None` otherwise.
    roles: Option<KernelRoles>,
}

/// The wasm local offset of `node`, if it is a parameter read of one of `params`.
///
/// # Invariant
/// `Err` is [`param_path`]'s: a parameter read whose index is not constant is
/// propagated rather than swallowed — treating it as "not a read" would let it
/// reach the emitter's catch-all and be reported as an unsupported index.
fn param_read_offset<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: NodeId,
) -> Result<Option<u32>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    for slot in params {
        let Some(path) = param_path(module, slot.expr, node)? else {
            continue;
        };
        // A struct parameter's scalars are addressed by the role table: the
        // slot's `shape` lists exactly those scalars.
        if let Some(roles) = &slot.roles {
            if let Some(offset) = roles.scalar_offset(&path) {
                return Ok(Some((slot.base + offset) as u32));
            }
            continue;
        }
        if let Ok(offset) = flatten_offset(&slot.shape, &path) {
            return Ok(Some((slot.base + offset) as u32));
        }
    }
    Ok(None)
}

/// Lower `[param_expr] → function.return` for the kernel-safe subset into a
/// [`KernelFragment`].
///
/// # Invariant
fn value_parameter_domain<P>(module: &mut Module<P>, node: NodeId) -> Option<LowShape>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    if !is_tuple_value(module, node) {
        return None;
    }
    // SAFETY: `node` is a live parameter value of this module.
    let items = unsafe { module.array_items(node) }?;
    let mut elements = Vec::with_capacity(items.len());
    for item in items {
        let mut element = module.element_value_node(item.node).unwrap_or(item.node);
        if let Some(node) = element.dynamic()
            && matches!(
                module
                    .node_operation(node)
                    .and_then(|operation| AsEnum::<LowOperator>::as_enum(&operation.operator)),
                Some(LowOperator::Apply)
            )
            && shape::slot_type(module, item.node).is_none()
            && !module.applied_exprs.contains_key(&node)
        {
            module.evaluate_node_deep(node, None);
            element = module.element_value_node(item.node).unwrap_or(item.node);
        }
        let shape = match element.dynamic() {
            Some(element) if is_tuple_value(module, element) => {
                value_parameter_domain(module, element)
            }
            _ => element
                .dynamic()
                .and_then(|node| module.applied_exprs.get(&node).copied())
                .and_then(|applied| module.expr_type_item(applied))
                .or_else(|| shape::slot_type(module, item.node))
                .or_else(|| {
                    item.node
                        .dynamic()
                        .and_then(|node| module.element_type_of(node))
                })
                .and_then(|ty| shape::low_type_of_slot(module, ty)),
        };
        elements.push(shape);
    }
    Some(LowShape::Tuple(elements))
}

/// Compile a kernel with a stated parameter class; an open class is refused.
/// Public for hosts reading a kernel value.
pub fn compile_fragment<P>(
    module: &mut Module<P>,
    function: AnyFunctionId,
) -> Result<KernelFragment, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let AnyFunctionId::Dynamic(fid) = function else {
        return Err("static (imported) functions are not kernel-compilable v1".into());
    };
    let param_expr = module.functions[fid].parameter;
    let ret = module.functions[fid].r#return;
    // A body's return is its `[value, type]` expr; the result is the expr's value.

    // SAFETY: `ret` is a live node of `module`; nothing here drops a block.
    let ret_value = match unsafe { module.array_items(ret) } {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => ret,
    };
    // The parameter's two cells, read once before the pass needs the module.

    // SAFETY: `param_expr` is a live node of `module`; nothing here drops a block.
    let Some(param_value) = expr_value(module, param_expr) else {
        return Err("parameter is not a [value, type] expr".into());
    };
    let param_type = expr_type(module, param_expr);

    // The domain is a **type read** through the interface, not a class read.
    let domain = shape::kernel_parameter_domain(module, function)
        .or_else(|| value_parameter_domain(module, param_value));
    // The body's operand classes are the interface's business, prepared once here.
    shape::prepare_template_low_types(module, fid);
    // What the parameter's value states — the fact the domain check needs and
    // the decoded shape cannot supply.
    let statement = if is_struct_value(module, AnyNodeId::Dynamic(param_value)) {
        DomainStatement::Struct
    } else if is_tuple_value(module, param_value) {
        DomainStatement::Shape
    } else {
        DomainStatement::Absent
    };
    let param_shape = kernel_domain(domain, statement)?;
    // The parameter's type node is the shape authority
    // (`docs/notes/compound-element-slots.md`).
    let type_classes =
        domain_type_classes(module, &[Some(AnyNodeId::Dynamic(param_value)), param_type]);
    let params = vec![ParamSlot {
        expr: param_expr,
        value: param_value,
        shape: param_shape.clone(),
        base: 0,
        // A scalar kernel's parameter is its domain, not a struct of roles.
        roles: None,
    }];

    // A scalar kernel has no buffers: the fragment declares no inputs or outputs.
    let mut tally = Positions::for_params(&params);
    // The body is emitted leaf by leaf: a scalar codomain is one leaf, a tuple
    // one per element, each its own stack slot.
    let leaves = codomain_leaves(module, ret_value, expr_type(module, ret))?;
    // A codomain may mix classes: each leaf is lowered in its own, so results
    // are typed per position (`result_classes`).
    let result_classes: Vec<ScalarClass> = leaves
        .iter()
        .map(|leaf| node_class_in(module, &params, *leaf))
        .collect();
    // The filler for positions a body never read: the ABI's integer default,
    // which only decides an empty list's class.
    let class = result_classes.first().copied().unwrap_or(ScalarClass::Int);
    // The class a buffer read is declared in; see [`Positions::element_class`].
    tally.element_class = Some(class);
    // **One walk of the graph into one SSA body**, so a shared subexpression is emitted
    // once.
    let type_classes_ref = &type_classes;
    if std::env::var("DBG_CLASSES").is_ok() {
        let mut ids: Vec<String> = type_classes.iter().map(|id| format!("{id:?}")).collect();
        ids.sort();
        eprintln!("CLASSES value {param_value:?} type {param_type:?} set {ids:?}");
    }
    let mut body = Lower::lower(
        module,
        &params,
        param_value,
        fid,
        ret_value,
        expr_type(module, ret),
        &mut tally,
        type_classes_ref,
    )?;
    // **Instructions become a target's before anything reads the body**: a
    // transcendental is defined by its expansion.
    body.expand_transcendentals()?;

    Ok(KernelFragment {
        param_shape: kernel_shape(&param_shape),
        // A scalar kernel's parameter is its domain: there are no named roles for
        // a walk to find, so the lists are empty.
        roles: KernelRoles::default(),
        body,
        inputs: tally.reads,
        // The ordinal space, not the emission count: a chain writes one output twice.
        outputs: tally.write_classes.len(),
        input_classes: tally.input_classes(tally.reads),
        // A scalar fragment has no output buffers, so this is empty: its values
        // are the wasm results below.
        output_classes: Vec::new(),
        result_classes,
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: std::mem::take(&mut tally.marks),
    })
}

/// The claim the kernel `function` declares, read at kernel-compile time.
///
/// # Invariant
/// The anchor is the **template's parameter node**, which every per-call clone's origin
/// chain reaches: the parameter is the one node of a function template that an
/// instantiation still names. So this is a table read, never a walk of the module to find
/// the claim.
fn compile_claim<P>(module: &Module<P>, function: AnyFunctionId) -> Option<KernelClaim>
where
    P: Program,
{
    let AnyFunctionId::Dynamic(function) = function else {
        return None;
    };
    let Some(function) = module.functions.get(function) else {
        return None;
    };
    module
        .marks_of(function.parameter)
        .iter()
        .find(|payload| payload.starts_with(CLAIM_PREFIX))
        .and_then(|payload| KernelClaim::parse(payload).ok())
}

/// The body's value resolved into the leaves a wasm body must leave on the stack.
///
/// # Invariant
/// A bare value node is the single-leaf form and a materialized compound value the
/// multi-leaf one; a *nested* compound is not flattened, because each leaf is one
/// value on the stack — it is refused by name with the path instead.
fn codomain_leaves<P>(
    module: &Module<P>,
    ret_value: NodeId,
    ret_type: Option<AnyNodeId>,
) -> Result<Vec<NodeId>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // Peel the `value_of` extraction first: asking it whether it is a tuple
    // would answer for the expr it wraps.
    let value = value_of_node(module, ret_value).unwrap_or(ret_value);
    // A compound's leaves are its elements; the layout's own slots are not.
    let Some(items) = value_element_items::<P>(module, AnyNodeId::Dynamic(value), ret_type) else {
        return Ok(vec![ret_value]);
    };
    if items.is_empty() {
        return Ok(vec![ret_value]);
    }
    let mut leaves = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        // An element is a complete slot: the leaf is its value position.
        let element = dyn_node(module.element_value_node(item.node).unwrap_or(item.node))?;
        if is_tuple_value(module, element) {
            return Err(format!(
                "a kernel codomain must be a scalar or a tuple of scalars, but element {index} \
                 of the returned tuple is itself a tuple"
            ));
        }
        leaves.push(element);
    }
    Ok(leaves)
}

/// Whether `node` is (or peels to) a materialized tuple value, for the codomain
/// and argument walks alike.
///
/// # Invariant
/// A struct value is a compound value too, and it is not a tuple: the nominal id
/// at its second slot is what tells the two apart
/// (`docs/notes/value-templates.md`).
fn is_tuple_value<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let value = value_of_node(module, node).unwrap_or(node);
    if is_struct_value(module, AnyNodeId::Dynamic(value)) {
        return false;
    }
    // SAFETY: `value` is a live node of `module`.
    matches!(unsafe { module.array_items(value) }, Some(items) if !items.is_empty())
}

/// Lower a single-arg index function into a parallel kernel ([`KernelFragment`]).
///
/// # Invariant
/// The every-ordinal-written rule holds by construction: the lowered body is
/// straight-line, its only conditional is the emitter's 2-element scalar
/// `select`, and a `select` can skip only a *value* — a write reached inside one
/// is refused by name. The walk reads the body as values, so it must have run:
/// `undecided-reads` §7.
///
/// Public for the same reason [`compile_fragment`] is.
pub fn compile_parallel_fragment<P>(
    module: &mut Module<P>,
    function: AnyFunctionId,
) -> Result<KernelFragment, RoleUnread>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let AnyFunctionId::Dynamic(fid) = function else {
        return Err("static (imported) functions are not kernel-compilable v1".into());
    };
    let cfg_expr = module.functions[fid].parameter;
    let body = module.functions[fid].r#return;
    // The kernel's result is the body value — a `Write`, or a tuple of them.
    // It is read as values, so it must have run.
    module.evaluate_node_deep(body, None);

    // SAFETY: `body` is a live node of `module`.
    let ret_value = match unsafe { module.array_items(body) } {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => body,
    };
    // A named struct parameter carries the roles; the older tuple `cfg` does not.

    // The role table is decoded from the parameter's type, whose shape the low
    // vocabulary reads as a tuple of its fields.
    let mut roles = parallel_roles(module, cfg_expr)?;
    // The parameter's output paths are its own (`docs/notes/kernel-buffer-declarations.md`).
    let mut parameter_roles = roles.clone();
    let type_classes = domain_type_classes(
        module,
        &[
            expr_value(module, cfg_expr).map(AnyNodeId::Dynamic),
            expr_type(module, cfg_expr),
        ],
    );
    let type_classes_ref = &type_classes;
    // The output count is the codomain's arity, fixed before any index runs.
    let outputs = parallel_output_nodes(module, ret_value, expr_type(module, body));
    // The codomain states the outputs; the parameter carries no `.out` group.
    if roles.outputs.is_empty()
        && let Ok(codomain) = codomain_roles(module, body)
    {
        roles.outputs = codomain.outputs;
        roles.output_classes = codomain.output_classes.clone();
        // The element class an output position carries is the codomain's.
        parameter_roles.output_classes = codomain.output_classes;
    }

    // The slot is seeded with the ABI's signature: the parameter's leaves then
    // the index, each leaf classed by its field.
    let cfg_value = expr_value(module, cfg_expr)
        .ok_or_else(|| "parallel cfg parameter is not a [value, type] expr".to_string())?;
    // Each scalar leaf's class is the parameter field's.
    let scalar_classes = scalar_leaf_classes(module, cfg_expr, &roles)?;
    // The body's operand classes are the interface's business.
    shape::prepare_template_low_types(module, fid);
    // The domain is the **flattened scalar leaves**, not the struct's low shape
    // (`docs/notes/lowlevel-low-types.md`).
    let cfg_shape = LowShape::Tuple(
        scalar_classes
            .iter()
            .copied()
            .map(|class| Some(low_shape_of(class)))
            .collect(),
    );
    let cfg_shape = kernel_domain(Some(cfg_shape), DomainStatement::Shape)?;
    let params = vec![ParamSlot {
        expr: cfg_expr,
        value: cfg_value,
        shape: cfg_shape,
        base: 0,
        roles: Some(parameter_roles),
    }];
    // The class a buffer read is declared in: the first write's own class.

    // Read through the slots like the emission ([`node_class_in`]), or a consumer
    // would refuse the chain.

    // **The class a buffer read is declared in**: the parameter's `.out` declaration,
    // else the first write's element.
    let class = match roles
        .output_classes
        .iter()
        .copied()
        .flatten()
        .next()
        .or_else(|| {
            outputs.iter().find_map(|output| {
                write_value_node(module, *output).map(|value| node_class_in(module, &params, value))
            })
        }) {
        Some(class) => class,
        None => {
            return Err(RoleUnread::Refusal(format!(
                "no output of this parallel index function carries a decided scalar class: its \
                 codomain names {} output(s) and neither its `.out` declaration nor a \
                 `compute.write` of a decided scalar states one, so the element class a \
                 `compute.Buf` read carries cannot be read. An undecided class is not an `Int` \
                 (`docs/notes/compile-order-class.md` §3)",
                outputs.len()
            )));
        }
    };
    // **The outputs are the codomain's**, and each position must hold a
    // `compute.write` naming that same position.
    let mut tally = Positions::for_params(&params);
    // The class a buffer read is declared in; see [`Positions::element_class`].
    tally.element_class = Some(class);
    // The walk lowers an output position whatever buffer it names.
    let mut body_instr = Lower::lower_index_function(
        module,
        &params,
        cfg_value,
        fid,
        &outputs,
        class,
        &mut tally,
        type_classes_ref,
    )?;
    body_instr.expand_transcendentals()?;
    // The declaration's length is the output's allocation.
    roles.output_extents = tally.output_extents.clone();
    // The codomain's fields are the outputs; the parameter names no `.out` group.
    if roles.outputs.is_empty() {
        roles.outputs = (0..outputs.len()).map(|at| vec![at]).collect();
    }
    if outputs.len() != roles.outputs.len() {
        return Err(format!(
            "this index function's codomain names {} output(s) but its parameter declares {} \
             `.out` field(s): the two are the same list, and a struct parameter states it in \
             the type",
            outputs.len(),
            roles.outputs.len()
        )
        .into());
    }
    // A write-only body leaves one dummy scalar, which is what `results` records.

    // The declared input count: `.in`'s field count, so the count and the paths
    // cannot disagree.
    let declared_inputs = roles.inputs.len();
    Ok(KernelFragment {
        // The parameter's leaves then the index; the buffers are bound, not passed.

        // A scalar leaf's class is the parameter's own; the extent and the index
        // stay `Int`.

        // The run reads the cfg by these paths — one enumeration, three readers.
        roles: roles.clone(),
        param_shape: KernelShape::Tuple(
            scalar_classes
                .iter()
                .map(|class| KernelShape::Scalar(*class))
                .chain([KernelShape::Scalar(ScalarClass::Int)])
                .collect(),
        ),
        body: body_instr,
        inputs: declared_inputs,
        outputs: roles.outputs.len(),
        input_classes: tally.input_classes(declared_inputs),
        output_classes: tally.output_classes(),
        // The dummy scalar the write-only body leaves, in the first write's class.
        result_classes: vec![class],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: std::mem::take(&mut tally.marks),
    })
}

/// Whether `node` is a `@loop`'s result, through the wrappers a position arrives in.
///
/// # Invariant
/// A codomain position arrives wrapped exactly as [`write_node`] peels it, so the same
/// peel is tried before the node is asked who its callee is; the answer is the
/// conversion's own — a marked function whose recursion converts.
fn is_loop_position<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut node = AnyNodeId::Dynamic(
        value_of_node(module, node)
            .or_else(|| expr_value(module, node))
            .unwrap_or(node),
    );
    for _ in 0..8 {
        let Some(current) = node.dynamic() else {
            return false;
        };
        if module
            .callee_function(current)
            .is_some_and(|callee| module.loop_conversion(callee).is_ok())
        {
            return true;
        }
        match concrete_element(module, current) {
            Some(next) => node = next,
            None => return false,
        }
    }
    false
}

/// Whether `node` is a buffer the body declared: its definition is a `compute.buffer`.
///
/// # Invariant
/// A declared buffer is the *target* a write names, not a parameter path
/// (`docs/notes/kernel-buffer-declarations.md`): the write's own position is the codomain's
/// order, and the declaration's index is that order, so this is what tells the two apart
/// before the body is lowered.
fn is_declared_position<P>(module: &Module<P>, function: FunctionId, node: NodeId) -> bool
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut node = AnyNodeId::Dynamic(
        value_of_node(module, node)
            .or_else(|| expr_value(module, node))
            .unwrap_or(node),
    );
    for _ in 0..8 {
        let Some(current) = node.dynamic() else {
            return false;
        };
        if let lichen_lowlevel::Define::Computed(definition) = module.define_in(function, current)
            && module.node_operation(definition).is_some_and(|operation| {
                matches!(
                    AsEnum::<ComputeOperator>::as_enum(&operation.operator),
                    Some(ComputeOperator::Buf)
                )
            })
        {
            return true;
        }
        match concrete_element(module, current) {
            Some(next) => node = next,
            None => return false,
        }
    }
    false
}

/// The `compute.write` node a position reaches, if it reaches one.
///
/// # Invariant
/// A position reaches the emitter wrapped: the slot-read destructuring leaves a
/// constant `Index` over a materialized array, and that element is reached
/// through the `[value, type]` expr or a `value_of` extraction. All three are
/// walked here, before the instructions are emitted. Bounded, not recursive: a
/// deeper chain is not a wrapper this walk understands.
fn write_node<P>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut node = AnyNodeId::Dynamic(
        value_of_node(module, node)
            .or_else(|| expr_value(module, node))
            .unwrap_or(node),
    );
    for _ in 0..8 {
        let current = node.dynamic()?;
        if let Some(operation) = module.node_operation(current)
            && matches!(
                AsEnum::<ComputeOperator>::as_enum(&operation.operator),
                Some(ComputeOperator::Write | ComputeOperator::WriteAtomic)
            )
        {
            return Some(current);
        }
        node = concrete_element(module, current)?;
    }
    None
}

/// The `at`-th operand of the `compute.write` a position reaches.
///
/// # Invariant
/// The operand array is `[to, at, value]`, so `0` is the buffer written and `2` the
/// value written.
fn write_item_node<P>(module: &Module<P>, node: NodeId, at: usize) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let write = write_node(module, node)?;
    let items = operand_items(module, module.node_operation(write)?.operand?).ok()?;
    Some(items.get(at)?.node)
}

/// The value a `compute.write` position writes — the expression whose class the
/// index function computes in.
fn write_value_node<P>(module: &Module<P>, node: NodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    write_item_node(module, node, 2)
}

/// The buffer a `compute.write` position names as its target.
///
/// # Invariant
/// This is the authority over which output the write fills. A codomain position is
/// not: it is the order the walk happens to reach the writes in, and a body that
/// names its targets in another order means what it names.
fn write_target_node<P>(module: &Module<P>, node: NodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    write_item_node(module, node, 0)
}

/// The element a constant `Index` into a materialized array value selects.
fn concrete_element<P>(module: &Module<P>, node: NodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let operation = module.node_operation(node)?;
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&operation.operator),
        Some(LowOperator::Index)
    ) {
        return None;
    }
    let (target, index) = operand_expr(module, operation.operand).ok()?;
    let k = usize_value(module, index)?;
    let target = target.dynamic()?;
    let array_value = value_of_node(module, target).or(Some(target))?;
    // SAFETY: `array_value` is a live node of `module`.
    let items = unsafe { module.array_items(array_value) }?;
    // The element is the caller's own operand only in the spelling; a slot from a
    // frozen module keeps its ref.
    Some(items.get(k)?.node)
}

/// Resolve an index function's return value into its per-position output nodes.
///
/// # Invariant
/// The return value is the codomain's value: a bare value is one output and a
/// materialized tuple one per element. The elements are handed to the emitter
/// unresolved — what a position *is* is the emitter's to determine, which keeps
/// the specific cause with the code that can name it.
fn parallel_output_nodes<P>(
    module: &Module<P>,
    ret_value: NodeId,
    ret_type: Option<AnyNodeId>,
) -> Vec<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // A `value_of` extraction still carries the expr's value: peel it before
    // asking whether the rest is a tuple.
    let value = value_of_node(module, ret_value).unwrap_or(ret_value);
    // A compound's elements are its outputs; the layout's own slots are not.
    let Some(items) = value_element_items::<P>(module, AnyNodeId::Dynamic(value), ret_type) else {
        return vec![ret_value];
    };
    if items.is_empty() {
        return vec![ret_value];
    }
    let mut nodes = Vec::with_capacity(items.len());
    for item in items {
        // An element is a complete slot: the output is its value position.
        match dyn_node(module.element_value_node(item.node).unwrap_or(item.node)) {
            Ok(node) => nodes.push(node),
            // A static element is not a kernel body: keep it as the single output
            // so the position is named.
            Err(_) => return vec![ret_value],
        }
    }
    nodes
}

/// A fragment's parameter leaves, as classes, in flattening order.
///

/// The host import name a buffer call of `class` resolves to.
///

/// The reason a `jit` reports when the parameter's domain is not decided.
///
/// # Invariant
/// A kernel is lowered for one class, so the function must state it: a wasm or
/// SPIR-V value is an `i64` or an `f32` and never either, and a template has no
/// apply to read a class off. A body that left its class open is refused here
/// rather than lowered in a class the compiler invented — committing to a member
/// is the author's statement.
const UNDECIDED_DOMAIN: &str = "the kernel parameter's class is not decided when the kernel is \
compiled: a kernel is lowered for one class, so the function must state it — annotate the \
parameter (for example `p <: (_x => (@any : Int, @any : Int))` for a tuple domain, or `y : Int` for a scalar one)";

/// Why a **polymorphic template** cannot be jitted:
/// a template's parameter has no domain before an apply binds it.
const POLYMORPHIC_TEMPLATE: &str = "a polymorphic template cannot be jitted: the kernel parameter's domain is not decided when the kernel is compiled, and a template's parameter has no domain before an apply binds it — annotate the parameter (for example `k : Par` for a named struct, or `p <: (_x => (@any : Int, @any : Int))` for a tuple)";

/// The other facts [`UNDECIDED_DOMAIN`] used to cover.
///
/// # Invariant
/// A struct reads as a tuple now, so the old single refusal would have told an
/// author to annotate a parameter that already is annotated.
const DOMAIN_IS_A_STRUCT: &str = "the kernel parameter's domain is a struct type, and a kernel \
domain must be a scalar or a tuple of scalars (a nominal struct type has no low shape at all)";

/// A stated domain whose **structure** has a position with no class, and no
/// position to name (a non-tuple structure).
const DOMAIN_STATED: &str = "the kernel parameter's domain has no class this lowering can read, \
and a kernel domain must be a scalar or a tuple of scalars";

/// What a parameter's own type states, which the decoded shape cannot supply.
///
/// # Invariant
/// "No class" is what an annotated struct and an unannotated template both decode
/// to, so the statement is read from the type cell rather than the shape.
enum DomainStatement {
    /// The parameter's type is a **nominal struct**, which has no low shape by
    /// design, so no class can be read out of it.
    Struct,
    /// The parameter's domain is a **structure** and a position of it has no
    /// class: the position is what to name.
    Shape,
    /// Nothing readable was stated for this parameter — the case
    /// [`UNDECIDED_DOMAIN`] is the honest refusal for.
    Absent,
}

/// All of [`UNDECIDED_DOMAIN`]'s facts, told apart by the caller's `statement`.
///
/// # Invariant
/// A kernel domain must be a decided scalar or a tuple of decided positions —
/// every other shape is a refusal that names its own cause. A float is a decided
/// position and is accepted at every position the walk reaches.
fn kernel_domain(domain: Option<LowShape>, statement: DomainStatement) -> Result<LowShape, String> {
    if !domain_is_known(&domain) {
        return Err(match statement {
            DomainStatement::Struct => DOMAIN_IS_A_STRUCT.into(),
            DomainStatement::Shape => match domain.as_ref().and_then(first_unreadable_position) {
                Some(position) => format!(
                    "the kernel parameter's domain has a position with no class: position {} of \
                     it is not a class this lowering can read, and a kernel domain must be a \
                     scalar or a tuple of scalars",
                    position
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(".")
                ),
                None => DOMAIN_STATED.into(),
            },
            DomainStatement::Absent => UNDECIDED_DOMAIN.into(),
        });
    }
    match domain {
        Some(domain @ (LowShape::USize | LowShape::Float | LowShape::Tuple(_))) => Ok(domain),
        Some(_) => Err("kernel domain must be a scalar or a tuple of scalars".into()),
        // `domain_is_known` above refuses a `None` first.
        None => Err(DOMAIN_STATED.into()),
    }
}

/// The first position of a tuple domain the lowering cannot read, as a path.
///
/// # Invariant
/// `None` when there is no position to name (a domain that is not a tuple);
/// nested only where the domain nests.
fn first_unreadable_position(domain: &LowShape) -> Option<Vec<usize>> {
    let LowShape::Tuple(items) = domain else {
        return None;
    };
    items.iter().enumerate().find_map(|(at, item)| {
        let Some(item) = item else {
            return Some(vec![at]);
        };
        if known_shape(item) {
            return None;
        }
        Some(match first_unreadable_position(item) {
            Some(deeper) => std::iter::once(at).chain(deeper).collect(),
            None => vec![at],
        })
    })
}

/// The IR's own expression of a kernel domain, in the two variants the
/// lowered-kernel IR has.
///
/// # Invariant
/// Total on purpose: a shape with no IR counterpart folds to [`KernelShape::Scalar`]
/// with its element's class, flattening to one leaf exactly as [`flat_arity`]
/// does — a refusal arm would change behaviour for a shape that path tolerates.
/// A compound leaf takes its element's class, so an `array{Float, 3}` is one
/// `f32` local.
fn kernel_shape(domain: &LowShape) -> KernelShape {
    match domain {
        LowShape::USize => KernelShape::Scalar(ScalarClass::Int),
        LowShape::Float => KernelShape::Scalar(ScalarClass::Float),
        LowShape::Tuple(items) => KernelShape::Tuple(
            items
                .iter()
                .map(|item| {
                    item.as_ref()
                        .map_or(KernelShape::Scalar(ScalarClass::Int), kernel_shape)
                })
                .collect(),
        ),
        LowShape::Array(element, _) => KernelShape::Scalar(
            element
                .as_ref()
                .as_ref()
                .map_or(ScalarClass::Int, scalar_class_of_shape),
        ),
        // A function and a table have no scalar element: the filler's integer
        // local is the answer.
        LowShape::Function(..) | LowShape::Table(..) => KernelShape::Scalar(ScalarClass::Int),
    }
}

/// [`scalar_class_of`] on a position the walk has not unwrapped.
fn scalar_class_of_shape(shape: &LowShape) -> ScalarClass {
    scalar_class_of(shape)
}

/// The class a single local takes for a position of `shape`.
fn scalar_class_of(shape: &LowShape) -> ScalarClass {
    match shape {
        LowShape::Float => ScalarClass::Float,
        LowShape::Array(element, _) => element
            .as_ref()
            .as_ref()
            .map_or(ScalarClass::Int, scalar_class_of_shape),
        LowShape::USize | LowShape::Tuple(_) | LowShape::Function(..) | LowShape::Table(..) => {
            ScalarClass::Int
        }
    }
}

/// The low shape a scalar leaf seeds its parameter slot with.
///
/// # Invariant
/// The interpreter has no `bf16` shape: a `BFloat16` leaf seeds the `Float` slot
/// [`ScalarValue::low`] reads, and the half's own sixteen bits are carried by the
/// pattern in that `f32`, not by the shape.
fn low_shape_of(class: ScalarClass) -> LowShape {
    match class {
        ScalarClass::Int => LowShape::USize,
        ScalarClass::Float | ScalarClass::BFloat16 => LowShape::Float,
    }
}

/// Whether every position in a shape is one the kernel ABI can lower to a local.
///
/// # Invariant
/// It recurses through every position, because a compound shape's own
/// `Some` says nothing about its elements. A function's codomain and a table's
/// value are walked too, since a declared `Int -> Float` puts a float in exactly
/// that position.
fn domain_is_known(shape: &Option<LowShape>) -> bool {
    shape.as_ref().is_some_and(known_shape)
}

/// [`domain_is_known`] on a shape the walk already unwrapped.
fn known_shape(shape: &LowShape) -> bool {
    match shape {
        LowShape::USize | LowShape::Float => true,
        LowShape::Tuple(items) => items.iter().all(domain_is_known),
        LowShape::Array(element, _) => domain_is_known(element),
        LowShape::Function(domain, codomain) => {
            domain_is_known(domain) && domain_is_known(codomain)
        }
        LowShape::Table(key, value) => domain_is_known(key) && domain_is_known(value),
    }
}

/// The number of scalar locals a domain shape flattens to — the wasm parameter
/// count.
///
/// # Invariant
/// A scalar is one local and a tuple the sum of its elements'; an undecided shape
/// cannot reach a wasm signature, so its arm counts one leaf rather than inventing
/// a count.
fn flat_arity(shape: &LowShape) -> usize {
    match shape {
        LowShape::USize => 1,
        // One leaf — the layout answer, consistent with `kernel_shape`.
        LowShape::Float => 1,
        LowShape::Tuple(items) => items.iter().map(flat_arity_slot).sum(),
        LowShape::Array(_, _) | LowShape::Function(..) | LowShape::Table(..) => 1,
    }
}

/// [`flat_arity`] of a position nothing states: it still occupies one local, so
/// the ABI's count is fixed.
fn flat_arity_slot(shape: &Option<LowShape>) -> usize {
    shape.as_ref().map_or(1, flat_arity)
}

/// How deep a parameter's field nesting a named read's resolution follows.
///
/// # Invariant
/// A chain deeper than this is not a struct read the resolution understands: it
/// stops and lets its caller name the cause, which is what keeps a type whose
/// encoding re-enters from spinning.
const MAX_PARAMETER_DEPTH: usize = 32;

/// The refusal the walk gives past [`MAX_KERNEL_BODY_DEPTH`].
///
/// # Invariant
/// One string rather than a `format!` at the call site, so every refusal of this
/// cause carries the same three facts.
fn kernel_body_too_deep() -> String {
    format!(
        "a kernel body's expression nests more than {MAX_KERNEL_BODY_DEPTH} levels, so lowering it \
         would recurse deeper than a compile should spend on its stack — a body-depth budget, not a \
         stack guarantee. An unmarked recursion is **expanded**, and expansion nests every copy \
         inside the last one's else arm, so the walk's depth is the trip count: it is the expansion \
         that is too big rather than the program. Mark the recursion `@loop` so it may become a \
         dynamic loop instead of an expansion (`docs/notes/loop-conversion.md` §1.1) — that form is \
         O(1) in the trip count and removes this limit by construction — or, if the trip count is \
         small, write the copies out by hand."
    )
}

/// The buffer positions one kernel body addresses, counted as it is emitted.
///
/// # Invariant
/// Two counters, one per buffer space: the positions are compile-time constants
/// (`parallel_buffer_pos` refuses anything else), and the fragment carries the
/// totals — so a caller handed the wrong number is refused rather than given a
/// shader that reads a binding nobody bound.
#[derive(Debug, Default, Clone)]
struct Positions {
    /// How many `compute.write`s were emitted.
    ///
    /// # Invariant
    /// A count of **emissions**, and no longer the ordinal the next write is given:
    /// a chain writes one output more than once, so the two numbers differ and only
    /// [`Self::record_write`] knows an ordinal. What still reads this is the refusal
    /// of a write inside a conditional arm, which asks only whether one happened.
    emissions: usize,
    /// One past the highest input position any `compute.read` named.
    ///
    /// # Invariant
    /// A max rather than a count, because read positions are a sparse space: a body
    /// that reads only the second input still needs two buffers.
    reads: usize,
    /// The element class of each output ordinal, **placed at its ordinal**.
    ///
    /// # Invariant
    /// Placed and not pushed, because a chain emits two writes for one ordinal and
    /// the emission order is then not the ordinal order. A hole cannot survive to
    /// [`Self::output_classes`]: every codomain position is checked to resolve to its
    /// own ordinal, so every ordinal below the declared count is written.
    write_classes: Vec<ScalarClass>,
    /// The element class of each input position read, **placed at its position**.
    ///
    /// # Invariant
    /// Placed and not pushed: read positions are a sparse space, so a body that reads
    /// position 1 and not 0 declares two inputs ([`Self::reads`]) and leaves two entries
    /// here. `None` at a position no read reached, so a hole is not mistaken for a read.
    read_classes: Vec<Option<ScalarClass>>,
    /// The class the parameter's own `Buf T` declares for each input position.
    ///
    /// # Invariant
    /// Parallel to the parameter's `roles.inputs` — the list `parallel_buffer_pos` numbers
    /// a read against — and `None` at a position the parameter leaves open. Held here so
    /// [`Self::declared_input_class`] is the only place that resolves one position.
    declared_inputs: Vec<Option<ScalarClass>>,
    /// The class a buffer read's element is declared in.
    ///
    /// # Invariant
    /// The one class that stays the fragment's: a read's element is the only value
    /// whose class no node carries, because the host binds the buffer. `Int` by
    /// default until the caller says otherwise.
    element_class: Option<ScalarClass>,
    /// The marks the body's labelled expressions published, in lowering order.
    marks: Vec<KernelMark>,
    /// The length each `compute.buffer` declaration states, in declaration order.
    ///
    /// # Invariant
    /// The run allocates each output at its declared length
    /// (`docs/notes/kernel-buffer-declarations.md`): a declaration whose count is a runtime
    /// scalar answers `None`, and the launch's element count is the extent.
    output_extents: Vec<Option<u32>>,
}

impl Positions {
    /// The tally one body's lowering records into, seeded with the parameter's declaration.
    ///
    /// # Invariant
    /// The declared classes are copied off the first parameter's `roles`, the same list
    /// [`Self::declared_input_class`] numbers: position `k` is the one the parameter
    /// declares and the one a read at `k` names, or the two are not the same position.
    fn for_params(params: &[ParamSlot]) -> Self {
        Self {
            declared_inputs: params
                .first()
                .and_then(|slot| slot.roles.as_ref())
                .map(|roles| roles.input_classes.clone())
                .unwrap_or_default(),
            ..Self::default()
        }
    }

    /// The class input `position` is declared in: the parameter's `Buf T`, else the
    /// fragment's own element class.
    ///
    /// # Invariant
    /// **The one answer for a position, read or not**: a read at `position` is lowered in
    /// this class and recorded with it, and a position no read reached takes the same one
    /// — so a sparse body cannot describe one buffer twice.
    fn declared_input_class(&self, position: usize) -> ScalarClass {
        self.declared_inputs
            .get(position)
            .copied()
            .flatten()
            .unwrap_or_else(|| self.element_class.unwrap_or(ScalarClass::Int))
    }

    /// The declared class of every input position, `declared` entries long.
    ///
    /// # Invariant
    /// One entry per **position**, the index every reader above uses. A position
    /// no read reached takes [`Self::declared_input_class`], and only a position neither a
    /// read nor a `Buf T` names takes the ABI's integer default: that is the open element
    /// cell, which declares no class the fragment could be wrong about.
    fn input_classes(&self, declared: usize) -> Vec<ScalarClass> {
        (0..declared.max(self.read_classes.len()))
            .map(|position| {
                self.read_classes
                    .get(position)
                    .copied()
                    .flatten()
                    .unwrap_or_else(|| self.declared_input_class(position))
            })
            .collect()
    }
    /// Record one read of input `position`, and answer the class its element is declared in.
    ///
    /// # Invariant
    /// The class goes **at the position**, as [`Self::write_classes`] puts a write's at its
    /// ordinal, so a body that reads position 1 first and position 0 second describes both.
    /// The answer travels back because the read's own instruction is lowered in it.
    fn record_read(&mut self, position: usize) -> ScalarClass {
        let class = self.declared_input_class(position);
        self.reads = self.reads.max(position + 1);
        self.read_classes.resize(position + 1, None);
        self.read_classes[position] = Some(class);
        class
    }
    /// Record one write of output `ordinal`, in the class its element is declared in.
    ///
    /// # Invariant
    /// The class goes **at the ordinal**, so a chain that writes one output twice
    /// leaves one entry and not two — the entry the fragment's `output_classes`
    /// invariant names, "the class of the buffer write `k` filled".
    fn record_write(&mut self, ordinal: usize, class: ScalarClass) {
        self.emissions += 1;
        let filler = self.element_class.unwrap_or(ScalarClass::Int);
        self.write_classes.resize(ordinal + 1, filler);
        self.write_classes[ordinal] = class;
    }

    /// The declared class of every output ordinal, one per ordinal.
    fn output_classes(&self) -> Vec<ScalarClass> {
        self.write_classes.clone()
    }
}

/// The class a fragment's own values are lowered in: the body's dummy result and
/// a loop's carried locals.
///
/// # Invariant
/// Not the class of its body — a body may compute in more than one class, and every
/// instruction carries its own — but the class of its *first produced value*, read
/// off `output_classes` and then `result_classes`. A hand-built fragment with
/// neither takes the ABI's integer default rather than panicking. A loop carrying
/// two classes needs its carried list typed per position.
pub(crate) fn fragment_class(fragment: &KernelFragment) -> ScalarClass {
    fragment
        .output_classes
        .first()
        .or_else(|| fragment.result_classes.first())
        .copied()
        .unwrap_or(ScalarClass::Int)
}

/// A constant in the representation the class's opcode reads.
///
/// # Invariant
/// The class is the instruction's own, and one body may hold constants of both, so a
/// class read off the fragment would answer a question the instruction already
/// answers; an integer a `Float` instruction carries is converted here, once.
fn const_bits(class: ScalarClass, value: i64) -> i64 {
    match class {
        ScalarClass::Int => value,
        // Exact for the integers a fragment carries: ordinals, and an index bounded
        // by `MAX_PARALLEL_ELEMENTS`.
        ScalarClass::Float => float_bits(value as f32),
        // A half is its float's high sixteen bits, so the word is read back to that width.
        ScalarClass::BFloat16 => i64::from(((value as u32) >> 16) << 16),
    }
}

/// The scalar leaves of a registered parallel fragment's signature, in field
/// order.
///
/// # Invariant
/// The parameter's scalars without the index the ABI appends
/// ([`param_classes`] folds the shape into one list); a launch's `cfg` carries
/// exactly these positions, and how many there are is the fragment's.
fn parallel_leaf_classes(id: KernelId) -> Result<Vec<ScalarClass>, String> {
    let fragments = kernels().lock().unwrap();
    let fragment = fragments
        .get(&id)
        .ok_or_else(|| format!("parallel kernel {id} is not registered"))?;
    Ok(param_classes(&fragment.fragment)[..fragment.fragment.scalar_leaves()].to_vec())
}

/// The facts a run reads off the fragment's register: the walk's paths, and each
/// declared output's element class.
struct FragmentFacts {
    roles: KernelRoles,
    output_classes: Vec<ScalarClass>,
}

fn fragment_facts(id: KernelId) -> FragmentFacts {
    kernels()
        .lock()
        .unwrap()
        .get(&id)
        .map(|entry| FragmentFacts {
            roles: entry.fragment.roles.clone(),
            output_classes: entry.fragment.output_classes.clone(),
        })
        .unwrap_or(FragmentFacts {
            roles: KernelRoles::default(),
            output_classes: Vec::new(),
        })
}

/// The run input a buffer node names: the payload's words, or a resident result.
///
/// # Invariant
/// `None`, with the refusal recorded, for anything else: a decided non-buffer is
/// the one way a launch can be handed something it cannot run on.
fn run_input<P>(
    module: &mut Module<P>,
    node: AnyNodeId,
    where_: &str,
    position: &str,
) -> Option<RunInput>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + AsEnum<LowValue>,
{
    let Some(held) = decided_buffer_value(module, node) else {
        return None;
    };
    if matches!(held.as_enum(), Some(LowValue::Error)) {
        return None;
    }
    match AsEnum::<ComputeValue>::as_enum(&held) {
        Some(ComputeValue::Buffer(payload, class)) => {
            // SAFETY: the buffer value is read on this borrow, so its payload's
            // block is alive below.
            let data = buffer_items(&payload)?;
            // The class travels with the words: a float input is read as `f32`s,
            // an integer one as `Int`s.
            Some(RunInput::Host(BufferWords {
                class,
                words: unpack_elements(class, data),
            }))
        }
        // Handed to the run as the id it is: a "gpu" intermediate never comes home
        // to be sent out again.
        Some(ComputeValue::DeviceBuffer(resident)) => Some(RunInput::Resident(resident)),
        _ => {
            not_a_buffer::<P>(module, node, where_, position);
            None
        }
    }
}

/// A decided buffer payload, or `None` while its value is absent or `@any`.
fn decided_buffer_value<P>(module: &Module<P>, node: AnyNodeId) -> Option<P::Value>
where
    P: Program,
    P::Value: AsEnum<LowValue>,
{
    let value = module.node_value(node)?;
    (!matches!(value.as_enum(), Some(LowValue::Any))).then_some(value)
}

/// The payload a `Buf` value carries — its `.native` slot.
///
/// # Invariant
/// A role path names the wrapper, because that is what the checker resolves for the
/// source's read; the wrapper's field order is fixed by [`is_buf_shape`], so the
/// payload is its first **field** — a struct value's fields start after the two
/// header slots.
///
/// The field is a complete expression, so the payload is the **value** the field
/// holds and not the field itself.
fn buf_payload<P>(module: &mut Module<P>, node: AnyNodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    let field = items_of(module, node)?
        .get(shape::STRUCT_VALUE_FIELD_BASE)?
        .node;
    slot_value_of(module, field).or(Some(field))
}

/// The items of `node`'s array value, whichever side of the freeze the node sits on.
///
/// # Invariant
/// A reused cell's value is an array whose items are refs into its artifact, so a walk
/// that required a live dynamic node would stop at the cell's own payload.
fn items_of<P>(module: &Module<P>, node: AnyNodeId) -> Option<&'static [ArrayItem]>
where
    P: Program,
    P::Value: ValueType,
{
    let value = module.node_value(node)?;
    let LowValue::Array(array) = value.as_enum()? else {
        return None;
    };
    // SAFETY: the value came from a live node or a pinned artifact; either keeps it.
    Some(unsafe { array.items() })
}

/// The value position of a **slot**, or [`None`] for a node that is not one.
///
/// # Invariant
/// A compound's element is a full-width `[value, type, attributes]` expr; a bare array
/// is a value of its own (`docs/notes/compound-element-slots.md`).
fn slot_value_of<P>(module: &Module<P>, node: AnyNodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: ValueType,
{
    let value = module.node_value(node)?;
    let LowValue::Array(array) = value.as_enum()? else {
        return None;
    };
    if array.layout() == 0 {
        return None;
    }
    // SAFETY: the value came from a live node or a pinned artifact; either keeps it.
    unsafe { array.items() }.first().map(|item| item.node)
}

/// The node a role path names in a cfg value: each step is a field index.
///
/// # Invariant
/// A role path counts **fields**, so a struct value's own header is skipped:
/// the name table and the nominal id hold the two slots before the first field
/// (`docs/notes/value-templates.md`). A value that is not a struct value — the
/// retired tuple `cfg` — keeps its own positions.
fn value_at_path<P>(module: &mut Module<P>, root: NodeId, path: &[usize]) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    let mut node = AnyNodeId::Dynamic(root);
    for &at in path {
        let base = if is_struct_value(module, node) {
            shape::STRUCT_VALUE_FIELD_BASE
        } else {
            0
        };
        let field = items_of(module, node)?.get(base + at)?.node;
        node = slot_value_of(module, field).unwrap_or(field);
    }
    Some(node)
}

/// The type a buffer's elements are, for the wrapper's `.element` slot.
///
/// # Invariant
/// The interpreter reads a bf16 element as the `f32` the half is the top of
/// ([`element_value`]), so the slot states `Float`: a class the language has no
/// type for cannot be spelled here, and the sixteen bits travel in the payload.
fn element_type(class: ScalarClass) -> TypeValue {
    match class {
        ScalarClass::Int => TypeValue::TypeInt,
        ScalarClass::Float | ScalarClass::BFloat16 => TypeValue::TypeFloat,
    }
}

/// The name table of a struct value the compute crate builds itself.
///
/// # Invariant
/// The names are the fields' own, in field order, and the table holds each
/// name's **relative** position — the shape the checker's own table has
/// (`docs/notes/value-templates.md`).
fn struct_names_node<P>(module: &mut Module<P>, block: BlockId, names: &[&'static str]) -> NodeId
where
    P: Program,
    P::Value: ValueType,
{
    let entries: Vec<(AnyNodeId, AnyNodeId)> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let key = module.add_node(
                block,
                None,
                Some(<P::Value as From<LowValue>>::from(LowValue::Str(name))),
            );
            let value = module.add_node(
                block,
                None,
                Some(<P::Value as From<LowValue>>::from(LowValue::USize(index))),
            );
            (AnyNodeId::Dynamic(key), AnyNodeId::Dynamic(value))
        })
        .collect();
    let handle = module.build_table(&entries, block);
    module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(LowValue::Table(handle))),
    )
}

/// One `Buf` value: a struct value over the payload and the element type.
///
/// # Invariant
/// The wrapper is a **struct value**: it carries the name table and the nominal
/// id before its two fields, so the payload is the first field and a reader
/// that walks the fields skips the header
/// (`docs/notes/value-templates.md`).
fn buf_value<P>(
    module: &mut Module<P>,
    block: BlockId,
    payload: AnyNodeId,
    class: ScalarClass,
) -> Option<NodeId>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    let element = module.add_node(
        block,
        None,
        Some(<P::Value as From<TypeValue>>::from(element_type(class))),
    );
    let names = struct_names_node::<P>(module, block, &[BUF_NATIVE_FIELD, BUF_ELEMENT_FIELD]);
    let id = module.add_node(
        block,
        None,
        Some(<P::Value as ValueType>::type_id_value(BUF_NOMINAL_ID)),
    );
    // A field is a complete expression: its value, its type and its attributes
    // are its own.
    let native = element_slot(module, block, dyn_node(payload).ok()?)?;
    let element = element_slot(module, block, element)?;
    let items = [
        ArrayItem::new(AnyNodeId::Dynamic(names)),
        ArrayItem::new(AnyNodeId::Dynamic(id)),
        ArrayItem::new(AnyNodeId::Dynamic(native)),
        ArrayItem::new(AnyNodeId::Dynamic(element)),
    ];
    let handle = module.alloc_array(&items, block);
    Some(module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(LowValue::Array(
            handle.into(),
        ))),
    ))
}

/// The current `Buf` value: payload, element type, and element count.
fn current_buf_value<P>(
    module: &mut Module<P>,
    block: BlockId,
    payload: AnyNodeId,
    class: ScalarClass,
    declared: Option<AnyNodeId>,
) -> Option<NodeId>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    let length = match module.node_value(payload).and_then(|value| value.as_enum()) {
        Some(ComputeValue::Buffer(payload, class)) => {
            buffer_items(&payload).map(|bytes| element_count(class, bytes.len()))
        }
        Some(ComputeValue::DeviceBuffer(resident)) => Some(resident.count),
        _ => None,
    };
    let length = module.add_node(
        block,
        None,
        length.map(|length| <P::Value as From<LowValue>>::from(LowValue::USize(length))),
    );
    let element = module.add_node(
        block,
        None,
        Some(<P::Value as From<TypeValue>>::from(element_type(class))),
    );
    let (names, id) = match declared {
        Some(declared) => {
            let items = shape::value_items(module, declared)?;
            let names = items.get(shape::STRUCT_VALUE_NAMES_SLOT)?.node;
            let id = items.get(shape::STRUCT_VALUE_ID_SLOT)?.node;
            (
                module.as_dynamic(names, block),
                module.as_dynamic(id, block),
            )
        }
        None => (
            struct_names_node::<P>(module, block, &[BUF_NATIVE_FIELD, "type", "length"]),
            module.add_node(
                block,
                None,
                Some(<P::Value as ValueType>::type_id_value(BUF_NOMINAL_ID)),
            ),
        ),
    };
    let native = element_slot(module, block, dyn_node(payload).ok()?)?;
    let element = element_slot(module, block, element)?;
    let length = element_slot(module, block, length)?;
    let items = [
        ArrayItem::new(AnyNodeId::Dynamic(names)),
        ArrayItem::new(AnyNodeId::Dynamic(id)),
        ArrayItem::new(AnyNodeId::Dynamic(native)),
        ArrayItem::new(AnyNodeId::Dynamic(element)),
        ArrayItem::new(AnyNodeId::Dynamic(length)),
    ];
    let handle = module.alloc_array(&items, block);
    Some(module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(LowValue::Array(
            handle.into(),
        ))),
    ))
}

/// A compound element's slot: the value, an undecided type, the wildcards.
///
/// # Invariant
/// The read's type comes from the declared compound, and the layout is the
/// module's full-width one.
fn element_slot<P>(module: &mut Module<P>, block: BlockId, value: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    // SAFETY: the default attributes are a live array of this module.
    let width = unsafe { module.default_attributes()?.items() }.len();
    // The array is `[value, type, attrs…]` wide.
    let attributes = width.checked_sub(2)?;
    let indices: Vec<u32> = (0..attributes as u32).collect();
    let layout = module.layout_with_indices(&indices)?;
    let mut items = vec![ArrayItem::new(AnyNodeId::Dynamic(value))];
    items.push(ArrayItem::new(AnyNodeId::Dynamic(
        module.add_node(block, None, None),
    )));
    for _ in 0..attributes {
        let any = module.add_node(
            block,
            None,
            Some(<P::Value as From<LowValue>>::from(LowValue::Any)),
        );
        items.push(ArrayItem::new(AnyNodeId::Dynamic(any)));
    }
    let handle = module.alloc_array(&items, block);
    Some(module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(LowValue::Array(
            ArrayValue::with_layout(handle, layout),
        ))),
    ))
}

/// A **declared group**: the value the surface states, and the type it carries.
///
/// # Invariant
/// The value is the declaration's own compound value and the type is the one
/// the declaration states, so a reader takes the **layout** from the type and
/// the **header** from the value (`docs/notes/value-templates.md`).
#[derive(Clone, Copy)]
struct DeclaredGroup {
    /// The declared compound value: the header a struct group inherits.
    value: AnyNodeId,
    /// The declared value's type: the layout the group is read at.
    ty: Option<AnyNodeId>,
}

/// The layout a declared group carries.
///
/// # Invariant
/// The layout is the group's **type** where the type names one; a group whose
/// type names none is read by its own shape, where a struct value's header
/// holds two slots before the first field and an array value's element
/// template one (`docs/notes/value-templates.md`).
fn declared_layout<P>(module: &Module<P>, declared: DeclaredGroup, arity: usize) -> CompoundLayout
where
    P: Program,
    P::Value: ValueType,
{
    if let Some(ty) = declared.ty
        && let Some(layout) = compound_layout::<P>(module, ty)
    {
        return layout;
    }
    if is_struct_value(module, declared.value) {
        return CompoundLayout::Struct;
    }
    // The slots the group's own elements do not cover are the layout's own.
    let own = shape::value_len(module, declared.value).map_or(0, |len| len.saturating_sub(arity));
    match own {
        1 => CompoundLayout::Array,
        _ => CompoundLayout::Tuple,
    }
}

/// The declared child at `at`: the value and the type it carries.
///
/// # Invariant
/// A group is walked through the value its own slot holds, so a nested group is
/// read at the declaration's own position and the layout's own slots — a
/// struct's header, an array's template — are never a child
/// (`docs/notes/value-templates.md`).
fn declared_child<P>(
    module: &Module<P>,
    declared: DeclaredGroup,
    layout: CompoundLayout,
    at: usize,
) -> Option<DeclaredGroup>
where
    P: Program,
    P::Value: ValueType,
{
    match layout {
        CompoundLayout::Struct => {
            let field =
                shape::value_item(module, declared.value, shape::STRUCT_VALUE_FIELD_BASE + at)?;
            let value = struct_field_value_of_expr(module, field)?;
            let ty = struct_field_type_of_expr(module, field);
            Some(DeclaredGroup { value, ty })
        }
        CompoundLayout::Array => {
            let item =
                shape::value_item(module, declared.value, shape::ARRAY_VALUE_ELEMENT_BASE + at)?;
            Some(DeclaredGroup {
                value: module.element_value_node(item)?,
                ty: shape::value_item(module, item, shape::EXPR_TYPE_SLOT),
            })
        }
        CompoundLayout::Tuple => {
            let item = shape::value_item(module, declared.value, at)?;
            Some(DeclaredGroup {
                value: module.element_value_node(item)?,
                ty: shape::value_item(module, item, shape::EXPR_TYPE_SLOT),
            })
        }
    }
}

fn declared_at_path<P>(
    module: &Module<P>,
    mut declared: DeclaredGroup,
    path: &[usize],
) -> Option<DeclaredGroup>
where
    P: Program,
    P::Value: ValueType,
{
    for &at in path {
        let layout = declared_layout::<P>(module, declared, at + 1);
        declared = declared_child::<P>(module, declared, layout, at)?;
    }
    Some(declared)
}

/// Assemble one level of a result structure, driven by the result paths.
///
/// # Invariant
/// A group's arity is one past the highest field any path names, and a position no
/// path names is not built — the structure is exactly what the walk found.
///
/// The group is the declaration's shape: a struct inherits the name table and the
/// nominal id, an array keeps its element template, a tuple has no header
/// (`docs/notes/value-templates.md`).
fn assemble_result<P>(
    module: &mut Module<P>,
    block: BlockId,
    outputs: &[(Vec<usize>, NodeId)],
    prefix: &[usize],
    depth: usize,
    declared: Option<DeclaredGroup>,
) -> Option<NodeId>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    let under = |path: &Vec<usize>| path.len() > depth && path[..depth] == *prefix;
    let arity = outputs
        .iter()
        .filter(|(path, _)| under(path))
        .filter_map(|(path, _)| path.get(depth))
        .max()
        .map(|highest| highest + 1)?;
    let layout = declared.map(|declared| declared_layout::<P>(module, declared, arity));
    let mut items: Vec<ArrayItem> = Vec::with_capacity(arity);
    for at in 0..arity {
        let leaf = outputs
            .iter()
            .find(|(path, _)| under(path) && path.len() == depth + 1 && path[depth] == at);
        let value = match leaf {
            Some((_, value)) => *value,
            None => {
                let mut nested = prefix.to_vec();
                nested.push(at);
                // A group is walked through the declared child it holds.
                let child = match (declared, layout) {
                    (Some(declared), Some(layout)) => {
                        declared_child::<P>(module, declared, layout, at)
                    }
                    _ => None,
                };
                assemble_result(module, block, outputs, &nested, depth + 1, child)?
            }
        };
        // An element is a complete slot: its value, an undecided type, the wildcards.
        let node = AnyNodeId::Dynamic(element_slot(module, block, value)?);
        items.push(ArrayItem::new(node));
    }
    // The group's header: a struct inherits the declaration's, an array its
    // element template, a tuple none.
    match (layout, declared) {
        (Some(CompoundLayout::Struct), Some(declared)) => {
            struct_value_node::<P>(module, block, declared.value, items)
        }
        (Some(CompoundLayout::Array), Some(declared)) => {
            let template =
                shape::value_item(module, declared.value, shape::ARRAY_VALUE_TEMPLATE_SLOT)?;
            let mut all = Vec::with_capacity(items.len() + 1);
            all.push(ArrayItem::new(template));
            all.extend(items);
            Some(array_node::<P>(module, block, &all))
        }
        _ => Some(array_node::<P>(module, block, &items)),
    }
}

/// A struct **value** the compute crate builds: the name table, the nominal id
/// and the fields' own expressions.
///
/// # Invariant
/// The header is the **declaration's own**: a placeholder inherits the name
/// table and the nominal id of the value it stands for, so a body's named read
/// finds the field it names (`docs/notes/value-templates.md`).
fn struct_value_node<P>(
    module: &mut Module<P>,
    block: BlockId,
    declared: AnyNodeId,
    fields: Vec<ArrayItem>,
) -> Option<NodeId>
where
    P: Program,
    P::Value: ValueType + From<LowValue>,
{
    let items = shape::value_items(module, declared)?;
    let names = items.get(shape::STRUCT_VALUE_NAMES_SLOT)?.node;
    let id = items.get(shape::STRUCT_VALUE_ID_SLOT)?.node;
    let mut all = vec![
        ArrayItem::new(AnyNodeId::Dynamic(module.as_dynamic(names, block))),
        ArrayItem::new(AnyNodeId::Dynamic(module.as_dynamic(id, block))),
    ];
    all.extend(fields);
    Some(array_node::<P>(module, block, &all))
}

/// The placeholder a recorded body is applied to, built from the parameter's
/// value and the role paths.
///
/// # Invariant
/// The descent is driven by the declaration's own field list, so a group with no
/// members still builds a placeholder and a position no role path reaches is
/// an empty group; a leaf the walk did not number is a hole the caller refuses.
///
/// The placeholder is a **struct value**: a body's named read goes through the
/// value's own name table (`docs/notes/value-templates.md`).
fn assemble_parameter<P>(
    module: &mut Module<P>,
    block: BlockId,
    value: AnyNodeId,
    cells: &[(Vec<usize>, NodeId)],
    path: &[usize],
) -> Option<NodeId>
where
    P: Program,
    P::Value: ValueType + From<LowValue> + From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    if let Ok(at) = cells.binary_search_by(|(candidate, _)| candidate.as_slice().cmp(path)) {
        return Some(cells[at].1);
    }
    // A group is a struct value; a leaf the walk did not number is a hole the
    // caller refuses.
    if !is_struct_value(module, value) {
        return None;
    }
    let fields = struct_field_exprs(module, value)?;
    let mut items: Vec<ArrayItem> = Vec::with_capacity(fields.len());
    for (at, field) in fields.into_iter().enumerate() {
        let mut nested = path.to_vec();
        nested.push(at);
        // A group is walked through the value the field holds.
        let held = struct_field_value_of_expr(module, field)?;
        let node = assemble_parameter::<P>(module, block, held, cells, &nested)?;
        // An element is a complete slot, like the result's own (`assemble_result`).
        let slot = element_slot(module, block, node)?;
        items.push(ArrayItem::new(AnyNodeId::Dynamic(slot)));
    }
    struct_value_node::<P>(module, block, value, items)
}

/// The `O` structure a run hands back, with every output in its role's place.
///
/// # Invariant
/// A role path starts at `.out` and ends at the buffer's `.native` slot; the result
/// *is* the codomain, so the builder drops both ends.
///
/// The result's **layout** is the declared codomain's own: the declaration says
/// which group is a struct, an array or a tuple, so a named read of the result
/// finds the field it names (`docs/notes/value-templates.md`).
fn build_outputs<P>(
    module: &mut Module<P>,
    block: BlockId,
    roles: &KernelRoles,
    outputs: &[(NodeId, ScalarClass)],
    declared: Option<AnyNodeId>,
) -> Option<P::Value>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    let mut placed: Vec<(Vec<usize>, NodeId)> = Vec::with_capacity(outputs.len());
    for (position, &(payload, class)) in outputs.iter().enumerate() {
        let path = roles.outputs.get(position)?;
        // The paths are the codomain's own: there is no `.out` step to drop.
        let declared_buffer = declared
            .and_then(|value| {
                declared_at_path::<P>(module, DeclaredGroup { value, ty: None }, path)
            })
            .map(|declared| declared.value);
        let value = current_buf_value::<P>(
            module,
            block,
            AnyNodeId::Dynamic(payload),
            class,
            declared_buffer,
        )?;
        placed.push((path.clone(), value));
    }
    let root = assemble_result::<P>(
        module,
        block,
        &placed,
        &[],
        0,
        declared.map(|value| DeclaredGroup { value, ty: None }),
    )?;
    module.node_value(AnyNodeId::Dynamic(root))
}

/// The class a node's value is, read with the kernel's parameter slots in hand.
///
/// # Invariant
/// [`node_class`] answers for a decided value; the two nodes it cannot answer are
/// what a template-lowered body is made of — a parameter read, whose class is the
/// slot's own [`ParamSlot::shape`], and an arithmetic operator over such reads,
/// whose class is its operands' (the emission's own rule, a comparison excepted).
fn node_class_in<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: impl Into<AnyNodeId>,
) -> ScalarClass
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let node = node.into();
    let Some(dynamic) = node.dynamic() else {
        return node_class(module, node);
    };
    if let Ok(Some(offset)) = param_read_offset(module, params, dynamic)
        && let Some(class) = slot_class(params, offset)
    {
        return class;
    }
    // The peel the emission makes: an extraction or a constant selection comes
    // before the operation behind it.
    let peeled = resolve_literal_node(module, node);
    if peeled != node {
        return node_class_in(module, params, peeled);
    }
    // The emission's third answer: a bare cell in an enclosing parameter's
    // equality class is a whole-parameter read.
    if module.node_operation(dynamic).is_none() {
        for slot in params {
            if shape::is_same_value(
                module,
                AnyNodeId::Dynamic(dynamic),
                AnyNodeId::Dynamic(slot.value),
            ) {
                return scalar_class_of(&slot.shape);
            }
        }
    }
    if let Some(operation) = module.node_operation(dynamic)
        && let Some(ty_op) = AsEnum::<TypeOperator>::as_enum(&operation.operator)
        && let Some(bin) = kernel_bin(ty_op)
    {
        // The emission's own rule, read off the same operand array, so the two
        // cannot disagree.
        let Ok((left, right)) = operand_expr(module, operation.operand) else {
            return node_class(module, node);
        };
        let operands = match (
            node_class_in(module, params, left),
            node_class_in(module, params, right),
        ) {
            (ScalarClass::Float, _) | (_, ScalarClass::Float) => ScalarClass::Float,
            _ => ScalarClass::Int,
        };
        return if bin.is_comparison() {
            ScalarClass::Int
        } else {
            operands
        };
    }
    node_class(module, node)
}

/// The class of the `offset`-th wasm local of the kernel's parameter slots.
fn slot_class(params: &[ParamSlot], offset: u32) -> Option<ScalarClass> {
    for slot in params {
        let arity = flat_arity(&slot.shape) as u32;
        if offset >= slot.base as u32 && offset < slot.base as u32 + arity {
            return Some(class_at(&slot.shape, (offset - slot.base as u32) as usize));
        }
    }
    None
}

/// The class of the `index`-th flattened leaf of a domain shape.
fn class_at(shape: &LowShape, index: usize) -> ScalarClass {
    let LowShape::Tuple(items) = shape else {
        return scalar_class_of(shape);
    };
    let mut remaining = index;
    for item in items.iter().flatten() {
        let arity = flat_arity(item);
        if remaining < arity {
            return class_at(item, remaining);
        }
        remaining -= arity;
    }
    ScalarClass::Int
}

/// The class a node's value is, read off the node's own value or low type.
///
/// # Invariant
/// A leaf's own value answers first — the low-type pass declines for a leaf, so its
/// value is the only thing that states its class — while a computed node is the one
/// the transfer answers for. A parameter leaf cannot be answered here: a kernel is
/// lowered from a template, so a caller holding the slots resolves it through them.
fn node_class<P>(module: &Module<P>, node: impl Into<AnyNodeId>) -> ScalarClass
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let node = node.into();
    // A frozen node's structure lives in its own module, so its value alone
    // states a class here.
    let value = match node {
        AnyNodeId::Dynamic(node) => resolve_literal_node(module, node),
        AnyNodeId::Static(_) => node,
    };
    if value
        .dynamic()
        .is_none_or(|node| module.node_operation(node).is_none())
        && let Some(held) = module
            .node_value(value)
            .and_then(|held| AsEnum::<LowValue>::as_enum(&held))
    {
        match held {
            LowValue::USize(_) => return ScalarClass::Int,
            LowValue::Float(_) => return ScalarClass::Float,
            _ => {}
        }
    }
    shape::value_low_shape(module, value).map_or(ScalarClass::Int, |shape| scalar_class_of(&shape))
}

/// The node whose value or low type states a term's class.
///
/// # Invariant
/// The `[value, type]` expr an expression is, the extraction over one, and the
/// element a constant selection picks — walked to the term that states the answer.
/// It is the unwrap [`node_class`] makes at every emit site, named once so there
/// are not two spellings of one walk.
fn resolve_literal_node<P>(module: &Module<P>, node: impl Into<AnyNodeId>) -> AnyNodeId
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let node = node.into();
    // A frozen node is where the walk stops: the structure a peel would read
    // belongs to the module that wrote it.
    let AnyNodeId::Dynamic(node) = node else {
        return node;
    };
    let mut value = AnyNodeId::Dynamic(
        value_of_node(module, node)
            .or_else(|| expr_value(module, node))
            .unwrap_or(node),
    );
    for _ in 0..8 {
        let Some(dynamic) = value.dynamic() else {
            break;
        };
        let Some(element) = concrete_element(module, dynamic) else {
            break;
        };
        value = element;
    }
    value
}

/// The scalar literal a term holds, if it is one.
///
/// # Invariant
/// The value slot answers first, exactly as `Lower::value` reads it: a node the
/// checker concretised carries an operation *and* a value, and asking about the
/// operation first would emit a float value as an integer bit pattern.
fn scalar_literal<P>(module: &Module<P>, node: impl Into<AnyNodeId>) -> Option<LowValue>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let value = resolve_literal_node(module, node);
    match module
        .node_value(value)
        .and_then(|held| AsEnum::<LowValue>::as_enum(&held))?
    {
        held @ (LowValue::USize(_) | LowValue::Float(_)) => Some(held),
        _ => None,
    }
}

/// The kernel-safe reading of a highlevel binary operator, if there is one.
///
/// # Invariant
/// The arithmetic is unsigned because the language's `Int` is: `Div` and `Rem` are
/// the unsigned operations and so are the order comparisons.
/// [`lichen_kernel_ir::KernelBin`] states the choice once for every backend.
fn kernel_bin(operator: TypeOperator) -> Option<KernelBin> {
    Some(match operator {
        TypeOperator::Add => KernelBin::Add,
        TypeOperator::Sub => KernelBin::Sub,
        TypeOperator::Mul => KernelBin::Mul,
        TypeOperator::Div => KernelBin::Div,
        TypeOperator::Rem => KernelBin::Rem,
        TypeOperator::Lt => KernelBin::Lt,
        TypeOperator::Gt => KernelBin::Gt,
        TypeOperator::Leq => KernelBin::Leq,
        TypeOperator::Geq => KernelBin::Geq,
        TypeOperator::Eq => KernelBin::Eq,
        TypeOperator::Neq => KernelBin::Neq,
        TypeOperator::BitAnd => KernelBin::BitAnd,
        TypeOperator::BitOr => KernelBin::BitOr,
        TypeOperator::BitXor => KernelBin::BitXor,
        TypeOperator::Max => KernelBin::Max,
        TypeOperator::Min => KernelBin::Min,
        // Unary: the class conversions and the transcendentals are lowered before this
        // mapping is reached.
        TypeOperator::Int2Float
        | TypeOperator::Float2Int
        | TypeOperator::Exp
        | TypeOperator::Sqrt
        | TypeOperator::Floor
        | TypeOperator::Sin
        | TypeOperator::Cos
        | TypeOperator::Ln => return None,
        // A membership test is a refinement's check, not a computation: the
        // checker registers it as an assert beside the body.
        TypeOperator::InDomain => return None,
        // The same reading of a named read's container kind: a check-time/
        // per-apply *assert*, never a value a kernel computes.
        TypeOperator::IsStructType => return None,
        // The positional read's container kind is the same reading.
        TypeOperator::IsTupleType => return None,
        // The array read's container kind is the same reading.
        TypeOperator::IsArrayType => return None,
        // The array's own layout: the template builds the value and the length
        // reads it, and neither is a binary operation.
        TypeOperator::ArrayTemplate | TypeOperator::ArrayLength => return None,
    })
}

/// The two classes a conversion operator crosses, and the word it is spelled with.
///
/// # Invariant
/// The direction is the operator's and nothing else's: `int2float` in a `Float`
/// fragment and `float2int` in an `Int` one have that class as destination in one
/// case and source in the other, so inferring it would swap the two programs.
fn conv_of(operator: TypeOperator) -> Option<(ScalarClass, ScalarClass, &'static str)> {
    match operator {
        TypeOperator::Int2Float => Some((ScalarClass::Int, ScalarClass::Float, "int2float")),
        TypeOperator::Float2Int => Some((ScalarClass::Float, ScalarClass::Int, "float2int")),
        _ => None,
    }
}

/// The class a unary prefix operator computes in, if the kernel emitter lowers one directly.
///
/// # Invariant
/// Every one of them is `Float`'s alone; the two conversions are not here because their own
/// lowering answers a literal in the language's classes before any instruction is emitted.
fn unary_class(operator: TypeOperator) -> Option<ScalarClass> {
    match operator {
        TypeOperator::Exp
        | TypeOperator::Sqrt
        | TypeOperator::Floor
        | TypeOperator::Sin
        | TypeOperator::Cos
        | TypeOperator::Ln => Some(ScalarClass::Float),
        _ => None,
    }
}

/// The buffer position `node` names in a parallel kernel.
///
/// # Invariant
/// A struct parameter declares its inputs, so the node's own [`param_path`] is the
/// answer; a `(n, (buffers…))` parameter declares nothing, so the position is the
/// constant the body wrote. `Err` is [`param_path`]'s — a read whose index is not
/// constant — and is not the "not an input" refusal.
fn parallel_buffer_pos<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: impl Into<AnyNodeId>,
) -> Result<Option<usize>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(slot) = params.first() else {
        return Ok(None);
    };
    // A position is a read of the parameter, the caller's own node: a frozen node
    // names no position.
    let Some(node) = node.into().dynamic() else {
        return Ok(None);
    };
    if let Some(roles) = &slot.roles {
        // The peeled node names the buffer; the wrapper's own projection chain does not.
        let path = param_path(module, slot.expr, node)?;
        let Some(path) = path else {
            return Ok(None);
        };
        let mut as_field = vec![0];
        as_field.extend(path.iter().copied());
        if let Some(position) = roles.input_pos(&as_field) {
            return Ok(Some(position));
        }
        return Ok(roles.input_pos(&path));
    }
    let cfg_value = slot.value;
    let operation = match module.node_operation(node) {
        Some(operation) => operation,
        None => return Ok(None),
    };
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&operation.operator),
        Some(LowOperator::Index)
    ) {
        return Ok(None);
    }
    let Some((target, index)) = operand_expr(module, operation.operand).ok() else {
        return Ok(None);
    };
    let Some(k) = usize_value(module, index) else {
        return Ok(None);
    };
    let Some(target) = target.dynamic() else {
        return Ok(None);
    };
    let Some(target_op) = module.node_operation(target) else {
        return Ok(None);
    };
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&target_op.operator),
        Some(LowOperator::Index)
    ) {
        return Ok(None);
    }
    let Some((tt, ti)) = operand_expr(module, target_op.operand).ok() else {
        return Ok(None);
    };
    // The body's cfg reads go through `Index(cfg_expr, 0)`: compare the two
    // slots by equality class, not node identity.
    let Some(tt) = tt.dynamic() else {
        return Ok(None);
    };
    if !shape::is_same_value(
        module,
        AnyNodeId::Dynamic(tt),
        AnyNodeId::Dynamic(cfg_value),
    ) || usize_value(module, ti) != Some(1)
    {
        return Ok(None);
    }
    Ok(Some(k))
}

/// The output position `node` names in a parallel kernel.
///
/// # Invariant
/// The write's counterpart to [`parallel_buffer_pos`]: the peeled node's own
/// [`param_path`], against the `.out` paths declared. No fallback route and no
/// positional guess — a node naming an input, a scalar, or no field is `Ok(None)`
/// and the caller's refusal says what *is* declared. `Err` is [`param_path`]'s.
fn parallel_output_pos<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: impl Into<AnyNodeId>,
) -> Result<Option<usize>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(slot) = params.first() else {
        return Ok(None);
    };
    let Some(roles) = &slot.roles else {
        return Ok(None);
    };
    // A position is a read of the parameter, the caller's own node: a frozen node
    // names no position.
    let Some(node) = node.into().dynamic() else {
        return Ok(None);
    };
    // The peeled node names the buffer; the wrapper's own projection chain does not.
    let Some(path) = param_path(module, slot.expr, node)? else {
        return Ok(None);
    };
    Ok(roles.output_pos(&path))
}

/// The output position a write's target names, resolved through a chain of writes.
///
/// # Invariant
/// A `compute.write` is the value of an output position, not an effect
/// ([code-audit D17](../../../docs/notes/code-audit.md)), so a target that *is* a
/// write names the position that write named, and a chain of them names one output
/// written as many times as the chain is long. The walk is bounded like
/// [`write_item_node`]'s: a deeper chain is not one this resolution understands.
fn write_chain_output_pos<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: impl Into<AnyNodeId>,
) -> Result<Option<usize>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut current = node.into();
    for _ in 0..8 {
        let peeled = peeled_argument(module, current)?;
        let Some(dynamic) = peeled.dynamic() else {
            return Ok(None);
        };
        match write_target_node(module, dynamic) {
            Some(target) => current = target,
            None => return parallel_output_pos(module, params, peeled),
        }
    }
    Ok(None)
}

/// The node a wrapped slot-read argument reaches.
///
/// # Invariant
/// A buffer operand arrives through the wrapper's slot-read destructuring
/// (`read = x => $read(x(0), x(1))` over `[k.in.x, i]` leaves `Index(arg, 0)`), so
/// constant `Index` layers are peeled to the element the author named. Bounded,
/// not recursive: a deeper chain is not a wrapper this walk understands.
fn peeled_argument<P>(module: &Module<P>, node: impl Into<AnyNodeId>) -> Result<AnyNodeId, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut value = node.into();
    for _ in 0..8 {
        let Some(value_node) = value.dynamic() else {
            break;
        };
        let target_oi = match module.node_operation(value_node).as_ref() {
            Some(op)
                if matches!(
                    AsEnum::<LowOperator>::as_enum(&op.operator),
                    Some(LowOperator::Index)
                ) =>
            {
                operand_expr(module, op.operand).ok()
            }
            _ => None,
        };
        let Some((target, index)) = target_oi else {
            break;
        };
        let Some(k) = usize_value(module, index) else {
            break;
        };
        let Some(target) = target.dynamic() else {
            break;
        };
        let Some(array_value) = value_of_node(module, target).or(Some(target)) else {
            break;
        };
        // SAFETY: `array_value` is a live node of `module`.
        let Some(items) = (unsafe { module.array_items(array_value) }) else {
            break;
        };
        let Some(item) = items.get(k) else { break };
        value = item.node;
    }
    // An element is a complete slot: what the read names is its value position
    // (`docs/notes/compound-element-slots.md`).
    Ok(module.element_value_node(value).unwrap_or(value))
}

/// The constant `USize` value behind `node`, if it is one.
///
/// # Invariant
/// A frozen node answers too: a selector the callee's body wrote is a reference
/// into the frozen module, and its value is the constant it is.
fn usize_value<P>(module: &Module<P>, node: impl Into<AnyNodeId>) -> Option<usize>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    match module
        .node_value(node.into())
        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
    {
        Some(LowValue::USize(n)) => Some(n),
        _ => None,
    }
}

/// A `[value, type]` expr's value half, or the node itself when it is not an expr.
fn param_value_of<P>(module: &Module<P>, expr: NodeId) -> Result<NodeId, String>
where
    P: Program,
{
    Ok(expr_value(module, expr).unwrap_or(expr))
}

/// The name a struct field read selects: the compile-time constant a named read
/// carries.
///
/// # Invariant
/// A named read `a.name` is `TableGet(name-table, "name")`, so the name is what a
/// resolution walks the type with; `Ok(None)` for a selector that is not a named
/// read at all.
fn field_name<P>(
    module: &Module<P>,
    selector: impl Into<AnyNodeId>,
) -> Result<Option<&'static str>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let selector = selector.into();
    if usize_value(module, selector).is_some() {
        return Ok(None);
    }
    let Some(selector) = selector.dynamic() else {
        return Ok(None);
    };
    // **A named read arrives in two shapes**: `TableGet(names, "name")`, or `Index`
    // carrying the name as its selector.
    let key = match module.node_operation(selector) {
        Some(operation) => match AsEnum::<LowOperator>::as_enum(&operation.operator) {
            Some(LowOperator::TableGet) => operand_expr(module, operation.operand)?.1,
            // A computed index is a conditional's selector, not a field read.
            _ => return Ok(None),
        },
        // **No operation: the selector is the name itself.**
        None => AnyNodeId::Dynamic(selector),
    };
    Ok(module
        .node_value(key)
        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
        .and_then(|v| match v {
            LowValue::Str(name) => Some(name),
            _ => None,
        }))
}

/// One level of a parameter read's index: the position, or the name, it selects.
///
/// # Invariant
/// The two forms are resolved against different things: a position is its own
/// answer, while a name is looked up in the field list of the type that names the
/// level — and only the whole chain says which type that is.
enum IndexStep {
    Position(usize),
    Named(&'static str),
}

/// The positional index path from the parameter to the value `node` reads.
///
/// # Invariant
/// `Ok(None)` is "not a parameter read"; `Err` is a read whose index is not a
/// compile-time constant — the undetermined type a kernel compile refuses, since it
/// runs on a concrete instantiation.
pub(crate) fn param_path<P>(
    module: &Module<P>,
    param_expr: NodeId,
    node: NodeId,
) -> Result<Option<Vec<usize>>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // Two passes: collect the steps along the chain, then resolve them against
    // the parameter's type, outside in.

    // A named read's index belongs to a type only the whole chain names, so a
    // level resolved as met would have to guess it.
    let mut steps: Vec<(IndexStep, bool, bool)> = Vec::new();
    let mut current = node;
    // **This walk cannot answer a struct field read**: the parameter is joined to the read
    // only by the equality class.
    for _ in 0..MAX_PARAMETER_DEPTH {
        let Some(operation) = module.node_operation(current) else {
            // **A bare value cell ends the chain, it does not void it**: a whole-parameter
            // read stops there too.
            break;
        };
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Index)
        ) {
            return Ok(None);
        }
        let Ok((target, selector)) = operand_expr(module, operation.operand) else {
            return Ok(None);
        };
        // A frozen node is not a step of a read of *this* parameter: the chain a
        // path walks is the caller's own.
        let Some(target_node) = target.dynamic() else {
            return Ok(None);
        };
        if target == AnyNodeId::Dynamic(param_expr) {
            // The expr's value half is the parameter itself, not a field, so the
            // peel contributes no step.
            match expr_value(module, param_expr) {
                Some(value) => {
                    current = value;
                    continue;
                }
                // An expr with no value half has nothing inside to walk, and the
                // path so far is what the caller's read reached.
                None => break,
            }
        }
        // A slot's value position is no path step: its second position is open.
        let dereference = matches!(
            module.class_low_type(target_node),
            Some(lichen_lowlevel::LowShape::Tuple(items))
                if items.len() == 2 && items[1].is_none()
        );
        // The selector is a *name* when the read was written `a.name`, and a
        // *position* when it was written `a(0)`.
        match field_name(module, selector)? {
            Some(name) => steps.push((IndexStep::Named(name), dereference, false)),
            None => match usize_value(module, selector) {
                Some(position) => {
                    steps.push((IndexStep::Position(position), dereference, position == 0))
                }
                None => return Ok(None),
            },
        }
        // The innermost read's target is the parameter's value; **the test is the equality
        // class**.
        if module.class_root(target_node) == module.class_root(param_value_of(module, param_expr)?)
        {
            break;
        }
        current = target_node;
    }
    if steps.is_empty() {
        return Ok(None);
    }
    // A constant `0` at an odd distance from the outer end is the slot's value read.
    let count = steps.len();
    let kept: Vec<IndexStep> = steps
        .into_iter()
        .enumerate()
        .rev()
        .filter_map(|(index, (step, deref, zero))| {
            let odd = (count - 1 - index) % 2 == 1;
            (!deref && !(zero && odd)).then_some(step)
        })
        .collect();
    resolve_steps(module, param_expr, &kept)
}

/// Resolve a read's collected steps against the parameter's value, outermost
/// first.
///
/// # Invariant
/// A struct's layout lives in its value, so a name is looked for in the value's
/// own name table and the next level is the value the step before it selected.
fn resolve_steps<P>(
    module: &Module<P>,
    param_expr: NodeId,
    steps: &[IndexStep],
) -> Result<Option<Vec<usize>>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(value) = expr_value(module, param_expr).map(AnyNodeId::Dynamic) else {
        // A scalar parameter has no field list and a read of it is the value: the
        // empty path.
        return resolve_without_type(steps);
    };
    // One level per step, each named by the value the step before it selected.

    // A name is looked for in that value's own name table.
    let mut current = Some(value);
    let mut path = Vec::with_capacity(steps.len());
    for step in steps {
        let at = match step {
            // A position is its own answer, so it is taken where no value reads.
            IndexStep::Position(position) => *position,
            IndexStep::Named(name) => {
                let names = current.and_then(|value| struct_field_names(module, value));
                match names
                    .as_ref()
                    .and_then(|names| names.iter().position(|field| *field == Some(*name)))
                {
                    Some(at) => at,
                    None => {
                        return Err(format!(
                            "a struct parameter field read names `{name}`, which is not a field of \
                             the struct it is read from — that struct's fields are {names:?}"
                        ));
                    }
                }
            }
        };
        path.push(at);
        current = current.and_then(|value| struct_field_value(module, value, at));
    }
    Ok(Some(path))
}

/// Resolve a read whose parameter states no field list — a scalar domain.
///
/// # Invariant
/// Only the constant form can be placed here, since a name has nothing to be looked
/// up in; the resulting path is the positions themselves.
fn resolve_without_type(steps: &[IndexStep]) -> Result<Option<Vec<usize>>, String> {
    let mut path = Vec::with_capacity(steps.len());
    for step in steps {
        match step {
            IndexStep::Position(position) => path.push(*position),
            IndexStep::Named(name) => {
                return Err(format!(
                    "a struct parameter field read names `{name}`, which is not a field of the \
                     struct it is read from"
                ));
            }
        }
    }
    Ok(Some(path))
}

/// Follow a `value_of` extraction — `Index(expr, 0)` — to the expr's value slot.
///
/// # Invariant
/// The checker accesses most values through such an extraction, so the JIT must look
/// through it to reach the actual value: a constant, a parameter read or a
/// computation.
fn value_of_node<P>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let operation = module.node_operation(node)?;
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&operation.operator),
        Some(LowOperator::Index)
    ) {
        return None;
    }
    let (target, index) = operand_expr(module, operation.operand).ok()?;
    if usize_value(module, index)? != 0 {
        return None;
    }
    // A frozen target is where this peel stops: only a value crosses.
    let target = target.dynamic()?;
    // A concrete `[value, type]` expr value → its value slot (element 0).
    // SAFETY: `target` is a live node of `module`.
    if let Some(items) = unsafe { module.array_items(target) } {
        return dyn_node(items.first()?.node).ok();
    }
    // An operator node as the target (a peeled call result): emit the operator.
    if module.node_operation(target).is_some() {
        return Some(target);
    }
    None
}

/// The **value** a struct value's first field holds.
///
/// # Invariant
/// Only a struct value has fields: a struct value's fields start after the two
/// header slots, and a field is a complete expression, so the value a field
/// holds is the one the expression keeps in its own value position
/// (`docs/notes/value-templates.md`).
fn first_field_value<P>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: ValueType,
{
    if !is_struct_value(module, AnyNodeId::Dynamic(node)) {
        return None;
    }
    // SAFETY: `node` is a live node of `module`.
    let items = (unsafe { module.array_items(node) })?;
    let field = items.get(shape::STRUCT_VALUE_FIELD_BASE)?.node;
    slot_value_of(module, field).and_then(|node| node.dynamic())
}

/// The [`KernelId`] of a kernel-value node, through `value_of` and `.native` reads.
fn kernel_id_of<P>(module: &Module<P>, node: NodeId) -> Option<KernelId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    if let Some(value) = module.node_value(AnyNodeId::Dynamic(node))
        && let Some(ComputeValue::Kernel(kid)) = AsEnum::<ComputeValue>::as_enum(&value)
    {
        return Some(kid);
    }
    if let Some(inner) = value_of_node(module, node)
        && let Some(kid) = kernel_id_of(module, inner)
    {
        return Some(kid);
    }
    // A kernel struct value reached by value: its first **field** is the bare
    // artifact.
    if let Some(first) = first_field_value::<P>(module, node)
        && let Some(kid) = kernel_id_of(module, first)
    {
        return Some(kid);
    }
    // A kernel struct `.native` read: `Index(struct, 2)`, the bare artifact.
    if let Some(operation) = module.node_operation(node)
        && let Some(LowOperator::Index) = AsEnum::<LowOperator>::as_enum(&operation.operator)
        && let Ok((target, index)) = operand_expr(module, operation.operand)
        && usize_value(module, index) == Some(shape::STRUCT_VALUE_FIELD_BASE)
        && let Some(target) = target.dynamic()
        && let Some(first) = first_field_value::<P>(module, target)
    {
        return kernel_id_of(module, first);
    }
    None
}

/// Whether `node` is a kernel in the making — the undecided half of
/// [`kernel_id_of`].
///
/// # Invariant
/// It follows the same two value edges `kernel_id_of` does, and its answer is a
/// superset: it can keep a callee lazy that turns out not to be a kernel, and never
/// refuses one the JIT would lower.
fn pending_kernel<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // The one operator that produces a `Kernel`; a lazy value here is a function
    // the pass has not run yet.
    if module.node_operation(node).is_some_and(|operation| {
        matches!(
            AsEnum::<ComputeOperator>::as_enum(&operation.operator),
            Some(ComputeOperator::Jit)
        )
    }) {
        return true;
    }
    if let Some(inner) = value_of_node(module, node)
        && pending_kernel(module, inner)
    {
        return true;
    }
    if let Some(first) = first_field_value::<P>(module, node)
        && pending_kernel(module, first)
    {
        return true;
    }
    false
}

/// The value node behind a `[value, type]` expr stored as a concrete array value.
///
/// # Invariant
/// The checker stores some argument values as such exprs, where [`value_of_node`]
/// handles the `Index(expr, 0)` form; the JIT must look through to the value before
/// emitting.
/// Flatten a parameter index `path` to a wasm local index, using the domain
/// `shape`.
fn flatten_offset(domain: &LowShape, path: &[usize]) -> Result<usize, String> {
    let mut offset = 0;
    let mut cur = domain;
    for &i in path {
        match cur {
            LowShape::Tuple(items) => {
                if i >= items.len() {
                    return Err(format!("parameter index {i} out of bounds"));
                }
                for item in items[..i].iter().flatten() {
                    offset += flat_arity(item);
                }
                cur = items[i].as_ref().unwrap_or(domain);
            }
            LowShape::USize => {
                // Descending into a scalar is a type error the checker should have
                // caught.
                return Err("index into a scalar parameter".into());
            }
            _ => return Err("unsupported parameter domain shape".into()),
        }
    }
    Ok(offset)
}

/// One scalar crossing a kernel's ABI: the class it is and the bits it takes.
///
/// # Invariant
/// The class travels with the value because the two are the same 64 bits to
/// everything downstream, and only the class says whether an `i64` or an `f32` is
/// meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScalarValue {
    class: ScalarClass,
    /// An `Int`'s own value, or an `f32`'s bits in the low 32 bits.
    bits: i64,
}

impl ScalarValue {
    fn int(value: i64) -> Self {
        ScalarValue {
            class: ScalarClass::Int,
            bits: value,
        }
    }

    fn float(value: f32) -> Self {
        ScalarValue {
            class: ScalarClass::Float,
            bits: float_bits(value),
        }
    }

    /// The value as the language's own scalar: an integer is the `USize` it
    /// always was, an `f32` its `Float`.
    ///
    /// # Invariant
    /// A bf16 reads as the `Float` the half is the top of, at its own sixteen bits:
    /// the low half is zero, so the number is the one the class holds.
    fn low(self) -> LowValue {
        match self.class {
            ScalarClass::Int => LowValue::USize(self.bits as usize),
            ScalarClass::Float => LowValue::Float(f32::from_bits(self.bits as u32)),
            ScalarClass::BFloat16 => LowValue::Float(f32::from_bits(self.bits as u32 & !0xFFFF)),
        }
    }
}

/// Flatten a kernel argument value into the wasm argument vector, each leaf with its
/// own class.
///
/// # Invariant
/// `Err` names the first element that is not a scalar leaf: the definition pass
/// reports the undecided result, but only this says which element was unusable. The
/// path is the offending element's position, built as the walk descends.
fn collect_args<P>(
    module: &Module<P>,
    node: AnyNodeId,
    out: &mut Vec<ScalarValue>,
    path: &str,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // A compound's element is a complete slot: its value is the position read.
    let Some(node) = module.element_value_node(node) else {
        return Err("the argument holds no value".to_string());
    };
    match module
        .node_value(node)
        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
    {
        Some(LowValue::USize(n)) => {
            out.push(ScalarValue::int(n as i64));
            Ok(())
        }
        Some(LowValue::Float(f)) => {
            out.push(ScalarValue::float(f));
            Ok(())
        }
        Some(LowValue::Array(arr)) => {
            // SAFETY: `arr` is the payload of a value read from a live node of
            // `module`.
            for (index, item) in unsafe { arr.items() }.into_iter().enumerate() {
                let element_path = if path.is_empty() {
                    index.to_string()
                } else {
                    format!("{path}.{index}")
                };
                collect_args(module, item.node, out, &element_path)?;
            }
            Ok(())
        }
        value => {
            let kind = argument_kind(value.as_ref());
            // At the root there is no position to name: this arm is the whole
            // argument, not one of its elements.
            let refusal = if path.is_empty() {
                format!("the argument is {kind}")
            } else {
                format!("argument element {path} is {kind}")
            };
            Err(format!(
                "{refusal}, not a concrete Int or Float, so the kernel's parameters could not be filled"
            ))
        }
    }
}

/// Whether every element of a kernel argument holds a value.
///
/// # Invariant
/// A value that is present is **decided**, even when it is the wrong class: the refusal names
/// that case, and only an empty position is undecided.
fn argument_decided<P>(module: &Module<P>, node: AnyNodeId) -> bool
where
    P: Program,
    P::Value: AsEnum<LowValue>,
{
    let Some(value) = module.node_value(node) else {
        return false;
    };
    let Some(low) = AsEnum::<LowValue>::as_enum(&value) else {
        return true;
    };
    let LowValue::Array(array) = low else {
        return true;
    };
    // SAFETY: the array is a value of a live node of `module`.
    let items = unsafe { array.items() };
    match array.layout() == 0 {
        true => items.iter().all(|item| argument_decided(module, item.node)),
        // A slot states its value in its first position.
        false => items
            .first()
            .is_some_and(|item| argument_decided(module, item.node)),
    }
}

/// The wasm argument vector for a `launch`/`call`, or the refusal to record.
///
/// # Invariant
/// The one place a kernel's argument is read out of the program, so both run
/// operators say the same thing about the same argument: a scalar is one value, a
/// tuple is flattened, and anything else is named by what the user wrote.
fn kernel_arguments<P>(module: &Module<P>, node: AnyNodeId) -> Result<Vec<ScalarValue>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut args: Vec<ScalarValue> = Vec::new();
    match module
        .node_value(node)
        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
    {
        Some(LowValue::USize(n)) => args.push(ScalarValue::int(n as i64)),
        Some(LowValue::Float(f)) => args.push(ScalarValue::float(f)),
        Some(LowValue::Array(_)) => collect_args(module, node, &mut args, "")?,
        argument => {
            return Err(format!(
                "the argument must be a concrete Int or a tuple of them (the kernel's parameter \
                 domain), and a concrete Float where the domain is one, but this one is {}",
                argument_kind(argument.as_ref())
            ));
        }
    }
    Ok(args)
}

/// What a launch argument that is not a parameter vector looks like, in the terms
/// the user wrote.
///
/// # Invariant
/// `USize` and `Array` are the two shapes a parameter vector may take and are
/// recognised first, so the last arm covers a node that carries no value at all.
fn argument_kind(value: Option<&LowValue>) -> &'static str {
    match value {
        // A float is a concrete scalar, so it is named as one: the refusal has to
        // say what the user wrote.
        Some(LowValue::Float(_)) => "a float",
        Some(LowValue::Str(_)) => "a string",
        Some(LowValue::Table(_)) => "a table",
        Some(LowValue::Function(_)) => "a function",
        Some(LowValue::None) => "the unit value",
        Some(LowValue::Error) => "nothing (the empty value of a failed read)",
        _ => "no value",
    }
}

/// What a node's value is, for a refusal that has to say — across both
/// vocabularies.
///
/// # Invariant
/// The value is a [`ComputeValue`] when it is a buffer and a [`LowValue`]
/// otherwise, so a refusal that named only one would say "no value" about an
/// ordinary array.
fn what_this_is<P>(module: &Module<P>, node: AnyNodeId) -> &'static str
where
    P: Program,
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue>,
{
    if let Some(value) = module
        .node_value(node)
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
    {
        return graph::describe(&value);
    }
    module
        .node_value(node)
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
        .as_ref()
        .map_or("no value", |value| match value {
            LowValue::Array(_) => "an array",
            LowValue::USize(_) => "a number",
            other => argument_kind(Some(other)),
        })
}

/// One collected element, materialized as a fresh scalar node of `block`.
///
/// # Invariant
/// This is the one place that turns one element into one node, so the two classes
/// cannot drift into two slightly different constructions: a collected array is
/// homogeneous, and the class is only known at run time.
///
/// The node is the element's **slot** and not the bare value: an array's element
/// read takes the element's value out of its slot
/// (`docs/notes/compound-element-slots.md`).
fn scalar_item<P>(module: &mut Module<P>, block: BlockId, element: LowValue) -> Option<ArrayItem>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    let value = module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(element)),
    );
    let slot = element_slot(module, block, value)?;
    Some(ArrayItem::new(AnyNodeId::Dynamic(slot)))
}

/// A buffer position holding something that is not a buffer.
///
/// # Invariant
/// A refusal: the undecided answer it replaces was a silent no-op. A program array
/// *is* decided, and it is not a buffer — `compute.buffer` is what makes one out of
/// it, so the honest answer names the operation rather than waiting for a buffer that
/// will not arrive.
fn not_a_buffer<P>(
    module: &mut Module<P>,
    node: AnyNodeId,
    subject: &str,
    at: &str,
) -> Option<P::Value>
where
    P: Program,
    P::Value: From<LowValue> + AsEnum<ComputeValue> + AsEnum<LowValue>,
{
    module.record_extension_diagnostic(ExtensionDiagnostic::failure(
        PARALLEL_DIAGNOSTIC,
        None,
        format!(
            "{subject}, and {at} holds {}. A buffer is what a `compute.plrun` or a \
             `compute.graphrun` hands back, or what `compute.buffer` makes of an array",
            what_this_is(module, node)
        ),
    ));
    None
}

/// The element class of an array's items and their words, or the refusal that stopped it.
///
/// # Invariant
/// The class is the array's own element class, read off an element rather than declared, so
/// an `Int` array is an `Int` buffer and a `Float` array a `Float` one. A position that is
/// not a scalar of the class the first position fixed is refused by name: a buffer's
/// elements are homogeneous, so there is no encoding for a mixture or for a compound.
fn buffer_words<P>(
    module: &mut Module<P>,
    elements: &[ArrayItem],
) -> Option<(ScalarClass, Vec<i64>)>
where
    P: Program,
    P::Value: AsEnum<LowValue>,
{
    let mut class: Option<ScalarClass> = None;
    let mut words = Vec::with_capacity(elements.len());
    for (position, item) in elements.iter().enumerate() {
        // An element is a complete slot: its value is the position read.
        let Some(item) = module.element_value_node(item.node) else {
            return None;
        };
        let Some(value) = module
            .node_value(item)
            .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
        else {
            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                BUFFER_DIAGNOSTIC,
                None,
                format!(
                    "argument element {position} is undecided, and `compute.buffer` copies \
                     elements whose class it can read: give the array a value before it is \
                     handed over"
                ),
            ));
            return None;
        };
        let (position_class, word) = match value {
            LowValue::USize(word) => (ScalarClass::Int, word as i64),
            LowValue::Float(word) => (ScalarClass::Float, float_bits(word)),
            _ => {
                module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                    BUFFER_DIAGNOSTIC,
                    None,
                    format!(
                        "argument element {position} is not a scalar, and a buffer's elements \
                         are: an `Int` array becomes an `Int` buffer and a `Float` array a \
                         `Float` one"
                    ),
                ));
                return None;
            }
        };
        match class {
            None => class = Some(position_class),
            Some(fixed) if fixed != position_class => {
                module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                    BUFFER_DIAGNOSTIC,
                    None,
                    format!(
                        "argument element {position} is {} where the first element is {}, and \
                         a buffer's elements are one class: `compute.buffer` copies an array \
                         of one element class",
                        class_name(position_class),
                        class_name(fixed)
                    ),
                ));
                return None;
            }
            Some(_) => {}
        }
        words.push(word);
    }
    // An empty array states no element: the ABI's integer default, as `element_class`
    // gives a fragment that declared none.
    Some((class.unwrap_or(ScalarClass::Int), words))
}

/// A class as a refusal spells it.
fn class_name(class: ScalarClass) -> &'static str {
    match class {
        ScalarClass::Int => "Int",
        ScalarClass::Float => "Float",
        ScalarClass::BFloat16 => "BFloat16",
    }
}

/// The diagnostic category a cooperative matrix refusal is reported under.
const MMA_DIAGNOSTIC: &str = "compute.mma";

/// One cooperative matrix operand, as the words its buffer holds, or a named refusal.
///
/// # Invariant
/// The elements are `Int` words inside the matrix's own component range: a byte operand
/// outside `s8` would be truncated by the module and a word outside `s32` wrapped, so the
/// range is checked rather than masked.
fn mma_words<P>(
    module: &mut Module<P>,
    node: Option<AnyNodeId>,
    count: u32,
    min: i64,
    max: i64,
) -> Option<Vec<i64>>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
{
    let Some(node) = node else {
        return None;
    };
    let Some(payload) = buf_payload::<P>(module, node) else {
        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
            MMA_DIAGNOSTIC,
            None,
            format!(
                "`compute.mma` takes buffers, and this operand is {}",
                what_this_is(module, node)
            ),
        ));
        return None;
    };
    let value = module.node_value(payload)?;
    let Some(ComputeValue::Buffer(payload, class)) = AsEnum::<ComputeValue>::as_enum(&value) else {
        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
            MMA_DIAGNOSTIC,
            None,
            "a cooperative matrix operand is still on the device: collect it first".to_string(),
        ));
        return None;
    };
    if class != ScalarClass::Int {
        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
            MMA_DIAGNOSTIC,
            None,
            "a cooperative matrix operand must be a buffer of integers".to_string(),
        ));
        return None;
    }
    let items = buffer_items(&payload)?;
    let elements = element_count(class, items.len());
    if elements != count as usize {
        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
            MMA_DIAGNOSTIC,
            None,
            format!(
                "a cooperative matrix operand holds {elements} element(s), and its shape takes {count}"
            ),
        ));
        return None;
    }
    let mut words = Vec::with_capacity(elements);
    for index in 0..elements {
        let Some(bytes) = element_bytes(class, items, index) else {
            return None;
        };
        let Some(LowValue::USize(word)) = element_value(class, bytes) else {
            return None;
        };
        let word = word as i64;
        if !(min..=max).contains(&word) {
            module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                MMA_DIAGNOSTIC,
                None,
                format!(
                    "a cooperative matrix operand holds {word}, outside the shape's component range"
                ),
            ));
            return None;
        }
        words.push(word);
    }
    Some(words)
}

/// The host's cooperative matrix product, in the same two's-complement words the device
/// answers.
///
/// # Invariant
/// The sum is taken in `i64` and kept to the low 32 bits: the device's accumulator is `s32`,
/// so wrapping there and truncating here are the same answer.
fn mma_product(a: &[i64], b: &[i64], c: &[i64]) -> Vec<i64> {
    let rows = lichen_kernel_ir::COOPERATIVE_MATRIX_M as usize;
    let steps = lichen_kernel_ir::COOPERATIVE_MATRIX_K as usize;
    let columns = lichen_kernel_ir::COOPERATIVE_MATRIX_N as usize;
    let mut product = vec![0i64; rows * columns];
    for row in 0..rows {
        for column in 0..columns {
            let mut sum = c[row * columns + column];
            for step in 0..steps {
                sum += a[row * steps + step] * b[step * columns + column];
            }
            product[row * columns + column] = i64::from(sum as i32);
        }
    }
    product
}

/// A `compute.buffer` handed something that is not an array.

fn not_an_array<P>(module: &mut Module<P>, node: AnyNodeId) -> Option<P::Value>
where
    P: Program,
    P::Value: From<LowValue> + AsEnum<ComputeValue> + AsEnum<LowValue>,
{
    module.record_extension_diagnostic(ExtensionDiagnostic::failure(
        BUFFER_DIAGNOSTIC,
        None,
        format!(
            "`compute.buffer` copies an array's elements, and its argument holds {}",
            what_this_is(module, node)
        ),
    ));
    None
}

/// The value a completed kernel run produces: the bare scalar, or the tuple of them.
///
/// # Invariant
/// Each result becomes a node of `block` first, so the array holds live nodes — the
/// same shape `ParLaunch` builds — and each result is the class the run said it was.
fn kernel_results_value<P>(
    module: &mut Module<P>,
    block: BlockId,
    results: Vec<ScalarValue>,
) -> Option<P::Value>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let scalar = |value: &ScalarValue| <P::Value as From<LowValue>>::from(value.low());
    let Some((first, rest)) = results.split_first() else {
        // A fragment always leaves at least one value: stay lazy for an empty run.
        return None;
    };
    if rest.is_empty() {
        // The single-result form is the bare scalar, exactly what it always was.
        return Some(scalar(first));
    }
    let mut items: Vec<ArrayItem> = Vec::with_capacity(results.len());
    for value in &results {
        let node = module.add_node(block, None, Some(scalar(value)));
        // An element is a complete slot: its value, an undecided type, the wildcards.
        let slot = element_slot(module, block, node)?;
        items.push(ArrayItem::new(AnyNodeId::Dynamic(slot)));
    }
    let handle = module.alloc_array(&items, block);
    Some(<P::Value as From<LowValue>>::from(LowValue::Array(
        handle.into(),
    )))
}

/// Read a binary op/`Index` operand array `[a, b]` as two operand nodes.
///
/// # Invariant
/// An operand may be a node of a frozen module: an apply of a static function leaves
/// the callee's unchanged subterms as references into it, so reading an operand is
/// not a question about which module it is in.
fn operand_expr<P>(
    module: &Module<P>,
    operand: Option<NodeId>,
) -> Result<(AnyNodeId, AnyNodeId), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(operand) = operand else {
        return Err("binary operator/Index operand is missing".into());
    };
    let items = operand_items(module, operand)?;
    if items.len() != 2 {
        return Err("binary/Index operand array must have two elements".into());
    }
    Ok((items[0].node, items[1].node))
}

fn operand_items<P>(module: &Module<P>, node: NodeId) -> Result<&'static [ArrayItem], String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // SAFETY: `node` is a live node of `module`.
    unsafe { module.array_items(node) }.ok_or_else(|| "operand is not an array value".into())
}

/// One runtime scalar a recorded dispatch fixes beside its extent, read at `path`.
///
/// # Invariant
/// **Named at the point it is missing.** A scalar leaf has to be a number the
/// build already has or one of this function's arguments, exactly as an extent
/// does, so anything else is refused here rather than left for the run to meet as
/// a buffer read from a number. `ordinal` is the leaf's own place in the
/// parameter's scalar leaves, which is where its class is read.
fn recorded_scalar<P>(
    module: &mut Module<P>,
    cfg: NodeId,
    path: &[usize],
    fragment: &KernelFragment,
    ordinal: usize,
) -> Option<graph::Scalar>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let refuse = |module: &mut Module<P>, reason: String| {
        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
            GRAPH_DIAGNOSTIC,
            None,
            reason,
        ));
    };
    let node = match value_at_path::<P>(module, cfg, path) {
        Some(node) => node,
        None => {
            refuse(
                module,
                format!(
                    "a dispatch's scalar leaf at parameter position {} is not a value at its own \
                     path in the argument, so there is nothing to record for it",
                    path_text(path)
                ),
            );
            return None;
        }
    };
    // The class the parameter field declared, so the number crosses at the leaf's width.
    let class = fragment
        .param_shape
        .leaf_classes()
        .get(ordinal)
        .copied()
        .unwrap_or(ScalarClass::Int);
    let found = module
        .node_value(node)
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
        .and_then(|value| match value {
            ComputeValue::GraphInput(slot) => Some(graph::Placed::Input(slot)),
            ComputeValue::GraphValue(id) => Some(graph::Placed::Value(id)),
            _ => None,
        });
    if let Some(placed) = found {
        // Carried for what it is: the class travels with the value, and the run is
        // the only point that sees what the value is.
        return Some(graph::Scalar::Value(placed));
    }
    // A literal only: inventing a node that produces a computed one writes nothing.
    let literal = module
        .node_value(node)
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
        .and_then(|value| scalar_bits(class, &value));
    if let Some(bits) = literal {
        return Some(graph::Scalar::Constant(ScalarLeaf { class, bits }));
    }
    let described = module
        .node_value(node)
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
        .map(|value| graph::describe(&value).to_string())
        .or_else(|| {
            module
                .node_value(node)
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
                .map(|value| format!("{value:?}"))
        })
        .unwrap_or_else(|| "not a value at all".to_string());
    refuse(
        module,
        format!(
            "a dispatch's scalar leaf at parameter position {} is {described}, and a runtime \
             scalar is a number the build already has or one of this function's arguments: a \
             graph's value table holds nothing else, so there is no edge to record it on",
            path_text(path)
        ),
    );
    None
}

/// The bits a literal carries at `class`, or `None` for a literal of another class.
fn scalar_bits(class: ScalarClass, value: &LowValue) -> Option<i64> {
    match (class, value) {
        (ScalarClass::Int, LowValue::USize(number)) => {
            usize::try_from(*number).ok().map(|n| n as i64)
        }
        (ScalarClass::Float, LowValue::Float(number)) => Some(number.to_bits() as i64),
        _ => None,
    }
}

/// The clause a refusal carries when a deeper one already refused.
///
/// # Invariant
/// **Empty when the recording refused nothing**: the sentence that would carry the
/// clause is then the whole account, and a clause naming no cause only makes it longer.
/// A recording that did refuse makes the sentence it is attached to a **symptom**, and
/// says so — the deeper sentence is already on the surface, so this points at it rather
/// than repeating it.
fn refused_because_of(refusals: &[String]) -> String {
    if refusals.is_empty() {
        return String::new();
    }
    ": a dispatch in this body refused before that, and this is only what it left behind"
        .to_string()
}

/// What the cell a refusal names holds, in the words its own vocabulary gives it.
///
/// # Invariant
/// One reading for both leaves of the host's value union, and never a `Debug` render:
/// a handle's address differs between runs, so a sentence built from one names nothing
/// a reader can act on and two runs of one program print two different complaints.
fn named_value<P>(module: &Module<P>, node: AnyNodeId) -> String
where
    P: Program,
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue>,
{
    let Some(value) = module.node_value(node) else {
        return "not a value at all".to_string();
    };
    if let Some(low) = AsEnum::<LowValue>::as_enum(&value) {
        return graph::describe_low_value(&low);
    }
    if let Some(compute) = AsEnum::<ComputeValue>::as_enum(&value) {
        return graph::describe(&compute).to_string();
    }
    "not a value at all".to_string()
}

/// Record one dispatch into the graph being built, in place of running it.
///
/// # Invariant
/// **The rule: the first scalar leaf is the extent, and every other leaf is
/// carried for what it is.** A leaf under `.in` or `.out` is a buffer; every
/// other leaf is a launch value, at whatever depth the role walk found it. The
/// extent keeps its position — `scalar_leaf_classes` reads the first scalar leaf
/// as the count — so this bounds the kind of a leaf, never how many there are.
fn record_launch<P>(module: &mut Module<P>, block: BlockId, operand: P::Value) -> Option<P::Value>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + From<LowValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let refuse = |module: &mut Module<P>, reason: String| {
        graph::record_refusal(&reason);
        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
            GRAPH_DIAGNOSTIC,
            None,
            reason,
        ));
    };
    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
        unreachable!("ParLaunch expects an operand array of [kernel, cfg]")
    };
    // SAFETY: `operands` is the array the VM just evaluated, and every walk below
    // stays inside this borrow.
    let operands = unsafe { operands.items() };
    if operands.len() < 2 {
        refuse(
            module,
            "a parallel launch's operand array is [kernel, cfg]".into(),
        );
        return None;
    }
    // The declared codomain, when the caller states one: the recorded result's
    // layout is the declaration's own.
    let declared = operands.get(2).map(|item| item.node);
    let Some(ComputeValue::ParKernel(id, backend)) = module
        .node_value(operands[0].node)
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
    else {
        refuse(
            module,
            "a recorded dispatch has to name a parallel kernel, and this one does not".into(),
        );
        return None;
    };
    let fragment = {
        let kernels = kernels().lock().unwrap();
        match kernels.get(&id) {
            Some(entry) => entry.fragment.clone(),
            None => {
                refuse(
                    module,
                    format!(
                        "parallel kernel {id} is not registered, so there is no fragment to record"
                    ),
                );
                return None;
            }
        }
    };
    let Ok(cfg) = dyn_node(operands[1].node) else {
        refuse(module, "a recorded dispatch's cfg is not a node".into());
        return None;
    };
    // Every leaf is read at the path the role walk found (`KernelRoles`), the one
    // enumeration all three readers share.
    let roles = fragment.roles.clone();
    // The declared class of each output: the `.element` of the wrapped `Buf`.
    let output_classes = fragment.output_classes.clone();
    // The extent, read on its own: a count is a different role from a buffer.
    let extent = match roles
        .scalars
        .first()
        .and_then(|path| value_at_path::<P>(module, cfg, path))
    {
        Some(node) => {
            let literal = module
                .node_value(node)
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
                .and_then(|value| match value {
                    // A literal the build already had; collapsing it would invent a
                    // free-producing node.
                    LowValue::USize(count) => Some(Extent::Constant(count)),
                    _ => None,
                });
            match literal {
                Some(extent) => extent,
                None => match module
                    .node_value(node)
                    .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
                    .and_then(|value| match value {
                        ComputeValue::GraphInput(slot) => Some(Extent::Value(Placed::Input(slot))),
                        ComputeValue::GraphValue(id) => Some(Extent::Value(Placed::Value(id))),
                        _ => None,
                    }) {
                    Some(extent) => extent,
                    None => {
                        let found = named_value::<P>(module, node);
                        refuse(
                            module,
                            format!(
                                "a dispatch's count is {found}, and a count has to be a literal or \
                                 one of this function's arguments: a graph's extent is either known \
                                 while it is built or read from its own value table at run time"
                            ),
                        );
                        return None;
                    }
                },
            }
        }
        None => {
            refuse(
                module,
                "a recorded dispatch's parameter states no extent at its first scalar leaf".into(),
            );
            return None;
        }
    };
    // Every leaf after the extent is a launch value, in the parameter's own order.
    let mut scalars: Vec<graph::Scalar> = Vec::with_capacity(roles.scalars.len());
    for (ordinal, path) in roles.scalars.iter().enumerate().skip(1) {
        match recorded_scalar::<P>(module, cfg, path, &fragment, ordinal) {
            Some(scalar) => scalars.push(scalar),
            None => return None,
        }
    }
    let mut inputs: Vec<Placed> = Vec::new();
    for (position, path) in roles.inputs.iter().enumerate() {
        // An input path names the `Buf` the caller passes; the placeholder rides in
        // its payload.
        let Some(node) =
            value_at_path::<P>(module, cfg, path).and_then(|buf| buf_payload::<P>(module, buf))
        else {
            refuse(
                module,
                format!("argument {position} of this dispatch is not a buffer"),
            );
            return None;
        };
        let Some(value) = module
            .node_value(node)
            .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
        else {
            refuse(
                module,
                format!("argument {position} of this dispatch is not a compute value at all"),
            );
            return None;
        };
        match graph::place(&value, position) {
            Ok(placed) => inputs.push(placed),
            Err(reason) => {
                refuse(module, reason);
                return None;
            }
        }
    }
    let placed = match graph::record_dispatch(fragment, backend, extent, &scalars, &inputs) {
        Ok(placed) => placed,
        Err(reason) => {
            refuse(module, reason);
            return None;
        }
    };
    // The shape a real launch produces: `.out` with a `Buf`-wrapped placeholder
    // per output, each a live node.
    if roles.outputs.is_empty() {
        return match placed.first() {
            // A parameter with no `.out` fields: the body produced nothing, and
            // the unit value is what a run of it would return too.
            Some(placed) => {
                let node = module.add_node(
                    block,
                    None,
                    Some(<P::Value as From<ComputeValue>>::from(
                        ComputeValue::GraphValue(placed.edge()),
                    )),
                );
                module.node_value(AnyNodeId::Dynamic(node))
            }
            None => Some(<P::Value as From<LowValue>>::from(LowValue::None)),
        };
    }
    let mut fields: Vec<(Vec<usize>, NodeId)> = Vec::with_capacity(placed.len());
    for (position, placed) in placed.iter().enumerate() {
        // The paths are the codomain's own: there is no `.out` step to drop.
        let Some(path) = roles.outputs.get(position) else {
            refuse(
                module,
                format!("output {position} has no path in the codomain"),
            );
            return None;
        };
        let payload = module.add_node(
            block,
            None,
            Some(<P::Value as From<ComputeValue>>::from(
                ComputeValue::GraphValue(placed.edge()),
            )),
        );
        let class = output_classes
            .get(position)
            .copied()
            .unwrap_or(ScalarClass::Int);
        let declared_buffer = declared
            .and_then(|value| {
                declared_at_path::<P>(module, DeclaredGroup { value, ty: None }, path)
            })
            .map(|declared| declared.value);
        let Some(buf) = current_buf_value::<P>(
            module,
            block,
            AnyNodeId::Dynamic(payload),
            class,
            declared_buffer,
        ) else {
            return None;
        };
        fields.push((path.clone(), buf));
    }
    match assemble_result::<P>(
        module,
        block,
        &fields,
        &[],
        0,
        declared.map(|value| DeclaredGroup { value, ty: None }),
    ) {
        Some(root) => module.node_value(AnyNodeId::Dynamic(root)),
        None => None,
    }
}

/// Build a graph by applying a function to placeholders and recording dispatches.
///
/// # Invariant
/// The apply is the VM's own: a plain `Apply` node over the function and a
/// placeholder tuple, run by the module's `evaluate_node`, so the clone pass, the
/// parameter unify and the pattern walk are the ones every other call gets.
fn build_graph<P>(
    module: &mut Module<P>,
    block: BlockId,
    function_node: AnyNodeId,
) -> Option<P::Value>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + From<LowValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator> + From<LowOperator>,
{
    let refuse = |module: &mut Module<P>, reason: String| {
        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
            GRAPH_DIAGNOSTIC,
            None,
            reason,
        ));
    };
    // A check, not a resolution: `Apply` addresses its callee as a node, so this
    // only asks if the operand is a function.
    let is_function = module
        .evaluate_node(function_node, Some(block))
        .and_then(|value| value.as_enum());
    let Some(LowValue::Function(function)) = is_function else {
        refuse(
            module,
            "a graph is recorded from a function, and this operand is not one".into(),
        );
        return None;
    };
    // The parameter's shape is the placeholder's shape: the type walk gives the
    // cells' paths.

    // Slots are numbered depth-first in field order, so the run's own walk lands
    // on the same numbers.
    let mut roles = None;
    let mut parameter_value = None;
    if let AnyFunctionId::Dynamic(fid) = function {
        let parameter = module.functions[fid].parameter;
        match parallel_roles(module, parameter) {
            Ok(decoded) => roles = Some(decoded),
            // The declared type is not a graph ABI: the build fails here.
            Err(RoleUnread::Refusal(why)) => {
                let at = match function_node {
                    AnyNodeId::Dynamic(node) => Some(node),
                    AnyNodeId::Static(_) => None,
                };
                module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                    GRAPH_DIAGNOSTIC,
                    at,
                    why,
                ));
                return None;
            }
            // A type that states no domain is a polymorphic template: refuse it.
            Err(RoleUnread::Undecided) => {
                if shape::kernel_parameter_domain(module, function).is_none() {
                    let at = match function_node {
                        AnyNodeId::Dynamic(node) => Some(node),
                        AnyNodeId::Static(_) => None,
                    };
                    module.record_extension_diagnostic(ExtensionDiagnostic::failure(
                        GRAPH_DIAGNOSTIC,
                        at,
                        POLYMORPHIC_TEMPLATE.to_string(),
                    ));
                    return None;
                }
            }
        }
        // The parameter's own value carries the struct's layout, so the
        // placeholder is built from it.
        parameter_value = expr_value(module, parameter).map(AnyNodeId::Dynamic);
    }
    // The ceiling a positional body's tuple is built at, and the number the
    // undecided-apply refusal names.
    let arity = graph::MAX_GRAPH_INPUTS;
    let placeholders = match roles {
        Some(roles) => {
            // A buffer role's cell is a `Buf` wrapper around the placeholder;
            // a scalar role stays bare.
            let mut paths: Vec<(&Vec<usize>, bool, Option<ScalarClass>)> = roles
                .scalars
                .iter()
                .map(|path| (path, false, None))
                .chain(
                    roles
                        .inputs
                        .iter()
                        .zip(&roles.input_classes)
                        .map(|(path, class)| (path, true, *class)),
                )
                .chain(
                    roles
                        .outputs
                        .iter()
                        .zip(&roles.output_classes)
                        .map(|(path, class)| (path, true, *class)),
                )
                .collect();
            paths.sort_by(|left, right| left.0.cmp(right.0));
            let mut cells: Vec<(Vec<usize>, NodeId)> = Vec::with_capacity(paths.len());
            for (slot, (path, wrapped, class)) in paths.into_iter().enumerate() {
                let value = <P::Value as From<ComputeValue>>::from(ComputeValue::GraphInput(slot));
                let node = module.add_node(block, None, Some(value));
                // A buffer's element class is the parameter's declaration
                // (`docs/notes/compile-order-class.md` §3).
                let declared_buffer = parameter_value
                    .and_then(|value| {
                        declared_at_path::<P>(module, DeclaredGroup { value, ty: None }, path)
                    })
                    .map(|declared| declared.value);
                let node = if wrapped {
                    let class = class.unwrap_or(ScalarClass::Int);
                    current_buf_value::<P>(
                        module,
                        block,
                        AnyNodeId::Dynamic(node),
                        class,
                        declared_buffer,
                    )?
                } else {
                    node
                };
                cells.push((path.clone(), node));
            }
            // The declaration builds the placeholder: a group with no leaf under
            // it has no path to reach it.

            // An unreadable parameter value falls back to the path walk for the
            // same placeholder.
            let built = match parameter_value {
                Some(value) => assemble_parameter::<P>(module, block, value, &cells, &[]),
                None => assemble_result::<P>(module, block, &cells, &[], 0, None),
            };
            match built {
                Some(root) => root,
                None => {
                    // A recording that answers nothing is refused by name: the body
                    // asked for numbers.
                    refuse(
                        module,
                        "this recording's parameter declares a named struct, and the placeholder \
                         it is applied to could not be built from it: a field is neither a role \
                         path nor a group, so no value can reach it"
                            .into(),
                    );
                    return None;
                }
            }
        }
        // The roles are undecided, so the arity is not knowable and the
        // tuple is built at a ceiling.

        // One placeholder per slot, and the slot is the number, so `ins(k)` is the
        // `k`-th argument.
        None => {
            let cells: Vec<ArrayItem> = (0..arity)
                .map(|slot| {
                    let value =
                        <P::Value as From<ComputeValue>>::from(ComputeValue::GraphInput(slot));
                    let node = module.add_node(block, None, Some(value));
                    ArrayItem::new(AnyNodeId::Dynamic(node))
                })
                .collect();
            let root = array_node::<P>(module, block, &cells);
            root
        }
    };
    // The argument is an expr: the value side is the placeholder tuple, the type
    // side left undecided.
    let undecided = module.add_node(block, None, None);
    let argument = array_node::<P>(
        module,
        block,
        &[
            ArrayItem::new(AnyNodeId::Dynamic(placeholders)),
            ArrayItem::new(AnyNodeId::Dynamic(undecided)),
        ],
    );
    let operands: Vec<ArrayItem> = [
        ArrayItem::new(AnyNodeId::Dynamic(module.as_dynamic(function_node, block))),
        ArrayItem::new(AnyNodeId::Dynamic(argument)),
    ]
    .to_vec();
    let operand = array_node::<P>(module, block, &operands);
    let apply = module.add_node(
        block,
        Some(Operation {
            operator: <P::Operator as From<LowOperator>>::from(LowOperator::Apply),
            operand: Some(operand),
        }),
        None,
    );
    // Started before the apply and finished after it, so a dispatch the body
    // performs is inside the window.
    graph::begin();
    // Deep, not shallow: a shallow pass stops at the block's tuple, so no
    // dispatch runs.

    // Not forced, and measured: an unread dispatch is not performed.
    let (result, recorded) = evaluate_until_the_return_is_nameable::<P>(module, block, apply);
    // Every path out of the recording closes it, refusals and all.
    if result.is_none() {
        let refusals = graph::abandon();
        refuse(
            module,
            format!(
                "applying this function to its own input placeholders stayed undecided, so \
                 nothing was recorded. A function whose parameter is not a plain tuple of cells \
                 cannot be recorded this way, and the parameter here has {} cell(s){}",
                arity,
                refused_because_of(&refusals)
            ),
        );
        return None;
    };
    // An unreadable return is a refusal: a graph with no recorded return answers
    // with its whole value table.
    let Some(values) = recorded else {
        let refusals = graph::abandon();
        refuse(
            module,
            format!(
                "this function's return is not a value a graph can hand back, so there is no \
                 graph to record: a graph's value table holds what its dispatches produced and the \
                 arguments it was run with, and this body's own value is not one of those{}",
                refused_because_of(&refusals)
            ),
        );
        return None;
    };
    if let Err(reason) = graph::record_return(values) {
        let refusals = graph::abandon();
        refuse(module, format!("{reason}{}", refused_because_of(&refusals)));
        return None;
    }
    let value = match graph::finish() {
        Ok((graph, backend)) => {
            let id = graph::intern(graph);
            <P::Value as From<ComputeValue>>::from(ComputeValue::Graph(id, backend))
        }
        Err(reason) => {
            // An empty recording with a decided result is a different mistake: the
            // refusals name different causes.
            let reason = format!(
                "{reason} (the body's own value came back as a decided value, not a lazy one)"
            );
            refuse(module, reason);
            return None;
        }
    };
    Some(value)
}

/// A lichen tuple as a node, which is what an operand or a result has to be.
///
/// # Invariant
/// An array value is a handle into `block`'s arena and an operand slot wants a node,
/// so the value is allocated and given one; every item is already a node, so the
/// tuple holds live nodes.
fn array_node<P>(module: &mut Module<P>, block: BlockId, items: &[ArrayItem]) -> NodeId
where
    P: Program,
    P::Value: From<LowValue>,
{
    let handle = module.alloc_array(items, block);
    let value = <P::Value as From<LowValue>>::from(LowValue::Array(handle.into()));
    module.add_node(block, None, Some(value))
}

/// The placeholders a result node names, walked through the structure.
///
/// # Invariant
/// A kernel's result is the parameter's `.out` group — a `Buf`-wrapped placeholder
/// per output, nested as the author wrote it — so the leaves are the values the graph
/// hands back, in field order; an empty list is "the body produced nothing".
fn place_of_node<P>(module: &Module<P>, node: AnyNodeId) -> Option<Vec<Placed>>
where
    P: Program,
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
{
    place_of_node_at(module, node, 0)
}

/// [`place_of_node`] with a depth bound: a block's tuple can name itself, and a
/// cyclic value names no placeholder.
fn place_of_node_at<P>(module: &Module<P>, node: AnyNodeId, depth: usize) -> Option<Vec<Placed>>
where
    P: Program,
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
{
    if depth > 16 {
        return None;
    }
    let value = module.node_value(node)?;
    if let Some(ComputeValue::GraphValue(id)) = AsEnum::<ComputeValue>::as_enum(&value) {
        return Some(vec![Placed::Value(id)]);
    }
    if let Some(ComputeValue::GraphInput(slot)) = AsEnum::<ComputeValue>::as_enum(&value) {
        return Some(vec![Placed::Input(slot)]);
    }
    if let Some(LowValue::None) = AsEnum::<LowValue>::as_enum(&value) {
        return Some(Vec::new());
    }
    let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(&value) else {
        return None;
    };
    // SAFETY: `array` is the value of a live node of `module`, alive for this walk.
    let items = unsafe { array.items() };
    let mut placed = Vec::new();
    for item in items {
        // A type slot is not a value: `.element` is the type half, and the payload
        // beside it is what a run reads.
        if module
            .node_value(item.node)
            .is_some_and(|value| ValueType::is_kind_marker(&value))
        {
            continue;
        }
        // A type cell or a wildcard names no value; only the placeholders beside them do.
        if let Some(found) = place_of_node_at::<P>(module, item.node, depth + 1) {
            placed.extend(found);
        }
    }
    (!placed.is_empty()).then_some(placed)
}

/// The leaves of a graph run's argument, in the order the recording numbered its
/// slots: depth-first in field order.
///
/// # Invariant
/// A nested array is a group and its leaves follow in field order, which is the order
/// the recording walked; a type slot is not a value and is skipped.
fn flatten_leaves<P>(module: &mut Module<P>, node: AnyNodeId, out: &mut Vec<AnyNodeId>)
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
{
    flatten_leaves_at::<P>(module, node, out, 0);
}

/// [`flatten_leaves`] with a depth bound.
///
/// # Invariant
/// The bound is the widest argument a graph can be handed, because a value nesting
/// deeper than the graph's input ceiling is a block naming itself: it is cut rather
/// than followed, since following it names no leaf.
fn flatten_leaves_at<P>(
    module: &mut Module<P>,
    node: AnyNodeId,
    out: &mut Vec<AnyNodeId>,
    depth: usize,
) where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
{
    if depth > graph::MAX_GRAPH_INPUTS {
        return;
    }
    let value = module.node_value(node);
    if value.as_ref().is_some_and(ValueType::is_kind_marker) {
        return;
    }
    if let Some(LowValue::Array(array)) = value
        .as_ref()
        .and_then(|value| AsEnum::<LowValue>::as_enum(value))
    {
        // SAFETY: the array is the value of a live node of `module`, and its
        // items are live nodes of the same module.
        let items = unsafe { array.items() };
        // Only a slot's value position is a dispatch argument
        // (`docs/notes/compound-element-slots.md`).
        let nodes: Vec<AnyNodeId> = if array.layout() != 0 {
            items.first().map(|item| item.node).into_iter().collect()
        } else {
            // A struct value's header is not a field: the name table and the
            // nominal id are the value's own bookkeeping.
            let skip = if is_struct_value(module, node) {
                shape::STRUCT_VALUE_FIELD_BASE
            } else {
                0
            };
            items.iter().skip(skip).map(|item| item.node).collect()
        };
        for item in nodes {
            flatten_leaves_at::<P>(module, item, out, depth + 1);
        }
        return;
    }
    out.push(node);
}

/// How many rounds one recording's deep pass may be re-driven.
///
/// # Invariant
/// A ceiling, not a schedule: a round re-opens only stalled operators and asks them
/// again, so a body that cannot decide stops here instead of turning the refusal into
/// a loop.
const RECORDING_ROUNDS: usize = 4;

/// Run the recording's deep pass until its return can be named, and hand back both.
///
/// # Invariant
/// The pass drives **its own** bounded re-evaluation instead of waiting for the
/// module-wide epoch to move: [`returned_value_ids`] reads raw node slots, so a
/// structural read that stalled once leaves its slot empty and the return unnameable.
///
/// A round that re-opens nothing ends the loop, and a dispatch that answered is not
/// re-opened, so the graph is the one the first round recorded.
fn evaluate_until_the_return_is_nameable<P>(
    module: &mut Module<P>,
    block: BlockId,
    apply: NodeId,
) -> (Option<P::Value>, Option<Vec<Placed>>)
where
    P: Program,
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType + From<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut result = module.evaluate_node_deep(apply, Some(block));
    // The apply forces the returned expr, not the value it carries
    // (`docs/notes/graph-recording-rounds.md`).
    if let Some(value) = result.as_ref()
        && let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(value)
        && let Some(item) = (unsafe { array.items() }).first()
        && let Some(node) = item.node.dynamic()
    {
        module.evaluate_node_deep(node, Some(block));
    }
    let mut recorded = result
        .as_ref()
        .and_then(|value| returned_value_ids::<P>(module, value));
    for _ in 1..RECORDING_ROUNDS {
        if recorded.is_some() || module.reopen_stalled(block) == 0 {
            break;
        }
        // Each round opens its own graph; the abandoned round's refusals carry over with it.
        let refusals = graph::abandon();
        graph::begin();
        for reason in &refusals {
            graph::record_refusal(reason);
        }
        result = module.evaluate_node_deep(apply, Some(block));
        recorded = result
            .as_ref()
            .and_then(|value| returned_value_ids::<P>(module, value));
    }
    (result, recorded)
}

/// The value references a recorded body's result names, if it is a placeholder or
/// a tuple of them.
///
/// # Invariant
/// A block's value is taken from its first item, the convention [`compile_fragment`]
/// and [`parallel_output_nodes`] both follow. Both placeholder kinds are accepted,
/// since a function returning its own argument returns an input value; `None` rather
/// than a refusal, because a body may return something a value table cannot name.
fn returned_value_ids<P>(module: &Module<P>, value: &P::Value) -> Option<Vec<Placed>>
where
    P: Program,
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType + From<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // The items of a result are nodes, so each is read back through the module
    // rather than matched in place.
    let place_of = |node: AnyNodeId| place_of_node::<P>(module, node);
    // One value read as the references it names: a placeholder, or a tuple with
    // one per element.
    let placed_of = |value: &P::Value| -> Option<Vec<Placed>> {
        if let Some(ComputeValue::GraphValue(id)) = AsEnum::<ComputeValue>::as_enum(value) {
            return Some(vec![Placed::Value(id)]);
        }
        if let Some(ComputeValue::GraphInput(slot)) = AsEnum::<ComputeValue>::as_enum(value) {
            return Some(vec![Placed::Input(slot)]);
        }
        let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(value) else {
            return None;
        };
        // SAFETY: `array` is a value of the apply's own result, so its home
        // block is alive for this walk.
        let items = unsafe { array.items() };
        let mut placed = Vec::new();
        for item in items {
            // An element is a complete slot: the value position is its value
            // (`docs/notes/compound-element-slots.md`).
            placed.extend(place_of(
                module.element_value_node(item.node).unwrap_or(item.node),
            )?);
        }
        Some(placed)
    };
    // A body that is **not** a block is already the returned value.
    let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(value) else {
        return placed_of(value);
    };
    // SAFETY: as above.
    let items = unsafe { array.items() };
    let first = items.first()?;
    // A block's first item reads the returned slot
    // (`docs/notes/compound-element-slots.md`).
    let node = dyn_node(peeled_argument(module, first.node).ok()?).ok()?;
    // A recorded body's return is reached through a clone
    // (`docs/notes/graph-recording-rounds.md`).
    let inner = module.node_value(AnyNodeId::Dynamic(node)).or_else(|| {
        module
            .node_origin(node)
            .and_then(|origin| module.node_value(AnyNodeId::Dynamic(origin)))
    })?;
    placed_of(&inner)
}

/// Run a graph over the values its source function took.
///
/// # Invariant
/// The arguments are sorted into roles here, not by the operator's caller: a buffer becomes
/// the slot a dispatch records against and a number the extent it runs over. The installed
/// graph pass runs immediately before the dispatcher and its answer is written back to the
/// store it came from, so a pass rewrites what runs and not a copy.
fn run_graph<P>(
    module: &mut Module<P>,
    block: BlockId,
    graph_node: AnyNodeId,
    arguments: Option<AnyNodeId>,
) -> Option<P::Value>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + From<LowValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let refuse = |module: &mut Module<P>, reason: String| {
        module.record_extension_diagnostic(ExtensionDiagnostic::failure(
            GRAPH_DIAGNOSTIC,
            None,
            reason,
        ));
    };
    let (id, backend) = match module
        .node_value(graph_node)
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
    {
        Some(ComputeValue::Graph(id, backend)) => (id, backend),
        _ => {
            refuse(
                module,
                "a graph run has to be given a graph, and this is not one".into(),
            );
            return None;
        }
    };
    // The argument is walked the way the recording numbered its slots: leaves in
    // depth-first order, a type slot skipped.
    let mut argument_leaves: Vec<AnyNodeId> = Vec::new();
    match arguments.map(|node| dyn_node(node)).transpose() {
        Ok(Some(node)) => flatten_leaves(module, AnyNodeId::Dynamic(node), &mut argument_leaves),
        Ok(None) => {}
        Err(reason) => {
            refuse(module, reason);
            return None;
        }
    }
    let mut run_arguments: Vec<RunArgument> = Vec::new();
    for (position, node) in argument_leaves.iter().enumerate() {
        // A compound's element is a slot, so a leaf is read at its value position
        // (`docs/notes/compound-element-slots.md`).
        let node = &module.element_value_node(*node).unwrap_or(*node);
        if std::env::var("DBG_ARG").is_ok() {
            eprintln!(
                "GRAPH-ARG {position} {:?} value {:?}",
                node,
                module
                    .node_value(*node)
                    .map(|value| format!("{value:?}").chars().take(50).collect::<String>())
            );
        }
        let Some(value) = module
            .node_value(*node)
            .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
        else {
            // Not a compute value, so it is a number — the only other role a
            // dispatch reads, held at the class it is.
            match module
                .node_value(*node)
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
            {
                Some(LowValue::USize(count)) => {
                    run_arguments.push(RunArgument::Count(count as i64));
                    continue;
                }
                Some(LowValue::Float(number)) => {
                    run_arguments.push(RunArgument::Float(number));
                    continue;
                }
                _ => {
                    refuse(
                        module,
                        format!(
                            "argument {position} is neither a buffer nor a number, and a graph run \
                             reads a buffer or a number — the two roles a dispatch has"
                        ),
                    );
                    return None;
                }
            }
        };
        run_arguments.push(match value {
            ComputeValue::Buffer(payload, class) => {
                // SAFETY: read out of `module` on this borrow, so the payload's
                // home block is alive while the data is copied out.
                match buffer_items(&payload) {
                    Some(data) => RunArgument::Buffer {
                        class,
                        data: data.to_vec(),
                    },
                    None => {
                        refuse(
                            module,
                            format!("argument {position} is a buffer this process cannot read"),
                        );
                        return None;
                    }
                }
            }
            ComputeValue::DeviceBuffer(resident) => RunArgument::Resident(resident),
            other => {
                refuse(
                    module,
                    format!(
                        "argument {position} is {}, and a graph run reads a buffer or a number — \
                         the two roles a dispatch has",
                        graph::describe(&other)
                    ),
                );
                return None;
            }
        });
    }
    // The pass runs before the dispatch, and what it answers is stored and run.
    let mut graph = match graph::lookup(id) {
        Ok(graph) => graph,
        Err(reason) => {
            refuse(module, reason);
            return None;
        }
    };
    let mut lines: Vec<String> = Vec::new();
    let extent = |node: usize| -> Option<usize> {
        match &graph.nodes().get(node)? {
            Node::Kernel(kernel) => match kernel.count {
                Count::Constant(count) => Some(count),
                Count::Value(value) => argument_extent(&run_arguments, value),
                // The number is on the device, so this host has no answer to hand a pass.
                Count::Device { .. } => None,
            },
        }
    };
    if let Some(rewritten) = consult_graph_pass(&graph, &extent) {
        // The rewrite takes an entry of its own, so the recording stays what it was.
        graph::store(&rewritten);
        graph = rewritten;
    }
    lines.extend(take_graph_pass_lines());
    for line in lines {
        module.record_extension_diagnostic(ExtensionDiagnostic::note(
            LANDING_DIAGNOSTIC,
            None,
            line,
        ));
    }
    let values = match graph::run(
        &graph,
        backend,
        run_arguments,
        GRAPH_POLICY.with(|policy| policy.get()),
    ) {
        Ok(values) => values,
        Err(reason) => {
            refuse(module, reason);
            return None;
        }
    };
    let produced = values;
    // Left where they are: a device result stays resident until the language asks
    // for host data.
    let value = if produced.len() == 1 {
        match &produced[0] {
            RunResult::Buffer { class, data } => <P::Value as From<ComputeValue>>::from(
                ComputeValue::Buffer(module.alloc_payload(data, block), *class),
            ),
            RunResult::Resident(resident) => {
                <P::Value as From<ComputeValue>>::from(ComputeValue::DeviceBuffer(*resident))
            }
            RunResult::Count(count) => <P::Value as From<LowValue>>::from(LowValue::USize(*count)),
            RunResult::Float(number) => {
                <P::Value as From<LowValue>>::from(LowValue::Float(*number))
            }
        }
    } else {
        // **Every compound element is a complete slot**
        // (`docs/notes/compound-element-slots.md`).
        let width = <P::Value as From<LowValue>>::from(LowValue::Any);
        let items: Vec<ArrayItem> = produced
            .iter()
            .map(|result| {
                let value = match result {
                    RunResult::Buffer { class, data } => <P::Value as From<ComputeValue>>::from(
                        ComputeValue::Buffer(module.alloc_payload(data, block), *class),
                    ),
                    RunResult::Resident(resident) => <P::Value as From<ComputeValue>>::from(
                        ComputeValue::DeviceBuffer(*resident),
                    ),
                    RunResult::Count(count) => {
                        <P::Value as From<LowValue>>::from(LowValue::USize(*count))
                    }
                    RunResult::Float(number) => {
                        <P::Value as From<LowValue>>::from(LowValue::Float(*number))
                    }
                };
                let value = module.add_node(block, None, Some(value));
                let r#type = module.add_node(block, None, Some(width));
                let slot: Vec<ArrayItem> = std::iter::once(value)
                    .chain(std::iter::once(r#type))
                    .chain(std::iter::repeat_n(
                        module.add_node(block, None, Some(width)),
                        lichen_lowlevel::EXPR_ATTR_BASE,
                    ))
                    .map(|node| ArrayItem::new(AnyNodeId::Dynamic(node)))
                    .collect();
                let handle = module.alloc_array(&slot, block);
                let layout = module.full_width_layout().unwrap_or(0);
                let node = module.add_node(
                    block,
                    None,
                    Some(<P::Value as From<LowValue>>::from(LowValue::Array(
                        lichen_lowlevel::ArrayValue::with_layout(handle, layout),
                    ))),
                );
                ArrayItem::new(AnyNodeId::Dynamic(node))
            })
            .collect();
        let handle = module.alloc_array(&items, block);
        <P::Value as From<LowValue>>::from(LowValue::Array(handle.into()))
    };
    Some(value)
}

/// The element count the argument at `value` names, when one does.
///
/// # Invariant
/// A dispatch's extent is the count role, not a buffer's length: the byte count a
/// [`RunArgument::Buffer`] carries is divided at the class's own width, and a resident
/// buffer's count is the count it was issued for. Anything else is `None` rather than an
/// answer with something else in it, a float included — an extent has no class to carry.
fn argument_extent(arguments: &[RunArgument], value: GraphValueId) -> Option<usize> {
    match arguments.get(value)? {
        RunArgument::Count(count) => usize::try_from(*count).ok(),
        RunArgument::Buffer { class, data } => Some(data.len() / class.byte_width()),
        RunArgument::BufferShared { class, data } => Some(data.len() / class.byte_width()),
        RunArgument::Resident(resident) => Some(resident.count),
        RunArgument::Float(_) => None,
    }
}

fn dyn_node(id: AnyNodeId) -> Result<NodeId, String> {
    id.dynamic()
        .ok_or_else(|| "static refs are not kernel-compilable v1".to_string())
}

/// Whether `node` is (or holds) a **frozen** function value — the callee of a
/// routed operator's apply.
fn is_static_function<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    matches!(
        module
            .node_value(AnyNodeId::Dynamic(node))
            .and_then(|value| AsEnum::<LowValue>::as_enum(&value)),
        Some(LowValue::Function(AnyFunctionId::Static(_)))
    )
}

/// Execute a compiled kernel on an argument vector with wasmi, returning its results.
///
/// # Invariant
/// The argument count and the output buffer are decided here, not by `wasmi`: all
/// three numbers are facts compute holds, so a refusal can state them. The argument
/// and class checks are the only account of a wrong-arity or mis-classed call, since
/// `compute.call`'s gate leaves the argument unconstrained.
fn run_kernel(id: KernelId, args: &[ScalarValue]) -> Result<Vec<ScalarValue>, String> {
    // These are read under one lock, released before assembly locks the registry
    // again.
    let (expected, results, class) = {
        let fragments = kernels().lock().unwrap();
        let fragment = &fragments
            .get(&id)
            .ok_or_else(|| format!("kernel {id} is not registered"))?
            .fragment;
        (
            param_classes(fragment),
            fragment.result_classes.len(),
            fragment_class(fragment),
        )
    };
    if args.len() != expected.len() {
        return Err(format!(
            "the callee kernel {id} takes {} {}, but this call supplied {}",
            expected.len(),
            if expected.len() == 1 {
                "argument"
            } else {
                "arguments"
            },
            args.len()
        ));
    }
    for (position, (argument, leaf)) in args.iter().zip(&expected).enumerate() {
        if argument.class != *leaf {
            return Err(format!(
                "argument {position} is {:?}, but the kernel's parameter there is {:?}: Int and \
                 Float do not convert",
                argument.class, leaf
            ));
        }
    }
    // A fragment's result arity is at least one: the guard states that rather than
    // resting on the emitter.
    let exprs: Vec<(ScalarClass, i64)> = args
        .iter()
        .map(|argument| (argument.class, argument.bits))
        .collect();
    let bits = run_on_engine(id, || assemble_launch_set(id), class, results, &exprs)?;
    Ok(bits
        .into_iter()
        .map(|bits| ScalarValue { class, bits })
        .collect())
}

/// The fragments of a kernel's relative launch set, in BFS order, and where each
/// id sits.
///
/// # Invariant
/// The set is the root plus every kernel it transitively cross-calls, and the
/// fragments are cloned with the registry lock released before this returns — an
/// emitter that reached the registry again would deadlock. One derivation, both
/// consumers: the wasm assembly and an installed backend.
fn ordered_launch_set(id: KernelId) -> Result<OrderedLaunchSet, String> {
    let mut ordered: Vec<KernelFragment> = Vec::new();
    let mut index: HashMap<KernelId, u32> = HashMap::new();
    let mut seen: HashSet<KernelId> = HashSet::new();
    let mut queue: VecDeque<KernelId> = VecDeque::new();
    seen.insert(id);
    queue.push_back(id);
    let fragments = kernels().lock().unwrap();
    while let Some(k) = queue.pop_front() {
        let frag = fragments
            .get(&k)
            .map(|entry| entry.fragment.clone())
            .ok_or_else(|| format!("kernel {k} is not registered"))?;
        index.insert(k, ordered.len() as u32);
        for instr in frag.body.instrs() {
            if let KernelInstr::CallKernel(kid) = *instr {
                if seen.insert(kid) {
                    if !fragments.contains_key(&kid) {
                        return Err(format!(
                            "cross-kernel callee kernel {kid} is not registered"
                        ));
                    }
                    queue.push_back(kid);
                }
            }
        }
        ordered.push(frag);
    }
    drop(fragments);
    Ok(OrderedLaunchSet { ordered, index })
}

/// The two halves of a launch set, owned so the registry lock is gone before
/// anything is emitted from them.
struct OrderedLaunchSet {
    ordered: Vec<KernelFragment>,
    index: HashMap<KernelId, u32>,
}

/// Assemble the wasm bytes of the root kernel's relative launch set.
///
/// # Invariant
/// The result is a function of the root id alone — the registry's fragments are
/// immutable and ids are never reused — which makes the id a sufficient cache key.
fn assemble_launch_set(id: KernelId) -> Result<Vec<u8>, String> {
    let OrderedLaunchSet { ordered, index } = ordered_launch_set(id)?;
    assemble_module(&ordered, &index)
}

/// The execution state a parallel kernel's host imports read and write against.
///
/// # Invariant
/// Every output buffer is **shared** by all workers and its words are atomic: a `write` is a
/// relaxed store into the slot its lane names, and an atomic add is a read-modify-write. The
/// disjointness a scatter would break is `compute.write`'s contract, so the runner partitions
/// lane ranges and never buffer storage. The state borrows its buffers, so the caller keeps
/// them back.
struct ParallelState<'a> {
    /// The input buffers, indexed by the input position the role walk records.
    ///
    /// # Invariant
    /// Never partitioned: a read is by a global index, so every worker reads the
    /// whole buffer. A float input's words are its `f32` bits.
    inputs: &'a [BufferWords],
    /// The output buffers, shared across workers, indexed by the ordinal.
    ///
    /// # Invariant
    /// One buffer per output the index function declares — the count is the fragment's
    /// `outputs`, never which slots happened to be written — each `count` words long.
    outputs: &'a [Vec<AtomicI64>],
    /// The class the whole fragment is lowered in: the type of the imports and of
    /// the kernel's own parameters and result.
    class: ScalarClass,
}

impl HostWords for ParallelState<'_> {
    fn read(&self, position: usize, index: usize) -> i64 {
        self.inputs
            .get(position)
            .and_then(|buffer| buffer.words.get(index))
            .copied()
            .unwrap_or(0)
    }

    fn write(&self, position: usize, index: usize, value: i64) {
        // A relaxed store: `compute.write`'s disjointness contract is what
        // makes the store the whole of a write.
        if let Some(word) = self
            .outputs
            .get(position)
            .and_then(|buffer| buffer.get(index))
        {
            word.store(value, Ordering::Relaxed);
        }
    }

    fn atomic_add(&self, position: usize, index: usize, value: i64) {
        let Some(word) = self
            .outputs
            .get(position)
            .and_then(|buffer| buffer.get(index))
        else {
            return;
        };
        match self.class {
            ScalarClass::Int => {
                word.fetch_add(value, Ordering::Relaxed);
            }
            // The float add is a compare-exchange loop over the word bits.
            _ => {
                let addend = f32::from_bits(value as u32);
                let mut current = word.load(Ordering::Relaxed);
                loop {
                    let next = f32::from_bits(current as u32) + addend;
                    match word.compare_exchange_weak(
                        current,
                        i64::from(next.to_bits()),
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => current = actual,
                    }
                }
            }
        }
    }
}

/// Hand a run to the installed [`lichen_kernel_ir::ParallelBackend`].
///
/// # Invariant
/// A missing backend is a refusal, not a fallback: `parallel` names its backend
/// explicitly and there is no "try either" value, so a program that said `"gpu"` and
/// silently ran on the CPU would be one whose timing means nothing. The fragments are
/// cloned out of the registry and the lock released before the call, which is not
/// reentrant.
fn run_on_installed_backend(
    id: KernelId,
    count: usize,
    inputs: &[RunInput],
    outputs: usize,
    leaves: &[i64],
    leaf_classes: &[ScalarClass],
) -> Result<RunOutcome, String> {
    let Some(backend) = lichen_kernel_ir::parallel_backend() else {
        return Err(format!(
            "this parallel kernel was compiled for the \"gpu\" backend, but no compute backend is \
             installed: a host program has to install one before a run can be dispatched to a \
             device"
        ));
    };
    let OrderedLaunchSet { ordered, index } = ordered_launch_set(id)?;
    let fragment = ordered
        .first()
        .expect("a launch set is built from a root, so it holds at least one fragment");
    if fragment.outputs != outputs {
        return Err(format!(
            "parallel kernel {id} declares {} output(s) here and {} in the fragment it names",
            outputs, fragment.outputs
        ));
    }
    // Each word at its own leaf's class; the extent is the count, not one.
    let runtime_scalars: Vec<ScalarLeaf> = leaves[1..]
        .iter()
        .zip(&leaf_classes[1..])
        .map(|(&bits, &class)| ScalarLeaf { class, bits })
        .collect();
    let launch = lichen_kernel_ir::LaunchSet::new(&ordered, &index).with_leaves(runtime_scalars);
    // The class check runs first: a host slot is raw bits, so a wrong class would
    // reach the device as the wrong numbers.
    check_input_classes(&fragment.input_classes, inputs)?;
    // The one place a run leaves an input where it is: a resident input is handed
    // back as the id it has.

    // The host slot is the buffer's packed elements: `BufferSlot` carries raw
    // bytes and no class.
    let payloads: Vec<Vec<u8>> = inputs
        .iter()
        .filter_map(|input| match input {
            RunInput::Host(buffer) => Some(buffer.packed()),
            RunInput::Resident(_) => None,
        })
        .collect();
    let mut host = 0;
    let slots: Vec<BufferSlot> = inputs
        .iter()
        .map(|input| match input {
            RunInput::Host(_) => {
                let slot = BufferSlot::Host(&payloads[host]);
                host += 1;
                slot
            }
            RunInput::Resident(resident) => BufferSlot::Resident(resident.id),
        })
        .collect();
    let resident = backend.run(&launch, &slots, count).map_err(|reason| {
        format!(
            "the {:?} backend declined this run: {reason}",
            backend.name()
        )
    })?;

    // Nothing is fetched and nothing is released: fetching would put the download
    // back on every run.

    // Each id's class is the fragment's declared class for that ordinal; a shorter
    // list falls back to the ABI default.
    let classes = &fragment.output_classes;
    Ok(RunOutcome::Resident(
        resident
            .into_iter()
            .enumerate()
            .map(|(ordinal, id)| ResidentBuffer {
                id,
                count,
                class: classes.get(ordinal).copied().unwrap_or(ScalarClass::Int),
            })
            .collect(),
    ))
}

/// The most elements one `plrun` may collect: the bound on the count the program
/// controls.
///
/// # Invariant
/// The count sizes the output buffer and, the kernel being called once per element,
/// the interpreted work one launch can do — both otherwise unbounded, since `2^40` is
/// a legal `Int`. A count past the limit is refused, never truncated: a short buffer
/// would be a wrong answer.
const MAX_PARALLEL_ELEMENTS: usize = 1 << 20;

/// The element count at or above which a parallel run spreads over worker threads.
///

// How many workers the calling thread's most recent parallel launch used.

// Thread-local, not a process counter: the value is about one launch and is
// written by the thread that launched it.
thread_local! {
    static PARALLEL_LAUNCH_WORKERS: Cell<usize> = const { Cell::new(1) };
}

/// How many workers the calling thread's most recent parallel launch used; `1` is a
/// sequential run.
///
/// # Invariant
/// Test- and measurement-visible because a parallel result is bit-identical to a
/// sequential one by construction, so no assertion on values can tell a working
/// fan-out from a dead path; this is what a test asserts on.
pub fn parallel_launch_workers() -> usize {
    PARALLEL_LAUNCH_WORKERS.get()
}

/// Run a parallel kernel over `[0, count)`, once per index, collecting the writes.
///
/// # Invariant
/// The output count is the fragment's `outputs` — a static fact, not a discovery of
/// which slots were written — and every ordinal is written on every index, so each
/// buffer is fully defined. The result equals the sequential loop's iff no two indices
/// write the same slot: the partition decides a collision. A count past
/// `MAX_PARALLEL_ELEMENTS` is refused before allocation.
fn run_parallel_kernel(
    id: KernelId,
    backend: Backend,
    leaves: Vec<i64>,
    inputs: Vec<RunInput>,
) -> Result<RunOutcome, String> {
    // The ABI's first leaf is the launch extent, the one ordinal this side needs.
    let Some(&extent) = leaves.first() else {
        return Err(
            "a parallel launch's signature has no scalar leaves, so there is no extent to \
             dispatch over"
                .to_string(),
        );
    };
    let count = usize::try_from(extent).unwrap_or(usize::MAX);
    if count > MAX_PARALLEL_ELEMENTS {
        return Err(format!(
            "parallel launch count {count} exceeds the limit of {MAX_PARALLEL_ELEMENTS} elements"
        ));
    }
    // The output count and class are the fragment's, read here rather than carried
    // in: an id is content-addressed.
    let (outputs, class, input_classes, leaf_classes, output_classes, output_extents) = {
        let fragments = kernels().lock().unwrap();
        let fragment = &fragments
            .get(&id)
            .ok_or_else(|| format!("parallel kernel {id} is not registered"))?
            .fragment;
        (
            fragment.outputs,
            fragment_class(fragment),
            fragment.input_classes.clone(),
            // The ABI's leaves are the scalars then the index; only the scalars are
            // handed in.
            param_classes(fragment)[..fragment.scalar_leaves()].to_vec(),
            fragment.output_classes.clone(),
            fragment.roles.output_extents.clone(),
        )
    };
    if leaf_classes.len() != leaves.len() {
        return Err(format!(
            "this parallel kernel's parameter declares {} scalar leaf/leaves and the launch \
             passes {}: a launch's `cfg` is the parameter's scalars in field order, so the two \
             have to agree",
            leaf_classes.len(),
            leaves.len()
        ));
    }
    // A borrow of data already out of the registry: the check is a function of the
    // classes alone.
    check_input_classes(&input_classes, &inputs)?;
    if let Backend::Gpu = backend {
        return run_on_installed_backend(id, count, &inputs, outputs, &leaves, &leaf_classes);
    }
    // A "cpu" run has no device, so an input a "gpu" run left there is brought home
    // before the run starts.
    let mut host_inputs: Vec<BufferWords> = Vec::with_capacity(inputs.len());
    for (position, input) in inputs.into_iter().enumerate() {
        match input {
            RunInput::Host(buffer) => host_inputs.push(buffer),
            RunInput::Resident(resident) => {
                let class = resident.class;
                match fetch_resident(resident, position)? {
                    ScalarData::Int(elements) => host_inputs.push(BufferWords::ints(elements)),
                    ScalarData::Float(elements) => host_inputs.push(BufferWords {
                        class,
                        words: elements.into_iter().map(float_bits).collect(),
                    }),
                }
            }
        }
    }
    let inputs = host_inputs;
    // Each output is allocated at its declared length, else the launch count.
    let outputs: Vec<Vec<AtomicI64>> = (0..outputs)
        .map(|ordinal| {
            let length = output_extents
                .get(ordinal)
                .copied()
                .flatten()
                .map(|extent| extent as usize)
                .unwrap_or(count);
            (0..length).map(|_| AtomicI64::new(0)).collect()
        })
        .collect();
    {
        let inputs = &inputs;
        let outputs = &outputs;
        let leaves = &leaves;
        let leaf_classes = &leaf_classes;
        let state = move || ParallelState {
            inputs,
            outputs,
            class,
        };
        let workers = run_parallel(
            id,
            || assemble_launch_set(id),
            state,
            leaves,
            leaf_classes,
            class,
            count,
        )?;
        PARALLEL_LAUNCH_WORKERS.set(workers);
    }
    // Read back after the scope joins: the scope is the ordering.
    let outputs: Vec<Vec<i64>> = outputs
        .into_iter()
        .map(|buffer| buffer.into_iter().map(AtomicI64::into_inner).collect())
        .collect();
    Ok(RunOutcome::Host(
        outputs
            .into_iter()
            .enumerate()
            // **Each output is tagged with its own declared class**, at its write ordinal.
            .map(|(ordinal, words)| BufferWords {
                class: output_classes.get(ordinal).copied().unwrap_or(class),
                words,
            })
            .collect(),
    ))
}

/// Refuse a run whose input buffers disagree with the fragment over the **shape** they hold.
///
/// # Invariant
/// The declarations are shapes, not precisions: the language's `Float` says real numbers and nothing
/// about how many bits one takes, because a pack decides that when it compiles the node. Two real
/// classes therefore agree, and only a real/integer disagreement — the one that would convert nothing —
/// is refused.
fn check_input_classes(declared: &[ScalarClass], inputs: &[RunInput]) -> Result<(), String> {
    for (position, input) in inputs.iter().enumerate() {
        let declared = declared.get(position).copied().unwrap_or(ScalarClass::Int);
        let held = match input {
            RunInput::Host(buffer) => buffer.class,
            RunInput::Resident(resident) => resident.class,
        };
        if held != declared && !(held.is_real() && declared.is_real()) {
            return Err(format!(
                "input buffer {position} holds {held:?} elements, but this kernel reads that \
                 position as {declared:?}: Int and Float do not convert"
            ));
        }
    }
    Ok(())
}

/// Bring one resident buffer home, naming the position it was read at.
///
/// # Invariant
/// The count and the class travel with the value, so a fetch asks for what the run
/// produced rather than the padded allocation, and the answer's class is the value's
/// own: a fetch has no fragment to consult.
fn fetch_resident(resident: ResidentBuffer, position: usize) -> Result<ScalarData, String> {
    let Some(backend) = lichen_kernel_ir::parallel_backend() else {
        return Err(format!(
            "input buffer {position} of this run is still on a device, but no compute backend \
             is installed any more, so there is nothing left that can bring it back"
        ));
    };
    let data = backend
        .fetch(resident.id, resident.count)
        .map_err(|reason| {
            format!(
                "the {:?} backend declined to return input buffer {position} (buffer {}): {reason}",
                backend.name(),
                resident.id.0
            )
        })?;
    match data.class() == resident.class {
        true => Ok(data),
        // The device's answer and the issued class disagree, and the fetch is the
        // only thing that can see both.

        // Refused by name: reading an `f32` payload as `i64` is a wrong number, not
        // a wrong shape.
        false => Err(format!(
            "input buffer {position} (buffer {}) was issued as a {:?} buffer but the backend \
             returned {:?} elements",
            resident.id.0,
            resident.class,
            data.class()
        )),
    }
}

// --- The parallel run: the worker rule, the partition, the fan-out ---

#[cfg(test)]
mod parallel_launch_tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    // **`KernelBody` and `FlatOp` reach a hand-written body**, which is the test's own
    // construction.
    use lichen_kernel_ir::{FlatOp, KernelBody, ResidentId, Slice, Space};

    /// A two-output parallel fragment over `(n, i)`: `out0[i] = i + 1`, `out1[i] = i + i`.
    ///
    /// # Invariant
    /// The trailing `Const(0)` is what the compiler appends: the body only has side
    /// effects, and the shared assembler's `-> i64` signature needs one value left on
    /// the stack.
    fn two_outputs() -> KernelFragment {
        KernelFragment {
            param_shape: KernelShape::Tuple(vec![
                KernelShape::Scalar(ScalarClass::Int),
                KernelShape::Scalar(ScalarClass::Int),
            ]),
            // The tuple form's shape: a count at 0 and the buffers at 1.
            roles: KernelRoles::default(),
            body: KernelBody::from_flat(
                2,
                &[
                    FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                    FlatOp::Read(1),
                    FlatOp::Read(1),
                    FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
                    FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                    FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                    FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
                    FlatOp::Read(1),
                    FlatOp::Read(1),
                    FlatOp::Read(1),
                    FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                    FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                    FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                ],
            ),
            inputs: 0,
            outputs: 2,
            input_classes: Vec::new(),
            output_classes: vec![ScalarClass::Int, ScalarClass::Int],
            result_classes: vec![ScalarClass::Int],
            target: TargetFacts {
                int_width: IntWidth::I64,
                lane_width: LaneWidth::new(64),
                lane_height: LaneHeight::new(1),
            },
            slices: Vec::new(),
            marks: Vec::new(),
        }
    }

    /// A body that writes one slice, leaving behind the version a read would name.
    ///
    /// # Invariant
    /// The write's operands are `[slice, index, value]` and its result is the version, so the body
    /// stages a value between lanes; the trailing constant is the one value the ABI's result wants.
    fn stages_a_slice() -> KernelFragment {
        KernelFragment {
            roles: KernelRoles::default(),
            param_shape: KernelShape::Tuple(vec![
                KernelShape::Scalar(ScalarClass::Int),
                KernelShape::Scalar(ScalarClass::Int),
            ]),
            body: KernelBody::from_flat(
                2,
                &[
                    FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // the slice ordinal
                    FlatOp::Read(1),                                        // the index into it
                    FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 7)), // the value written
                    FlatOp::Instr(KernelInstr::SliceWriteCall(ScalarClass::Int)),
                    FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                ],
            ),
            inputs: 1,
            outputs: 1,
            input_classes: vec![ScalarClass::Int],
            output_classes: vec![ScalarClass::Int],
            result_classes: vec![ScalarClass::Int; 1],
            target: TargetFacts {
                int_width: IntWidth::I64,
                lane_width: LaneWidth::new(64),
                lane_height: LaneHeight::new(1),
            },
            slices: vec![Slice {
                class: ScalarClass::Int,
                count: 64,
                space: Space::Shared,
            }],
            marks: Vec::new(),
        }
    }

    /// A body that stages is refused by name: the host calls the body once per index.
    #[test]
    fn a_slice_access_is_refused_by_name() {
        let refusal = assemble_module(&[stages_a_slice()], &std::collections::HashMap::new())
            .expect_err("a per-index host cannot stage a value between lanes");
        assert!(refusal.contains("slice"), "names the cause: {refusal}");
        assert!(
            refusal.contains("once per index"),
            "says why this backend cannot serve it: {refusal}"
        );
    }

    /// The backend slot is process-global, so tests that install one must be
    /// serialized against each other.
    static BACKEND_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A "gpu" kernel goes to the installed backend, not to this crate's thread pool.
    ///
    /// # Invariant
    /// A stub stands in for a device: what is under test is the routing, and a test
    /// that drove a real device would fail on a machine with no GPU.
    #[test]
    fn a_gpu_kernel_is_routed_to_the_installed_backend() {
        let _serialized = BACKEND_SLOT.lock().unwrap();
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Stub {
            seen: AtomicUsize,
            fetched: AtomicUsize,
            released: Mutex<Vec<u64>>,
        }
        impl lichen_kernel_ir::ParallelBackend for Stub {
            fn name(&self) -> &'static str {
                "stub"
            }
            fn run(
                &self,
                _launch: &lichen_kernel_ir::LaunchSet<'_>,
                _inputs: &[BufferSlot],
                _count: usize,
            ) -> Result<Vec<ResidentId>, String> {
                self.seen.fetch_add(1, Ordering::SeqCst);
                Ok(vec![ResidentId(41)])
            }
            fn fetch(&self, _id: ResidentId, count: usize) -> Result<ScalarData, String> {
                self.fetched.fetch_add(1, Ordering::SeqCst);
                Ok(ScalarData::Int(vec![7; count]))
            }
            fn release(&self, id: ResidentId) {
                self.released.lock().unwrap().push(id.0);
            }
        }

        let stub = std::sync::Arc::new(Stub {
            seen: AtomicUsize::new(0),
            fetched: AtomicUsize::new(0),
            released: Mutex::new(Vec::new()),
        });
        lichen_kernel_ir::install_parallel_backend(stub.clone());
        let id = intern_kernel(two_outputs(), None);
        let outputs = run_parallel_kernel(id, Backend::Gpu, vec![8], vec![])
            .expect("the installed backend runs");
        lichen_kernel_ir::clear_parallel_backend();

        assert_eq!(
            outputs,
            RunOutcome::Resident(vec![ResidentBuffer {
                id: ResidentId(41),
                count: 8,
                class: ScalarClass::Int,
            }]),
            "a \"gpu\" run hands back a resident id per output, and the count travels with it \
             so a later fetch knows what to ask for"
        );
        assert_eq!(
            stub.fetched.load(Ordering::SeqCst),
            0,
            "the run itself fetches nothing — a run whose results are never read must not pay \
             for them, and a fetch here is what put the download back on every dispatch's path"
        );
        assert_eq!(
            *stub.released.lock().unwrap(),
            Vec::<u64>::new(),
            "nor does it release: the id is the caller's now, and freeing it here would hand \
             the next kernel a buffer that is gone"
        );
    }

    /// A stub that records what each run was handed, one entry per run.
    ///
    /// # Invariant
    /// A value comparison cannot tell a resident intermediate from one that came home
    /// and went back out, so the handover is what has to be pinned.
    struct ChainRecorder {
        seen: Mutex<Vec<Vec<Slot>>>,
        fetched: AtomicUsize,
    }

    /// What one input slot was, as the backend was handed it.
    ///
    /// # Invariant
    /// A host slot records its elements — the payload over the class's width — since a
    /// host slot is bytes, and a count of bytes is not a count of elements.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Slot {
        Host(usize),
        Resident(u64),
    }

    impl lichen_kernel_ir::ParallelBackend for ChainRecorder {
        fn name(&self) -> &'static str {
            "stub"
        }
        fn run(
            &self,
            _launch: &lichen_kernel_ir::LaunchSet<'_>,
            inputs: &[BufferSlot],
            _count: usize,
        ) -> Result<Vec<ResidentId>, String> {
            self.seen.lock().unwrap().push(
                inputs
                    .iter()
                    .map(|slot| match slot {
                        BufferSlot::Host(data) => {
                            Slot::Host(data.len() / ScalarClass::Int.byte_width())
                        }
                        BufferSlot::Resident(id) => Slot::Resident(id.0),
                    })
                    .collect(),
            );
            Ok(vec![ResidentId(41)])
        }
        fn fetch(&self, _id: ResidentId, count: usize) -> Result<ScalarData, String> {
            self.fetched.fetch_add(1, Ordering::SeqCst);
            Ok(ScalarData::Int(vec![0; count]))
        }
        fn release(&self, _id: ResidentId) {}
    }

    fn chain_recorder() -> std::sync::Arc<ChainRecorder> {
        std::sync::Arc::new(ChainRecorder {
            seen: Mutex::new(Vec::new()),
            fetched: AtomicUsize::new(0),
        })
    }

    /// The chain, at the boundary the language controls.
    #[test]
    fn a_gpu_chain_hands_the_next_run_an_id_rather_than_the_data() {
        let _serialized = BACKEND_SLOT.lock().unwrap();
        let stub = chain_recorder();
        lichen_kernel_ir::install_parallel_backend(stub.clone());
        let id = intern_kernel(two_outputs(), None);
        let count = 8;

        let first = run_parallel_kernel(
            id,
            Backend::Gpu,
            vec![count as i64],
            vec![RunInput::Host(BufferWords::ints(
                (0..count as i64).collect(),
            ))],
        )
        .expect("the first run completes");
        let RunOutcome::Resident(resident) = first else {
            panic!("a \"gpu\" run leaves its results on the device");
        };
        let second = run_parallel_kernel(
            id,
            Backend::Gpu,
            vec![count as i64],
            vec![RunInput::Resident(resident[0])],
        )
        .expect("the second run consumes the id");
        lichen_kernel_ir::clear_parallel_backend();

        assert!(
            matches!(second, RunOutcome::Resident(_)),
            "and it leaves its own result there too, so a chain of any length pays one upload"
        );
        assert_eq!(
            *stub.seen.lock().unwrap(),
            vec![vec![Slot::Host(count)], vec![Slot::Resident(41)]],
            "the second run is handed the id the first one issued; a host buffer in that slot \
             would mean the intermediate had come home only to be sent straight back out"
        );
        assert_eq!(
            stub.fetched.load(Ordering::SeqCst),
            0,
            "neither run fetched: a chain nobody reads costs no transfer at all"
        );
    }

    /// A "cpu" run has no device, so an input a "gpu" run left there comes home.
    #[test]
    fn a_cpu_run_brings_home_an_input_a_gpu_run_left_on_the_device() {
        let _serialized = BACKEND_SLOT.lock().unwrap();
        let stub = chain_recorder();
        lichen_kernel_ir::install_parallel_backend(stub.clone());
        let id = intern_kernel(two_outputs(), None);
        let count = 8;
        run_parallel_kernel(
            id,
            Backend::Cpu,
            vec![count as i64],
            vec![RunInput::Resident(ResidentBuffer {
                id: ResidentId(41),
                count,
                class: ScalarClass::Int,
            })],
        )
        .expect("the run succeeds — the input is on the device, not lost");
        lichen_kernel_ir::clear_parallel_backend();

        assert_eq!(
            stub.fetched.load(Ordering::SeqCst),
            1,
            "exactly one fetch, before the run: the boundary between the two backends is where \
             data moves, and it is a move rather than a refusal"
        );
    }

    /// A "gpu" run with no backend installed is refused by name: there is no third
    /// value to fall back to.
    #[test]
    fn a_gpu_run_without_an_installed_backend_is_refused_by_name() {
        let _serialized = BACKEND_SLOT.lock().unwrap();
        lichen_kernel_ir::clear_parallel_backend();
        let id = intern_kernel(two_outputs(), None);
        let refusal = run_parallel_kernel(id, Backend::Gpu, vec![8], vec![])
            .expect_err("there is nothing to dispatch to");
        assert!(
            refusal.contains("gpu") && refusal.contains("installed"),
            "the refusal names the backend asked for and what is missing: {refusal}"
        );
    }

    /// A backend that declines is reported, not swallowed: the CPU is no fallback.
    #[test]
    fn a_declining_backend_is_reported_with_its_reason() {
        let _serialized = BACKEND_SLOT.lock().unwrap();
        struct Stub;
        impl lichen_kernel_ir::ParallelBackend for Stub {
            fn name(&self) -> &'static str {
                "stub"
            }
            fn run(
                &self,
                _launch: &lichen_kernel_ir::LaunchSet<'_>,
                _inputs: &[BufferSlot],
                _count: usize,
            ) -> Result<Vec<ResidentId>, String> {
                Err("this device has no compute queue".to_string())
            }

            fn fetch(&self, _id: ResidentId, _count: usize) -> Result<ScalarData, String> {
                unreachable!("a run that declined never hands back an id to fetch")
            }

            fn release(&self, _id: ResidentId) {
                unreachable!("a run that declined never hands back an id to release")
            }
        }
        lichen_kernel_ir::install_parallel_backend(std::sync::Arc::new(Stub));
        let id = intern_kernel(two_outputs(), None);
        let refusal = run_parallel_kernel(id, Backend::Gpu, vec![8], vec![])
            .expect_err("the backend declined");
        lichen_kernel_ir::clear_parallel_backend();
        assert!(
            refusal.contains("stub") && refusal.contains("no compute queue"),
            "the refusal names the backend and passes its reason through: {refusal}"
        );
    }

    /// The parse is strict: a typo is reported with both what was written and what is
    /// accepted.
    #[test]
    fn an_unknown_backend_name_is_refused_with_both_the_value_and_the_choices() {
        assert_eq!(parse_backend("cpu"), Ok(Backend::Cpu));
        assert_eq!(parse_backend("gpu"), Ok(Backend::Gpu));
        let refusal = parse_backend("Gpu").expect_err("the name is case-sensitive");
        assert!(
            refusal.contains("\"Gpu\"")
                && refusal.contains("\"cpu\"")
                && refusal.contains("\"gpu\""),
            "the refusal quotes the value and the accepted names: {refusal}"
        );
    }

    /// The host buffers of a "cpu" run: a resident outcome here is a routing bug.
    fn host_outputs(id: KernelId, count: usize) -> Vec<BufferWords> {
        match run_parallel_kernel(id, Backend::Cpu, vec![count as i64], vec![])
            .expect("the run must succeed")
        {
            RunOutcome::Host(outputs) => outputs,
            RunOutcome::Resident(_) => panic!("a \"cpu\" run must produce host buffers"),
        }
    }

    /// The fan-out itself: the worker count is what a test can see.
    #[test]
    fn a_run_over_the_threshold_fans_out_and_covers_every_index() {
        let id = intern_kernel(two_outputs(), None);
        let count = SEQUENTIAL_PARALLEL_ELEMENTS;
        let outputs = host_outputs(id, count);
        assert_eq!(outputs.len(), 2, "one buffer per declared output");
        // The machine must have the processors to fan out at all; on one processor
        // the run is the sequential one.
        if std::thread::available_parallelism().map_or(1, |n| n.get()) > 1 {
            assert!(
                parallel_launch_workers() > 1,
                "a run of {count} elements must not be the single-worker one"
            );
        }
        // First, middle and last index of each buffer: where a wrong rebase or a
        // skipped chunk would show.
        for index in [0, count / 2, count - 1] {
            assert_eq!(outputs[0].words[index], index as i64 + 1, "out0[{index}]");
            assert_eq!(outputs[1].words[index], index as i64 * 2, "out1[{index}]");
        }
    }

    /// The same kernel below the threshold, on the calling thread: the
    /// sequential result the fan-out has to reproduce.
    #[test]
    fn a_run_below_the_threshold_is_sequential_and_writes_every_index() {
        let id = intern_kernel(two_outputs(), None);
        let count = SEQUENTIAL_PARALLEL_ELEMENTS - 1;
        let outputs = host_outputs(id, count);
        assert_eq!(parallel_launch_workers(), 1, "no spawn below the threshold");
        for index in [0, count / 2, count - 1] {
            assert_eq!(outputs[0].words[index], index as i64 + 1, "out0[{index}]");
            assert_eq!(outputs[1].words[index], index as i64 * 2, "out1[{index}]");
        }
    }
}

// --- Native-op registry: the plugin's native-operator registry ---

/// The `lichen-compute` native plugin marker.
///
/// # Invariant
/// The trait carries no methods and nothing is generic-bound on it, so the `impl` is a
/// declaration: the contract is carried by the macro-based composition. A unit marker —
/// the plugin contributes its value and operator leaves and its native op registry, and
/// never names a concrete host program.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputePlugin;

impl lichen_highlevel::plugin::NativePlugin for ComputePlugin {}

/// Assemble `lichen-compute`'s private native-operator registry for a host program.
///
/// # Invariant
/// The table is leaked deliberately and the leak is bounded: fourteen entries, one
/// allocation per call, and the one call site runs once per store. It cannot be a
/// `static` — a `static` may not name a type parameter, and two invocations are
/// distinct items even for one program — and caching it on the host would serve a
/// compilation that already happens per store.
#[macro_export]
macro_rules! compute_native_ops {
    ($program:ty) => {{
        // The `static`s below are the operator structs, which are program-independent.
        static JIT: $crate::JitOp = $crate::JitOp;
        static LAUNCH: $crate::LaunchOp = $crate::LaunchOp;
        static CALL: $crate::CallOp = $crate::CallOp;
        static PARALLEL: $crate::ParallelOp = $crate::ParallelOp;
        static PARLAUNCH: $crate::ParLaunchOp = $crate::ParLaunchOp;
        static RANGE: $crate::RangeOp = $crate::RangeOp;
        static ROW: $crate::RowOp = $crate::RowOp;
        static COL: $crate::ColOp = $crate::ColOp;
        static READ: $crate::ReadOp = $crate::ReadOp;
        static WRITE: $crate::WriteOp = $crate::WriteOp;
        static WRITE_ATOMIC: $crate::WriteAtomicOp = $crate::WriteAtomicOp;
        static COLLECT: $crate::BufferCollectOp = $crate::BufferCollectOp;
        static BUF_TYPE: $crate::BufTypeOp = $crate::BufTypeOp;
        static BUF: $crate::BufOp = $crate::BufOp;
        static MMA: $crate::MmaOp = $crate::MmaOp;
        static GRAPH: $crate::GraphOp = $crate::GraphOp;
        static GRAPHRUN: $crate::GraphRunOp = $crate::GraphRunOp;
        let ops: Vec<(&'static str, &'static dyn $crate::NativeOp<$program>)> = vec![
            ("jit", &JIT as &dyn $crate::NativeOp<$program>),
            ("launch", &LAUNCH as &dyn $crate::NativeOp<$program>),
            ("call", &CALL as &dyn $crate::NativeOp<$program>),
            ("parallel", &PARALLEL as &dyn $crate::NativeOp<$program>),
            ("plrun", &PARLAUNCH as &dyn $crate::NativeOp<$program>),
            ("range", &RANGE as &dyn $crate::NativeOp<$program>),
            ("row", &ROW as &dyn $crate::NativeOp<$program>),
            ("col", &COL as &dyn $crate::NativeOp<$program>),
            ("read", &READ as &dyn $crate::NativeOp<$program>),
            ("write", &WRITE as &dyn $crate::NativeOp<$program>),
            (
                "write_atomic",
                &WRITE_ATOMIC as &dyn $crate::NativeOp<$program>,
            ),
            ("collect", &COLLECT as &dyn $crate::NativeOp<$program>),
            ("buf_type", &BUF_TYPE as &dyn $crate::NativeOp<$program>),
            ("buf", &BUF as &dyn $crate::NativeOp<$program>),
            ("mma", &MMA as &dyn $crate::NativeOp<$program>),
            ("graph", &GRAPH as &dyn $crate::NativeOp<$program>),
            ("graphrun", &GRAPHRUN as &dyn $crate::NativeOp<$program>),
        ];
        Box::leak(ops.into_boxed_slice()) as $crate::NativeOps<$program>
    }};
}

// --- Native operators: the private contract with the plugin's source -------

/// `$jit(f)` — compile a function to a kernel.
///
/// # Invariant
/// The function-ness gate unifies the argument's type with an arrow shape; the bare
/// artifact's type is a fresh cell, and the lichen wrapper builds the kernel struct
/// around it. The program marker is generic, so the `$jit`/`$launch` names stay private
/// to the plugin's embedded source.
pub struct JitOp;

/// `$launch(native, a, aty, kty)` — run kernel `native` on `a`.
///
/// # Invariant
/// The wrapper reads the kernel's `.I`/`.O` fields and passes the argument's type and
/// the declared domain as values; this op unifies them and emits the `Launch` node.
pub struct LaunchOp;

impl<P> NativeOp<P> for JitOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        // The operand is `[function]`, the shape every kernel operator reads.
        let f = &args[0];
        let operands = ctx.array_node(&[f.value]);
        // The bare native artifact, opaque: its type is the call's fresh cell.
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Jit), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for LaunchOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator> + From<LowOperator>,
{
    /// `$launch(native, a)` — run kernel `native` on `a`.
    ///
    /// # Invariant
    /// The wrapper states both constraints and hands over raw values; the op emits the
    /// `Launch` node over `[native, a]` and states no type at all.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let native = &args[0];
        let a = &args[1];
        // The `Launch` operator reads exactly these two elements: the kernel's
        // domain is not an operand.
        let operands = ctx.array_node(&[native.value, a.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Launch), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

/// `$call(k, a)` — a cross-kernel call on native kernel `k`.
///
/// # Invariant
/// The wrapper extracts `.native` out of the kernel struct; the op emits the `Call` node
/// over two raw values. The callee signature is read at launch-time assembly, so nothing
/// about the argument or the result is stated here.
pub struct CallOp;

impl<P> NativeOp<P> for CallOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let k = &args[0];
        let a = &args[1];
        let operands = ctx.array_node(&[k.value, a.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Call), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

/// `$parallel(f, backend)` — compile an index function into a parallel kernel.
///
/// # Invariant
/// The function-ness gate verifies `f`; the body is lowered over the loop index and the
/// parameter's `.in` buffers. A tuple codomain of `Write`s is the multi-output form:
/// a write's position becomes its output ordinal at emission, and the codomain's arity
/// the launch's output count.
pub struct ParallelOp;

/// `$plrun(pk, cfg, codomain)` — run a parallel kernel over `[0, cfg.n)` and collect
/// into the declared codomain.
pub struct ParLaunchOp;

/// `$range(n)` — the loop index `i ∈ [0, n)` of the current invocation.
pub struct RangeOp;

/// `$row(cfg)` — the row of the current invocation, over the launch's second extent.
pub struct RowOp;

/// `$col(cfg)` — the column of the current invocation, over the launch's second extent.
pub struct ColOp;

/// `$read(buf, i)` — read one buffer element → `?b`.
pub struct ReadOp;

/// `$write(n, i, val)` — a pending parallel write into the output buffer at `i`.
///
/// # Invariant
/// Which buffer is decided by the write's position in the codomain, not here.
pub struct WriteOp;

/// `$write_atomic(n, i, val)` — a pending parallel **atomic add** into the output at `i`.
///
/// # Invariant
/// Same operand shape as [`WriteOp`], and the same codomain position decides the buffer: the
/// difference is the instruction the body lowers to, not how the write reaches its output.
pub struct WriteAtomicOp;

/// `$collect(buf)` — collect a whole buffer into a lichen array `[?b]`.
pub struct BufferCollectOp;

/// `$buffer(array)` — copy an array's elements into a `Buf` payload.
///
/// # Invariant
/// The result is the `Buf` wrapper itself, whose `.element` slot is the array's element
/// type, so a buffer made here is the one buffer whose class is not a fresh cell. The
/// wrapper's `.native` slot is the `Buffer` node, exactly as `compute.plrun`'s is: the
/// read/write role walk and `buf_payload` know one carrier, not two.
pub struct BufTypeOp;

/// `(type, length)` — the native buffer **value**, kernel-only.
pub struct BufOp;

/// `$mma(s, a, b, c)` — the cooperative matrix product over three buffers.
///
/// # Invariant
/// The backend is a **required** argument, as `parallel`'s is: a program says where its
/// product runs, and the two paths answer the same two's-complement words.
pub struct MmaOp;

/// `$graph(f)` — record `f`'s dispatches into a graph instead of running them.
///
/// # Invariant
/// The function-ness gate is the same arrow the apply will check, so a non-function is
/// a check error here rather than a refusal at run time.
pub struct GraphOp;

/// `$graphrun(g, a)` — run a graph over the values its source function took.
///
/// # Invariant
/// Both types are fresh cells, deliberately: a graph's type is host-owned and opaque
/// — what it may be applied to is a question about the function it was recorded from,
/// and the recording is not available to the checker — so the arguments unify against
/// a fresh cell too. The cost is static precision, not safety: an out-of-range ordinal
/// is still refused at check time with a span.
pub struct GraphRunOp;

impl<P> NativeOp<P> for ParallelOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let f = &args[0];
        let backend = &args[1];
        // The bare parallel kernel artifact, opaque: its type is the call's fresh cell.
        let operands = ctx.array_node(&[f.value, backend.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Parallel), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for ParLaunchOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$plrun(native, a, codomain)` — run parallel kernel `native` over `a` (the `cfg`).
    ///
    /// # Invariant
    /// The result type is the call's own fresh cell, deliberately: the parameter's
    /// `.out` group decides the result's shape and cannot be read here, because `build`
    /// runs on the frozen template where the kernel's `.I`/`.O` are undecided. What
    /// that costs is static precision, not safety: an ordinal that does not exist is
    /// still refused at check time with a span.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let native = &args[0];
        let a = &args[1];
        // The declared codomain rides as the third operand: the result's layout is
        // the declaration's own.
        let mut items = vec![native.value, a.value];
        if let Some(declared) = args.get(2) {
            items.push(declared.value);
        }
        let operands = ctx.array_node(&items);
        let op = ctx.op_node(
            P::Operator::from(ComputeOperator::ParLaunch),
            Some(operands),
        );
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for RangeOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$range(n)` — the loop index of the current parallel invocation.
    ///
    /// # Invariant
    /// The index's class is the class the body computes in: a parallel fragment's two
    /// scalar parameters are both that class, and the host converts both roles back to
    /// ordinals. Nothing is stated here — the call's fresh cell is resolved from the
    /// value the fragment produces.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let n = &args[0];
        let operands = ctx.array_node(&[n.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Range), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

/// The node a lane-coordinate call builds: `op` over the launch it was handed.
///
/// # Invariant
/// The operand array is `[cfg]`, exactly `range`'s shape, and the lowering reads
/// neither: a body's coordinates are read from the launch's own role table when the
/// body is lowered (`compute/body.rs`, `second_extent`), which is the only place that
/// table is known.
fn coordinate_op<P>(ctx: &mut dyn Ctx<P>, op: ComputeOperator, cfg: NodeId) -> NativeApply
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    let operands = ctx.array_node(&[cfg]);
    let op = ctx.op_node(P::Operator::from(op), Some(operands));
    NativeApply {
        value: op,
        decided: false,
    }
}

impl<P> NativeOp<P> for RowOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$row(cfg)` — the row of the current parallel invocation.
    ///
    /// # Invariant
    /// [`coordinate_op`]'s: the launch is carried in the operand array and read by
    /// neither side, so what a body passes cannot change what it answers.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        coordinate_op(ctx, ComputeOperator::Row, args[0].value)
    }
}

impl<P> NativeOp<P> for ColOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$col(cfg)` — the column of the current parallel invocation.
    ///
    /// # Invariant
    /// [`RowOp::build`]'s, over [`ComputeOperator::Col`] — one node each, and the walk
    /// tells them apart by the operator rather than by an operand it would have to read.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        coordinate_op(ctx, ComputeOperator::Col, args[0].value)
    }
}

impl<P> NativeOp<P> for ReadOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$read(buf, i)` — read one buffer element.
    ///
    /// # Invariant
    /// The element class is a fact of the value and resolves there: the producing
    /// fragment's declared class, or the one a backend issued, decides the result cell —
    /// which is why a forwarded buffer prints `?a` and a `buffer<Float>` is readable.
    /// The index is an ordinal in every class: the `read` import is `(i64, i64) ->
    /// element`.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let b = &args[0];
        let i = &args[1];
        let operands = ctx.array_node(&[b.value, i.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Read), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for WriteOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$write(n, i, val)` — a pending parallel write into the output buffer at `i`.
    ///
    /// # Invariant
    /// The written value's class is the buffer's element class, so a `buffer<Float>` is
    /// expressible: the call's fresh cell resolves from the value the run produces. The
    /// length and the index are ordinals in every class — the `write` import is
    /// `(i64, i64, element)` — not data.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let n = &args[0];
        let i = &args[1];
        let val = &args[2];
        let operands = ctx.array_node(&[n.value, i.value, val.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Write), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for WriteAtomicOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$write_atomic(n, i, val)` — a pending atomic add into the output at `i`.
    ///
    /// # Invariant
    /// The operands are `write`'s, in the same order, so the codomain walk reads the buffer
    /// and the value from the same positions it reads a write's from.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let n = &args[0];
        let i = &args[1];
        let val = &args[2];
        let operands = ctx.array_node(&[n.value, i.value, val.value]);
        let op = ctx.op_node(
            P::Operator::from(ComputeOperator::WriteAtomic),
            Some(operands),
        );
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for BufferCollectOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let b = &args[0];
        // The result is an array of the buffer's element, class left open.
        let operands = ctx.array_node(&[b.value]);
        let op = ctx.op_node(
            P::Operator::from(ComputeOperator::BufferCollect),
            Some(operands),
        );
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for BufTypeOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$buf_type(type, length)` — the native buffer **type** `$buf(T, L)`.
    ///
    /// # Invariant
    /// A decided type term built where it is written: the shape is `[type, length]` and the
    /// kind carries the buffer marker, so a type position reads it without a run
    /// (`docs/notes/kernel-buffer-declarations.md`). It allocates nothing.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let shape = ctx.array_node(&[args[0].value, args[1].value]);
        let kind = ctx.kind_expr(ctx.buffer_marker_node());
        let term = ctx.expr(shape, kind);
        NativeApply {
            value: term,
            decided: true,
        }
    }
}

impl<P> NativeOp<P> for BufOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$buf(type, length)` — the native buffer **value**: the payload a declaration holds.
    ///
    /// # Invariant
    /// Kernel-only: the payload is what the runtime allocates, so the op is never evaluated
    /// on the host, and a body that declares a buffer lowers it to an output.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let operands = ctx.array_node(&[args[0].value, args[1].value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Buf), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for MmaOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$mma(s, a, b, c)` — the cooperative matrix product over three buffers.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let operands =
            ctx.array_node(&[args[0].value, args[1].value, args[2].value, args[3].value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Mma), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for GraphOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$graph(f)` — record `f`, don't run it.
    ///
    /// # Invariant
    /// The wrapper's `f : _ -> _` is the function-ness gate. The arity is deliberately
    /// not checked: it is a runtime fact of a value the checker has not cloned, and a
    /// gate naming an arity it cannot read would refuse programs the recording accepts.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let f = &args[0];
        // The graph is host-owned and opaque: what it may be run over is what the
        // recording found.
        let operands = ctx.array_node(&[f.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Graph), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

impl<P> NativeOp<P> for GraphRunOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$graphrun(g, a)` — run a graph over the values its source function took.
    ///
    /// # Invariant
    /// The arguments and the result are the call's own fresh cell, which is what makes a
    /// graph reusable across runs: a graph's parameter tuple is a runtime shape, so a
    /// fixed domain type would name an arity the checker cannot know. The cost is static
    /// precision, not safety — an out-of-range ordinal is still refused with a span.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let g = &args[0];
        let a = &args[1];
        let operands = ctx.array_node(&[g.value, a.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::GraphRun), Some(operands));
        // The full width is stated where the attribute order is known
        // (`docs/notes/compound-element-slots.md`).
        let full_width = ctx.full_width_layout();
        ctx.record_full_width_layout(full_width);
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

/// The `lichen-compute` plugin's embedded lichen source, kept as a `.lichen` file and
/// included with [`include_str!`].
///
/// # Invariant
/// It defines the user-facing `jit`/`launch` functions as ordinary typed lichen and
/// exports them as a named struct. Its `type_of` helper is a private `let` binding that
/// repeats `lichen-std`'s definition, because an embedded native source is compiled
/// against this plugin's private registry and cannot depend on a package.
pub const WRAPPER_SOURCE: &str = include_str!("compute.lichen");

/// The class roots of the nodes a parameter's **type** tree names.
///
/// # Invariant
/// The type node is the shape authority (`docs/notes/compound-element-slots.md`), so a
/// read's base is accepted when its class is one this set names; the walk is bounded by
/// [`DOMAIN_DEPTH`], which stops a cyclic encoding rather than a real shape.
fn domain_type_classes<P>(module: &Module<P>, roots: &[Option<AnyNodeId>]) -> HashSet<NodeId>
where
    P: Program,
    P::Value: AsEnum<LowValue> + ValueType,
{
    let mut classes = HashSet::new();
    let mut pending: Vec<(AnyNodeId, usize)> =
        roots.iter().flatten().map(|node| (*node, 0usize)).collect();
    while let Some((node, depth)) = pending.pop() {
        if depth > DOMAIN_DEPTH {
            continue;
        }
        let Some(value) = module.node_value(node) else {
            continue;
        };
        let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(&value) else {
            continue;
        };
        if let AnyNodeId::Dynamic(node) = node {
            classes.insert(module.class_root(node));
        }
        // SAFETY: the array is a value of a live node of `module`, and its items are
        // live nodes of the same module.
        for item in unsafe { array.items() } {
            pending.push((item.node, depth + 1));
        }
    }
    classes
}

/// The deepest a type tree walk descends.
const DOMAIN_DEPTH: usize = 8;
