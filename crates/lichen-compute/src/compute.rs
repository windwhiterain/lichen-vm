//! The `lichen-compute` native plugin.
//! See docs/notes/lichen-compute.md and compute-kernel-struct.md.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use lichen_graph_ir::Policy;
use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp};
use lichen_highlevel::program::{
    Ctx, HighProgram, LeafKindMarkers, TypeOperator, TypeValue, ValueType,
};
use lichen_highlevel::shape::{
    PAIR_ATTR_BASE, PAIR_TYPE_SLOT, PAIR_VALUE_SLOT, TypeRef, array_items as array_items_any,
    field_list, field_names, field_type, low_type_of_slot,
};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBin, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    ResidentId, ScalarClass, ScalarData, fragment_digest,
};
use lichen_lowlevel::codec::{OperatorCodec, Reader, ValueCodec, Writer};
use lichen_lowlevel::{
    AnyFunctionId, AnyHandle, AnyNodeId, ArrayItem, BlockId, FunctionId, LowOperator, LowShape,
    LowValue, Module, ModuleKey, NodeId, Operation, OperatorExt, Program, Release, StaticModule,
    ValueExt,
};
use lichen_utils::extend::AsEnum;

mod body;
use body::Lower;
pub mod graph;
pub(crate) mod wasm;

use graph::{Extent, Placed, RunArgument, RunResult};
use wasm::assemble_module;

pub use graph::GraphId;

/// The diagnostic a graph's own refusals are recorded under.
///
/// # Invariant
/// Every graph refusal is about the recording, never about a run, so filing one
/// under `compute.parallel` would blame the launch the author did not write.
pub const GRAPH_DIAGNOSTIC: &str = "compute.graph";

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

/// The process kernel registry: compiled fragments, keyed by [`KernelId`].
///
/// # Invariant
/// Kernels are immutable artifacts shared across modules; the fragment is the
/// durable JIT output and the module bytes are derived on demand at launch.
static KERNELS: OnceLock<Mutex<HashMap<KernelId, KernelFragment>>> = OnceLock::new();
fn kernels() -> &'static Mutex<HashMap<KernelId, KernelFragment>> {
    KERNELS.get_or_init(Default::default)
}
/// The fallback id allocator — monotone, so an id minted here never aliases.
static NEXT_KERNEL_ID: AtomicUsize = AtomicUsize::new(0);
fn alloc_kernel_id() -> KernelId {
    NEXT_KERNEL_ID.fetch_add(1, Ordering::Relaxed)
}

/// The content index: a fragment's digest → the id it is registered under.
///
/// # Invariant
/// Ids are content-addressed (`D15`): the same source recompiled yields the same
/// id, which the derived-module cache is keyed on. A digest is not an identity —
/// the index is an intern table, [`intern_kernel`] verifies the fragment, and a
/// collision falls back to a unique id, costing a recompile and never a wrong
/// fragment.
static KERNEL_INDEX: OnceLock<Mutex<HashMap<u64, KernelId>>> = OnceLock::new();
fn kernel_index() -> &'static Mutex<HashMap<u64, KernelId>> {
    KERNEL_INDEX.get_or_init(Default::default)
}

/// Register `fragment` and return its id — the existing id when an identical
/// fragment is already registered.
///
/// # Invariant
/// The lookup and the insert are one decision, so both registries are held
/// across it: otherwise a concurrent intern of the same digest can land its own
/// id after this caller read the entry, handing out a second id for one
/// fragment. Lock order is `kernels()` before `kernel_index()`.
fn intern_kernel(fragment: KernelFragment) -> KernelId {
    let digest = fragment_digest(&fragment);
    // Lock order: `kernels()` then `kernel_index()`, as the doc says.
    let mut registry = kernels().lock().unwrap();
    let mut index = kernel_index().lock().unwrap();
    if let Some(id) = index.get(&digest).copied()
        && registry.get(&id) == Some(&fragment)
    {
        return id;
    }
    let id = alloc_kernel_id();
    registry.insert(id, fragment);
    index.insert(digest, id);
    id
}

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
            int_width: IntWidth::I64,
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
        let first = intern_kernel(fragment(one.clone()));
        let second = intern_kernel(fragment(one));
        assert_eq!(
            first, second,
            "a recompiled identical fragment must reuse its id"
        );
    }

    /// ...and different fragments must not collide onto one id, or the cache
    /// would serve one kernel's module for another's.
    #[test]
    fn a_different_fragment_gets_a_different_id() {
        let a = intern_kernel(fragment(KernelBody::from_flat(1, &[FlatOp::Read(0)])));
        let b = intern_kernel(fragment(KernelBody::from_flat(2, &[FlatOp::Read(1)])));
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
            for item in items {
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
        // A buffer is runtime-only: the codec refuses to serialize a frozen one.
        AnyHandle::Static(_) => None,
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
            module.record_extension_diagnostic(category, None, reason);
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
    /// `[buffer, index]` — read one buffer element → `?b`.
    Read,
    /// `[length, index, value]` — a pending parallel write; kernel-only.
    Write,
    /// `[buffer]` operand — collect the whole buffer into a lichen array `[?b]`.
    BufferCollect,
    /// `[function]` — record the dispatches that function performs, and hand back
    /// a `Graph`.
    ///
    /// # Invariant
    /// The operand is deliberately not evaluated first: the function's body is
    /// what has to be walked, and a deep pass has already erased the shape that
    /// says which nodes are the body's own. This is the one operator that
    /// overrides [`OperatorExt::run_deferred`].
    Graph,
    /// `[graph, arguments]` — run a graph over its source function's values.
    GraphRun,
}

// Compute values are runtime-only: a frozen module is a type artifact, and
// these arms refuse rather than panic.

impl ValueCodec for ComputeValue {
    fn write_value<P: Program>(
        _w: &mut Writer,
        value: Self,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<(), String> {
        match value {
            // The value's own name, so a reader can tell which of the three it
            // met without a debugger.
            ComputeValue::Kernel(_) => {
                return Err(
                    "this package is not cached: it holds a jit'd kernel at its top level, \
                     and a kernel is a runtime value with no on-disk form"
                        .into(),
                );
            }
            ComputeValue::ParKernel(..) => {
                return Err(
                    "this package is not cached: it holds a jit'd parallel kernel at its top \
                     level, and a kernel is a runtime value with no on-disk form"
                        .into(),
                );
            }
            ComputeValue::Buffer(..) => {
                return Err(
                    "this package is not cached: it holds a live buffer at its top level, \
                     and a buffer is a runtime value with no on-disk form"
                        .into(),
                );
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
        _self_key: ModuleKey,
        _self_arena: &[u8],
        _self_base: *const u8,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<Self, String> {
        // No tag reaches here: every compute value is runtime-only, and
        // `write_value` refuses each of them.
        Err(format!("unknown compute-value tag {}", r.u8()?))
    }
}

impl OperatorCodec for ComputeOperator {
    fn write_operator(_w: &mut Writer, op: Self) -> Result<(), String> {
        match op {
            // An operator is an operation the VM performs, not a value an artifact
            // describes; refused like a kernel value.
            ComputeOperator::Jit
            | ComputeOperator::Launch
            | ComputeOperator::Call
            | ComputeOperator::Parallel
            | ComputeOperator::ParLaunch
            | ComputeOperator::Range
            | ComputeOperator::Read
            | ComputeOperator::Write
            | ComputeOperator::Graph
            | ComputeOperator::GraphRun
            | ComputeOperator::BufferCollect => Err(
                "this package is not cached: it holds a compute operation, and a compute \
                 operation is a runtime form with no on-disk representation"
                    .into(),
            ),
        }
    }

    fn read_operator(r: &mut Reader<'_>) -> Result<Self, String> {
        Err(format!("unknown compute-operator tag {}", r.u8()?))
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
                module.record_extension_diagnostic(GRAPH_DIAGNOSTIC, None, reason);
                return None;
            }
            match self {
                ComputeOperator::Jit => {
                    let Some(LowValue::Function(function)) = AsEnum::<LowValue>::as_enum(&operand)
                    else {
                        // A non-function jit target is a reported type error; stay
                        // lazy rather than panicking.
                        return None;
                    };
                    match compile_fragment(module, function) {
                        Ok(fragment) => {
                            // Content-addressed (`D15`): a recompile keeps one id,
                            // so the derived-module cache hits.
                            let id = intern_kernel(fragment);
                            Some(<P::Value as From<ComputeValue>>::from(
                                ComputeValue::Kernel(id),
                            ))
                        }
                        Err(err) => {
                            // Outside the kernel-safe subset, or an undecided
                            // domain: a lazy value plus the reason.
                            module.record_extension_diagnostic(JIT_DIAGNOSTIC, None, err);
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
                    // A scalar for an arity-1 kernel, an `Array` for a tuple-domain
                    // one; anything else stays lazy, and records why.
                    let args = match kernel_arguments(module, operands[1].node) {
                        Ok(args) => args,
                        Err(reason) => {
                            module.record_extension_diagnostic(
                                KERNEL_LAUNCH_DIAGNOSTIC,
                                None,
                                reason,
                            );
                            return None;
                        }
                    };
                    match run_kernel(id, &args) {
                        Ok(results) => kernel_results_value(module, block, results),
                        Err(err) => {
                            // Whatever the wasm run said is recorded as it stands,
                            // not replaced by a summary that names none of them.
                            module.record_extension_diagnostic(KERNEL_LAUNCH_DIAGNOSTIC, None, err);
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
                    let args = match kernel_arguments(module, operands[1].node) {
                        Ok(collected) => collected,
                        Err(reason) => {
                            module.record_extension_diagnostic(
                                KERNEL_LAUNCH_DIAGNOSTIC,
                                None,
                                reason,
                            );
                            return None;
                        }
                    };
                    match run_kernel(id, &args) {
                        Ok(results) => kernel_results_value(module, block, results),
                        Err(err) => {
                            // As in `Launch`: the run's own message is this
                            // refusal's cause, so it is recorded as it stands.
                            module.record_extension_diagnostic(KERNEL_LAUNCH_DIAGNOSTIC, None, err);
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
                            // Content-addressed like `jit`'s: the backend is not in
                            // the fragment, so both backends share one id.
                            let id = intern_kernel(fragment);
                            Some(<P::Value as From<ComputeValue>>::from(
                                ComputeValue::ParKernel(id, backend),
                            ))
                        }
                        Err(err) => {
                            // An unresolved parameter annotation leaves the operator
                            // undecided and silent — not an error: a later pass
                            // compiles it.
                            if err == PARALLEL_PARAMETER_UNDECIDED {
                                return None;
                            }
                            module.record_extension_diagnostic(PARALLEL_DIAGNOSTIC, None, err);
                            None
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
                            module.record_extension_diagnostic(PARALLEL_DIAGNOSTIC, None, err);
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
                                module.record_extension_diagnostic(
                                    PARALLEL_DIAGNOSTIC,
                                    None,
                                    if position == 0 {
                                        "the launch count is Float, but a dispatch extent is Int: \
                                     Int and Float do not convert"
                                    } else {
                                        "a parallel parameter's scalar leaf is Int here and the \
                                     launch passes a Float for it: Int and Float do not convert"
                                    },
                                );
                                return None;
                            }
                            (ScalarClass::Float, Some(LowValue::USize(_))) => {
                                module.record_extension_diagnostic(
                                    PARALLEL_DIAGNOSTIC,
                                    None,
                                    "a parallel parameter's scalar leaf is Float here and the \
                                 launch passes an Int for it: Int and Float do not convert",
                                );
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
                    let mut inputs: Vec<RunInput> = Vec::new();
                    if !facts.roles.inputs.is_empty() {
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
                            if !facts.roles.outputs.is_empty() {
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
                                return build_outputs::<P>(module, block, &facts.roles, &outputs);
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
                                    handle,
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
                            if !facts.roles.outputs.is_empty() {
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
                                return build_outputs::<P>(module, block, &facts.roles, &outputs);
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
                                    handle,
                                )));
                            }
                            Some(value(results[0]))
                        }
                        Err(err) => {
                            // The refusal is recorded: `BudgetExhausted`'s renderings
                            // would say "never terminates", which is false here.
                            module.record_extension_diagnostic(PARALLEL_DIAGNOSTIC, None, err);
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
                    // The operand is a `Buf` value — the wrapper the type level
                    // reads — so the buffer it names is that wrapper's payload.
                    let buffer =
                        buf_payload::<P>(module, operands[0].node).unwrap_or(operands[0].node);
                    let element = match module
                        .node_value(buffer)
                        .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                    {
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
                                        PARALLEL_DIAGNOSTIC,
                                        None,
                                        err,
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
                ComputeOperator::Range | ComputeOperator::Write => {
                    // Kernel-only operators: `range`/`write` never reach the VM as a
                    // standalone apply — stay lazy.
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
                    // chain's results cross the bus.
                    let buffer =
                        buf_payload::<P>(module, operands[0].node).unwrap_or(operands[0].node);
                    let results: ScalarData = match module
                        .node_value(buffer)
                        .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                    {
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
                                ScalarClass::Float => ScalarData::Float(
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
                                        PARALLEL_DIAGNOSTIC,
                                        None,
                                        err,
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
                            .collect(),
                        ScalarData::Float(values) => values
                            .into_iter()
                            .map(|value| scalar_item::<P>(module, block, LowValue::Float(value)))
                            .collect(),
                    };
                    let handle = module.alloc_array(&items, block);
                    Some(<P::Value as From<LowValue>>::from(LowValue::Array(handle)))
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
    /// from the decided cell beside it ([`seed_template_term_low_types`]).
    fn low_type(&self, _arguments: &[Option<LowShape>]) -> Option<LowShape> {
        match self {
            ComputeOperator::Launch
            | ComputeOperator::Call
            | ComputeOperator::Range
            | ComputeOperator::Read => Some(LowShape::USize),
            ComputeOperator::Jit
            | ComputeOperator::Parallel
            | ComputeOperator::ParLaunch
            | ComputeOperator::Write
            // `Graph` is host-owned and `GraphRun`'s length is the graph's own
            // return: both decline like `Jit`/`BufferCollect`.
            | ComputeOperator::Graph
            | ComputeOperator::GraphRun
            | ComputeOperator::BufferCollect => None,
        }
    }
}

// --- Codegen: lichen graph → a scalar `(i64) -> i64` wasm function body -----

/// One parameter group of a kernel being emitted.
///
/// # Invariant
/// A type slot's type may be the `[shape, kind]` term itself or a node that holds
/// one; which is the encoding's question, so both are asked and the decode
/// decides ([`shape::TypeRef`]). The decode is the strong one — [`field_names`] —
/// so every reader of the parameter's field list reads the same node.
fn parameter_type_ref<P>(module: &Module<P>, slot: AnyNodeId) -> Option<TypeRef>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    [TypeRef::Term(slot), TypeRef::Slot(slot)]
        .into_iter()
        .find(|ty| field_names(module, *ty).is_some())
}

/// The named fields and field-type list of the type a **type slot** names.
fn struct_fields_of_slot<P>(
    module: &Module<P>,
    slot: AnyNodeId,
) -> Option<(Vec<Option<&'static str>>, AnyNodeId)>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let ty = parameter_type_ref(module, slot)?;
    let names = field_names(module, ty)?;
    let shape = field_list(module, ty)?;
    Some((names, shape))
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
    cfg_pair: NodeId,
    roles: &KernelRoles,
) -> Result<Vec<ScalarClass>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // SAFETY: `cfg_pair` is a live node of `module`.
    let ty = unsafe { module.array_items(cfg_pair) }
        .and_then(|items| items.get(PAIR_TYPE_SLOT).map(|item| item.node))
        .and_then(|slot| parameter_type_ref(module, slot));
    // A scalar role is a **direct** field (`[field]`), so its one position is the
    // field the class is read from.
    let class_of_role = |module: &Module<P>, path: &[usize]| -> ScalarClass {
        path.first()
            .and_then(|&at| field_type(module, ty?, at))
            .map(|field| scalar_class_of(&low_type_of_slot(module, field)))
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

/// The role table of a parallel kernel's parameter struct, decoded from the
/// parameter's **type slot**.
///
/// # Invariant
/// `Ok(None)` when the parameter is not a named struct term (the older
/// `cfg = (n, (buffers…))` shape); `Err` when it is one but lacks both reserved
/// fields, because a fallback would silently read it as the old shape.
fn parallel_roles<P>(module: &mut Module<P>, cfg_pair: NodeId) -> Result<KernelRoles, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // SAFETY: `cfg_pair` is a live node of `module`.
    let Some(items) = (unsafe { module.array_items(cfg_pair) }) else {
        return Err(PARALLEL_PARAMETER_UNDECIDED.into());
    };
    let Some(type_slot) = items.get(PAIR_TYPE_SLOT).map(|item| item.node) else {
        return Err(PARALLEL_PARAMETER_UNDECIDED.into());
    };
    // Undecided is not a mistake: a kernel compiled before its annotation
    // resolved waits for a later pass.

    // The type slot holds either the type term or a node that holds one; the
    // decode decides which ([`shape::TypeRef`]).
    let Some((names, shape)) = struct_fields_of_slot(module, type_slot) else {
        return Err(PARALLEL_PARAMETER_UNDECIDED.into());
    };
    let named = |wanted: &str| names.iter().position(|name| *name == Some(wanted));
    // A reserved group may be absent or written empty: the walk treats both
    // alike, so neither needs a filler field.
    let inputs_at = named("in");
    let outputs_at = named("out");
    // SAFETY: `shape` is a live node of `module`.
    let Some(fields) = (unsafe { array_items_any(module, shape) }) else {
        return Err(PARALLEL_PARAMETER_UNDECIDED.into());
    };
    let fields: Vec<AnyNodeId> = fields.iter().map(|item| item.node).collect();
    let mut roles = KernelRoles::default();
    for (field, &field_type) in fields.iter().enumerate() {
        let role = match field {
            at if inputs_at == Some(at) => LeafRole::Input,
            at if outputs_at == Some(at) => LeafRole::Output,
            _ => LeafRole::Scalar,
        };
        walk_role(module, field_type, &[field], role, &mut roles)?;
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

/// Whether a struct's field names are the `Buf` wrapper's.
fn is_buf_shape(names: &[Option<&'static str>]) -> bool {
    names
        .iter()
        .filter_map(|name| *name)
        .eq([BUF_NATIVE_FIELD, "element"])
}

/// Record the role of one field of a parameter type, descending into groups.
///
/// # Invariant
/// A buffer is a `Buf`-shaped struct and its recorded path ends at the `.native`
/// slot, which is the payload the prelude's `$read`/`$write` names; any other
/// struct is a group, walked in field order. An unresolvable leaf is refused by
/// name with its path.
fn walk_role<P>(
    module: &mut Module<P>,
    field: AnyNodeId,
    path: &[usize],
    role: LeafRole,
    roles: &mut KernelRoles,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    if let Some((names, shape)) = struct_fields_of_slot(module, field) {
        if is_buf_shape(&names) {
            // The path is the `Buf` field, as the checker resolves it for the
            // source's read; the payload is one step in.
            return match role {
                LeafRole::Input => {
                    roles.inputs.push(path.to_vec());
                    Ok(())
                }
                LeafRole::Output => {
                    roles.outputs.push(path.to_vec());
                    Ok(())
                }
                LeafRole::Scalar => Err(role_refusal(path, "a buffer outside `.in`/`.out`")),
            };
        }
        // SAFETY: `shape` is a live node of `module`.
        let Some(fields) = (unsafe { array_items_any(module, shape) }) else {
            return Err(role_refusal(
                path,
                "a struct whose field list cannot be read",
            ));
        };
        let fields: Vec<AnyNodeId> = fields.iter().map(|item| item.node).collect();
        for (at, field) in fields.into_iter().enumerate() {
            let mut path = path.to_vec();
            path.push(at);
            walk_role(module, field, &path, role, roles)?;
        }
        return Ok(());
    }
    let shape = low_type_of_slot(module, field);
    match (role, &shape) {
        // An output is produced by a write, and a write produces a buffer: any
        // other output type has no mechanism.
        (LeafRole::Output, _) => Err(role_refusal(
            path,
            &format!("an output that is a {}", shape_name(&shape)),
        )),
        (_, LowShape::USize | LowShape::Float | LowShape::Unknown) => {
            roles.scalars.push(path.to_vec());
            Ok(())
        }
        (_, other) => Err(role_refusal(path, &format!("a {}", shape_name(other)))),
    }
}

/// The name a shape goes by in a refusal, so the refusal says what it met.
fn shape_name(shape: &LowShape) -> &'static str {
    match shape {
        LowShape::USize => "runtime scalar",
        LowShape::Float => "runtime float",
        LowShape::Array(..) => "array",
        LowShape::Tuple(_) => "tuple",
        LowShape::Function(..) => "function",
        LowShape::Table(..) => "table",
        LowShape::Unknown => "leaf with no stated type",
    }
}

/// The refusal a role walk gives: the path it was walking, and what it found.
fn role_refusal(path: &[usize], what: &str) -> String {
    let path: Vec<String> = path.iter().map(usize::to_string).collect();
    format!(
        "a parallel kernel's parameter holds {what} at position {}, and the ABI has no place for \
         it",
        path.join(".")
    )
}

/// A parallel kernel whose parameter's type has not resolved yet.
///
/// # Invariant
/// The annotation is still a cell with no value, so there is nothing to read and
/// nothing to report: the operator answers undecided, and a later pass compiles
/// the kernel once the annotation states the struct.
const PARALLEL_PARAMETER_UNDECIDED: &str =
    "a parallel kernel's parameter type has not resolved yet";

/// A parameter slot in a kernel's wasm signature.
///
/// # Invariant
/// A scalar `jit` kernel has one slot; a parallel kernel has the config slot then
/// the index slot. Each slot carries the `[value, type]` pair, the value node, its
/// flattened domain shape, and the wasm local base offset.
#[derive(Clone)]
struct ParamSlot {
    /// The `[value, type]` parameter pair node.
    pair: NodeId,
    /// The parameter's value node (`Index(pair, 0)`), where the shape marker is.
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
        let Some(path) = param_path(module, slot.pair, node)? else {
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

/// Lower `[param_pair] → function.return` for the kernel-safe subset into a
/// [`KernelFragment`].
///
/// # Invariant
/// A kernel must be explicitly specialized: this runs before any apply, so the
/// parameter's type is where the language states the class — a body whose class
/// is left open is refused by name ([`UNDECIDED_DOMAIN`]). The domain is seeded,
/// passed, then read back, and the seed is a low-type write, never a type cell.
fn compile_fragment<P>(
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
    let param_pair = module.functions[fid].parameter;
    let ret = module.functions[fid].r#return;
    // A body's return is its `[value, type]` pair; the result is the pair's value.

    // SAFETY: `ret` is a live node of `module`; nothing here drops a block.
    let ret_value = match unsafe { module.array_items(ret) } {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => ret,
    };
    // The parameter's two cells, read once before the pass needs the module.

    // SAFETY: `param_pair` is a live node of `module`; nothing here drops a block.
    let (param_value, param_type) = match unsafe { module.array_items(param_pair) } {
        Some(items) if !items.is_empty() => (
            dyn_node(items[PAIR_VALUE_SLOT].node)?,
            items.get(PAIR_TYPE_SLOT).map(|item| item.node),
        ),
        _ => return Err("parameter is not a [value, type] pair".into()),
    };

    // The class comes from the parameter's type alone; without one, `Unknown`,
    // which the read refuses by name.
    let seed = match param_type.map(|slot| low_type_of_slot(module, slot)) {
        Some(shape) if shape.is_known() => shape,
        _ => LowShape::Unknown,
    };
    // Whether the parameter states its domain, read before the seed and pass
    // write anything.
    module.seed_class_low_type(param_value, seed);
    // 2. Pass.
    module.infer_template_low_types(fid);
    // 3. Read.
    let Some(domain) = module.low_type_of_node(param_value) else {
        return Err(UNDECIDED_DOMAIN.into());
    };
    // What the parameter's type states — the fact the domain check needs and the
    // decoded shape cannot supply.
    let statement = match param_type {
        Some(slot) if struct_fields_of_slot(module, slot).is_some() => DomainStatement::Struct,
        Some(slot) if matches!(low_type_of_slot(module, slot), LowShape::Tuple(_)) => {
            DomainStatement::Shape
        }
        _ => DomainStatement::Absent,
    };
    let param_shape = kernel_domain(domain, statement)?;
    let params = vec![ParamSlot {
        pair: param_pair,
        value: param_value,
        shape: param_shape.clone(),
        base: 0,
        // A scalar kernel's parameter is its domain, not a struct of roles.
        roles: None,
    }];

    // A scalar kernel has no buffers: the fragment declares no inputs or outputs.
    let mut tally = Positions::default();
    // The body is emitted leaf by leaf: a scalar codomain is one leaf, a tuple
    // one per element, each its own stack slot.
    let leaves = codomain_leaves(module, ret_value)?;
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
    let body = Lower::lower(module, &params, param_value, fid, ret_value, &mut tally)?;

    Ok(KernelFragment {
        param_shape: kernel_shape(&param_shape),
        // A scalar kernel's parameter is its domain: there are no named roles for
        // a walk to find, so the lists are empty.
        roles: KernelRoles::default(),
        body,
        inputs: tally.reads,
        outputs: tally.writes,
        input_classes: tally.input_classes(tally.reads),
        // A scalar fragment has no output buffers, so this is empty: its values
        // are the wasm results below.
        output_classes: Vec::new(),
        result_classes,
        int_width: IntWidth::I64,
    })
}

/// The body's value resolved into the leaves a wasm body must leave on the stack.
///
/// # Invariant
/// A bare value node is the single-leaf form and a materialized tuple value the
/// multi-leaf one; a *nested* tuple is not flattened, because each leaf is one
/// value on the stack — it is refused by name with the path instead.
fn codomain_leaves<P>(module: &Module<P>, ret_value: NodeId) -> Result<Vec<NodeId>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // Peel the `value_of` extraction first: asking it whether it is a tuple
    // would answer for the pair it wraps.
    let value = value_of_node(module, ret_value).unwrap_or(ret_value);
    // SAFETY: `value` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = match unsafe { module.array_items(value) } {
        Some(items) if !items.is_empty() => items,
        _ => return Ok(vec![ret_value]),
    };
    let mut leaves = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let element = dyn_node(item.node)?;
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
fn is_tuple_value<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let value = value_of_node(module, node).unwrap_or(node);
    // SAFETY: `value` is a live node of `module`.
    matches!(unsafe { module.array_items(value) }, Some(items) if !items.is_empty())
}

/// Lower a single-arg index function into a parallel kernel ([`KernelFragment`]).
///
/// # Invariant
/// The every-ordinal-written rule holds by construction: the lowered body is
/// straight-line, the only conditional in the subset is the emitter's 2-element
/// scalar `select`, and a `select` can skip only a *value* — a write reached
/// inside one is refused by name, so write `k` runs on every index.
fn compile_parallel_fragment<P>(
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
    let cfg_pair = module.functions[fid].parameter;
    let body = module.functions[fid].r#return;
    // The kernel's result is the body value — a `Write`, or a tuple of them.

    // SAFETY: `body` is a live node of `module`.
    let ret_value = match unsafe { module.array_items(body) } {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => body,
    };
    // A named struct parameter carries the roles; the older tuple `cfg` does not.

    // The role table is decoded from the parameter's type: `low_type_of`
    // answers `Unknown` for every struct by design.
    let roles = parallel_roles(module, cfg_pair)?;
    // The output count is the codomain's arity, fixed before any index runs.
    let outputs = parallel_output_nodes(module, ret_value);
    // The slot is seeded with the ABI's signature: the parameter's leaves then
    // the index, each leaf classed by its field.
    let cfg_value = pair_value_half(module, cfg_pair)
        .ok_or_else(|| "parallel cfg parameter is not a [value, type] pair".to_string())?;
    // Each scalar leaf's class is the parameter field's, not an all-`Int` seed.
    let scalar_classes = scalar_leaf_classes(module, cfg_pair, &roles)?;
    module.seed_class_low_type(
        cfg_value,
        LowShape::Tuple(scalar_classes.iter().copied().map(low_shape_of).collect()),
    );
    seed_template_term_low_types(module, fid);
    module.infer_template_low_types(fid);
    let Some(cfg_shape) = module.low_type_of_node(cfg_value) else {
        return Err(UNDECIDED_DOMAIN.into());
    };
    // The parameter is the declared named struct: a leaf with no class is
    // refused by name.
    let cfg_shape = kernel_domain(cfg_shape, DomainStatement::Shape)?;
    let params = vec![ParamSlot {
        pair: cfg_pair,
        value: cfg_value,
        shape: cfg_shape,
        base: 0,
        roles: Some(roles.clone()),
    }];
    // The class a buffer read is declared in: the first write's class, a fallback.

    // Read through the slots like the emission ([`node_class_in`]), or a consumer
    // would refuse the chain.
    let class = outputs
        .iter()
        .filter_map(|output| {
            write_value_node(module, *output).map(|value| node_class_in(module, &params, value))
        })
        .next()
        .unwrap_or(ScalarClass::Int);
    // **The outputs are the codomain's**, one `compute.write` per position in position
    // order.
    let mut tally = Positions::default();
    // The class a buffer read is declared in; see [`Positions::element_class`].
    tally.element_class = Some(class);
    // **A conditional is left to the walk**, which knows *why* it did not reduce the
    // write.
    for (position, output) in outputs.iter().enumerate() {
        let conditional = match module.define_in(fid, *output) {
            lichen_lowlevel::Define::Computed(definition) => matches!(
                module.selection_of(definition),
                Some(lichen_lowlevel::Selection::Computed)
            ),
            _ => false,
        };
        if !conditional && write_value_node(module, *output).is_none() {
            return Err(format!(
                "output {position} of the parallel index function is not a `compute.write` \
                 (an index function must write every output it declares)"
            ));
        }
    }
    let body_instr =
        Lower::lower_index_function(module, &params, cfg_value, fid, &outputs, class, &mut tally)?;
    if tally.writes != outputs.len() {
        return Err(format!(
            "a parallel index function emitted {} write(s) but its codomain names {} \
             output(s): every output must be exactly one `compute.write`",
            tally.writes,
            outputs.len()
        ));
    }
    if outputs.len() != roles.outputs.len() {
        return Err(format!(
            "this index function's codomain names {} output(s) but its parameter declares {} \
             `.out` field(s): the two are the same list, and a struct parameter states it in \
             the type",
            outputs.len(),
            roles.outputs.len()
        ));
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
        int_width: IntWidth::I64,
    })
}

/// Seed every class a template's own terms decide, from the type cell each term
/// carries.
///
/// # Invariant
/// A template is never evaluated, so nothing in the body is observed — but the
/// checker wrote the decision into every term's type cell, and without this seed
/// the whole body reads `Unknown` and every class falls back to `Int`. Both reads
/// of a term are seeded, a cell nothing decides stays undecided, and a
/// disagreement degrades to `Unknown`.
fn seed_template_term_low_types<P>(module: &mut Module<P>, function: FunctionId)
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    for node in module.functions[function].nodes.clone() {
        // A term is the node's value: the checker's `[value, type, attrs…]` array.

        // SAFETY: `node` is a live node of `module`; nothing here drops a block.
        let Some(items) = (unsafe { module.array_items(node) }) else {
            continue;
        };
        if items.len() < PAIR_ATTR_BASE {
            continue;
        }
        let shape = low_type_of_slot(module, items[PAIR_TYPE_SLOT].node);
        if !shape.is_known() {
            continue;
        }
        let Ok(value) = dyn_node(items[PAIR_VALUE_SLOT].node) else {
            continue;
        };
        module.seed_class_low_type(value, shape.clone());
        module.seed_class_low_type(node, LowShape::Tuple(vec![shape, LowShape::Unknown]));
    }
}

/// The value a `compute.write` position writes — the expression whose class the
/// index function computes in.
///
/// # Invariant
/// A position reaches the emitter wrapped: the slot-read destructuring leaves a
/// constant `Index` over a materialized array, and that element is reached
/// through the `[value, type]` pair or a `value_of` extraction. All three are
/// walked here, before the instructions are emitted.
fn write_value_node<P>(module: &Module<P>, node: NodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut node = AnyNodeId::Dynamic(
        value_of_node(module, node)
            .or_else(|| pair_value_half(module, node))
            .unwrap_or(node),
    );
    for _ in 0..8 {
        let current = node.dynamic()?;
        if let Some(operation) = module.node_operation(current)
            && matches!(
                AsEnum::<ComputeOperator>::as_enum(&operation.operator),
                Some(ComputeOperator::Write)
            )
        {
            let items = operand_items(module, operation.operand?).ok()?;
            return Some(items.get(2)?.node);
        }
        node = concrete_element(module, current)?;
    }
    None
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
    let (target, index) = operand_pair(module, operation.operand).ok()?;
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
fn parallel_output_nodes<P>(module: &Module<P>, ret_value: NodeId) -> Vec<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // A `value_of` extraction still carries the pair's value: peel it before
    // asking whether the rest is a tuple.
    let value = value_of_node(module, ret_value).unwrap_or(ret_value);
    // SAFETY: `value` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = match unsafe { module.array_items(value) } {
        Some(items) if !items.is_empty() => items,
        _ => return vec![ret_value],
    };
    let mut nodes = Vec::with_capacity(items.len());
    for item in items {
        match dyn_node(item.node) {
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
/// # Invariant
/// A leaf's own class, not the fragment's: an `array<Float, 3>` position is one
/// `f32` local inside a body that may compute integers, and the argument encoding
/// follows the same per-leaf answer.
pub(crate) fn param_classes(fragment: &KernelFragment) -> Vec<ScalarClass> {
    fn walk(shape: &KernelShape, out: &mut Vec<ScalarClass>) {
        match shape {
            KernelShape::Scalar(class) => out.push(*class),
            KernelShape::Tuple(items) => items.iter().for_each(|item| walk(item, out)),
        }
    }
    let mut classes = Vec::with_capacity(fragment.param_shape.flat_arity());
    walk(&fragment.param_shape, &mut classes);
    classes
}

/// The host import name a buffer call of `class` resolves to.
///
/// # Invariant
/// A wasm import has one signature, so one `read` could not serve both classes:
/// the pair is declared per class, and this is where the name is spelled.
pub(crate) fn buffer_import_name(class: ScalarClass, kind: &str) -> String {
    let class = match class {
        ScalarClass::Int => "i64",
        ScalarClass::Float => "f32",
    };
    format!("{kind}_{class}")
}

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
parameter (for example `p : <Int, Int>` for a tuple domain, or `y : Int` for a scalar one)";

/// The other facts [`UNDECIDED_DOMAIN`] used to cover.
///
/// # Invariant
/// A struct's low shape is `Unknown` by design, so the old single refusal told an
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
fn kernel_domain(domain: LowShape, statement: DomainStatement) -> Result<LowShape, String> {
    if !domain_is_known(&domain) {
        return Err(match statement {
            DomainStatement::Struct => DOMAIN_IS_A_STRUCT.into(),
            DomainStatement::Shape => match first_unreadable_position(&domain) {
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
    match &domain {
        LowShape::USize | LowShape::Float | LowShape::Tuple(_) => Ok(domain),
        _ => Err("kernel domain must be a scalar or a tuple of scalars".into()),
    }
}

/// The first position of a tuple domain the lowering cannot read, as a path.
///
/// # Invariant
/// `None` when there is no position to name (a domain that is `Unknown`, or a
/// non-tuple structure); nested only where the domain nests.
fn first_unreadable_position(domain: &LowShape) -> Option<Vec<usize>> {
    let LowShape::Tuple(items) = domain else {
        return None;
    };
    items.iter().enumerate().find_map(|(at, item)| {
        if domain_is_known(item) {
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
/// A compound leaf takes its element's class, so an `array<Float, 3>` is one
/// `f32` local.
fn kernel_shape(domain: &LowShape) -> KernelShape {
    match domain {
        LowShape::USize => KernelShape::Scalar(ScalarClass::Int),
        LowShape::Float => KernelShape::Scalar(ScalarClass::Float),
        LowShape::Tuple(items) => KernelShape::Tuple(items.iter().map(kernel_shape).collect()),
        LowShape::Array(element, _) => KernelShape::Scalar(scalar_class_of(element)),
        // A function, a table or an unknown leaf has no scalar element: the
        // filler's integer local is the answer.
        LowShape::Function(..) | LowShape::Table(..) | LowShape::Unknown => {
            KernelShape::Scalar(ScalarClass::Int)
        }
    }
}

/// The class a single local takes for a position of `shape`.
fn scalar_class_of(shape: &LowShape) -> ScalarClass {
    match shape {
        LowShape::Float => ScalarClass::Float,
        LowShape::Array(element, _) => scalar_class_of(element),
        LowShape::USize
        | LowShape::Tuple(_)
        | LowShape::Function(..)
        | LowShape::Table(..)
        | LowShape::Unknown => ScalarClass::Int,
    }
}

/// The low shape a scalar leaf seeds its parameter slot with.
fn low_shape_of(class: ScalarClass) -> LowShape {
    match class {
        ScalarClass::Int => LowShape::USize,
        ScalarClass::Float => LowShape::Float,
    }
}

/// Whether every position in a shape is one the kernel ABI can lower to a local.
///
/// # Invariant
/// It recurses through every position, because asking a compound shape's own
/// `is_known` answers `true` for `Array(Float, _)`. A function's codomain and a
/// table's value are walked too, since a declared `Int -> Float` puts a float in
/// exactly that position.
fn domain_is_known(shape: &LowShape) -> bool {
    match shape {
        LowShape::Unknown => false,
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
        LowShape::Tuple(items) => items.iter().map(flat_arity).sum(),
        LowShape::Array(_, _) | LowShape::Function(..) | LowShape::Table(..) => 1,
        LowShape::Unknown => 1,
    }
}

/// The disjoint-set representative of `node`, walked without path compression.
fn equality_rep<P>(module: &Module<P>, node: NodeId) -> NodeId
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut root = node;
    while let Some(parent) = module.node_equality(root).parent() {
        root = parent;
    }
    root
}

/// How many levels deep [`emit_node`]'s walk may go before it refuses.
///
/// # Invariant
/// The budget is on the walk, and the walk's depth is the trip count: an unmarked
/// recursion is expanded into a nested chain, so depth is linear in the trips.
/// `emit_node` is `#[stacksafe]` and this limit is the policy — a constant, never
/// derived from the thread's stack, and unreachable except by expansion.
const MAX_KERNEL_BODY_DEPTH: usize = 512;

/// How deep a parameter's field nesting a named read's resolution follows.
///
/// # Invariant
/// A chain deeper than this is not a struct read the resolution understands: it
/// stops and lets its caller name the cause, which is what keeps a type whose
/// encoding re-enters from spinning.
const MAX_PARAMETER_DEPTH: usize = 32;

/// The refusal [`emit_node`] gives past [`MAX_KERNEL_BODY_DEPTH`].
///
/// # Invariant
/// One string rather than a `format!` at the call site, so every refusal of this
/// cause carries the same three facts.
fn kernel_body_too_deep() -> String {
    format!(
        "a kernel body's expression nests more than {MAX_KERNEL_BODY_DEPTH} levels, so lowering it \
         would recurse deeper than a compile should spend on its stack — the walk costs about 3 \
         levels per expanded copy, so this is a trip count in the low hundreds. An unmarked \
         recursion is **expanded**, and expansion nests every copy inside the last one's else arm, \
         so the walk's depth is the trip count: it is the expansion that is too big rather than the \
         program. Mark the recursion `@loop` so it may become a dynamic loop instead of an \
         expansion (`docs/notes/loop-conversion.md` §1.1) — that form is O(1) in the trip count and \
         removes this limit by construction — or, if the trip count is small, write the copies out \
         by hand."
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
    /// The `out_pos` the next write is given — its position in the codomain.
    writes: usize,
    /// One past the highest input position any `compute.read` named.
    ///
    /// # Invariant
    /// A max rather than a count, because read positions are a sparse space: a body
    /// that reads only the second input still needs two buffers.
    reads: usize,
    /// The element class of each write ordinal, in ordinal order.
    write_classes: Vec<ScalarClass>,
    /// The element class of each input position read, in read order.
    ///
    /// # Invariant
    /// A list of the *reads*, not of the positions: [`Self::reads`] is a max over a
    /// sparse space, so a body that reads position 1 and not 0 has one entry here
    /// and a count of two. Every entry is the class the reads are *declared* in
    /// ([`Self::element_class`]), because a buffer's element class is a fact of the
    /// value the host binds at the run, not of any node.
    read_classes: Vec<ScalarClass>,
    /// The class a buffer read's element is declared in.
    ///
    /// # Invariant
    /// The one class that stays the fragment's: a read's element is the only value
    /// whose class no node carries, because the host binds the buffer. `Int` by
    /// default until the caller says otherwise.
    element_class: Option<ScalarClass>,
}

impl Positions {
    /// The declared class of every input position, `declared` entries long.
    ///
    /// # Invariant
    /// The count is the fragment's declaration — the highest position a tuple-form
    /// body read, the `.in` field count for a struct one — so a position a body
    /// never read still takes the declared class.
    fn input_classes(&self, declared: usize) -> Vec<ScalarClass> {
        let class = self.element_class.unwrap_or(ScalarClass::Int);
        let mut classes = self.read_classes.clone();
        classes.resize(declared.max(classes.len()), class);
        classes
    }
    /// The declared class of every write ordinal, one per ordinal — this is the
    /// list the entries were pushed in.
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
    let classes = param_classes(fragment);
    Ok(classes[..classes.len().saturating_sub(1)].to_vec())
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
        .map(|fragment| FragmentFacts {
            roles: fragment.roles.clone(),
            output_classes: fragment.output_classes.clone(),
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
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
{
    match module
        .node_value(node)
        .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
    {
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

/// The payload a `Buf` value carries — its `.native` slot.
///
/// # Invariant
/// A role path names the wrapper, because that is what the checker resolves for the
/// source's read; the wrapper's field order is fixed by [`is_buf_shape`], so the
/// payload is its first item.
fn buf_payload<P>(module: &mut Module<P>, node: AnyNodeId) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
{
    let dynamic = node.dynamic()?;
    // SAFETY: `dynamic` is a live node of `module` on this borrow.
    (unsafe { module.array_items(dynamic) })?
        .first()
        .map(|item| item.node)
}

/// The node a role path names in a cfg value: each step is an array index.
fn value_at_path<P>(module: &mut Module<P>, root: NodeId, path: &[usize]) -> Option<AnyNodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
{
    let mut node = AnyNodeId::Dynamic(root);
    for &at in path {
        let dynamic = node.dynamic()?;
        // SAFETY: every node on the way is live on this borrow — the root is the
        // cfg the VM just evaluated.
        let items = unsafe { module.array_items(dynamic) }?;
        node = items.get(at)?.node;
    }
    Some(node)
}

/// The type a buffer's elements are, for the wrapper's `.element` slot.
fn element_type(class: ScalarClass) -> TypeValue {
    match class {
        ScalarClass::Int => TypeValue::TypeInt,
        ScalarClass::Float => TypeValue::TypeFloat,
    }
}

/// One `Buf` value: the payload, then the element type, in the wrapper's order.
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
    let items = [
        ArrayItem::new(payload),
        ArrayItem::new(AnyNodeId::Dynamic(element)),
    ];
    let handle = module.alloc_array(&items, block);
    Some(module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(LowValue::Array(handle))),
    ))
}

/// Assemble one level of a result structure, driven by the result paths.
///
/// # Invariant
/// A group's arity is one past the highest field any path names, and a position no
/// path names is not built — the structure is exactly what the walk found.
fn assemble_result<P>(
    module: &mut Module<P>,
    block: BlockId,
    outputs: &[(Vec<usize>, NodeId)],
    prefix: &[usize],
    depth: usize,
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
    let mut items: Vec<ArrayItem> = Vec::with_capacity(arity);
    for at in 0..arity {
        let leaf = outputs
            .iter()
            .find(|(path, _)| under(path) && path.len() == depth + 1 && path[depth] == at);
        let node = match leaf {
            Some((_, value)) => AnyNodeId::Dynamic(*value),
            None => {
                let mut nested = prefix.to_vec();
                nested.push(at);
                AnyNodeId::Dynamic(assemble_result(module, block, outputs, &nested, depth + 1)?)
            }
        };
        items.push(ArrayItem::new(node));
    }
    let handle = module.alloc_array(&items, block);
    Some(module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(LowValue::Array(handle))),
    ))
}

/// The placeholder tuple a recorded body is applied to, built from the parameter
/// type and the role paths.
///
/// # Invariant
/// A parameter that declares a group with no members still builds a placeholder
/// for it: the descent is driven by the declaration's field list, so a position no
/// role path reaches is an empty group rather than a hole, while a leaf the walk
/// did not number is a hole the caller refuses.
fn assemble_parameter<P>(
    module: &mut Module<P>,
    block: BlockId,
    type_slot: AnyNodeId,
    item: Option<AnyNodeId>,
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
    let (_, shape) = match item {
        Some(item) => struct_fields_of_slot(module, item)?,
        None => struct_fields_of_slot(module, type_slot)?,
    };
    // SAFETY: `shape` is the field list of the live type term just read.
    let fields: Vec<AnyNodeId> = unsafe { array_items_any(module, shape) }?
        .iter()
        .map(|entry| entry.node)
        .collect();
    let mut items: Vec<ArrayItem> = Vec::with_capacity(fields.len());
    for (at, field) in fields.into_iter().enumerate() {
        let mut nested = path.to_vec();
        nested.push(at);
        let node = assemble_parameter(module, block, type_slot, Some(field), cells, &nested)?;
        items.push(ArrayItem::new(AnyNodeId::Dynamic(node)));
    }
    Some(array_node::<P>(module, block, &items))
}

/// The `O` structure a run hands back, with every output in its role's place.
///
/// # Invariant
/// A role path starts at `.out` and ends at the buffer's `.native` slot; the result
/// *is* the codomain, so the builder drops both ends.
fn build_outputs<P>(
    module: &mut Module<P>,
    block: BlockId,
    roles: &KernelRoles,
    outputs: &[(NodeId, ScalarClass)],
) -> Option<P::Value>
where
    P: Program,
    P::Value: ValueType + From<ComputeValue> + AsEnum<ComputeValue>,
{
    let mut placed: Vec<(Vec<usize>, NodeId)> = Vec::with_capacity(outputs.len());
    for (position, &(payload, class)) in outputs.iter().enumerate() {
        let path = roles.outputs.get(position)?;
        // Drop the `.out` step at the front: the result is the codomain itself.
        let inner = path.get(1..)?;
        let value = buf_value::<P>(module, block, AnyNodeId::Dynamic(payload), class)?;
        placed.push((inner.to_vec(), value));
    }
    let root = assemble_result::<P>(module, block, &placed, &[], 0)?;
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
            if equality_rep(module, dynamic) == equality_rep(module, slot.value) {
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
        let Ok((left, right)) = operand_pair(module, operation.operand) else {
            return node_class(module, node);
        };
        let operands = match (
            node_class_in(module, params, left),
            node_class_in(module, params, right),
        ) {
            (ScalarClass::Float, _) | (_, ScalarClass::Float) => ScalarClass::Float,
            _ => ScalarClass::Int,
        };
        return match bin {
            KernelBin::Lt
            | KernelBin::Gt
            | KernelBin::Leq
            | KernelBin::Geq
            | KernelBin::Eq
            | KernelBin::Neq => ScalarClass::Int,
            _ => operands,
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
    for item in items {
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
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
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
    value
        .dynamic()
        .and_then(|node| module.low_type_of_node(node))
        .map_or(ScalarClass::Int, |shape| scalar_class_of(&shape))
}

/// The node whose value or low type states a term's class.
///
/// # Invariant
/// The `[value, type]` pair an expression is, the extraction over one, and the
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
            .or_else(|| pair_value_half(module, node))
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
/// The value slot answers first, exactly as [`emit_node`] reads it: a node the
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
        // The class conversions are unary: they lower to [`KernelInstr::Conv`],
        // answered before this mapping is reached.
        TypeOperator::Int2Float | TypeOperator::Float2Int => return None,
        // A membership test is a refinement's check, not a computation: the
        // checker registers it as an assert beside the body.
        TypeOperator::InDomain => return None,
        // The same reading of a named read's container kind: a check-time/
        // per-apply *assert*, never a value a kernel computes.
        TypeOperator::IsStructType => return None,
        TypeOperator::Fresh => return None,
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
    unpeeled: AnyNodeId,
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
        // **A struct field read is read by the path on its own chain**, from the *unpeeled*
        // operand.
        let positions = unpeeled
            .dynamic()
            .and_then(|unpeeled| named_path(module, unpeeled));
        if let Some(positions) = positions.as_deref() {
            for (position, candidate) in roles.inputs.iter().enumerate() {
                // **The chain is the role path's tail**: the relation is a suffix, with the
                // chain's own head dropped.
                let Some(tail) = positions.get(1..).filter(|tail| !tail.is_empty()) else {
                    continue;
                };
                if candidate.ends_with(tail) {
                    return Ok(Some(position));
                }
            }
        }
        // **The chain walk is the fallback**, for a positional parameter read where the role
        // table is empty.
        let path = param_path(module, slot.pair, node)?;
        if let Some(path) = path {
            let mut as_field = vec![0];
            as_field.extend(path.iter().copied());
            if let Some(position) = roles.input_pos(&as_field) {
                return Ok(Some(position));
            }
            return Ok(roles.input_pos(&path));
        }
        return Ok(None);
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
    let Some((target, index)) = operand_pair(module, operation.operand).ok() else {
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
    let Some((tt, ti)) = operand_pair(module, target_op.operand).ok() else {
        return Ok(None);
    };
    // The body's cfg reads go through `Index(cfg_pair, 0)`: compare the two
    // slots by equality class, not node identity.
    let Some(tt) = tt.dynamic() else {
        return Ok(None);
    };
    if equality_rep(module, tt) != equality_rep(module, cfg_value)
        || usize_value(module, ti) != Some(1)
    {
        return Ok(None);
    }
    Ok(Some(k))
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
                operand_pair(module, op.operand).ok()
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
    Ok(value)
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

/// A `[value, type]` pair's value half, or the node itself when it is not a pair.
fn param_value_of<P>(module: &Module<P>, pair: NodeId) -> Result<NodeId, String>
where
    P: Program,
{
    Ok(module.pair_value_half(pair).unwrap_or(pair))
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
            Some(LowOperator::TableGet) => operand_pair(module, operation.operand)?.1,
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
    param_pair: NodeId,
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
    let mut steps: Vec<IndexStep> = Vec::new();
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
        let Ok((target, selector)) = operand_pair(module, operation.operand) else {
            return Ok(None);
        };
        // A frozen node is not a step of a read of *this* parameter: the chain a
        // path walks is the caller's own.
        let Some(target_node) = target.dynamic() else {
            return Ok(None);
        };
        if target == AnyNodeId::Dynamic(param_pair) {
            // The pair's value half is the parameter itself, not a field, so the
            // peel contributes no step.
            match pair_value_half(module, param_pair) {
                Some(value) => {
                    current = value;
                    continue;
                }
                // A pair with no value half has nothing inside to walk, and the
                // path so far is what the caller's read reached.
                None => break,
            }
        }
        // The selector is a *name* when the read was written `a.name`, and a
        // *position* when it was written `a(0)`.
        match field_name(module, selector)? {
            Some(name) => steps.push(IndexStep::Named(name)),
            None => match usize_value(module, selector) {
                Some(position) => steps.push(IndexStep::Position(position)),
                None => return Ok(None),
            },
        }
        // The innermost read's target is the parameter's value; **the test is the equality
        // class**.
        if module.class_root(target_node) == module.class_root(param_value_of(module, param_pair)?)
        {
            break;
        }
        current = target_node;
    }
    if steps.is_empty() {
        return Ok(None);
    }
    // Collected innermost-first; the path reads outermost-first.
    steps.reverse();
    resolve_steps(module, param_pair, &steps)
}

/// Resolve a read's collected steps against the parameter's type, outermost first.
fn resolve_steps<P>(
    module: &Module<P>,
    param_pair: NodeId,
    steps: &[IndexStep],
) -> Result<Option<Vec<usize>>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(type_slot) = (unsafe { module.array_items(param_pair) })
        .and_then(|items| items.get(PAIR_TYPE_SLOT).map(|item| item.node))
    else {
        // A scalar parameter has no field list and a read of it is the value: the
        // empty path.
        return resolve_without_type(steps);
    };
    // One level per step, each carrying two parallel lists: the field types and
    // the field names.

    // Keeping them together stops a name being looked for in the wrong list.
    let (mut names, mut types) = struct_fields_of_slot(module, type_slot)
        .map(|(names, shape)| (Some(names), Some(shape)))
        .unwrap_or_default();
    let mut path = Vec::with_capacity(steps.len());
    for step in steps {
        let at = match step {
            IndexStep::Position(position) => *position,
            IndexStep::Named(name) => {
                match names
                    .as_ref()
                    .and_then(|names| names.iter().position(|field| *field == Some(*name)))
                {
                    Some(at) => at,
                    None => {
                        return Err(format!(
                            "a struct parameter field read names `{name}`, which is not a field of \
                             the type it is read from — that type's fields are {names:?}"
                        ));
                    }
                }
            }
        };
        path.push(at);
        // The level below: the entry this step selected, taken as it stands.
        let entry = types
            .and_then(|types| unsafe { array_items_any(module, types) })
            .and_then(|entries| entries.get(at))
            .map(|entry| entry.node);
        types = entry;
        names = entry
            .and_then(|entry| parameter_type_ref(module, entry))
            .and_then(|ty| field_names(module, ty));
    }
    Ok(Some(path))
}

/// Resolve a read whose parameter type states no field list — a scalar domain.
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
                     type it is read from"
                ));
            }
        }
    }
    Ok(Some(path))
}

/// Follow a `value_of` extraction — `Index(pair, 0)` — to the pair's value slot.
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
    let (target, index) = operand_pair(module, operation.operand).ok()?;
    if usize_value(module, index)? != 0 {
        return None;
    }
    // A frozen target is where this peel stops: only a value crosses.
    let target = target.dynamic()?;
    // A concrete `[value, type]` pair value → its value slot (element 0).
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

/// The [`KernelId`] of a kernel-value node, through `value_of` and `.native` reads.
fn kernel_id_of<P>(module: &Module<P>, node: NodeId) -> Option<KernelId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
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
    // A kernel *struct value* `[.native, .I, .O]` reached by value (not through an
    // `Index` op): its element 0 is the bare `.native` kernel artifact.
    // SAFETY: `node` is a live node of `module`; the note covers this
    // function's `items()` calls.
    if let Some(items) = (unsafe { module.array_items(node) })
        && let Some(first) = items.first()
        && let Ok(first) = dyn_node(first.node)
        && let Some(kid) = kernel_id_of(module, first)
    {
        return Some(kid);
    }
    // A kernel struct `.native` field read: `Index(struct, 0)`, where the
    // struct value's element 0 is the bare kernel artifact.
    if let Some(operation) = module.node_operation(node)
        && let Some(LowOperator::Index) = AsEnum::<LowOperator>::as_enum(&operation.operator)
            && let Ok((target, index)) = operand_pair(module, operation.operand)
            && usize_value(module, index) == Some(0)
            && let Some(target) = target.dynamic()
            // SAFETY: `target` is a live node of `module`.
            && let Some(items) = (unsafe { module.array_items(target) })
            && let Ok(first) = dyn_node(items.first()?.node)
    {
        return kernel_id_of(module, first);
    }
    None
}

/// Whether `node` is a kernel **in the making** — the undecided half of
/// [`kernel_id_of`], for [`ComputeOperator::is_callable`], which the lowlevel
/// consults in the middle of the deep pass, before the struct pair's value slot
/// has been evaluated.  A `Jit` that has not run yet counts as the kernel it
/// will produce, and the walk follows the same two value edges `kernel_id_of`
/// follows ([`value_of_node`]'s `Index(pair, 0)` extraction and a struct
/// pair's element 0).  The answer is therefore a superset of the JIT's: it can
/// keep a callee lazy that later turns out not to be a kernel (the conservative
/// direction), and never refuses one the JIT would lower.
fn pending_kernel<P>(module: &Module<P>, node: NodeId) -> bool
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // The one operator that produces a `Kernel`; a value that is still lazy
    // here is a function the definition pass has not run yet.
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
    // SAFETY: `node` is a live node of `module`.
    if let Some(items) = unsafe { module.array_items(node) }
        && let Some(first) = items.first()
        && let Ok(first) = dyn_node(first.node)
        && pending_kernel(module, first)
    {
        return true;
    }
    false
}

/// The *value* node behind a `[value, type]` pair stored as a **concrete array
/// value** — a node whose value is an array, its element 0 the value.  The
/// checker stores some argument values as such pairs (whereas [`value_of_node`]
/// handles the `Index(pair, 0)` extraction form); the JIT must look through to
/// the value before emitting.  Returns `None` when `node` is not such a pair.
fn pair_value_half<P>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // SAFETY: `node` is a live node of `module`.
    let items = unsafe { module.array_items(node) }?;
    dyn_node(items.first()?.node).ok()
}

/// The **field-type list** of a parameter's value: the shape slot of the
/// parameter's type expression.
///
/// The field positions a read's own chain names, walking **down from the read**.
///
/// # Invariant
/// A named read reaches a lowering already resolved: `TableGet(names, "in")` is specialised
/// into `Index(field, 0)`, so the chain carries **positions** rather than names, and the
/// positions are the role table's own. Walking from the parameter cannot answer this: its
/// value half is an alias with no operation, and the link to a read is the equality class.
fn named_path<P>(module: &Module<P>, node: NodeId) -> Option<Vec<usize>>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let mut positions = Vec::new();
    let mut cursor = node;
    for _ in 0..MAX_PARAMETER_DEPTH {
        let Some(operation) = module.node_operation(cursor) else {
            // The chain ends here. **An empty chain names nothing**, which is the
            // whole-parameter read and not a field.
            return (!positions.is_empty()).then_some(positions);
        };
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Index)
        ) {
            return None;
        }
        let (target, selector) = operand_pair(module, operation.operand).ok()?;
        let step = usize_value(module, selector);
        positions.push(step?);
        let Some(target) = target.dynamic() else {
            return None;
        };
        cursor = target;
    }
    None
}

/// Flatten a parameter index `path` to a wasm local index, using the domain
/// `shape`: a tuple's element `i` starts at the sum of its `[..i]` elements'
/// flattened arities.  A scalar domain (empty path) is `local 0`.
fn flatten_offset(domain: &LowShape, path: &[usize]) -> Result<usize, String> {
    let mut offset = 0;
    let mut cur = domain;
    for &i in path {
        match cur {
            LowShape::Tuple(items) => {
                if i >= items.len() {
                    return Err(format!("parameter index {i} out of bounds"));
                }
                for item in &items[..i] {
                    offset += flat_arity(item);
                }
                cur = &items[i];
            }
            LowShape::USize => {
                // Descending into a scalar (a non-empty path) is a type error
                // the checker should have caught; a scalar domain is only ever
                // read as the empty path.
                return Err("index into a scalar parameter".into());
            }
            _ => return Err("unsupported parameter domain shape".into()),
        }
    }
    Ok(offset)
}

/// One scalar crossing a kernel's ABI — a launch/call argument, or a value a run
/// handed back: the class it is and the bits that class's wasm value takes.
///
/// The class travels with the value because the two are the same 64 bits to
/// everything downstream — the argument vector, the wasm call, the result
/// buffer — and only the class says whether an `i64` or an `f32` is meant
/// (`docs/notes/floating-point.md` §4.2, §4.4).
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
    fn low(self) -> LowValue {
        match self.class {
            ScalarClass::Int => LowValue::USize(self.bits as usize),
            ScalarClass::Float => LowValue::Float(f32::from_bits(self.bits as u32)),
        }
    }

    /// The value as the class's wasm value.
    fn wasmi(self) -> wasmi::Val {
        match self.class {
            ScalarClass::Int => wasmi::Val::I64(self.bits),
            ScalarClass::Float => wasmi::Val::F32(wasmi::F32::from_bits(self.bits as u32)),
        }
    }

    /// The zero a run's result vector starts from — `wasmi` sizes its result
    /// slots, it does not fill them.
    fn wasmi_zero(class: ScalarClass) -> wasmi::Val {
        ScalarValue { class, bits: 0 }.wasmi()
    }

    /// The value `v` holds, read as `class` — the read-back half of [`Self::wasmi`].
    fn from_wasmi(class: ScalarClass, v: &wasmi::Val) -> Option<Self> {
        let bits = match class {
            ScalarClass::Int => v.i64()?,
            ScalarClass::Float => i64::from(v.f32()?.to_bits()),
        };
        Some(ScalarValue { class, bits })
    }
}

/// Flatten a kernel argument value (a scalar `Int`/`Float` leaf, or a possibly
/// nested `Array` of them, as a tuple-of-tuples domain needs) into the wasm
/// argument vector, each leaf with its own class.  Returns `Err` naming the
/// first element that is not a scalar leaf — the definition pass reports the
/// undecided result, but only this says *which* element was unusable.
///
/// `path` is the offending element's position in the argument: `""` at the
/// root, then `"1"`, `"1.0"`, …  It is built as the walk descends, because for
/// a tuple-of-tuples argument a position is the only way to point at the one
/// element that could not be lowered; an empty `path` is the root, whose
/// refusal names the argument as a whole.
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
            // At the root there is no position to name, and the caller has
            // already established that a tuple argument is an `Array`, so this
            // arm is the root itself — the whole argument, not one of its
            // elements.
            let refusal = if path.is_empty() {
                format!("the argument is {kind}")
            } else {
                format!("argument element {path} is {kind}")
            };
            Err(format!(
                "{refusal}, not a concrete Int or Float, so the kernel's parameters could not be \
                 filled"
            ))
        }
    }
}

/// The wasm argument vector for a `launch`/`call`, or the refusal to record —
/// the one place a kernel's argument is read out of the program, so both run
/// operators say the same thing about the same argument.
///
/// A scalar argument is one value of its own class; a tuple argument is
/// flattened ([`collect_args`]).  Anything else is named by what the user wrote:
/// `launch`'s checker gate catches the shape for an annotated kernel, but
/// `call`'s domain is a fresh cell, so this is where a string or a computed
/// scalar becomes a diagnostic rather than a lazy marker.
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

/// What a launch argument that is not a kernel parameter vector at all looks
/// like, in the terms a lichen program is written in: a refusal that names the
/// runtime variant instead names the value the user wrote.  `USize` and
/// `Array` are the two shapes a parameter vector may take and are recognised
/// before this is asked, so the last arm covers a node that carries no value at
/// all.
fn argument_kind(value: Option<&LowValue>) -> &'static str {
    match value {
        // A float is a concrete scalar, so it is named as one: the refusal this
        // feeds has to say what the user actually wrote.  It is only *refused*
        // where the parameter it is handed to is an `Int` — that check is the
        // run's, where the callee's leaf classes are in hand
        // (`docs/notes/floating-point.md` §4.2).
        Some(LowValue::Float(_)) => "a float",
        Some(LowValue::Str(_)) => "a string",
        Some(LowValue::Table(_)) => "a table",
        Some(LowValue::Function(_)) => "a function",
        Some(LowValue::None) => "the unit value",
        Some(LowValue::Error) => "nothing (the empty value of a failed read)",
        _ => "no value",
    }
}

/// What a node's value is, for a refusal that has to say — across **both**
/// vocabularies, in that order.
///
/// A buffer position can be reached by a `compute.read` or a `compute.collect`,
/// and the value there is a [`ComputeValue`] when it is a buffer and a
/// [`LowValue`] when it is anything else, so a refusal that named only one of
/// the two would say "no value" about a perfectly ordinary array. `argument_kind`
/// cannot be reused for this: it is written for launch arguments, where `USize`
/// and `Array` are the two shapes a parameter vector may take and are recognised
/// before it is asked, and an array is exactly the case here.
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

/// One collected element, materialized as a fresh scalar node of `block` and
/// wrapped as an array item.
///
/// A buffer's elements all have the buffer's class, so a collected array is
/// homogeneous — but an integer element and a float element are two different
/// [`LowValue`] variants, and the class is only known at run time.  This is the
/// one place that turns one element into one node, so the two classes cannot
/// drift into two slightly different constructions.
fn scalar_item<P>(module: &mut Module<P>, block: BlockId, element: LowValue) -> ArrayItem
where
    P: Program,
    P::Value: From<LowValue>,
{
    let node = module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(element)),
    );
    ArrayItem::new(AnyNodeId::Dynamic(node))
}

/// A buffer position holding something that is not a buffer.
///
/// **This is a refusal, and the undecided answer it replaces was a silent
/// no-op.** The lazy cell is what makes a kernel's own read deferrable and what
/// makes an undecided argument stay undecided — but a program array *is*
/// decided, it is an ordinary lichen value with ordinary elements, and
/// answering `undecided` for it made `compute.read ((compute.Read _)(.from
/// data, .at i))` a plausible-looking program that produced an empty value while
/// still
/// printing `array<?a, ?b>`. There is no way to make a buffer out of a program
/// value, so the honest answer names that rather than waiting for a buffer that
/// will not arrive.
///
/// `subject` says what the position is *for* and `at` names it, so the three
/// sites that reach this describe their own mistake rather than sharing one
/// sentence.
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
    module.record_extension_diagnostic(
        PARALLEL_DIAGNOSTIC,
        None,
        format!(
            "{subject}, and {at} holds {}. A buffer is what a `compute.plrun` or a \
             `compute.graphrun` hands back, and there is no way to make one out of a program value",
            what_this_is(module, node)
        ),
    );
    None
}

/// The value a completed kernel run produces: the bare scalar for a
/// single-result kernel, and the **tuple** of them for a multi-result one.
///
/// This is the same shape `ParLaunch` builds for its several output buffers —
/// each result becomes a node of `block` first, so the array holds live nodes
/// rather than detached values — and it is the direct counterpart of
/// [`codomain_leaves`], which is what decided how many results there are.  A
/// lichen tuple is an ordinary array value, so `r(0)`/`r(1)` index it with no
/// special case, exactly as the codomain's leaves were ordinary scalars.
///
/// **Each result is the class the run said it was**: an integer result is the
/// `USize` it always was, and a float result is the language's own `Float`, not
/// the bit pattern of one read as an integer.
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
        // A fragment always leaves at least one value, so an empty run is not
        // reachable; stay lazy rather than fabricating a value for it.
        return None;
    };
    if rest.is_empty() {
        // The single-result form is the bare scalar, exactly what it always was.
        return Some(scalar(first));
    }
    let items: Vec<ArrayItem> = results
        .iter()
        .map(|value| {
            let node = module.add_node(block, None, Some(scalar(value)));
            ArrayItem::new(AnyNodeId::Dynamic(node))
        })
        .collect();
    let handle = module.alloc_array(&items, block);
    Some(<P::Value as From<LowValue>>::from(LowValue::Array(handle)))
}

/// Read a binary op/`Index` operand array `[a, b]` as two operand nodes.
///
/// An operand may be a node of a **frozen** module: an apply of a static
/// function leaves the callee's unchanged subterms as references into the frozen
/// module, so a constant the callee's body wrote (`operands[0]`'s `0`) arrives
/// beside nodes of the caller's own.  Reading an operand is therefore not a
/// question about *which* module it is in — [`emit_operand`] is where the walk
/// decides what to do with the answer, and `dyn_node` is where a caller says it
/// needs a node of its own.
fn operand_pair<P>(
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

/// Record one dispatch into the graph being built, in place of running it.
///
/// **The argument is read for its placeholders, not for its data.** The extent
/// and each buffer sit at the paths the role walk found (`KernelRoles`), the same
/// positions a real launch reads, so the body is walked by exactly the path a run
/// would take and the only thing that differs is what comes back. A value that is
/// neither a placeholder nor an input the parameter supplied is refused here by
/// name rather than coerced: this is the filter, and it is where a jit'd
/// function's arbitrary values are sorted into the two roles a graph's value
/// table has.
fn record_launch<P>(module: &mut Module<P>, block: BlockId, operand: P::Value) -> Option<P::Value>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + From<LowValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let refuse = |module: &mut Module<P>, reason: String| {
        module.record_extension_diagnostic(GRAPH_DIAGNOSTIC, None, reason);
    };
    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
        unreachable!("ParLaunch expects an operand array of [kernel, cfg]")
    };
    // SAFETY: `operands` is the operand array the VM just evaluated for this
    // operation, and every walk below stays inside this borrow of `module`.
    let operands = unsafe { operands.items() };
    if operands.len() < 2 {
        refuse(
            module,
            "a parallel launch's operand array is [kernel, cfg]".into(),
        );
        return None;
    }
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
            Some(fragment) => fragment.clone(),
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
    // Every leaf of the argument is read at the path the role walk found
    // (`KernelRoles`), not at a position: that walk is the one enumeration the
    // emitter, the run and this recorder read, so a recorded body walks the
    // parameter the way a real run does.
    let roles = fragment.roles.clone();
    // The declared class of each output, read before the fragment is handed to
    // the recorder: the `.element` of the `Buf` the result structure wraps.
    let output_classes = fragment.output_classes.clone();
    // The extent, read on its own: a count is a different role from a buffer, and
    // a caller who swapped the two deserves to be told which was wrong rather
    // than that a shape did not match.
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
                    // A literal the build already had. Collapsing it into a value
                    // would mean inventing a node that produces a number for
                    // free, doing no work.
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
                        let found = module
                            .node_value(node)
                            .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
                            .map(|value| format!("{value:?}"))
                            .unwrap_or_else(|| {
                                module
                                    .node_value(node)
                                    .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
                                    .map(|value| graph::describe(&value).to_string())
                                    .unwrap_or_else(|| "not a value at all".to_string())
                            });
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
    // **The argument's leaves are the parameter's**, whatever shape the author
    // wrote, so the runtime scalars are `roles.scalars` rather than a positional
    // count.  A recorded dispatch carries the extent alone for now: a runtime
    // scalar would have to be an edge like the extent is, so it is refused by
    // name rather than read as an input buffer.
    let scalar_leaves = roles.scalars.len().saturating_sub(1);
    if scalar_leaves > 1 {
        refuse(
            module,
            format!(
                "this dispatch's kernel parameter declares {scalar_leaves} runtime scalars, and a \
                 recorded body carries the extent alone: a runtime scalar would have to be an \
                 edge of the recording, and that is not written yet"
            ),
        );
        return None;
    }
    let mut inputs: Vec<Placed> = Vec::new();
    for (position, path) in roles.inputs.iter().enumerate() {
        // An input path names the `Buf` the caller passes; the placeholder rides
        // in the wrapper's payload, which is what `buf_payload` takes.
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
    let placed = match graph::record_dispatch(fragment, backend, extent, &inputs) {
        Ok(placed) => placed,
        Err(reason) => {
            refuse(module, reason);
            return None;
        }
    };
    // **The same shape a real launch produces**: the parameter's `.out`
    // structure, one `Buf`-wrapped placeholder per declared output at the path
    // the walk found.  Each placeholder becomes a node of this block first, so
    // the structure holds live nodes rather than detached values, and the body
    // downstream reads a result exactly as it reads a run's.
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
        let Some(inner) = roles.outputs.get(position).and_then(|path| path.get(1..)) else {
            refuse(
                module,
                format!("output {position} has no path in the parameter's `.out` group"),
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
        let Some(buf) = buf_value::<P>(module, block, AnyNodeId::Dynamic(payload), class) else {
            return None;
        };
        fields.push((inner.to_vec(), buf));
    }
    match assemble_result::<P>(module, block, &fields, &[], 0) {
        Some(root) => module.node_value(AnyNodeId::Dynamic(root)),
        None => None,
    }
}

/// Build a graph by applying a function to placeholders and recording what it
/// dispatches.
///
/// **The apply is the VM's own.** No lowlevel seam grows for this: a plain
/// `Apply` node is built over the function and a placeholder tuple, and the
/// module's `evaluate_node` runs it, so the clone pass, the parameter unify and
/// the pattern walk are the ones every other call gets. A lowering that hand-rolled
/// those would be a second apply with its own bugs, and the bugs would be in the
/// part nobody tests.
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
        module.record_extension_diagnostic(GRAPH_DIAGNOSTIC, None, reason);
    };
    // A **check, not a resolution.** The apply below consumes the operand *node*,
    // because a function id and a node id are different id spaces and `Apply`
    // addresses its callee as a node. So this only has to answer "is this
    // operand a function at all" — and it answers it by reading the node's own
    // value, which is the same read the apply's callee extraction will do.
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
    // **The parameter's shape is the placeholder's shape.**  When the parameter's
    // type reads as the named struct, the walk over that type gives the cells'
    // paths, and the slots are numbered **depth-first in field order** — which is
    // lexicographic in the paths — so the run's own depth-first walk of the same
    // structure lands on the same numbers, and the two ends agree by construction
    // rather than through a stored table.
    //
    // A parameter that is *not* a named struct — an unannotated `ins => …` body,
    // whose reads are positional (`ins(0)`) — keeps the flat ceiling below: a flat
    // tuple is exactly the shape such a body reads.
    let mut roles = None;
    let mut parameter_slot = None;
    if let AnyFunctionId::Dynamic(fid) = function {
        let parameter = module.functions[fid].parameter;
        roles = parallel_roles(module, parameter).ok();
        // SAFETY: `parameter` is a live node of `module`.
        parameter_slot = unsafe { module.array_items(parameter) }
            .and_then(|items| items.get(PAIR_TYPE_SLOT).map(|item| item.node));
    }
    // The ceiling a positional body's tuple is built at, and the number the
    // undecided-apply refusal names.
    let arity = graph::MAX_GRAPH_INPUTS;
    let placeholders = match roles {
        Some(roles) => {
            // A **buffer role's cell is a `Buf` wrapper** around the placeholder,
            // because that is what the field's type is: the body hands `s.in.b` on
            // as an argument, and the dispatch reads the wrapper's payload
            // (`buf_payload`).  A scalar role stays bare, because the extent is a
            // number the dispatch reads as one.
            let mut paths: Vec<(&Vec<usize>, bool)> = roles
                .scalars
                .iter()
                .map(|path| (path, false))
                .chain(roles.inputs.iter().map(|path| (path, true)))
                .chain(roles.outputs.iter().map(|path| (path, true)))
                .collect();
            paths.sort_by(|left, right| left.0.cmp(right.0));
            let mut cells: Vec<(Vec<usize>, NodeId)> = Vec::with_capacity(paths.len());
            for (slot, (path, wrapped)) in paths.into_iter().enumerate() {
                let value = <P::Value as From<ComputeValue>>::from(ComputeValue::GraphInput(slot));
                let node = module.add_node(block, None, Some(value));
                let node = if wrapped {
                    buf_value::<P>(module, block, AnyNodeId::Dynamic(node), ScalarClass::Int)?
                } else {
                    node
                };
                cells.push((path.clone(), node));
            }
            // **The declaration builds the tuple, not the paths.**  A group the
            // role walk found no leaf under — `In1 = struct<>` under `.in` — has
            // no path to reach it, so a walk driven by the paths has no arity for
            // it and answers nothing at all; the type says the group is there.
            //
            // A parameter whose type slot is not readable is the one case the
            // declaration cannot answer, and the paths are all there is: the
            // roles were decoded, so the slot was readable a moment ago, and
            // falling back to the path walk keeps a readable parameter from
            // losing its placeholder without a word.
            let built = match parameter_slot {
                Some(type_slot) => {
                    assemble_parameter::<P>(module, block, type_slot, None, &cells, &[])
                }
                None => assemble_result::<P>(module, block, &cells, &[], 0),
            };
            match built {
                Some(root) => root,
                None => {
                    // **A recording that answers nothing is refused by name.**
                    // The parameter declared a named struct — the roles were
                    // decoded from it — so a placeholder the type walk could not
                    // fill is a leaf the ABI has no local for, and a graph whose
                    // placeholder is `none` answers `none` for a program that
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
        // **The arity is not knowable here, and that is the finding.** A read of
        // `ins(i)` compiles to a bare cell with no operation and no subscript, so
        // the unapplied body contains nothing that says which slot a read wants; and
        // the parameter's own value node cannot answer it either, because in a
        // template that value is an undecided cell rather than a tuple — so there is
        // no length to read anywhere before the apply. The tuple is therefore built
        // at a ceiling and the graph is trimmed to the slots the body actually read.
        //
        // One placeholder per slot, **and the slot is the number**, so the apply's
        // tuple walk binds the `k`-th argument to the `k`-th cell and `ins(k)` is the
        // `k`-th argument with no ordering to guess.
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
    // The argument is a **pair**, because a parameter is one: the value side is
    // the placeholder tuple and the type side is left undecided. The type side
    // has to stay undecided rather than be invented, because a parameter's type
    // is what says which role each argument has — and that is precisely the
    // question this recording is going to answer by looking at the values.
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
    // Started **before** the apply and finished after it, so a dispatch the body
    // performs is inside the window. A recording started after would miss the
    // body's own work and build an empty graph that still looked like a graph.
    graph::begin();
    // **Deep, not shallow, and that is the whole difference between recording a
    // body and applying one.** A block's value is the tuple of its statements'
    // values, so a shallow evaluation of the body produces that tuple and stops —
    // the statements inside it have not run, and a recording taken here is an
    // empty graph that still looks like a graph. The deep pass is what *demands*
    // the tuple, and demanding it is what performs the dispatches.
    //
    // **Not forced, and this is now measured rather than argued.** A graph is not
    // a transcript of the source, it is a transcript of the run, and the run does
    // not perform a dispatch whose result nothing reads: the same body, called
    // directly with no graph anywhere in the program, reaches the backend twice
    // where it writes three dispatches. There is no expression-level CSE in this
    // compiler to account for the missing one, so the elimination is the laziness
    // every unread binding already gets. A walk that forced the rest would be
    // adding a dispatch the program never makes.
    //
    // **Forcing was tried anyway, and it broke more than it reached.**
    // `Module::evaluate_node_forced` — an entry point since deleted, together
    // with the operand forcing it was named for — performed every statement, and
    // it also left
    // the function's return slot empty, so the reader that has to name the return
    // finds no value and *every* recording refuses — including bodies with no
    // unread statement at all. The empty slot was isolated to the operand forcing
    // rather than the shallow descent: a walk that descends every position in
    // order (`skip_shallow` off, `force_operand` off) recorded the same two
    // dispatches, and turning `force_operand` on alone emptied the return slot
    // with the shallow mask untouched. See the landmine. (The `force_operand`
    // knob those two rows name has since been deleted — see the operand-arm
    // follow-up in `docs/notes/code-audit.md` — so only the first row is still
    // buildable; the measurement is what stands.)
    let result = module.evaluate_node_deep(apply, Some(block));
    let Some(result) = result else {
        refuse(
            module,
            format!(
                "applying this function to its own input placeholders stayed undecided, so \
                 nothing was recorded. A function whose parameter is not a plain tuple of cells \
                 cannot be recorded this way, and the parameter here has {} cell(s)",
                arity
            ),
        );
        return None;
    };
    let recorded = returned_value_ids::<P>(module, &result);
    // **An unreadable return is a refusal, not an unrecorded one.** A graph with
    // no recorded return answers with its whole value table, so treating "I
    // could not read what this function returns" as "it returns everything" hands
    // the caller an extent and a buffer nobody asked to have returned — a wrong
    // answer that runs. The permissive reading of an unrecorded return belongs to
    // a graph nobody recorded, and the language path has just recorded one.
    let Some(values) = recorded else {
        refuse(
            module,
            "this function's return is not a value a graph can hand back, so there is no \
             graph to record: a graph's value table holds what its dispatches produced and the \
             arguments it was run with, and this body's own value is not one of those"
                .into(),
        );
        return None;
    };
    if let Err(reason) = graph::record_return(values) {
        refuse(module, reason);
        return None;
    }
    let value = match graph::finish() {
        Ok((graph, backend)) => {
            let id = graph::intern(graph);
            <P::Value as From<ComputeValue>>::from(ComputeValue::Graph(id, backend))
        }
        Err(reason) => {
            // **An empty recording with a decided result is a different mistake
            // from an empty one with no result**, and only this one says the body
            // *ran* — so the two refusals have to name different causes or a
            // caller will go looking in the wrong place.  An undecided result
            // was refused above, so the body's own value here is decided.
            let reason = format!(
                "{reason} (the body's own value came back as a decided value, not a lazy one)"
            );
            refuse(module, reason);
            return None;
        }
    };
    Some(value)
}

/// A lichen tuple as a **node**, which is what an operand or a result has to be.
///
/// An array value is a handle into `block`'s arena and an operand slot wants a
/// node, so the value is allocated and then given a node of its own. Every item
/// is already a node, so the tuple holds live nodes rather than detached values —
/// the same discipline every other multi-value result in this file follows.
fn array_node<P>(module: &mut Module<P>, block: BlockId, items: &[ArrayItem]) -> NodeId
where
    P: Program,
    P::Value: From<LowValue>,
{
    let handle = module.alloc_array(items, block);
    let value = <P::Value as From<LowValue>>::from(LowValue::Array(handle));
    module.add_node(block, None, Some(value))
}

/// The placeholders a result node names, walked **through the structure** a named
/// parameter's result is.
///
/// A kernel's result is the parameter's `.out` group — a `Buf`-wrapped
/// placeholder per output, nested exactly as the author wrote it — so the leaves
/// are the values the graph can hand back, in field order.  A body whose
/// parameter has no `.out` fields returns the unit value, which names nothing: a
/// graph's return is a list of value references, and the empty list is what "the
/// body produced nothing" is recorded as.
fn place_of_node<P>(module: &Module<P>, node: AnyNodeId) -> Option<Vec<Placed>>
where
    P: Program,
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
{
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
        // **A type slot is not a value.**  The `Buf` wrapper's `.element` holds
        // the element's type, which is not something the table names — it is the
        // type level's half of the wrapper, and the payload beside it is the
        // half a run reads.  Anything else this walk cannot name is the refusal
        // it reports.
        if module
            .node_value(item.node)
            .is_some_and(|value| ValueType::is_kind_marker(&value))
        {
            continue;
        }
        placed.extend(place_of_node::<P>(module, item.node)?);
    }
    Some(placed)
}

/// The leaves of a graph run's argument, in the order the recording numbered its
/// slots: **depth-first in field order**.
///
/// A nested array is a group — the parameter's shape — and its leaves follow in
/// field order, which is the order the recording walked the same structure in
/// (`build_graph` sorts the role paths, and a lexicographic path order *is*
/// depth-first field order).  A **type slot** — the `Buf` wrapper's `.element` —
/// is not a value and is skipped, the same rule the recording's return walk
/// follows.
fn flatten_leaves<P>(module: &mut Module<P>, node: AnyNodeId, out: &mut Vec<AnyNodeId>)
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
{
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
        let nodes: Vec<AnyNodeId> = items.iter().map(|item| item.node).collect();
        for item in nodes {
            flatten_leaves::<P>(module, item, out);
        }
        return;
    }
    out.push(node);
}

/// The value references a recorded body's result names, if it is a placeholder or
/// a tuple of them.
///
/// **A block's value is taken from its first item, and that is this crate's
/// existing convention rather than a new one:** [`compile_fragment`] and
/// [`parallel_output_nodes`] both resolve a function's return by reading
/// `array_items(body)[0]`, because a block's tuple is wider than the one value
/// the function returns. Reading the whole tuple instead would ask the graph to
/// return every statement's value, which is a different function's answer.
///
/// **Both placeholder kinds are accepted, and an extent is the reason.** A
/// function that returns its own argument is returning an *input* value, which
/// is in the table like any other; refusing it would leave the return
/// unrecorded, and a graph with no recorded return answers with its whole value
/// table — which is a number and a buffer nobody asked to have returned.
///
/// `None` rather than a refusal, because a body may return something a graph's
/// value table cannot name at all — a kernel, say. The graph's return is then
/// unrecorded, and the caller reads that as "take everything", which is
/// permissive rather than broken.
fn returned_value_ids<P>(module: &Module<P>, value: &P::Value) -> Option<Vec<Placed>>
where
    P: Program,
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue> + ValueType,
{
    // The items of a result are **nodes**, not values, so each one is read back
    // through the module rather than matched in place. That is the same reason a
    // multi-output launch gives every buffer a node of its own before wrapping
    // them in a tuple.
    let place_of = |node: AnyNodeId| place_of_node::<P>(module, node);
    // One lichen value read as the references it names: a bare placeholder is
    // one, and a **materialized tuple** is one per element — the multi-value
    // form, a function that genuinely returns `(a, b)`. Anything else is not
    // something a graph's value table can name, which is the `None` this whole
    // function reports.
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
            placed.extend(place_of(item.node)?);
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
    let node = dyn_node(first.node).ok()?;
    let inner = module.node_value(AnyNodeId::Dynamic(node))?;
    placed_of(&inner)
}

/// Run a graph over the values its source function took.
///
/// **The arguments are sorted into roles here rather than by the operator's
/// caller**, and that is the whole contract of a value table: a buffer becomes
/// the slot a dispatch records against, a number becomes the extent a dispatch
/// runs over, and anything else is refused by name. A jit'd function may take
/// arbitrary lichen values, and this is where they are asked what role they
/// have.
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
        module.record_extension_diagnostic(GRAPH_DIAGNOSTIC, None, reason);
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
    // **The argument is walked the way the recording numbered its slots**: leaves
    // in depth-first field order, with a wrapper's type slot skipped.  The flat
    // tuple a positional body reads is a structure too — one level, in order — so
    // one walk serves both shapes.
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
        let Some(value) = module
            .node_value(*node)
            .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
        else {
            // Not a compute value at all, so it is a number — the only other role
            // a dispatch can read. Asked on its own terms rather than through the
            // compute vocabulary, because a count is a lichen `Int` and not a
            // compute leaf.
            match module
                .node_value(*node)
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
            {
                Some(LowValue::USize(count)) => {
                    run_arguments.push(RunArgument::Count(count as i64));
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
    let values = match graph::run(
        id,
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
    // **Left where they are.** A value the device wrote stays a resident id and
    // crosses the bus when the language asks for host data, which is the same
    // discipline a single launch follows; a run that fetched everything on the
    // way out would put a download on the path of every result.
    let value = if produced.len() == 1 {
        match &produced[0] {
            RunResult::Buffer { class, data } => <P::Value as From<ComputeValue>>::from(
                ComputeValue::Buffer(module.alloc_payload(data, block), *class),
            ),
            RunResult::Resident(resident) => {
                <P::Value as From<ComputeValue>>::from(ComputeValue::DeviceBuffer(*resident))
            }
            RunResult::Count(count) => <P::Value as From<LowValue>>::from(LowValue::USize(*count)),
        }
    } else {
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
                };
                let node = module.add_node(block, None, Some(value));
                ArrayItem::new(AnyNodeId::Dynamic(node))
            })
            .collect();
        let handle = module.alloc_array(&items, block);
        <P::Value as From<LowValue>>::from(LowValue::Array(handle))
    };
    Some(value)
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

/// The compiled **module cache** — the launch paths' derived-data cache: an
/// assembled, validated wasm module per launch, so a repeated launch of the
/// same kernel does not re-assemble and re-validate it.
///
/// The key is everything the module depends on: the launch mode (which owns
/// the *fragment set* — [`run_kernel`] assembles the root's relative launch
/// set, [`run_parallel_kernel`] one parallel fragment) and the root
/// [`KernelId`].  The id alone is sufficient because a registered fragment is
/// immutable and an id is never reused, so a key's assembly is fixed for the
/// process's life; it is also process-unique, so two different programs can
/// never share an entry — only a repeated launch of one kernel hits.  Both
/// facts are the kernel registry's contract (see its doc): the day an entry
/// can be removed or replaced there, this cache must be keyed on the fragments
/// themselves or cleared with it.
///
/// An entry here is *derived*: it can always be rebuilt from the fragment, so
/// eviction cannot lose anything a value refers to — unlike the kernel and
/// buffer registries, whose entries **are** the referents of `Kernel`/
/// `ParKernel`/`Buffer` values.  That is why a bound with eviction is sound
/// here and not there.
///
/// The [`wasmi::Engine`] is cached **with** its module and deliberately not
/// shared process-wide: wasmi's default `CompilationMode::LazyTranslation`
/// validates eagerly and translates each function on first use into the
/// **engine's** code map, which is append-only and freed only with the engine.
/// Dropping an evicted entry's engine is therefore what frees its translated
/// code, and one shared engine would turn this cache into a second unbounded
/// accumulator — the defect it exists to avoid.
static MODULES: OnceLock<Mutex<ModuleCache>> = OnceLock::new();
fn modules() -> &'static Mutex<ModuleCache> {
    MODULES.get_or_init(Default::default)
}

/// How many compiled modules stay resident.  The entries are rebuildable, so
/// this only trades recompiles against memory; it is a bound, not a policy.
const MAX_CACHED_MODULES: usize = 64;

/// Compiled modules this process built rather than served from [`MODULES`].
///
/// Test- and measurement-visible on purpose, in the same spirit as the package
/// store's `compiled`/`loaded_from_cache`: the value of content-addressed
/// kernel ids (`D15`) is precisely that this stops growing when the same
/// function is compiled again, and a counter is the only way to see that
/// without timing a wasm compile.
static MODULE_CACHE_MISSES: AtomicUsize = AtomicUsize::new(0);

/// See [`MODULE_CACHE_MISSES`].
pub fn module_cache_misses() -> usize {
    MODULE_CACHE_MISSES.load(Ordering::Relaxed)
}

/// One cached module and the engine that compiled it.
struct CachedModule {
    /// The engine the module was compiled by — see [`MODULES`] for why it is
    /// cached here rather than shared.
    engine: wasmi::Engine,
    module: wasmi::Module,
    /// Insertion stamp: the eviction order, oldest (smallest) first.
    stamp: u64,
}

/// The bounded map behind [`MODULES`].
#[derive(Default)]
struct ModuleCache {
    entries: HashMap<KernelId, CachedModule>,
    next_stamp: u64,
}

impl ModuleCache {
    /// A resident module, cloned out (both are `Arc` handles) so the caller
    /// does not hold the lock while instantiating.
    fn get(&self, key: KernelId) -> Option<(wasmi::Engine, wasmi::Module)> {
        self.entries
            .get(&key)
            .map(|cached| (cached.engine.clone(), cached.module.clone()))
    }

    /// Insert a freshly compiled module, evicting the oldest entry once the
    /// bound is reached.  Replacing a resident key evicts nothing: the assembly
    /// for a key is deterministic, so the entry is the same module.
    fn insert(&mut self, key: KernelId, engine: wasmi::Engine, module: wasmi::Module) {
        if self.entries.len() >= MAX_CACHED_MODULES
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, cached)| cached.stamp)
                .map(|(key, _)| *key)
        {
            self.entries.remove(&oldest);
        }
        let stamp = self.next_stamp;
        self.next_stamp += 1;
        self.entries.insert(
            key,
            CachedModule {
                engine,
                module,
                stamp,
            },
        );
    }
}

/// The compiled module for `root`, assembling and compiling it on a miss.
///
/// **One entry per kernel id, and no mode beside it.** Both launch paths assemble
/// the root's relative launch set — a parallel fragment cross-calls exactly as a
/// scalar one does — so the id decides the module and a second key component
/// would only be a second way to reach one assembly.
fn cached_module(
    root: KernelId,
    assemble: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<(wasmi::Engine, wasmi::Module), String> {
    let key = root;
    if let Some(cached) = modules().lock().unwrap().get(key) {
        return Ok(cached);
    }
    MODULE_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);
    let bytes = assemble()?;
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes).map_err(|e| e.to_string())?;
    modules()
        .lock()
        .unwrap()
        .insert(key, engine.clone(), module.clone());
    Ok((engine, module))
}

/// Execute a compiled kernel on an argument vector with wasmi, returning its
/// results — one [`ScalarValue`] per value the fragment declares
/// ([`KernelFragment::results`]).  The dynamic [`wasmi::Func::call`] API accepts
/// any number of values, so a tuple-domain kernel (arity N) launches with N
/// arguments and a scalar kernel (arity 1) with one, each of the class its
/// parameter leaf is.
///
/// **Both the argument count and the output buffer are decided here, not by
/// `wasmi`.**  All three numbers are facts compute already holds — the callee's
/// registered domain flattened by [`flat_arity`], its registered result arity,
/// and the vector built from the argument — so the refusals can state them,
/// where a mismatch left to `wasmi::Func::call` reports only that a count was
/// wrong.  The **argument** check is the *only* place it happens for a
/// `compute.call`, whose `CallOp` gate deliberately leaves the argument's shape
/// unconstrained (a fresh domain cell), so this message is the only account of a
/// wrong-arity call.  The **output** buffer is sized from the fragment because a
/// too-small one would make wasmi report a type error against a signature this
/// crate itself emitted.  A registered fragment's `param_shape` is a decided
/// scalar or tuple of them ([`kernel_domain`]), which is what makes
/// [`flat_arity`] an exact parameter count here rather than its filler.
///
/// **The argument classes are checked here too**, and this is the one place they
/// can be: the callee's leaf classes and the vector built from the argument are
/// both in hand, and a mis-classed argument is otherwise an `i64` a float
/// parameter would silently read as its bits.  `launch`'s checker gate catches
/// the mismatch for an annotated kernel; `call`'s fresh domain cell does not, so
/// this is its only account of it
/// (`docs/notes/floating-point.md` §4.2).
///
/// The kernel's **relative launch set** — the kernel itself plus every kernel
/// it (transitively) cross-calls, discovered by scanning each fragment's
/// cross-kernel instructions — is assembled into one wasm module (launch-time
/// assembly, the deferred linker), the root exported as `main`.  The module is
/// fetched through [`cached_module`], so a repeat launch of the same kernel
/// reuses it (`P1-18`).
fn run_kernel(id: KernelId, args: &[ScalarValue]) -> Result<Vec<ScalarValue>, String> {
    // The leaf classes, the result arity and the fragment's class are read
    // under one lock and released before assembly, which locks the same registry
    // again through [`assemble_launch_set`].
    let (expected, results, class) = {
        let fragments = kernels().lock().unwrap();
        let fragment = fragments
            .get(&id)
            .ok_or_else(|| format!("kernel {id} is not registered"))?;
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
    // A fragment's result arity is at least one (a body always leaves a value),
    // so the buffer is never empty; the guard keeps the invariant stated here
    // rather than resting on the emitter alone.
    let outputs = results.max(1);
    let (engine, module) = cached_module(id, || assemble_launch_set(id))?;
    let mut store = wasmi::Store::new(&engine, ());
    let linker = wasmi::Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_func(&store, "main")
        .ok_or_else(|| "kernel has no export `main`".to_string())?;
    let inputs: Vec<wasmi::Val> = args.iter().map(|argument| argument.wasmi()).collect();
    let mut results: Vec<wasmi::Val> = (0..outputs)
        .map(|_| ScalarValue::wasmi_zero(class))
        .collect();
    main.call(&mut store, &inputs, &mut results)
        .map_err(|e| e.to_string())?;
    results
        .iter()
        .enumerate()
        .map(|(position, value)| {
            ScalarValue::from_wasmi(class, value).ok_or_else(|| {
                format!(
                    "kernel `main` returned a non-{class:?} value at result {position}, against \
                     the {class:?} signature this crate emitted"
                )
            })
        })
        .collect()
}

/// The fragments of a kernel's **relative launch set**, in BFS order, and where
/// each kernel id sits in that order.
///
/// The set is the root plus every kernel it (transitively) cross-calls, so
/// `ordered[i]` is position `i` and `index` maps a callee kernel-id to that
/// position. **The fragments are cloned and the registry lock released before
/// this returns**, because every consumer emits from the set and an emitter that
/// reaches the registry again would deadlock on a lock that is not reentrant.
///
/// One derivation, and both consumers take it: [`assemble_launch_set`] links it
/// into wasm and [`run_on_installed_backend`] hands it to an installed backend,
/// so the two cannot enumerate a call graph differently.
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
            .cloned()
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

/// Assemble the wasm bytes of the root kernel's **relative launch set** — the
/// root plus every kernel it (transitively) cross-calls.
///
/// The result is a function of the root id alone (the registry's fragments are
/// immutable and ids are never reused), which is what makes the id a sufficient
/// cache key for [`cached_module`].
fn assemble_launch_set(id: KernelId) -> Result<Vec<u8>, String> {
    let OrderedLaunchSet { ordered, index } = ordered_launch_set(id)?;
    assemble_module(&ordered, &index)
}

/// The execution state a parallel kernel's host imports read/write against:
/// the input buffers (indexed by cfg position) and **one worker's partition**
/// of the output buffers (indexed by output ordinal, then by element).  Carried
/// as the wasmi [`wasmi::Store`] data, so the `read`/`write` imports reach it
/// through `Caller::data`/`data_mut`.
///
/// **The partition invariant.**  A worker owns one contiguous element span of
/// *every* output buffer, and no two workers own overlapping spans, so the
/// `write` import is a plain store into a slice it exclusively borrows: there
/// is no lock anywhere on the write path, and no slot is written by two
/// workers.  A sequential run is the same state with `base = 0` and the whole
/// buffer as the span.
///
/// The state *borrows* its output buffers rather than owning them, so the
/// caller keeps the `Vec<Vec<i64>>` it hands back as the result and
/// `std::thread::scope` can lend a partition that is not `'static`.  The base
/// offset lives here rather than in the `write` closure because
/// [`wasmi::Linker::func_new`] requires a `'static` host function — a
/// per-worker base could not be captured, only reached through the store.
struct ParallelState<'a> {
    /// The input buffers, indexed by the input position the role walk records.
    /// They
    /// are never partitioned: a read is by a **global** index, so every worker
    /// reads the whole buffer.  A float input's words are its elements' `f32`
    /// bits, so the `read` import answers `F32` rather than `I64`.
    inputs: &'a [BufferWords],
    /// This worker's span of each output buffer, indexed by the write's
    /// `out_pos` ordinal.  There is one span per output the index function
    /// declares — the count comes from the compiled fragment
    /// ([`KernelFragment::outputs`]), never from which slots happened to be
    /// written.  The words are the same for both classes, one per element.
    outputs: Vec<&'a mut [i64]>,
    /// The global index of this partition's first element, which the `write`
    /// import adds to the index the kernel passes.
    base: usize,
    /// The class the whole fragment is lowered in — the value type of the
    /// `read`/`write` imports and of the kernel's own parameters and result.
    class: ScalarClass,
    /// The launch's **scalar leaves**, one word per leaf in field order: the
    /// parameter's scalars as the ABI passes them, the launch extent first
    /// (`docs/notes/compute-runtime-scalars.md` §1).  The index is not here — it
    /// is the worker's loop variable, appended per element.
    leaves: &'a [i64],
    /// Each leaf's own class, so a leaf reaches `main` as the value its parameter
    /// field declared rather than as the fragment's element class: a runtime
    /// `Float` scalar is an `f32` argument beside `i64` ordinals.
    leaf_classes: &'a [ScalarClass],
}

/// Hand a run to the installed [`lichen_kernel_ir::ParallelBackend`].
///
/// **A missing backend is a refusal, not a fallback.** `parallel` names its
/// backend explicitly and there is no "try either" value, so a program that said
/// `"gpu"` and silently ran on the CPU would be a program whose timing means
/// nothing. There is no third value to fall back *to* either: the choice is the
/// author's, and overriding it here would be the one place the language's
/// explicit dataflow quietly stopped being explicit.
///
/// The fragment is cloned out of the registry — as is every fragment it
/// cross-calls, into one launch set — and the lock released before the call,
/// because a backend that emits a cross-kernel call reads the set again, and the
/// lock is not reentrant.
fn run_on_installed_backend(
    id: KernelId,
    count: usize,
    inputs: &[RunInput],
    outputs: usize,
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
    let launch = lichen_kernel_ir::LaunchSet::new(&ordered, &index);
    // The class check runs before the dispatch: a host slot is a raw bit
    // payload, so a buffer of the wrong class would reach the device as the
    // right shape and the wrong numbers.
    check_input_classes(&fragment.input_classes, inputs)?;
    // The one place a run can be wired to leave its inputs where they are: an
    // input a previous run left on the device is handed back as the id it already
    // has, so a chain of kernels pays one upload for the whole chain rather than
    // one per link.
    //
    // **The host slot is the buffer's packed elements, and its class is the
    // fragment's.** `BufferSlot` carries a raw byte payload and no class of its
    // own, so the bytes a backend reads a host slot as are the ones the class's
    // width packs — which is exactly why [`check_input_classes`] runs first.
    // The packed payloads are named first rather than built in the slot
    // expression: a slot borrows its bytes, so they have to outlive it.
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

    // Nothing is fetched and nothing is released.  Fetching here would put the
    // download back on the path of every run, which is the cost the id exists to
    // remove; releasing here would free buffers the caller is about to hand to the
    // next kernel.  Both happen at the point the language actually wants host
    // data — `collect` and `read` — and the ids live as long as the values do.
    //
    // Each id's class is the producing fragment's declared class for that output
    // ordinal, read here because this is the one place that holds both the
    // fragment and the id it produced.  A fragment that declared fewer classes
    // than outputs would be a fragment whose class list disagrees with its own
    // count, so the fallback is the ABI's integer default rather than a panic in
    // a run's result path.
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

/// The most elements one `plrun` may collect — the bound on the count the
/// program controls (the parameter's `.n` the index function is run over).
///
/// The count sizes the output buffer (`count` × 8 bytes = 8 MiB at the limit)
/// and, because the kernel is called once per element, the interpreted work a
/// single launch can do.  Both are otherwise unbounded: `2^40` is a legal
/// `Int`, and it asks for 8 TiB and 10^12 wasm calls.  A count past the limit
/// is **refused**, never truncated: a short buffer would be a wrong answer,
/// and there is no diagnostic channel for a runtime refusal in this plugin's
/// vocabulary (see `run_parallel_kernel`).
const MAX_PARALLEL_ELEMENTS: usize = 1 << 20;

/// The element count at or above which a parallel run spreads its indices over
/// worker threads; below it the run stays on the calling thread.
///
/// Below this bound a run gives up the fan-out's wall-clock time to keep its
/// own latency: every worker costs a thread spawn plus its own store, linker
/// and instantiation, and that is only repaid once a worker owns enough indices
/// for the interpreted calls to dominate it.  [`MAX_PARALLEL_ELEMENTS`] already
/// separates the two regimes (the first few thousand indices are launch
/// overhead, the rest is work), so this bound is read off the same trade — a few
/// thousand indices cost milliseconds of interpretation against a spawn measured
/// in tens of microseconds, which is where the fan-out stops being a
/// pessimisation.
const SEQUENTIAL_PARALLEL_ELEMENTS: usize = 1 << 12;

/// How many workers a parallel run of `count` indices uses, the calling thread
/// among them (it runs the first chunk itself, so the count of 1 is the
/// sequential run and spawns nothing).
///
/// Two rules: never below [`SEQUENTIAL_PARALLEL_ELEMENTS`] — the fan-out must
/// pay for itself — and never above what the machine has available or what
/// there are indices for, because a worker with no index to run is pure
/// overhead.  An unavailable [`std::thread::available_parallelism`] reads as
/// one, which is the sequential run.
fn parallel_worker_count(count: usize) -> usize {
    if count < SEQUENTIAL_PARALLEL_ELEMENTS {
        return 1;
    }
    let available = std::thread::available_parallelism().map_or(1, |count| count.get());
    available.clamp(1, count)
}

// How many workers the calling thread's most recent parallel launch used.  It is
// a **thread-local**, not a process counter like `MODULE_CACHE_MISSES`: the value
// is about *one* launch and is written by the thread that launched it, so a
// shared global would be overwritten by a concurrent launch before the observer
// read it.  (A `thread_local!` is a macro invocation, so it takes a plain
// comment; the contract is on the accessor.)
thread_local! {
    static PARALLEL_LAUNCH_WORKERS: Cell<usize> = const { Cell::new(1) };
}

/// How many workers the calling thread's most recent parallel launch used; `1`
/// is a sequential run (below [`SEQUENTIAL_PARALLEL_ELEMENTS`], or a machine
/// with one available processor).
///
/// Test- and measurement-visible on purpose, for the reason
/// [`module_cache_misses`] gives: a parallel result is bit-identical to a
/// sequential one by construction, so *no assertion on values* can tell a
/// working fan-out from a dead code path.  This is what a test asserts on.
pub fn parallel_launch_workers() -> usize {
    PARALLEL_LAUNCH_WORKERS.get()
}

/// Run a **parallel** kernel over the index range `[0, count)`, computing the
/// index function once per index with the parameter's extent `count` and its
/// input buffers fixed, and collecting the writes into the output buffers.
///
/// The kernel is a wasm function with two host imports — `read(cfg_pos, idx)`
/// reads an input buffer element, `write(out_pos, idx, val)` writes an output
/// buffer element — wired to the host-side input/output buffers through the
/// [`ParallelState`] the store carries.  The kernel is called once per index;
/// the writes accumulate into the output buffers (last write to a slot wins, a
/// scatter).
///
/// **The run is parallel.**  The index range is cut into one contiguous chunk
/// per worker, each worker gets a **disjoint span of every output buffer**
/// ([`ParallelState`]'s partition invariant) and its own store, linker and
/// instance over the one cached module and engine, and the calling thread runs
/// the first chunk itself.  The kernel sees **global** indices in every chunk
/// and the `write` import rebases by the worker's base, so the emitted wasm is
/// identical whichever worker runs it.
///
/// How many output buffers there are is the compiled fragment's
/// [`KernelFragment::outputs`] — the index function's codomain arity, read when
/// the kernel was compiled — so the allocation here is a static fact and never
/// a discovery of which slots happened to be written.  Every ordinal is written
/// on every index (see [`compile_parallel_fragment`]), so each buffer is fully
/// defined after the run.
///
/// **Determinism.**  The result is bit-identical to the sequential loop's, for
/// every `count` and whatever the worker count: the chunks depend only on the
/// count and the worker count (never on a schedule or a timing), each worker
/// writes only the slots it owns, and no value is accumulated or reduced — so
/// regrouping the indices cannot change what is computed.  This holds exactly
/// for an index function that writes its own slot, which is what the primitive
/// is a map over.  The general statement is narrower: the result equals the
/// sequential loop's **iff no two indices write the same slot**, and a kernel
/// that writes a slot that is not its own
/// (`compute.write ((compute.Write _)(.to n, .at i - i, .value v))`,
/// whose index is `0` for every `i`, so all of them collide) already had an
/// order-dependent winner sequentially — the partition, not the launch, is then
/// what decides it.
///
/// `count > `[`MAX_PARALLEL_ELEMENTS`] is refused with an `Err` before the
/// buffers are allocated.  The caller records the reason through
/// [`Module::record_extension_diagnostic`] and returns the lazy
/// (undecided) answer, which is this plugin's channel for every runtime
/// refusal: `Module::eval_errors` is a closed enum of structural value facts,
/// and its `BudgetExhausted` names the apply/depth budgets — false here, the
/// program terminated and merely asked for too much.  **Queueing is not the
/// alternative:** `plrun` is a synchronous, caller-blocking call, so there is
/// nothing to queue onto — the choice is refuse or run.
///
/// The module comes from [`cached_module`], and one `Engine` backs every
/// worker's store.  The [`wasmi::Linker`] is deliberately rebuilt per worker
/// rather than cached or shared: it is the object that carries host-function
/// bindings to *that* worker's state, so building it here makes "no host
/// binding is shared between two launches, or between two workers of one"
/// true by construction, and its cost is the two fixed registrations below.
fn run_parallel_kernel(
    id: KernelId,
    backend: Backend,
    leaves: Vec<i64>,
    inputs: Vec<RunInput>,
) -> Result<RunOutcome, String> {
    // The ABI's first leaf is the launch extent (`docs/notes/compute-runtime-scalars.md`
    // §1): it is how many indices the dispatch covers, so it is also the one
    // ordinal this side needs.
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
    // The output count **and the element class** are properties of the
    // *registered fragment*, so they are read here rather than carried in: a
    // kernel id is content-addressed, so the fragment it names cannot be a
    // different one.  The lock is released before any emission or assembly,
    // which locks the same registry again.
    let (outputs, class, input_classes, leaf_classes, output_classes) = {
        let fragments = kernels().lock().unwrap();
        let fragment = fragments
            .get(&id)
            .ok_or_else(|| format!("parallel kernel {id} is not registered"))?;
        let classes = param_classes(fragment);
        (
            fragment.outputs,
            fragment_class(fragment),
            fragment.input_classes.clone(),
            // The ABI's leaves are the parameter's scalars followed by the
            // index; the index is the worker's loop variable, so only the
            // scalars are handed in.
            classes[..classes.len().saturating_sub(1)].to_vec(),
            fragment.output_classes.clone(),
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
    // A borrow of data already out of the registry, so the class check is a
    // function of the classes rather than a second reason to hold the lock.
    check_input_classes(&input_classes, &inputs)?;
    if let Backend::Gpu = backend {
        return run_on_installed_backend(id, count, &inputs, outputs);
    }
    // A `"cpu"` run has no device, so an input a `"gpu"` run left there is
    // brought home before the run starts.  This is the one place the two
    // backends meet, and it is a *fetch* rather than a silent refusal: the
    // program's data is on the device and the CPU has no way to reach it, so
    // moving it is what running on the CPU means — not a change in what is
    // computed.  A fetched payload is turned into the same words a host input
    // holds, so the interpreted run cannot tell the two apart.
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
    let (engine, module) = cached_module(id, || assemble_launch_set(id))?;
    let mut outputs: Vec<Vec<i64>> = (0..outputs).map(|_| vec![0i64; count]).collect();
    let workers = parallel_worker_count(count);
    PARALLEL_LAUNCH_WORKERS.set(workers);
    if workers == 1 {
        // The sequential run *is* the one-worker run: the calling thread owns
        // every slot, and nothing is spawned.
        let state = ParallelState {
            inputs: &inputs,
            outputs: output_spans(&mut outputs),
            base: 0,
            class,
            leaves: &leaves,
            leaf_classes: &leaf_classes,
        };
        run_parallel_range(&engine, &module, state, 0, count)?;
    } else {
        // Contiguous chunk bounds, a function of the count and the worker count
        // alone: the leading chunks carry the remainder elements, so the
        // calling thread's chunk is never the shortest.
        let bounds = chunk_bounds(count, workers);
        let partitions = partition_outputs(output_spans(&mut outputs), &bounds);
        let mut failures: Vec<Option<Result<(), String>>> = (0..workers).map(|_| None).collect();
        // Scoped, not detached: every worker borrows its partition and the
        // engine, so the scope joins them all before the borrowed buffers are
        // read back.  A scoped thread cannot be aborted, so a failing worker is
        // **joined like any other** and the run reports the first failure in
        // index order below — a failure is never dropped for a partial result.
        std::thread::scope(|scope| {
            for ((failure, partition), window) in
                failures.iter_mut().zip(partitions).zip(bounds.windows(2))
            {
                let (base, end) = (window[0], window[1]);
                let engine = &engine;
                let module = &module;
                let inputs = &inputs;
                let leaves = &leaves;
                let leaf_classes = &leaf_classes;
                let run = move || {
                    run_parallel_range(
                        engine,
                        module,
                        ParallelState {
                            inputs,
                            outputs: partition,
                            base,
                            class,
                            leaves,
                            leaf_classes,
                        },
                        base,
                        end,
                    )
                };
                // The first chunk is the calling thread's own work: it runs here
                // and joins only the others.
                if base == 0 {
                    *failure = Some(run());
                    continue;
                }
                scope.spawn(move || *failure = Some(run()));
            }
        });
        for (worker, failure) in failures.into_iter().enumerate() {
            if let Some(Err(message)) = failure {
                return Err(format!(
                    "parallel launch of kernel {id} failed in worker {worker}, \
                     over indices [{}, {}): {message}",
                    bounds[worker],
                    bounds[worker + 1]
                ));
            }
        }
    }
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

/// Refuse a run whose input buffers are not the class the fragment reads them
/// as.
///
/// **The one place the two halves of a buffer's class meet before an upload**:
/// the fragment declares a class per read position, and each value carries the
/// class it was produced with.  A disagreement is a wrong number rather than a
/// wrong shape — the words are the same 64 bits either way — so it is refused by
/// name rather than reinterpreted, the same discipline
/// [`fetch_resident`] applies to a device's answer
/// (`docs/notes/floating-point.md` §4.2, §4.4).
fn check_input_classes(declared: &[ScalarClass], inputs: &[RunInput]) -> Result<(), String> {
    for (position, input) in inputs.iter().enumerate() {
        let declared = declared.get(position).copied().unwrap_or(ScalarClass::Int);
        let held = match input {
            RunInput::Host(buffer) => buffer.class,
            RunInput::Resident(resident) => resident.class,
        };
        if held != declared {
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
/// The count the buffer holds travels with the value, so a fetch asks for what
/// the run actually produced rather than for the device's padded allocation.  The
/// class travels with it too, which is what makes the answer's own class the
/// value's rather than a guess: a fetch has no fragment to consult, so the only
/// thing that can say whether these elements are integers or floats is the
/// resident value it was asked about.
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
        // The device's answer and the class this value was issued with disagree,
        // and the fetch is the only thing that can see both.  Refused by name
        // rather than reinterpreted: reading an `f32` payload as `i64` (or the
        // reverse) is not a wrong shape, it is a wrong number, and it would
        // check perfectly (`docs/notes/floating-point.md` §4.3).
        false => Err(format!(
            "input buffer {position} (buffer {}) was issued as a {:?} buffer but the backend \
             returned {:?} elements",
            resident.id.0,
            resident.class,
            data.class()
        )),
    }
}

/// The chunk end of each of `workers` contiguous chunks of `count` indices, so
/// `bounds.len() == workers + 1`, `bounds[0] == 0` and `bounds[workers] ==
/// count`.
///
/// A function of its two arguments and nothing else — not of a schedule or a
/// timing — so a given partition is reproducible, and (each index being handled
/// by exactly one worker) the result does not depend on how the indices are
/// grouped.  Chunk `k` is `count / workers` long, and the first
/// `count % workers` chunks are one element longer, so the chunks differ in
/// length by at most one element and always cover every index exactly once.  The
/// calling thread takes chunk `0`, so giving the *front* chunks the extra
/// elements keeps the later workers — which pay an extra wakeup before they
/// start — off the shortest chunk.
fn chunk_bounds(count: usize, workers: usize) -> Vec<usize> {
    let (length, extra) = (count / workers, count % workers);
    (0..=workers)
        .map(|index| index * length + index.min(extra))
        .collect()
}

/// Every output buffer as a mutable span, in output-ordinal order — the whole
/// buffer, for a worker that owns every slot.
fn output_spans(outputs: &mut [Vec<i64>]) -> Vec<&mut [i64]> {
    outputs
        .iter_mut()
        .map(|buffer| buffer.as_mut_slice())
        .collect()
}

/// Cut every buffer of `spans` into one contiguous chunk per `[bounds[i],
/// bounds[i + 1])`, in worker order.
///
/// Each split consumes the spans by value so that both halves come back with
/// the full lifetime the store's state needs: a `&mut` reborrowed *through* a
/// vector element cannot outlive the vector borrow, so moving each span's
/// ownership out first is what makes the partition possible without `unsafe`.
/// Every span is split once per level, and the result is the disjoint
/// `bounds` partition of `[0, count)`.
fn partition_outputs<'a>(
    mut spans: Vec<&'a mut [i64]>,
    bounds: &[usize],
) -> Vec<Vec<&'a mut [i64]>> {
    let mut partitions: Vec<Vec<&'a mut [i64]>> = Vec::with_capacity(bounds.len() - 1);
    for index in 0..bounds.len() - 1 {
        // The last chunk is what is left, so it is not split again.
        if index + 1 == bounds.len() - 1 {
            partitions.push(spans);
            break;
        }
        let (heads, tails) = split_spans(
            std::mem::take(&mut spans),
            bounds[index + 1] - bounds[index],
        );
        partitions.push(heads);
        spans = tails;
    }
    partitions
}

/// Cut the leading `len` elements off every span of `spans` — the same length in
/// each, because a worker owns a span of *every* output buffer.
fn split_spans<'a>(
    spans: Vec<&'a mut [i64]>,
    len: usize,
) -> (Vec<&'a mut [i64]>, Vec<&'a mut [i64]>) {
    let mut heads = Vec::with_capacity(spans.len());
    let mut tails = Vec::with_capacity(spans.len());
    for span in spans {
        let (head, tail) = span.split_at_mut(len);
        heads.push(head);
        tails.push(tail);
    }
    (heads, tails)
}

/// Run one worker's index range `[base, end)` — the store, the linker, the
/// instance and the per-index call loop, for a worker that owns exactly
/// `state.outputs`' slots.
///
/// The extent is the first of `state.leaves`, and it is the **whole launch's**:
/// the index function is a function of the full extent, not of the chunk, so a
/// worker must not see a narrowed one.
fn run_parallel_range(
    engine: &wasmi::Engine,
    module: &wasmi::Module,
    state: ParallelState<'_>,
    base: usize,
    end: usize,
) -> Result<(), String> {
    let class = state.class;
    let mut store = wasmi::Store::new(engine, state);
    let mut linker = wasmi::Linker::<ParallelState<'_>>::new(engine);

    // **One `read`/`write` pair per class**, matching what `assemble_module`
    // declared: the position and the index are `i64` in both, and only the
    // element's own type follows the class.  Both pairs are defined even when the
    // module declares only one, and the two sides derive the names from the class
    // through [`buffer_import_name`] rather than agreeing by construction.
    for (element_class, value_type) in [
        (ScalarClass::Int, wasmi::ValType::I64),
        (ScalarClass::Float, wasmi::ValType::F32),
    ] {
        let read_ty =
            wasmi::FuncType::new([wasmi::ValType::I64, wasmi::ValType::I64], [value_type]);
        let write_ty =
            wasmi::FuncType::new([wasmi::ValType::I64, wasmi::ValType::I64, value_type], []);
        linker
            .func_new(
                "env",
                &buffer_import_name(element_class, "read"),
                read_ty,
                move |caller: wasmi::Caller<'_, ParallelState<'_>>,
                      params: &[wasmi::Val],
                      results: &mut [wasmi::Val]| {
                    let pos = position_of(params.first());
                    let idx = position_of(params.get(1));
                    // The index is **global** — inputs are never partitioned — so
                    // no rebase here; a worker reads the whole input buffer.  The
                    // word is the element's own bits for both classes: an `f32`'s
                    // for a float buffer, the value for an integer one.
                    let value = caller
                        .data()
                        .inputs
                        .get(pos)
                        .and_then(|buffer| buffer.words.get(idx))
                        .copied()
                        .unwrap_or(0);
                    results[0] = word_value(element_class, value);
                    Ok(())
                },
            )
            .map_err(|e| e.to_string())?;
        linker
            .func_new(
                "env",
                &buffer_import_name(element_class, "write"),
                write_ty,
                move |mut caller: wasmi::Caller<'_, ParallelState<'_>>,
                      params: &[wasmi::Val],
                      _results: &mut [wasmi::Val]| {
                    let out_pos = position_of(params.first());
                    let idx = position_of(params.get(1));
                    let value = params.get(2).map_or(0, |v| value_word(element_class, v));
                    let state = caller.data_mut();
                    // **The rebase.**  The kernel is handed a global index, and the
                    // worker owns `[base, base + span.len())` of this buffer, so the
                    // store is at `idx - base`.  A write outside the worker's own
                    // span cannot be rebased into it — `checked_sub` yields `None`
                    // and the write is dropped, exactly as an out-of-range write is
                    // today, rather than aliasing another worker's slots.
                    if let Some(slot) = state
                        .outputs
                        .get_mut(out_pos)
                        .and_then(|buffer| buffer.get_mut(idx.checked_sub(state.base)?))
                    {
                        *slot = value;
                    }
                    Ok(())
                },
            )
            .map_err(|e| e.to_string())?;
    }

    let instance = linker
        .instantiate_and_start(&mut store, module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_func(&store, "main")
        .ok_or_else(|| "parallel kernel has no export `main`".to_string())?;
    // The leaves are fixed for the whole range and the index is the argument the
    // loop rewrites, so the list is built once: a leaf is handed over as the
    // value its own parameter field declared.
    let mut args: Vec<wasmi::Val> = {
        let state = store.data();
        state
            .leaf_classes
            .iter()
            .zip(state.leaves)
            .map(|(class, word)| word_value(*class, *word))
            .collect()
    };
    args.push(word_value(ScalarClass::Int, base as i64));
    let mut results = [word_value(class, 0)];
    for i in base..end {
        // **The ABI's arguments are the parameter's scalar leaves in field order,
        // then the index** — so the index is the one argument that moves per
        // element, and every leaf is handed over as the value *its own field*
        // declared (`docs/notes/compute-runtime-scalars.md` §1).  The extent is
        // leaf 0, which is why `count` is passed whole: the index function is a
        // function of the full extent, not of the chunk.
        //
        // A leaf that is not data — the extent and the index — is `i64` whatever
        // class the body computes in.  The result is the dummy the fragment leaves
        // on the stack, in the fragment's own class.
        if let Some(index) = args.last_mut() {
            *index = word_value(ScalarClass::Int, i as i64);
        }
        main.call(&mut store, &args, &mut results)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The position or the index a `read`/`write` import's argument carries.
///
/// **Always an `i64`, in every class**: a position is a compile-time ordinal in
/// the buffer space and an index is a lane number, so neither is ever the data.
/// The conversion this replaces read an `f32` back into an integer, which
/// existed only because a fragment had one class
/// (`docs/notes/floating-point.md` §4.4).
fn position_of(value: Option<&wasmi::Val>) -> usize {
    value.and_then(|value| value.i64()).unwrap_or(0) as usize
}

/// The word a `read`/`write` import's value argument carries, in the fragment's
/// class — an `f32`'s bits for a float fragment, the value for an integer one.
fn value_word(class: ScalarClass, value: &wasmi::Val) -> i64 {
    match class {
        ScalarClass::Int => value.i64().unwrap_or(0),
        ScalarClass::Float => float_bits(value.f32().map_or(0.0, |bits| bits.to_float())),
    }
}

/// One buffer word as the wasm value a class takes.
fn word_value(class: ScalarClass, word: i64) -> wasmi::Val {
    match class {
        ScalarClass::Int => wasmi::Val::I64(word),
        ScalarClass::Float => wasmi::Val::F32(wasmi::F32::from_bits(word as u32)),
    }
}

// --- The parallel run: the worker rule, the partition, the fan-out ---

#[cfg(test)]
mod parallel_launch_tests {
    use super::*;
    // **`KernelBody` and `FlatOp` reach a hand-written body**, which is the test's own
    // construction.
    use lichen_kernel_ir::{FlatOp, KernelBody, ResidentId};

    /// A two-output parallel fragment over `(n, i)`: `out0[i] = i + 1` and
    /// `out1[i] = i + i`, with each `BufferWriteCall` fed the
    /// `[out_pos, idx, val]` stack its host import takes.  The trailing
    /// `Const(0)` is what `compile_parallel_fragment` appends: the index
    /// function only has side effects, and the shared assembler's `-> i64`
    /// signature needs one value left on the stack.
    fn two_outputs() -> KernelFragment {
        KernelFragment {
            param_shape: KernelShape::Tuple(vec![
                KernelShape::Scalar(ScalarClass::Int),
                KernelShape::Scalar(ScalarClass::Int),
            ]),
            // The tuple form's parameter shape: a count at position 0 and the
            // buffers at position 1, which is what a walk-less ABI reads.
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
            int_width: IntWidth::I64,
        }
    }

    /// The compute backend slot is **process-global**, so tests that install or
    /// clear one have to be serialized against each other: cargo runs a test
    /// binary's tests on parallel threads, and two of them installing different
    /// backends would make each other's assertion a race. This is not a test
    /// harness artefact — the same global is what a host program configures once.
    static BACKEND_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A parallel kernel compiled for the `"gpu"` backend goes to the installed
    /// backend, not to this crate's thread pool.  A stub stands in for a device
    /// here: what is under test is the **routing**, and the real backend's
    /// execution is proved by `lichen-compute-gpu`'s own tests.
    ///
    /// The two are deliberately tested apart. A test that drove a real device
    /// would prove the routing and the device at once, and would then fail on a
    /// machine with no GPU — turning a routing regression into a hardware
    /// question.
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
        let id = intern_kernel(two_outputs());
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
    /// A real device cannot report this, and it is the thing worth pinning: if the
    /// chain fell back to host data the *values* would still be right, so a
    /// value-comparison cannot tell the difference between "the intermediate
    /// stayed on the device" and "the intermediate came home and went back out".
    struct ChainRecorder {
        seen: Mutex<Vec<Vec<Slot>>>,
        fetched: AtomicUsize,
    }

    /// What one input slot was, as the backend was handed it.
    ///
    /// A host slot records its **elements**, which is the payload over the
    /// class's width ([`ScalarClass::byte_width`]) — a host slot is bytes, and a
    /// count of bytes is not a count of the elements a run reads.
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
        let id = intern_kernel(two_outputs());
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

    /// A `"cpu"` run has no device to read from, so an input a `"gpu"` run left
    /// there is brought home before the run starts.  This is a *fetch* rather
    /// than a refusal: the program's data is on the device, and moving it is
    /// what running on the CPU means — not a change in what gets computed.
    #[test]
    fn a_cpu_run_brings_home_an_input_a_gpu_run_left_on_the_device() {
        let _serialized = BACKEND_SLOT.lock().unwrap();
        let stub = chain_recorder();
        lichen_kernel_ir::install_parallel_backend(stub.clone());
        let id = intern_kernel(two_outputs());
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

    /// A `"gpu"` run with **no** backend installed is refused by name. There is
    /// no third value to fall back to: the choice is the author's, and quietly
    /// running on the CPU would make the program's timing meaningless.
    #[test]
    fn a_gpu_run_without_an_installed_backend_is_refused_by_name() {
        let _serialized = BACKEND_SLOT.lock().unwrap();
        lichen_kernel_ir::clear_parallel_backend();
        let id = intern_kernel(two_outputs());
        let refusal = run_parallel_kernel(id, Backend::Gpu, vec![8], vec![])
            .expect_err("there is nothing to dispatch to");
        assert!(
            refusal.contains("gpu") && refusal.contains("installed"),
            "the refusal names the backend asked for and what is missing: {refusal}"
        );
    }

    /// A backend that declines is reported, not swallowed: the CPU path is not a
    /// fallback here, so the reason has to reach the diagnostic.
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
        let id = intern_kernel(two_outputs());
        let refusal = run_parallel_kernel(id, Backend::Gpu, vec![8], vec![])
            .expect_err("the backend declined");
        lichen_kernel_ir::clear_parallel_backend();
        assert!(
            refusal.contains("stub") && refusal.contains("no compute queue"),
            "the refusal names the backend and passes its reason through: {refusal}"
        );
    }

    /// The backend names a string parameter can be checked against, and the
    /// language has no enum type to lean on, so the parse is strict: a typo is
    /// reported with both what was written and what is accepted.
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

    /// The threshold is a **count** boundary and nothing else: a run below it
    /// must not spawn, which is what keeps a handful of indices from getting
    /// slower than the single-store run it replaced.
    #[test]
    fn a_run_below_the_threshold_uses_one_worker() {
        assert_eq!(parallel_worker_count(0), 1);
        assert_eq!(parallel_worker_count(1), 1);
        assert_eq!(parallel_worker_count(SEQUENTIAL_PARALLEL_ELEMENTS - 1), 1);
    }

    /// ...and at or above it the fan-out is the machine's, never more: one
    /// worker per index would be pure overhead, and `available_parallelism` is
    /// what bounds it above.
    #[test]
    fn a_run_over_the_threshold_is_capped_by_the_machine() {
        let count = SEQUENTIAL_PARALLEL_ELEMENTS;
        let available = std::thread::available_parallelism().map_or(1, |n| n.get());
        let workers = parallel_worker_count(count);
        assert_eq!(workers, available.clamp(1, count));
        assert!(
            workers <= count,
            "never more workers than indices to process: {workers} > {count}"
        );
    }

    /// The chunk bounds are the determinism argument in one assertion: they are
    /// a function of the count and the worker count alone, they tile
    /// `[0, count)` with no gap and no overlap, and no chunk is more than one
    /// element longer than the shortest.
    #[test]
    fn the_chunk_bounds_tile_every_index_exactly_once() {
        for count in [SEQUENTIAL_PARALLEL_ELEMENTS, 4097, MAX_PARALLEL_ELEMENTS] {
            for workers in [1, 2, 3, 8, 20] {
                let bounds = chunk_bounds(count, workers);
                assert_eq!(bounds.len(), workers + 1, "{count} over {workers}");
                assert_eq!(bounds[0], 0);
                assert_eq!(bounds[workers], count, "the chunks must cover the range");
                let lengths: Vec<usize> = bounds
                    .windows(2)
                    .map(|window| window[1] - window[0])
                    .collect();
                assert!(
                    lengths.iter().all(|len| *len > 0),
                    "every chunk must be non-empty: {lengths:?}"
                );
                assert_eq!(
                    lengths.iter().sum::<usize>(),
                    count,
                    "every index belongs to exactly one chunk: {lengths:?}"
                );
                let (shortest, longest) = (
                    *lengths.iter().min().unwrap(),
                    *lengths.iter().max().unwrap(),
                );
                assert!(
                    longest - shortest <= 1,
                    "chunks must differ by at most one element: {lengths:?}"
                );
                assert!(
                    lengths[0] >= longest,
                    "the calling thread's chunk must not be the shortest: {lengths:?}"
                );
            }
        }
    }

    /// The partition the workers actually get: every output buffer cut into
    /// the same disjoint spans, so the union is each whole buffer exactly once.
    #[test]
    fn the_partitions_are_a_disjoint_cover_of_every_output_buffer() {
        let count = 100;
        let workers = 4;
        let mut outputs: Vec<Vec<i64>> = (0..3).map(|_| (0..count as i64).collect()).collect();
        let bounds = chunk_bounds(count, workers);
        let partitions = partition_outputs(output_spans(&mut outputs), &bounds);
        assert_eq!(partitions.len(), workers);
        for (worker, partition) in partitions.iter().enumerate() {
            assert_eq!(partition.len(), 3, "one span per output buffer");
            let expected: Vec<i64> = (bounds[worker]..bounds[worker + 1])
                .map(|index| index as i64)
                .collect();
            for span in partition {
                assert_eq!(
                    &span[..],
                    &expected[..],
                    "worker {worker} must own exactly indices [{}, {})",
                    bounds[worker],
                    bounds[worker + 1]
                );
            }
        }
    }

    /// The host buffers of a `"cpu"` run.  A CPU run has no device to leave
    /// anything on, so a resident outcome here would be a routing bug rather
    /// than a shape to allow.
    fn host_outputs(id: KernelId, count: usize) -> Vec<BufferWords> {
        match run_parallel_kernel(id, Backend::Cpu, vec![count as i64], vec![])
            .expect("the run must succeed")
        {
            RunOutcome::Host(outputs) => outputs,
            RunOutcome::Resident(_) => panic!("a \"cpu\" run must produce host buffers"),
        }
    }

    /// The fan-out itself.  A parallel result is bit-identical to a sequential
    /// one, so no value can distinguish a working fan-out from a dead code
    /// path — [`PARALLEL_LAUNCH_WORKERS`] is what a test can see, and this
    /// checks the run actually took it.
    #[test]
    fn a_run_over_the_threshold_fans_out_and_covers_every_index() {
        let id = intern_kernel(two_outputs());
        let count = SEQUENTIAL_PARALLEL_ELEMENTS;
        let outputs = host_outputs(id, count);
        assert_eq!(outputs.len(), 2, "one buffer per declared output");
        // The machine must have the processors to fan out at all; on a
        // single-processor run the worker count is 1 and the run is the
        // sequential one, which the values below still pin.
        if std::thread::available_parallelism().map_or(1, |n| n.get()) > 1 {
            assert!(
                parallel_launch_workers() > 1,
                "a run of {count} elements must not be the single-worker one"
            );
        }
        // First, middle and last index of each buffer: the boundaries between
        // two workers are where a wrong rebase or a skipped chunk would show.
        for index in [0, count / 2, count - 1] {
            assert_eq!(outputs[0].words[index], index as i64 + 1, "out0[{index}]");
            assert_eq!(outputs[1].words[index], index as i64 * 2, "out1[{index}]");
        }
    }

    /// The same kernel below the threshold, on the calling thread: the
    /// sequential result the fan-out has to reproduce.
    #[test]
    fn a_run_below_the_threshold_is_sequential_and_writes_every_index() {
        let id = intern_kernel(two_outputs());
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

/// The `lichen-compute` native plugin marker — the nominal declaration that
/// this crate plays the native-plugin role
/// ([`lichen_highlevel::plugin::NativePlugin`]).
///
/// The trait carries no methods and nothing is generic-bound on it, so the
/// `impl` is a declaration, not a check: the contract is carried by the
/// macro-based composition, not by the trait.  A unit marker: the plugin
/// contributes its [`ComputeValue`] / [`ComputeOperator`] leaves and its native
/// op registry (via [`compute_native_ops!`]), and never names a concrete host
/// program.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputePlugin;

impl lichen_highlevel::plugin::NativePlugin for ComputePlugin {}

/// Assemble `lichen-compute`'s private native-operator registry for a host
/// program `$program`, expanding to a `&'static` [`NativeOps`].
///
/// Invoked by a host that composes the plugin (see `lichen-language`'s
/// `package.rs`), so the `$jit`/`$launch` names stay private to the plugin's
/// own embedded source.  The host names only the plugin crate and its program
/// marker — never the plugin's op structs — so this is the composition point a
/// package manager would generate.
///
/// The table is **leaked deliberately, and the leak is bounded**: the slice is
/// nine entries of `(&'static str, &'static dyn NativeOp<$program>)`, 144 bytes
/// on a 64-bit target, and exactly one is allocated per call.  The slice cannot
/// be hoisted into a `static` initializer instead: a `static` may not name a
/// type parameter, and two invocations of this macro are distinct items even
/// when they name the same program, so no shape of `static` or `OnceLock` can be
/// shared across them.  Caching it on the host would be no better: a table held
/// per store serves a compilation that already happens per store, so the same
/// single allocation is made either way.  The one call site
/// (`PackageStore::register_compute`, reached from `PackageStore::new`) runs
/// once per store — the store compiles its embedded wrapper source once and
/// keeps the frozen module — so this is a fixed handful of bytes per store, not
/// a per-compile or per-keystroke cost.
#[macro_export]
macro_rules! compute_native_ops {
    ($program:ty) => {{
        // The self-supporting `static`s below are the operator structs, which
        // are program-independent; the leaked slice is the table above.
        static JIT: $crate::JitOp = $crate::JitOp;
        static LAUNCH: $crate::LaunchOp = $crate::LaunchOp;
        static CALL: $crate::CallOp = $crate::CallOp;
        static PARALLEL: $crate::ParallelOp = $crate::ParallelOp;
        static PARLAUNCH: $crate::ParLaunchOp = $crate::ParLaunchOp;
        static RANGE: $crate::RangeOp = $crate::RangeOp;
        static READ: $crate::ReadOp = $crate::ReadOp;
        static WRITE: $crate::WriteOp = $crate::WriteOp;
        static COLLECT: $crate::BufferCollectOp = $crate::BufferCollectOp;
        static GRAPH: $crate::GraphOp = $crate::GraphOp;
        static GRAPHRUN: $crate::GraphRunOp = $crate::GraphRunOp;
        let ops: Vec<(&'static str, &'static dyn $crate::NativeOp<$program>)> = vec![
            ("jit", &JIT as &dyn $crate::NativeOp<$program>),
            ("launch", &LAUNCH as &dyn $crate::NativeOp<$program>),
            ("call", &CALL as &dyn $crate::NativeOp<$program>),
            ("parallel", &PARALLEL as &dyn $crate::NativeOp<$program>),
            ("plrun", &PARLAUNCH as &dyn $crate::NativeOp<$program>),
            ("range", &RANGE as &dyn $crate::NativeOp<$program>),
            ("read", &READ as &dyn $crate::NativeOp<$program>),
            ("write", &WRITE as &dyn $crate::NativeOp<$program>),
            ("collect", &COLLECT as &dyn $crate::NativeOp<$program>),
            ("graph", &GRAPH as &dyn $crate::NativeOp<$program>),
            ("graphrun", &GRAPHRUN as &dyn $crate::NativeOp<$program>),
        ];
        Box::leak(ops.into_boxed_slice()) as $crate::NativeOps<$program>
    }};
}

// --- Native operators: the private contract with the plugin's source -------

/// `$jit(f)` — compile a function to a kernel.  The function-ness gate unifies
/// the argument's type with an arrow shape (the *gate*); the bare artifact's
/// type is a fresh cell, and the lichen wrapper builds the kernel struct around
/// it (`.native` = the artifact, `.I`/`.O` = its signature).
///
/// The program marker is generic: a host composes this op into its own
/// `NativeOps` registry (a `&'static [(&str, &dyn NativeOp<P>)]`), so the
/// `$jit`/`$launch` names stay private to the plugin's own embedded source.
pub struct JitOp;

/// `$launch(native, a, aty, kty)` — run kernel `native` on `a`.  The wrapper
/// reads the kernel's `.I`/`.O` fields and passes the argument's type and the
/// declared domain as values; this op unifies them and emits the `Launch` node.
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
        let f = &args[0];
        // The bare native kernel artifact — the lichen wrapper wraps this value
        // into a `kernel` struct (`.native`).  It is opaque: its type is the
        // call's fresh cell, and the signature rides in the struct's `.I`/`.O`
        // fields, which the wrapper binds from its own annotation `f: I -> O`.
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Jit), Some(f.value));
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
    /// `$launch(native, a)` — run kernel `native` on `a`.  The wrapper states
    /// both constraints (`a: k.I`, `r: k.O`) and hands over raw values; the op
    /// emits the `Launch` node over `[native, a]` and states no type at all.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let native = &args[0];
        let a = &args[1];
        // The `Launch` operator reads exactly these two elements.  The kernel's
        // declared domain is not an operand: the wrapper's own `a: k.I` is where
        // it is read, so it is already in the application's graph.
        let operands = ctx.array_node(&[native.value, a.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Launch), Some(operands));
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

/// `$call(k, a)` — a **cross-kernel call** on native kernel `k`.  The lichen
/// wrapper extracts `.native` out of the kernel struct (`call = k => a =>
/// $call(k.native, a)`); the native op emits the `Call` node over two raw
/// values.  The callee signature is read at launch-time assembly; nothing about
/// the argument or the result is stated here.
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

/// `$parallel(f, backend)` — compile a single-arg `compute.P` index function
/// into a parallel kernel, and record the backend its runs are dispatched to.
/// The function-ness gate verifies `f` is a function; the body is lowered over the
/// loop index (from `compute.range n`) and the parameter's `.in` buffers (read via
/// `compute.read`).  A **tuple** codomain of `Write`s is the multi-output form;
/// which position a write is becomes its output ordinal at emission time, and the
/// codomain's arity becomes the launch's output count.
pub struct ParallelOp;

/// `$plrun(pk, cfg)` — run a parallel kernel over the index range `[0, cfg.n)`
/// with the parameter's `.in` buffers fixed, collecting the writes into the
/// parameter's `.out` structure, one `Buf` field per output.  The count is
/// `cfg.n`.
pub struct ParLaunchOp;

/// `$range(n)` — the loop index `i ∈ [0, n)` of the current parallel
/// invocation.  Kernel-only (lowers to the index parameter).
pub struct RangeOp;

/// `$read(buf, i)` — read one buffer element → `?b`.  In-kernel this lowers to
/// the host `read` import; at the VM it reads a `Buffer` value's element.
pub struct ReadOp;

/// `$write(n, i, val)` — a pending parallel write into the output buffer at `i`
/// (length `n`).  Which output buffer is decided by the write's position in the
/// index function's codomain, not here.  Kernel-only (lowers to the host
/// `write` import).
pub struct WriteOp;

/// `$collect(buf)` — collect a whole buffer into a lichen array `[?b]`.
pub struct BufferCollectOp;

/// `$graph(f)` — record `f`'s dispatches into a graph instead of running them.
///
/// The function-ness gate is the same arrow the apply will check, so a
/// non-function is a check error here rather than a refusal at run time.
pub struct GraphOp;

/// `$graphrun(g, a)` — run a graph over the values its source function took.
///
/// **Both types are fresh cells, and both are deliberate.** A graph's type is
/// host-owned and opaque — typed `_`, like a kernel's `.native` — because what a
/// graph may be applied to is a question about the function it was recorded from,
/// and the recording is not available to the checker. The arguments therefore
/// unify against a fresh cell too, which is what lets a graph take values whose
/// types only the run knows. The cost is static precision, not safety: a
/// `graphrun` result is a fresh cell and an out-of-range ordinal on it is still
/// refused at check time with a span, exactly as it is for a `plrun` result.
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
        // The bare native parallel kernel artifact — the lichen wrapper wraps
        // this value into a `kernel` struct (`.native`).  Opaque: the call's own
        // fresh cell.  `f`'s function-ness and the backend's `string`-ness are
        // stated by the wrapper's annotations, not here.
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
    /// `$plrun(native, a)` — run parallel kernel `native` over `a` (the `cfg`).
    /// The lichen wrapper extracts `.native` out of the kernel struct; this op
    /// emits the `ParLaunch` node over the two raw values.
    ///
    /// **The result type is the call's own fresh cell**, and that is a
    /// deliberate limit, not an oversight.  The parameter's `.out` group is what
    /// decides the result's shape — one `Buf` field per declared output — and it
    /// cannot be read here: `build` runs once, on the frozen `plrun` template,
    /// where the kernel's `.I`/`.O` are undecided cells that only resolve at run
    /// time.  A struct type is a value node with one cell per field, so no
    /// check-time node can name a structure whose fields are not known until the
    /// run.
    ///
    /// What the fresh cell costs is **static precision, not safety**: the
    /// element type is no longer named by the signature, so `read` on a `plrun`
    /// result binds its own element cell and resolves it from the value the
    /// run produces — the buffer's own class, `Int` or `Float`, which is what
    /// makes a float `plrun` readable as an array of `Float`.  An ordinal that
    /// does not exist is still **refused, at check time, with a span** — the
    /// checker's evaluation pass reconciles the constant index against the
    /// container the launch produced and records an out-of-bounds `Index` — so
    /// the two shapes stay distinguishable exactly where it matters.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let native = &args[0];
        let a = &args[1];
        let operands = ctx.array_node(&[native.value, a.value]);
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
    /// **The index's class is the class the body computes in.**  A parallel
    /// fragment's two scalar parameters are both that class —
    /// `compile_parallel_fragment` writes its `param_shape` as
    /// `[Scalar(class), Scalar(class)]` — the emitter pushes the index local
    /// unchanged, and the host converts both roles back to ordinals
    /// (`const_bits(class, …)`, `run_parallel_range`).  Nothing is stated here:
    /// the call's fresh cell is resolved from the value the fragment produces,
    /// and the fragment's own ABI is what carries the class
    /// (`docs/notes/floating-point.md` §4.2, §4.4).
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

impl<P> NativeOp<P> for ReadOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$read(buf, i)` — read one buffer element.  The wrapper gates its
    /// argument as the `Read` struct (`read = (x : Read _) => $read(x.from,
    /// x.at)`) and leaves the result to the call's fresh cell; this op emits
    /// the `Read` node over the two raw values.  In a kernel body it lowers to
    /// the host `read` import; at the VM it reads a buffer value's element.
    ///
    /// **The element class is a fact of the *value*, and it resolves there.**
    /// The producing fragment's declared class, or the class a backend issued a
    /// resident buffer with, is what the result cell turns out to be — the cell
    /// stays open until something decides it, which is why a program that only
    /// forwards a buffer prints its element as `?a` rather than as the class the
    /// value turns out to be, and why a `buffer<Float>` is readable at all
    /// (`docs/notes/floating-point.md` §3.7, §4.2, §4.4).
    ///
    /// **The index is an ordinal in every class.**  The `read` import's
    /// signature is `(i64, i64) -> element`, with the buffer ordinal and the
    /// index `i64` regardless of class and only the element's type following it
    /// (`assemble_module`; `run_parallel_range` declares the same closures).  A
    /// position and a lane number are ordinals, not data, so pinning them to the
    /// element's cell — which is what this used to do — made an index "the class
    /// the buffer is", exactly what a `Float` buffer beside a decided `Int`
    /// count could not express.
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
    /// `$write(n, i, val)` — a pending parallel write into the output buffer at
    /// index `i` (length `n`).  The wrapper gates its argument as the `Write`
    /// struct (`write = (x : Write _) => $write(x.to, x.at, x.value)`); this op
    /// emits the `Write` node over the three raw values.  Which output buffer a
    /// write belongs to is decided by its position in the index function's
    /// codomain, not here.  Kernel-only (lowers to the host `write` import).
    ///
    /// **The written value's class is the buffer's element class**, so a
    /// `buffer<Float>` is expressible and a `buffer<Int>` is unchanged: the
    /// call's fresh cell resolves from the value the run produces, the same
    /// mechanism `plrun`'s result type relies on.  An index function that only
    /// forwards a buffer read keeps working: its element class is read off the
    /// buffer at run time, and its emission defaults to `Int` exactly as it
    /// always did (`docs/notes/floating-point.md` §3.7, §4.2).
    ///
    /// **The length and the index are ordinals in every class.**  The `write`
    /// import is `(i64, i64, element)`: the count and the loop index are `i64`
    /// regardless of class and only the written value follows the element's
    /// class (`assemble_module`; `run_parallel_range` declares the same
    /// closures).  A length and a lane number are ordinals rather than data, so
    /// tying them to the *value's* cell — which is what this used to do — made an
    /// ordinal "the class the data is", and that is what refused a float write
    /// beside a decided `Int` count.
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
        // The result is an array of the buffer's element, and **the element
        // class stays open**: it is a fact of the value the collection reads, so
        // naming `Int` here is exactly what would make `collect` on a float
        // buffer inexpressible.  The call's fresh cell is what the run resolves
        // to `[[?b, len], [TypeArray, Type]]`, with the length a runtime count.
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

impl<P> NativeOp<P> for GraphOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$graph(f)` — record `f`, don't run it.
    ///
    /// The wrapper's `f : _ -> _` is the function-ness gate — the same arrow the
    /// apply will check, so a non-function is a check error rather than a refusal
    /// at run time. **The arity is deliberately not checked**: it is the length
    /// of `f`'s parameter tuple, which is a *runtime* fact of a value the checker
    /// has not cloned yet, and a gate that named an arity it cannot read would
    /// refuse programs the recording accepts.
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        _loc: Loc,
    ) -> NativeApply {
        let f = &args[0];
        // The graph itself is host-owned and opaque: what a graph may be run over
        // is a question about the function it was recorded from, and that
        // function is not available to the checker — a run is what finds out.
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
    /// **The arguments and the result are the call's own fresh cell, and that is
    /// what makes a graph reusable across runs.** A graph's parameter tuple is a
    /// *runtime* shape — how many arguments it takes is the length of the tuple
    /// the recording read off a function value — so a fixed domain type would
    /// name an arity the checker cannot know, and would refuse exactly the
    /// programs a recording accepts. The cost is static precision, not safety: an
    /// out-of-range ordinal on the result is still refused at check time with a
    /// span, exactly as it is for a `plrun` result.
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
        NativeApply {
            value: op,
            decided: false,
        }
    }
}

/// The `lichen-compute` plugin's embedded lichen source — the real `compute`
/// plugin file, kept as a `.lichen` source file and embedded with
/// [`include_str!`].  It defines the user-facing `jit`/`launch` functions as
/// ordinary typed lichen (whose bodies call the native `$jit`/`$launch`), and
/// exports them as a **named struct** (`compute.jit`, `compute.launch`).
///
/// Its `type_of` helper is a private `let` binding — not a field of the
/// exported struct — and repeats [`lichen-std`]'s definition, because an
/// embedded native source is compiled against this plugin's private native
/// registry and cannot depend on a package.
///
/// [`lichen-std`]: https://github.com/windwhiterain/lichen-vm/tree/dev/lichen-std
pub const WRAPPER_SOURCE: &str = include_str!("compute.lichen");
