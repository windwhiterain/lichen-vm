//! The `lichen-compute` extension: a native "compute" wrapper package.
//!
//! The native part injects a [`ComputeValue`] vocabulary — the **`Kernel`**
//! value (a compiled, runnable wasm artifact), a **`ParKernel`** value, a
//! **`Buffer`** value, and the **`TypeBuffer`**/`TypeWrite` kind markers — plus
//! the first-class **`Native` Operator values** (`jit`, `launch`, `call`,
//! `parallel`, `plrun`, `range`, `read`, `write`, `collect`), and the
//! [`ComputeOperator`]s
//! `Jit`/`Launch`/`Call`/`Parallel`/`ParLaunch`/`Range`/`Read`/`Write`/
//! `BufferCollect`, whose [`OperatorExt::run`] does the wasm compile/execute
//! and the global kernel/buffer registries.
//!
//! A **kernel value is a lichen struct** `struct<.native _, .sig sig>`: the
//! `.native` field holds the opaque artifact (a `Kernel`/`ParKernel`), the
//! `.sig` field the function signature.  There is **no** `TypeKernel`/
//! `TypeParKernel` kind marker — the vocabulary does not special-case a kernel
//! type, so the checker and the shared renderer treat a kernel as an ordinary
//! struct.
//!
//! Operators are bound to source through the `NativeCall` IR: `$jit`, `$launch`,
//! and friends parse to a `NativeCall` that the checker routes to the matching
//! [`NativeOp`] builder, which emits the [`ComputeOperator`] and does that
//! operator's type check.  The runtime `LowOperator::Apply` never sees them.
//!
//! The plugin is **program-generic**: it never names a concrete host
//! `Program`.  Every entry point is bounded by the same set of
//! associated-type constraints a host program satisfies automatically when
//! its composed value/operator vocabularies carry [`ComputeValue`],
//! [`ComputeOperator`], [`LowOperator`], and [`TypeOperator`] (all leaves of
//! `LangProgram`'s `enum_ext!` composition).  A host composes those leaves
//! and wires the plugin's native registry itself (see the `lichen-language`
//! crate's `package.rs`).
//!
//! ## Type-checking coverage
//!
//! - `jit f` requires `f` to be a *function* (function-ness gate) and wraps the
//!   bare artifact into a kernel struct `struct<.native _, .sig (type_of f)>`.
//! - `launch k a` reads `k.native`/`k.sig`, gates the `.sig` (a function type,
//!   binding the domain/codomain lazily), unifies `a` against the domain, and
//!   its result is the kernel's codomain — a function-style apply over a kernel.
//!   A **tuple** codomain is the multi-result form: the body is flattened to one
//!   stack slot per leaf, the wasm function returns one `i64` per leaf, and the
//!   launch yields the tuple of them.
//! - `parallel f` lifts a single-arg `?cfg -> Write` index function into a
//!   parallel kernel struct (`cfg = (n, (buffer…))` — the count is `cfg(0)`,
//!   the input buffers a tuple at `cfg(1)`); `plrun k cfg` runs it over
//!   `[0, cfg(0))`, the index function reading inputs via
//!   `compute.read [cfg(1)(k), i]` and writing via `compute.write [n, i, val]`.
//!   A **tuple** codomain of `Write`s is the multi-output form: the `k`-th write
//!   is output buffer `k`, and `plrun` returns the buffers as a tuple.

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use lichen_graph_ir::Policy;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp};
use lichen_highlevel::program::{Ctx, HighProgram, TypeOperator, ValueType};
use lichen_highlevel::shape::{PAIR_TYPE_SLOT, PAIR_VALUE_SLOT, low_type_of_slot};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape, ResidentId,
    fragment_digest,
};
use lichen_lowlevel::codec::{OperatorCodec, Reader, ValueCodec, Writer};
use lichen_lowlevel::{
    AnyFunctionId, AnyHandle, AnyNodeId, ArrayItem, BlockId, Handle, LowOperator, LowShape,
    LowValue, Module, ModuleKey, NodeId, Operation, OperatorExt, Program, Release, StaticHandle,
    StaticModule, ValueExt,
};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

pub mod graph;

use graph::{Extent, Placed, RunArgument, RunResult};

pub use graph::GraphId;

/// The diagnostic a graph's own refusals are recorded under.
///
/// **Separate from the parallel one on purpose.** Every refusal this module emits
/// for a graph is about the *graph* — what it captured, which backend it named,
/// what it returned — and none of them is about a run. Filing them under
/// `compute.parallel` would attribute a mistake in the recording to the launch
/// the author did not write.
pub const GRAPH_DIAGNOSTIC: &str = "compute.graph";

/// The schedule a graph run uses.
///
/// **Rust-side only, and deliberately not in the language.** A graph says what has
/// to happen before what; it must never say when the host is allowed to notice
/// something finished, because a graph that could name its own synchronisation
/// would be a graph whose correctness depended on where somebody put a keyword —
/// and the builder and the runner would then disagree about the same graph.
/// [`Policy::Async`] is the default because it costs nothing over `Serial` and
/// everything under `Batch` depends on a host that asks for it explicitly.
pub fn set_graph_policy(policy: Policy) {
    GRAPH_POLICY.with(|current| current.set(policy));
}

thread_local! {
    static GRAPH_POLICY: Cell<Policy> = const { Cell::new(Policy::Async) };
}

/// The program-generic bounds the kernel-safe JIT requires.
///
/// A value vocabulary that composes [`ComputeValue`] (so a `Kernel` value
/// fits as a sibling leaf) and an operator vocabulary that composes the
/// structural [`LowOperator`], the highlevel's [`TypeOperator`] (the scalar
/// arithmetic the kernel-safe subset lowers), and [`ComputeOperator`]
/// (`Jit`/`Launch`).
///
/// [`ValueType`] is among them because a kernel's domain is *seeded* from the
/// parameter's type slot, and decoding a type slot is the encoding
/// authority's job ([`lichen_highlevel::shape::low_type_of`]) — the one place
/// this crate is allowed to read the `[value, type]` pair, and the read that
/// the low-type layer exists to end.
///
/// A host program satisfies these automatically whenever its `enum_ext!`
/// vocabulary carries those leaves (as `LangProgram` does).  Every codegen
/// entry point carries this same associated-type bound set as its `where`
/// clause, so all of them share one canonical constraint.
///
/// A compiled kernel artifact's identity — a compact index into the process
/// kernel registry (the compiled wasm bytes).  A kernel value is host-owned
/// (a small `Copy` scalar), so it is never an arena payload and never needs GC
/// re-homing or static freeze.
///
/// The id itself belongs to the lowered-kernel IR, because a cross-kernel call
/// in that IR has to name a callee and the IR must not depend on this crate to
/// say so.  Re-exported here because this is where a host finds it.
pub use lichen_kernel_ir::KernelId;

/// A runtime parallel-buffer artifact's payload: the `n` collected element
/// results, held **in the block arena** as an `i64` slice rather than in a
/// process registry (`D15`) — see [`ComputeValue::Buffer`].
///
/// The value is a `Copy` handle, exactly like the lowlevel's own array/table
/// payloads, so the buffer dies with its block and the crate's copy path
/// relocates it like any other payload.  There is no id, no registry and no
/// eviction: the arena's block lifetime *is* the ownership.
pub type BufferPayload = AnyHandle<[i64]>;

/// The process kernel registry: compiled kernel **fragments** (bytecode units),
/// keyed by [`KernelId`].  Kernels are immutable artifacts shared across
/// modules in the process.  The fragment is the durable JIT output; the module
/// bytes are derived on demand by [`assemble_module`] at launch.
static KERNELS: OnceLock<Mutex<HashMap<KernelId, KernelFragment>>> = OnceLock::new();
fn kernels() -> &'static Mutex<HashMap<KernelId, KernelFragment>> {
    KERNELS.get_or_init(Default::default)
}
/// The next kernel id — a **fallback** allocator, used only when a content
/// digest is already taken by a *different* fragment (see [`intern_kernel`]).
/// Monotone, so an id minted here never aliases another entry.
static NEXT_KERNEL_ID: AtomicUsize = AtomicUsize::new(0);
fn alloc_kernel_id() -> KernelId {
    NEXT_KERNEL_ID.fetch_add(1, Ordering::Relaxed)
}

/// The content index: a fragment's digest → the id it is registered under.
///
/// **Kernel ids are content-addressed** (`D15`).  A compiled fragment is a pure
/// function of the function it came from, so the same source recompiled — which
/// an editor does on every keystroke — must produce the *same* id, or nothing
/// downstream can be reused: the derived-module cache is keyed on
/// `(LaunchMode, KernelId)`, so a fresh id per compile means it can never hit
/// and every keystroke re-assembles and re-runs `wasmi::Module::new`.
///
/// A digest is not an identity, so the index is an **intern table and nothing
/// more**: [`intern_kernel`] verifies the fragment it finds, and a genuine
/// collision falls back to a unique id.  The loser of a collision simply stops
/// being interned — the cost is a recompile, never a wrong fragment.
static KERNEL_INDEX: OnceLock<Mutex<HashMap<u64, KernelId>>> = OnceLock::new();
fn kernel_index() -> &'static Mutex<HashMap<u64, KernelId>> {
    KERNEL_INDEX.get_or_init(Default::default)
}

/// Register `fragment` and return its id — the existing id when an identical
/// fragment is already registered.
fn intern_kernel(fragment: KernelFragment) -> KernelId {
    let digest = fragment_digest(&fragment);
    // The lookup and the insert are **one** decision, so both registries are held
    // across it.  Releasing the index in between lets a concurrent intern of the
    // same digest land its own id after this caller already read the entry, and
    // this caller is then handed a *second* id for the same fragment on a later
    // call — defeating content addressing, which every downstream cache is keyed
    // on.  Serializing the decision also means the reuse path never overwrites
    // the entry, so the index converges on one id per digest.
    //
    // **Lock order: `kernels()` before `kernel_index()`.**  This is the only
    // place that nests the two; the registry comes first because verifying a
    // candidate id means reading it, and the digest index is the outer
    // decision's lookup, not a prerequisite for it.
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

    fn fragment(body: Vec<KernelInstr>) -> KernelFragment {
        KernelFragment {
            param_shape: KernelShape::Scalar,
            body: body.into(),
            inputs: 0,
            outputs: 0,
            results: 1,
            int_width: IntWidth::I64,
        }
    }

    /// The point of content addressing: the same function compiled twice — which
    /// an editor does on every keystroke — must intern to **one** id, because
    /// everything downstream is keyed on it (the derived-module cache on
    /// `(LaunchMode, KernelId)`).  A fresh id per compile is what made that
    /// cache unable to hit.
    #[test]
    fn the_same_fragment_interns_to_one_id() {
        let first = intern_kernel(fragment(vec![KernelInstr::LocalGet(0)]));
        let second = intern_kernel(fragment(vec![KernelInstr::LocalGet(0)]));
        assert_eq!(
            first, second,
            "a recompiled identical fragment must reuse its id"
        );
    }

    /// ...and different fragments must not collide onto one id, or the cache
    /// would serve one kernel's module for another's.
    #[test]
    fn a_different_fragment_gets_a_different_id() {
        let a = intern_kernel(fragment(vec![KernelInstr::LocalGet(0)]));
        let b = intern_kernel(fragment(vec![KernelInstr::LocalGet(1)]));
        assert_ne!(a, b, "different content must not share an id");
        // Both stay registered under their own ids.
        let registry = kernels().lock().unwrap();
        assert!(registry.contains_key(&a) && registry.contains_key(&b));
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
fn buffer_items(payload: &BufferPayload) -> Option<&[i64]> {
    match payload {
        // SAFETY: the caller holds the value on a module borrow, so the
        // payload's home block — and therefore this slice — is alive.
        AnyHandle::Dynamic(handle) => Some(unsafe { &*handle.as_ptr() }),
        // A static payload would be a *frozen* buffer, which `P1-29` refuses to
        // serialize and so cannot exist: a buffer is runtime-only.
        AnyHandle::Static(_) => None,
    }
}

// The lowered-kernel IR — `KernelBin`, `KernelInstr`, `KernelFragment`,
// `KernelShape`, `IntWidth` and the content digest — lives in the dependency-free
// `lichen-kernel-ir` crate, not here, so that a backend other than this crate's
// wasm one can consume a fragment without pulling in `wasmi`.  What follows is
// the *wasm* half: this crate compiles a checked graph down to that IR and then
// lowers the IR to a wasm module.  See `docs/notes/compute-jit-low-types.md`.

/// Whether any operand of `operator` is one of a graph's placeholders.
///
/// **One level, and one level is enough.** Every operator that reads a dispatch's
/// output takes it as a direct item of its operand array — `collect [b]`, `read
/// [b, i]`, `call [k, a]` — so a graph's value is never buried inside a tuple the
/// scan would have to walk to find. A `plrun`'s cfg *is* a tuple of placeholders,
/// which is why this is asked about operators other than a parallel launch.
fn handed_a_placeholder<P>(module: &Module<P>, operand: &P::Value) -> bool
where
    P: Program,
    P::Value: AsEnum<LowValue> + AsEnum<ComputeValue>,
{
    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(operand) else {
        return false;
    };
    // SAFETY: the operand array is the operator's own, and this reads only, for
    // the length of the call.
    let items = unsafe { operands.items() };
    items.iter().any(|item| {
        matches!(
            module
                .node_value(item.node)
                .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value)),
            Some(ComputeValue::GraphInput(_) | ComputeValue::GraphValue(_))
        )
    })
}

/// Why this operator cannot appear in a body that is being recorded, if it cannot.
///
/// **Two shapes, and the difference is whether a graph has anywhere to put the
/// value at all.** A scalar kernel has no node to be: the one node kind is a
/// parallel dispatch with its buffers bound to it, while a scalar fragment works
/// on the kernel's own operand stack and names no buffer whatsoever. So whatever
/// it produces has no home in a graph, and that is true whether or not a
/// placeholder is involved — which is why this one is refused outright.
///
/// The other two are fine in general and wrong *here*. A host read and a collect
/// both say "give me this as host data", and the value either would hand back is
/// a number or an array that a graph has no edge for. They are refused only when
/// they were actually handed a placeholder, because a read of a host buffer the
/// body closed over is ordinary host arithmetic, and if its result is used as a
/// count or an input the value-table filter already refuses that by name.
///
/// Both of these used to fall through to a bare `Parameterized` with no
/// diagnostic, and that is not a smaller mistake than a wrong number — it is an
/// absent one. A body that collected a dispatch's result mid-chain recorded a
/// graph that was quietly missing the collect, and the chain's own numbers looked
/// right anyway.
fn unrecordable<P>(
    operator: &ComputeOperator,
    module: &Module<P>,
    operand: &P::Value,
) -> Option<String>
where
    P: Program,
    P::Value: AsEnum<LowValue> + AsEnum<ComputeValue>,
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

/// The diagnostic categories this plugin records through the lowlevel's
/// general extension channel ([`Module::record_extension_diagnostic`]).  They
/// are the plugin's own compile-time constants, so a consumer selects on them
/// without parsing a message.
const JIT_DIAGNOSTIC: &str = "compute.jit";
const PARALLEL_DIAGNOSTIC: &str = "compute.parallel";
/// Running an already-compiled kernel: the `Launch` and `Call` arms, which
/// give a [`KernelId`] an argument and read the result back.  Named for the
/// **path**, not for either operator tag, because the two refuse on the same
/// three facts — an argument that is not a parameter vector, a tuple element
/// that is not a concrete `Int`, and a run that failed — so a consumer
/// selecting on this category catches a `compute.call` refusal too instead of
/// needing to know which arm produced it.
const KERNEL_LAUNCH_DIAGNOSTIC: &str = "compute.kernel_launch";

/// Where a parallel kernel's runs are dispatched.
///
/// A **named, required** choice rather than a default: `parallel` takes the
/// backend as an argument, so a program always says where its parallel kernels
/// run and there is no ambient setting to be surprised by.  There is deliberately
/// no "try either" value — a backend that declines is a *named* failure, because
/// a program that silently ran somewhere other than where it asked is a program
/// whose timing means nothing.
///
/// Only `parallel` takes one.  A `jit` kernel is launched a single invocation at
/// a time, and a device is for the thousands a dispatch runs at once, so there is
/// nothing there for a backend to choose between.
///
/// The IR is unaffected: a fragment does not record a backend, so the choice
/// rides on the **value** rather than on the compiled kernel — which is why it
/// is a field of [`ComputeValue::ParKernel`] and not something the digest hashes.
/// Two programs that compile the same function for different backends therefore
/// share one fragment id, and the choice cannot make a cache serve one backend's
/// module for another's.
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
/// The message quotes what was written and what is accepted, because a string
/// parameter with no enum to check it against is exactly the case where a typo
/// must be reported rather than defaulted.  The language has no enum type yet,
/// which is why this is a string at all — and why the parse has to be strict.
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
/// A backend argument that is not a string at all is as much a refusal as one
/// that is the wrong string, and it is reported the same way — a `Str` parameter
/// has no type to lean on, so both are the programmer's to fix.
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
/// A `usize`, and stable for the life of the graph: a node's outputs are
/// consecutive from the first, so three outputs yield `first`, `first + 1`,
/// `first + 2` and nothing has to be looked up to know which is which.
pub type GraphValueId = usize;

/// The compute value vocabulary — injected as a sibling leaf into a host's
/// value union (see a host `program` module).  A plain enum of exactly this
/// extension's variants, composed with [`lichen_utils::enum_ext!`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComputeValue {
    /// A compiled, runnable kernel artifact.
    ///
    /// **No backend, deliberately.** A `jit` kernel is launched one invocation
    /// at a time, and a device is for the thousands a dispatch runs at once, so
    /// there is nothing for a backend to choose between. The choice belongs to
    /// [`Self::ParKernel`], which is the only variant a run is dispatched from.
    Kernel(KernelId),
    /// A compiled **parallel** kernel artifact, and where its runs are dispatched.
    ///
    /// A curried `?a -> USize -> ?b` function flattened to a `(?a, USize) -> ?b`
    /// wasm function (the config is the first group of parameters, the *index*
    /// the last scalar).
    ParKernel(KernelId, Backend),
    /// A runtime results **buffer**: `plrun` ran the parallel kernel over the
    /// index range `[0, n)` and collected the `n` `?b` results here.
    ///
    /// The payload is an `i64` slice **in the block arena** — a `Copy` handle,
    /// like the lowlevel's own array/table payloads — so a buffer is owned by
    /// the block it was created in and dies with it rather than living in a
    /// process registry that nothing can bound (`D15`, and see
    /// [`Self::is_handle`]).  `reads`/`collect`s therefore dereference the
    /// arena, and the crate's copy path relocates the payload when the value is
    /// copied into another block.
    Buffer(BufferPayload),
    /// A run's results, still **on the device**.
    ///
    /// A `"gpu"` `plrun` produces one of these per declared output rather than a
    /// [`Self::Buffer`], so nothing crosses the bus until something asks for the
    /// values as host data.  `compute.collect` and `compute.read` are what ask.
    ///
    /// **Not a handle, and that is the point:** the id is plain data, so the copy
    /// path copies it rather than relocating a pointer into a block arena.  That
    /// is also why there is no per-value release — the id names device memory
    /// that is reclaimed when the backend is dropped, or when an allocation is
    /// refused because the device is full.  The discipline is deliberately the
    /// same as [`Self::Buffer`], which likewise has no individual free.
    DeviceBuffer(ResidentBuffer),
    /// The kind marker of buffer types — a buffer's type is
    /// `[element_type, [TypeBuffer, Type]]`.
    TypeBuffer,
    /// The kind marker of write types — a `Write`'s type is
    /// `[element_type, [TypeWrite, Type]]`.
    TypeWrite,
    /// A built graph, as its registry slot.
    ///
    /// **A `usize` and nothing else.** A graph is kernel ids, edge numbers and
    /// counts, all plain data with no block, no arena and no device lifetime in
    /// them, so a graph value needs no tracing, states no host obligation and
    /// costs no copy to move. What the registry holds is the *program's* data,
    /// not the graph's, and nothing in the graph points at it.
    ///
    /// Not a handle, for the same reason a [`Self::ParKernel`] is not: the copy
    /// path relocates arena payloads and this is a slot number.
    ///
    /// **The backend rides here rather than in the registry entry**, exactly as it
    /// rides on a [`Self::ParKernel`]: the graph's *shape* is content-addressed, so
    /// two programs that record the same chain share one id, and an entry that also
    /// stored which device to run it on would answer for whichever built it first.
    /// Reading it off the value is what lets the registry store no answer.
    Graph(GraphId, Backend),
    /// A placeholder for a graph's own `slot`-th input.
    ///
    /// **The slot is the number**, so `ins(i)` binds to the `i`-th argument and
    /// no ordering has to be guessed. Inert: it is a `usize` and no operation
    /// turns one into device memory, so a value holding it cannot read a buffer
    /// — which is what makes a closure that captured one harmless rather than a
    /// capture that reaches inside the graph.
    ///
    /// **Invisible to the checker, and that is what makes it usable here.** The
    /// type at a `cfg` position is fixed by `check_unify` at compile time;
    /// nothing re-derives a type from a runtime value, so a new variant is not a
    /// type error anywhere.
    GraphInput(usize),
    /// A placeholder for a value this recording has already produced.
    ///
    /// **Carrying the value number is the whole design.** The placeholder sits in
    /// the node the next dispatch reads, so the edge follows the operand without a
    /// side table saying which node produced which value — a second copy of a
    /// fact that could disagree with the first.
    GraphValue(GraphValueId),
}

/// A run's results as the language holds them while they are still on a device.
///
/// The id alone would not be enough to use: fetching needs to know how many
/// elements to ask for, and the device allocation is padded up to a whole
/// workgroup, so the allocation's size is not the answer. The count travels with
/// the value rather than in a side table so that an id cannot outlive the length
/// it was issued with, and so two backends' ids can never be confused for one
/// another's by a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentBuffer {
    /// The backend's own name for the buffer. Meaningless outside its issuer,
    /// which is why it is not comparable and not persistent.
    pub id: ResidentId,
    /// How many elements the run produced — the count, not the padded length.
    pub count: usize,
}

/// What one parallel run produced: host data, or results left on the device.
///
/// A `"cpu"` run always produces the first and a `"gpu"` run the second.  The
/// distinction is the whole point: a run that produced host data would have paid
/// a download whether or not anyone ever looked at the result.
#[derive(Debug, PartialEq, Eq)]
enum RunOutcome {
    Host(Vec<Vec<i64>>),
    Resident(Vec<ResidentBuffer>),
}

/// One input to a run, as the language holds it.
#[derive(Debug)]
enum RunInput {
    /// Data the host already has.
    Host(Vec<i64>),
    /// Results a previous run left on the device.
    Resident(ResidentBuffer),
}

/// The compute leaf's payload contract: only [`ComputeValue::Buffer`] carries
/// arena data, and this is what makes the crate's copy path relocate it.
///
/// The lowlevel routes a **program-specific** value to `copy_ext`, which
/// consults `is_handle` on the composed value; a leaf that owns a payload and
/// does not answer `true` here would be copied by reference, leaving a handle
/// pointing into a block that may be released.  `Kernel`/`ParKernel` name
/// process-registry *code*, not arena data, and the markers carry none.
impl ValueExt for ComputeValue {
    fn is_handle(&self) -> bool {
        matches!(self, ComputeValue::Buffer(_))
    }

    /// The buffer's payload viewed as bytes — `i64` elements, so eight bytes
    /// each, which is how the crate's copy path and the codec move it.
    fn handle(&self) -> AnyHandle<[u8]> {
        match self {
            ComputeValue::Buffer(AnyHandle::Dynamic(handle)) => {
                // SAFETY: `handle` points at a live `[i64]` payload (the value's
                // handle), and re-viewing those bytes as `u8` neither moves nor
                // invalidates anything.
                AnyHandle::Dynamic(unsafe {
                    Handle::from_raw(std::ptr::slice_from_raw_parts(
                        handle.as_ptr() as *const u8,
                        buffer_byte_len(handle),
                    ))
                })
            }
            ComputeValue::Buffer(AnyHandle::Static(handle)) => {
                // SAFETY: as above, for a payload in a static module's arena.
                AnyHandle::Static(unsafe {
                    StaticHandle::from_raw(
                        handle.module,
                        std::ptr::slice_from_raw_parts(
                            handle.as_ptr() as *const u8,
                            static_buffer_byte_len(handle),
                        ),
                    )
                })
            }
            _ => unreachable!("only Buffer carries a payload"),
        }
    }

    fn set_handle(&mut self, payload: AnyHandle<[u8]>) {
        match self {
            ComputeValue::Buffer(slot) => {
                *slot = match payload {
                    // The payload is the same allocation, re-viewed as `i64`
                    // elements: the copy path allocated `len` bytes for this
                    // value's payload, so the element count is that over eight.
                    AnyHandle::Dynamic(handle) => {
                        let elements = handle.as_ptr().len() / std::mem::size_of::<i64>();
                        AnyHandle::Dynamic(unsafe {
                            Handle::from_raw(std::ptr::slice_from_raw_parts(
                                handle.as_ptr() as *const i64,
                                elements,
                            ))
                        })
                    }
                    AnyHandle::Static(handle) => {
                        let elements = handle.as_ptr().len() / std::mem::size_of::<i64>();
                        AnyHandle::Static(unsafe {
                            StaticHandle::from_raw(
                                handle.module,
                                std::ptr::slice_from_raw_parts(
                                    handle.as_ptr() as *const i64,
                                    elements,
                                ),
                            )
                        })
                    }
                };
            }
            _ => unreachable!("only Buffer carries a payload"),
        }
    }

    /// `i64` elements need 8-byte alignment.  The composition takes the
    /// strictest alignment over its leaves, so this raises the vocabulary's
    /// payload alignment and the freeze layout follows it.
    fn alignment() -> usize {
        std::mem::align_of::<i64>()
    }

    /// A value that left its results **on the device** owns device memory, and a
    /// freeze of it takes the obligation to give that memory back.
    ///
    /// The id alone cannot release: it names a buffer in the backend that issued
    /// it.  That backend is the **installed** one — the same assumption `collect`
    /// and `read` already make when they fetch by id — so the obligation holds the
    /// `Arc` the lookup hands back, which also keeps the issuer alive for as long
    /// as the obligation does.  A backend that is not installed has nothing to
    /// release: its memory went with it.
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

/// One device buffer's release, owed by the artifact that froze the value naming
/// it (see [`ValueExt::release_obligations`]).
struct ReleaseResident {
    backend: std::sync::Arc<dyn lichen_kernel_ir::ParallelBackend>,
    id: ResidentId,
}

impl Release for ReleaseResident {
    fn release(self: Box<Self>) {
        self.backend.release(self.id);
    }
}

/// The byte length of a dynamic `[i64]` payload, read from the fat pointer's
/// metadata without forming a reference.
fn buffer_byte_len(handle: &Handle<[i64]>) -> usize {
    handle.as_ptr().len() * std::mem::size_of::<i64>()
}

/// The byte length of a static `[i64]` payload.
fn static_buffer_byte_len(handle: &StaticHandle<[i64]>) -> usize {
    handle.as_ptr().len() * std::mem::size_of::<i64>()
}

/// The compute operator vocabulary — the `Jit`/`Launch` operations dispatched
/// by the VM through [`OperatorExt::run`].  Composed into a host's operator
/// union (see a host `program` module).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComputeOperator {
    /// Compile a function value to wasm bytecode → a `Kernel` value.
    Jit,
    /// `[kernel, arg]` operand — run the kernel on the arg → the result.
    Launch,
    /// `[kernel, arg]` operand — a cross-kernel call: run kernel `k` (a
    /// `.native` extracted from a kernel struct) on `arg` → the result.
    Call,
    /// Compile a `?cfg -> ?write` index function to a parallel kernel
    /// (the kernel body is lowered over the loop index; a tuple codomain of
    /// `Write`s is the multi-output form) → a `ParKernel` value.
    Parallel,
    /// `[parallel_kernel, cfg]` operand — run the parallel kernel over the
    /// index range `[0, cfg(0))` (the count is fixed at cfg position 0) → one
    /// output buffer per output the kernel declares, as a bare `Buffer` value
    /// for a single output and the **tuple** of them for several.
    ParLaunch,
    /// `[n]` operand — the loop index of the current parallel invocation,
    /// `i ∈ [0, n)`.  Kernel-only; the VM sees `Parameterized`.
    Range,
    /// `[buffer, index]` operand — read one buffer element → `?b`.  Inside a
    /// kernel this lowers to a host `read` import; at the VM it reads a
    /// `Buffer` value's element (the post-`plrun` read).
    Read,
    /// `[length, index, value]` operand — a pending parallel write.  Kernel-only
    /// (lowers to a host `write` import); the VM sees `Parameterized`.
    Write,
    /// `[buffer]` operand — collect the whole buffer into a lichen array `[?b]`.
    BufferCollect,
    /// `[function]` operand — **record** the dispatches that function performs,
    /// rather than run them, and hand back a `Graph`.
    ///
    /// The operand is deliberately **not** evaluated before this operator runs:
    /// the function's body is what has to be walked, and a deep pass has already
    /// erased the shape that says which of its nodes are the body's own. So this
    /// is the one operator that overrides
    /// [`OperatorExt::run_deferred`] and builds its own `Apply`.
    Graph,
    /// `[graph, arguments]` operand — run a graph over the values its source
    /// function took, and hand back what that function returned.
    GraphRun,
}

// --- the compute leaves' per-leaf artifact codec ----------------------------
//
// A kernel/par-kernel/buffer value and every compute operator are **runtime
// only**: they are process-local registry handles/operations with no stable
// on-disk identity, so a persistent artifact must never carry them (a frozen
// module is a *type* artifact, not a runnable kernel).  `TypeBuffer` is a pure
// type-constant marker and is serializable like the other kind markers.
//
// These arms **refuse** rather than panic, and the difference is reachable, not
// cosmetic: a package that `$jit`s at its top level and is then imported holds a
// live kernel in a module that the importer must freeze, so the shape is
// ordinary user code.  The program is valid — the same `$jit` in a single file
// runs — so the refusal is of the *cache*, not of the compile: the caller leaves
// the package uncached and the program still runs.

impl ValueCodec for ComputeValue {
    fn write_value<P: Program>(
        w: &mut Writer,
        value: Self,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<(), String> {
        match value {
            ComputeValue::TypeBuffer => w.u8(0),
            ComputeValue::TypeWrite => w.u8(1),
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
            ComputeValue::Buffer(_) => {
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
            // **The two placeholders are refused here rather than encoded**, and
            // the reason is sharper than "runtime only": a placeholder's whole
            // meaning is a position in *this* recording's value table, so a number
            // on disk would not name the same value in a rebuilt package. Writing
            // one as a plain integer would be a graph that loads and computes the
            // wrong thing.
            ComputeValue::GraphInput(_) | ComputeValue::GraphValue(_) => {
                return Err(
                    "this package is not cached: it holds a graph's own placeholder at its top \
                     level, and a placeholder means nothing outside the recording that made it — \
                     the number is a position in that recording's value table and nothing else"
                        .into(),
                );
            }
        }
        Ok(())
    }

    fn read_value<P: Program>(
        r: &mut Reader<'_>,
        _self_key: ModuleKey,
        _self_arena: &[u8],
        _self_base: *const u8,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) -> Result<Self, String> {
        Ok(match r.u8()? {
            0 => ComputeValue::TypeBuffer,
            1 => ComputeValue::TypeWrite,
            tag => return Err(format!("unknown compute-value tag {tag}")),
        })
    }
}

impl OperatorCodec for ComputeOperator {
    fn write_operator(_w: &mut Writer, op: Self) -> Result<(), String> {
        match op {
            // Every compute operator is an operation the VM performs, not a
            // value the artifact describes: a frozen module holds the *types* a
            // package exports, and no exported type is a kernel call.  Refused
            // for the same reason a kernel value is (see `ValueCodec` above).
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
    /// The compute leaf's applicability policy: a **kernel** apply is the one
    /// lowlevel `Apply` this vocabulary can lower, as a cross-kernel call, so
    /// it stays lazy instead of being refused.  Both halves of the answer are
    /// the JIT's own predicate: [`kernel_id_of`] for a kernel whose value is
    /// decided — the same call `emit_cross_kernel_call` needs to emit — and
    /// [`pending_kernel`] for the struct pair whose value slot the lowlevel
    /// consults before the deep pass has evaluated it.  A static node is never
    /// a kernel: a kernel artifact is process-local, so a frozen module
    /// carries none.
    fn is_callable(module: &Module<P>, callee: AnyNodeId) -> bool {
        match callee {
            AnyNodeId::Dynamic(node) => {
                kernel_id_of(module, node).is_some() || pending_kernel(module, node)
            }
            AnyNodeId::Static(_) => false,
        }
    }

    fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> P::Value {
        // **A recorded body is a sequence of dispatches, and this is where
        // anything else is stopped.** A `plrun` is the one operator that can
        // consume a graph's placeholders, because a graph's values are edges into
        // a run; every other operator handed one is asking for something a graph
        // has no way to be. Checked here, once, rather than in each arm, because
        // the arms all fail the same way — a bare `Parameterized` with no
        // diagnostic — and a boundary that is drawn in four places is not a
        // boundary.
        //
        // The reason is computed on an immutable borrow and recorded after it
        // ends, so the walk's own module is not borrowed across the diagnostic.
        if graph::is_recording()
            && let Some(reason) = unrecordable(self, module, &operand)
        {
            module.record_extension_diagnostic(GRAPH_DIAGNOSTIC, None, reason);
            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
        }
        match self {
            ComputeOperator::Jit => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Function(function)) = AsEnum::<LowValue>::as_enum(&operand)
                else {
                    // A non-function jit target is a *reported* type error (the
                    // checker's function-ness gate), not an invariant violation —
                    // stay lazy rather than panicking.
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                match compile_fragment(module, function) {
                    Ok(fragment) => {
                        // Content-addressed, so recompiling the same function —
                        // which is what a keystroke does — keeps one id and
                        // lets the derived-module cache hit (`D15`).
                        let id = intern_kernel(fragment);
                        <P::Value as From<ComputeValue>>::from(ComputeValue::Kernel(id))
                    }
                    Err(err) => {
                        // The body is outside the kernel-safe subset, or the
                        // parameter's domain is undecided.  Either way the
                        // honest result is a lazy value plus a recorded reason:
                        // the definition pass reports the unbound result, and
                        // this says *why* — which is the difference between a
                        // user who can fix the program and one who cannot.
                        module.record_extension_diagnostic(JIT_DIAGNOSTIC, None, err);
                        <P::Value as From<LowValue>>::from(LowValue::Parameterized)
                    }
                }
            }
            ComputeOperator::Launch => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("Launch expects an operand array of [kernel, arg]")
                };
                // SAFETY: `operands` is the operand array the VM just evaluated
                // for this operation; its home block is alive for the duration
                // of the run.
                let operands = unsafe { operands.items() };
                let Some(ComputeValue::Kernel(id)) = module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                // The argument is a scalar `USize` for an arity-1 kernel, or
                // an `Array` (possibly nested for a tuple-of-tuples domain)
                // for a tuple-domain kernel.  Flatten it to the wasm argument
                // vector.  Anything else (a non-literal element, e.g. a
                // computed scalar) stays lazy — the definition pass reports
                // the unbound result — and each way that can happen records the
                // cause it is, because the lazy marker alone tells the user
                // nothing about the argument they wrote.
                let mut args: Vec<i64> = Vec::new();
                match module
                    .node_value(operands[1].node)
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                {
                    Some(LowValue::USize(n)) => args.push(n as i64),
                    Some(LowValue::Array(_)) => {
                        if let Err(reason) = collect_args(module, operands[1].node, &mut args, "") {
                            module.record_extension_diagnostic(
                                KERNEL_LAUNCH_DIAGNOSTIC,
                                None,
                                reason,
                            );
                            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                        }
                    }
                    argument => {
                        module.record_extension_diagnostic(
                            KERNEL_LAUNCH_DIAGNOSTIC,
                            None,
                            format!(
                                "the argument must be a concrete Int or a tuple of them (the kernel's \
                                 parameter domain), but this one is {}",
                                argument_kind(argument.as_ref())
                            ),
                        );
                        return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                    }
                };
                match run_kernel(id, &args) {
                    Ok(results) => kernel_results_value(module, block, results),
                    Err(err) => {
                        // Whatever the wasm run said — the assembly, the `main`
                        // export, or the call itself — is this refusal's own
                        // cause, so it is recorded as it stands rather than
                        // replaced by a summary that would name none of them.
                        module.record_extension_diagnostic(KERNEL_LAUNCH_DIAGNOSTIC, None, err);
                        <P::Value as From<LowValue>>::from(LowValue::Parameterized)
                    }
                }
            }
            ComputeOperator::Call => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("Call expects an operand array of [kernel, arg]")
                };
                // SAFETY: `operands` is the operand array the VM just evaluated
                // for this operation; its home block is alive for the duration
                // of the run.
                let operands = unsafe { operands.items() };
                let Some(ComputeValue::Kernel(id)) = module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                let mut args: Vec<i64> = Vec::new();
                match module
                    .node_value(operands[1].node)
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                {
                    Some(LowValue::USize(n)) => args.push(n as i64),
                    Some(LowValue::Array(_)) => {
                        if let Err(reason) = collect_args(module, operands[1].node, &mut args, "") {
                            module.record_extension_diagnostic(
                                KERNEL_LAUNCH_DIAGNOSTIC,
                                None,
                                reason,
                            );
                            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                        }
                    }
                    argument => {
                        module.record_extension_diagnostic(
                            KERNEL_LAUNCH_DIAGNOSTIC,
                            None,
                            format!(
                                "the argument must be a concrete Int or a tuple of them (the kernel's \
                                 parameter domain), but this one is {}",
                                argument_kind(argument.as_ref())
                            ),
                        );
                        return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                    }
                };
                match run_kernel(id, &args) {
                    Ok(results) => kernel_results_value(module, block, results),
                    Err(err) => {
                        // As in `Launch`: the run's own message is this
                        // refusal's cause, so it is recorded as it stands.
                        module.record_extension_diagnostic(KERNEL_LAUNCH_DIAGNOSTIC, None, err);
                        <P::Value as From<LowValue>>::from(LowValue::Parameterized)
                    }
                }
            }
            ComputeOperator::Parallel => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
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
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                let Some(LowValue::Function(function)) = module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                else {
                    // A non-function parallel target is the checker's
                    // function-ness gate; stay lazy rather than panicking.
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                match compile_parallel_fragment(module, function) {
                    Ok(fragment) => {
                        // Content-addressed like `jit`'s, and for the same
                        // reason: the parallel launch path keys the module cache
                        // on `(LaunchMode::Parallel, KernelId)`, so a fresh id
                        // per compile is a re-assembly per compile.  The backend
                        // is *not* part of the fragment, so the same body
                        // compiled for either backend shares this one id.
                        let id = intern_kernel(fragment);
                        <P::Value as From<ComputeValue>>::from(ComputeValue::ParKernel(id, backend))
                    }
                    Err(err) => {
                        module.record_extension_diagnostic(PARALLEL_DIAGNOSTIC, None, err);
                        <P::Value as From<LowValue>>::from(LowValue::Parameterized)
                    }
                }
            }
            ComputeOperator::ParLaunch => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                // **A recording intercepts here, before anything is parsed.** A
                // recorded dispatch has no buffers to look at and no count to
                // read: its arguments are placeholders, which is the whole reason
                // the body can be walked at all. So the interception is first and
                // the real launch is the rest of the arm.
                if graph::is_recording() {
                    return record_launch::<P>(module, block, operand);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("ParLaunch expects an operand array of [kernel, cfg]")
                };
                // SAFETY: `operands` is the operand array the VM just evaluated
                // for this operation; the note covers this arm's `items()`
                // calls, all of live nodes of `module`.
                let operands = unsafe { operands.items() };
                let Some(ComputeValue::ParKernel(id, backend)) = module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                // The cfg value `(n, (buffer…))`.  Element 0 is the count `n`;
                // element 1 is a tuple of input `Buffer` values.
                let Ok(cfg_node) = dyn_node(operands[1].node) else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                // SAFETY: as above — `cfg_node` names a live node of `module`.
                let Some(cfg_items) = (unsafe { module.array_items(cfg_node) }) else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                // count = cfg(0), an `Int`/`USize`.
                let count = match cfg_items
                    .first()
                    .and_then(|item| module.node_value(item.node))
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                {
                    Some(LowValue::USize(n)) => n,
                    _ => return <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                };
                // input buffers = cfg(1), a tuple of `Buffer` values.
                let mut inputs: Vec<RunInput> = Vec::new();
                if let Some(buf_tuple) = cfg_items.get(1)
                    && let Ok(buf_tuple_node) = dyn_node(buf_tuple.node)
                    // SAFETY: `buf_tuple_node` names a live node of `module`.
                    && let Some(buf_items) = (unsafe { module.array_items(buf_tuple_node) })
                {
                    for (position, item) in buf_items.iter().enumerate() {
                        match module
                            .node_value(item.node)
                            .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                        {
                            Some(ComputeValue::Buffer(payload)) => {
                                // SAFETY: the buffer value is read out of
                                // `module` on this borrow, so the payload's home
                                // block is alive for the walk below.
                                if let Some(data) = buffer_items(&payload) {
                                    inputs.push(RunInput::Host(data.to_vec()));
                                } else {
                                    return <P::Value as From<LowValue>>::from(
                                        LowValue::Parameterized,
                                    );
                                }
                            }
                            Some(ComputeValue::DeviceBuffer(resident)) => {
                                // Handed to the run as the id it already is.  This
                                // is the whole point: an intermediate result of a
                                // "gpu" chain never comes home in order to be sent
                                // straight back out.
                                inputs.push(RunInput::Resident(resident));
                            }
                            // **A decided non-buffer here is the one way a launch
                            // can be handed something it cannot run on**, and it
                            // used to answer `parameterized` with no diagnostic —
                            // so a program that passed a plain array where a
                            // buffer belonged ran to completion, printed
                            // `parameterized` and computed nothing.
                            _ => {
                                return not_a_buffer::<P>(
                                    module,
                                    item.node,
                                    "a parallel launch's `cfg(1)` is the tuple of buffers its \
                                     kernel reads",
                                    &format!("position {position} of it"),
                                );
                            }
                        }
                    }
                }
                match run_parallel_kernel(id, backend, count, inputs) {
                    Ok(RunOutcome::Host(results)) => {
                        // Several outputs are the **tuple** of them, which
                        // `compute.read`/`compute.collect` address by ordinal.
                        // Each buffer value becomes a node of this block first,
                        // the way a collected element does, so the tuple holds
                        // live nodes rather than detached values.
                        let buffer = |payload: BufferPayload| {
                            <P::Value as From<ComputeValue>>::from(ComputeValue::Buffer(payload))
                        };
                        if results.len() != 1 {
                            let items: Vec<ArrayItem> = results
                                .iter()
                                .map(|result| {
                                    let node = module.add_node(
                                        block,
                                        None,
                                        Some(buffer(module.alloc_payload(result, block))),
                                    );
                                    ArrayItem::new(AnyNodeId::Dynamic(node))
                                })
                                .collect();
                            let handle = module.alloc_array(&items, block);
                            return <P::Value as From<LowValue>>::from(LowValue::Array(handle));
                        }
                        // A single output is a bare `Buffer` — the single-output
                        // form, exactly what it was.  Each payload lands in the
                        // arena, so the buffer is owned by this block and dies
                        // with it (`D15`) — the same bump allocation every other
                        // payload uses.
                        buffer(module.alloc_payload(&results[0], block))
                    }
                    Ok(RunOutcome::Resident(results)) => {
                        // The same single-or-tuple shape, with the results left
                        // where the shader wrote them.  A resident buffer is plain
                        // data rather than an arena pointer, so a node holding one
                        // needs no payload and the copy path leaves it alone.
                        let value = |resident: ResidentBuffer| {
                            <P::Value as From<ComputeValue>>::from(ComputeValue::DeviceBuffer(
                                resident,
                            ))
                        };
                        if results.len() != 1 {
                            let items: Vec<ArrayItem> = results
                                .iter()
                                .map(|resident| {
                                    let node = module.add_node(block, None, Some(value(*resident)));
                                    ArrayItem::new(AnyNodeId::Dynamic(node))
                                })
                                .collect();
                            let handle = module.alloc_array(&items, block);
                            return <P::Value as From<LowValue>>::from(LowValue::Array(handle));
                        }
                        value(results[0])
                    }
                    Err(err) => {
                        // The refusal is the reason this launch produced no
                        // value, so it is recorded rather than discarded: the
                        // lazy marker alone would tell the user nothing about
                        // why they got `parameterized`.  The general channel
                        // owns it (see `P1-30`), because `BudgetExhausted` is
                        // the *non-termination* verdict and every one of its
                        // renderings says "never terminates" — false here, the
                        // program terminated and merely asked for too much.
                        module.record_extension_diagnostic(PARALLEL_DIAGNOSTIC, None, err);
                        <P::Value as From<LowValue>>::from(LowValue::Parameterized)
                    }
                }
            }
            ComputeOperator::Read => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("Read expects an operand array of [buffer, index]")
                };
                // SAFETY: `operands` is the operand array the VM just evaluated
                // for this operation; its home block is alive for the duration
                // of the run.
                let operands = unsafe { operands.items() };
                let index = match module
                    .node_value(operands[1].node)
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                {
                    Some(LowValue::USize(n)) => n,
                    _ => return <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                };
                match module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                {
                    Some(ComputeValue::Buffer(payload)) => {
                        // SAFETY: the buffer value was just read out of `module`, so its
                        // payload's home block is alive for this read.
                        match buffer_items(&payload).and_then(|items| items.get(index)) {
                            Some(&value) => {
                                <P::Value as From<LowValue>>::from(LowValue::USize(value as usize))
                            }
                            None => <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                        }
                    }
                    Some(ComputeValue::DeviceBuffer(resident)) => {
                        // This is where a resident buffer stops being on the
                        // device: a read asks for one number, so it is the point
                        // at which the program has said it wants host data.
                        if index >= resident.count {
                            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                        }
                        // The fetch brings back everything *up to* the index,
                        // not one element, because the trait hands over owned
                        // data rather than a view into a mapping.  Naming that
                        // cost is better than a `fetch_range` nothing else needs
                        // yet — and a read near the end of a large buffer is
                        // therefore a whole-buffer transfer.
                        match fetch_resident(
                            ResidentBuffer {
                                id: resident.id,
                                count: index + 1,
                            },
                            0,
                        ) {
                            Ok(data) => <P::Value as From<LowValue>>::from(LowValue::USize(
                                data[index] as usize,
                            )),
                            Err(err) => {
                                module.record_extension_diagnostic(PARALLEL_DIAGNOSTIC, None, err);
                                <P::Value as From<LowValue>>::from(LowValue::Parameterized)
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
                }
            }
            ComputeOperator::Range | ComputeOperator::Write => {
                // Kernel-only operators: `range`/`write` are lowered by the
                // parallel JIT to the index/write host imports and never reach
                // the VM as a standalone apply.  Stay lazy.
                <P::Value as From<LowValue>>::from(LowValue::Parameterized)
            }
            ComputeOperator::BufferCollect => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("BufferCollect expects an operand array of [buffer]")
                };
                // SAFETY: `operands` is the operand array the VM just evaluated
                // for this operation; its home block is alive for the duration
                // of the run.
                let operands = unsafe { operands.items() };
                // A resident buffer is fetched here, in full: `collect` is the
                // operation that says "give me these as host values", so this is
                // the one point at which a `"gpu"` chain's results cross the bus.
                let results: Vec<i64> = match module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                {
                    Some(ComputeValue::Buffer(payload)) => {
                        // SAFETY: the buffer value was just read out of `module`, so its
                        // payload's home block is alive while the elements are
                        // materialized below.
                        let Some(items) = buffer_items(&payload) else {
                            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                        };
                        items.to_vec()
                    }
                    Some(ComputeValue::DeviceBuffer(resident)) => {
                        match fetch_resident(resident, 0) {
                            Ok(data) => data,
                            Err(err) => {
                                module.record_extension_diagnostic(PARALLEL_DIAGNOSTIC, None, err);
                                return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
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
                // Materialize each element as a fresh scalar node and build a
                // real lichen array value over them, so `collect` yields an
                // ordinary array the user can index/treat as `array<Int, n>`.
                let items: Vec<ArrayItem> = results
                    .iter()
                    .map(|value| {
                        let node = module.add_node(
                            block,
                            None,
                            Some(<P::Value as From<LowValue>>::from(LowValue::USize(
                                *value as usize,
                            ))),
                        );
                        ArrayItem::new(AnyNodeId::Dynamic(node))
                    })
                    .collect();
                let handle = module.alloc_array(&items, block);
                <P::Value as From<LowValue>>::from(LowValue::Array(handle))
            }
            ComputeOperator::Graph => {
                // The operand has already been evaluated by the time `run` sees
                // it, so the only way to reach the function is the deep value.
                // That is enough here: a graph is a bare function, and the shape
                // that `run_deferred` needed to preserve is the shape of the
                // *body*, which the apply below walks itself.
                //
                // **A missing operand stays lazy rather than panicking**, and the
                // other arms' `unreachable!` does not apply: a deep pass over an
                // operand array holding a deferred function can come back holding
                // something other than an array, and a panic in a recording would
                // take down a program that has a perfectly good answer — that its
                // function is not decided yet.
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                // SAFETY: `operands` is the operand array the VM just evaluated
                // for this operation; its home block is alive for the run.
                let operands = unsafe { operands.items() };
                let Some(function) = operands.first().map(|item| item.node) else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                build_graph::<P>(module, block, function)
            }
            ComputeOperator::GraphRun => {
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                // SAFETY: as above — a live node of `module`.
                let operands = unsafe { operands.items() };
                let Some(graph_node) = operands.first().map(|item| item.node) else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                let arguments = operands.get(1).map(|item| item.node);
                run_graph::<P>(module, block, graph_node, arguments)
            }
        }
    }

    /// The compute operators' low-type transfer: what a kernel's own operators
    /// state they produce.
    ///
    /// The launch operators are the load-bearing ones — a cross-kernel call in
    /// a pre-apply template is exactly the position where no argument exists
    /// yet, so a `Launch`/`Call` that declines leaves the whole surrounding
    /// expression undecided.  They are honest scalars: `run` yields a `USize`
    /// or stays lazy, and the arithmetic they wrap is Int-only.
    ///
    /// `Jit`/`Parallel` produce a host-owned artifact (a kernel id), and
    /// `Write` is a side effect with no value at all, so all three decline.
    /// So does `BufferCollect`: it does produce an array of scalars, but of a
    /// length no low type can name — and a length nobody has decided is
    /// `Unknown`, not a guess.
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
            // `Graph` is a host-owned artifact (a registry slot) and `GraphRun`'s
            // result is a value of a length the graph's own return decides, so
            // both decline — the same two reasons `Jit` and `BufferCollect` do,
            // and for the same reason they do rather than by symmetry.
            | ComputeOperator::Graph
            | ComputeOperator::GraphRun
            | ComputeOperator::BufferCollect => None,
        }
    }
}

// --- Codegen: lichen graph → a scalar `(i64) -> i64` wasm function body -----

/// One parameter group of a kernel being emitted.
///
/// A scalar `jit` kernel has one slot (its single parameter).  A **parallel**
/// kernel (`?a -> USize -> ?b` flattened to `(?a, USize) -> ?b`) has two: the
/// config group (the domain `?a`, a scalar or tuple of scalars) followed by
/// the index slot (the last scalar `USize`).  Each slot carries the
/// `[value, type]` parameter pair, the parameter's value node (where the shape
/// marker is stored), its flattened domain shape, and the wasm local base
/// offset it starts at (the sum of the earlier slots' arities).
#[derive(Clone)]
struct ParamSlot {
    /// The `[value, type]` parameter pair node.
    pair: NodeId,
    /// The parameter's value node (`Index(pair, 0)`), where the shape marker is.
    value: NodeId,
    /// The flattened domain shape this slot reads as.
    shape: LowShape,
    /// The wasm local base offset — `0` for the first slot, the running sum of
    /// the earlier slots' [`flat_arity`] for a later one.
    base: usize,
}

/// The wasm local offset of `node`, if it is a parameter read of one of
/// `params` — the slot's base plus the flattened index path within the slot's
/// domain.  `None` when `node` is not a parameter read of any slot.
fn param_read_offset<P>(module: &Module<P>, params: &[ParamSlot], node: NodeId) -> Option<u32>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    for slot in params {
        if let Some(path) = param_path(module, slot.pair, node)
            && let Ok(offset) = flatten_offset(&slot.shape, &path)
        {
            return Some((slot.base + offset) as u32);
        }
    }
    None
}

/// Lower `[param_pair] → function.return` for the kernel-safe subset (scalar
/// arith over a scalar or a tuple of scalars) into a [`KernelFragment`] — the
/// function's **body**, not a module.  `jit` emits a fragment per function;
/// module assembly is a separate step ([`assemble_module`]), so a later JIT
/// can emit fragments lazily and a `launch` assembles a reachable set of them
/// into one module.
///
/// The domain is **seeded, passed, then read back** — never computed and held
/// in a local:
///
/// 1. **Seed.** The parameter's type slot is the one thing the value graph can
///    never decide here: a template is never evaluated, and an apply binds the
///    clones, so the template's own cell stays empty.  The encoding authority
///    decodes it ([`lichen_highlevel::shape::low_type_of`]) and the lowlevel
///    takes it as a lower bound on the parameter's class.
/// 2. **Pass.** The body's own low types are computed to a fixed point, before
///    any apply — what makes a pre-apply `jit` work at all.
/// 3. **Read.** The domain is then the class's lower bound, exactly as a
///    backend reads any other node's.
///
/// A parameter whose type is undecided at that point is the hard boundary the
/// design names (a polymorphic template's domain is not a fact any mechanism
/// can recover before an apply), so this refuses with an actionable reason
/// rather than compiling a domain it invented.
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
    // The function's `return` is the `[value, type]` pair node; the kernel's
    // result is the pair's *value* (element 0).  A body whose return is a bare
    // value node (the checker leaves a direct kernel-apply's codomain unbound,
    // so it stores the body's value node directly instead of a pair) is used
    // as the value itself.
    // SAFETY: `ret` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let ret_value = match unsafe { module.array_items(ret) } {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => ret,
    };
    // The parameter's two cells, read once and released before the pass needs
    // the module mutably.  `None` for the type cell is not a failure here — it
    // is an undecided domain, and the refusal below says so.
    // SAFETY: `param_pair` is a live node of `module`; nothing in this crate
    // calls `Module::drop_block`.
    let (param_value, param_type) = match unsafe { module.array_items(param_pair) } {
        Some(items) if !items.is_empty() => (
            dyn_node(items[PAIR_VALUE_SLOT].node)?,
            items.get(PAIR_TYPE_SLOT).map(|item| item.node),
        ),
        _ => return Err("parameter is not a [value, type] pair".into()),
    };

    // 1. Seed.  A parameter with no type cell seeds `Unknown`, which is the
    //    honest statement: this class has been traced and nothing has decided
    //    it.
    let seed = param_type.map_or(LowShape::Unknown, |slot| low_type_of_slot(module, slot));
    module.seed_class_low_type(param_value, seed);
    // 2. Pass.
    module.infer_template_low_types(fid);
    // 3. Read.
    let Some(domain) = module.low_type_of_node(param_value) else {
        return Err(UNDECIDED_DOMAIN.into());
    };
    let param_shape = kernel_domain(domain)?;

    let params = vec![ParamSlot {
        pair: param_pair,
        value: param_value,
        shape: param_shape.clone(),
        base: 0,
    }];

    let mut body: Vec<KernelInstr> = Vec::new();
    // A scalar kernel has no buffers in either space: `compute.write` and a
    // buffer `compute.read` are both parallel-only operators, so both counters
    // below stay at 0 and the fragment declares `inputs: 0, outputs: 0`. They
    // are read off the tally rather than written as literals so that a body
    // which ever did reach one of them would be counted instead of mis-declared.
    let mut tally = Positions::default();
    // The body is emitted **leaf by leaf**: a scalar codomain is the one leaf,
    // a tuple codomain one leaf per element, each becoming its own stack slot.
    // The count of what the walk returns is the fragment's result arity, so the
    // wasm signature and the value the launcher reads back are both a function
    // of the body's own value rather than of a hardcoded one.
    let leaves = codomain_leaves(module, ret_value)?;
    for leaf in &leaves {
        emit_node(module, &params, *leaf, &mut body, &mut tally)?;
    }

    Ok(KernelFragment {
        param_shape: kernel_shape(&param_shape),
        body: body.into(),
        inputs: tally.reads,
        outputs: tally.writes,
        results: leaves.len(),
        int_width: IntWidth::I64,
    })
}

/// The body's value resolved into the **leaves** a wasm body must leave on the
/// stack — one stack slot each, in source order.
///
/// This is the codomain counterpart of [`parallel_output_nodes`], and it is the
/// same walk: a bare value node is the single-leaf form (a scalar codomain),
/// and a **materialized tuple value** is the multi-leaf one (a tuple codomain,
/// `p => (p(0), p(1))`), reached through a `value_of` extraction or a
/// `Parameterized` cell exactly as an argument tuple is.
///
/// A *nested* tuple is not flattened into extra leaves here: each leaf is emitted
/// by [`emit_node`], which produces exactly one value, so a nested tuple would
/// leave its elements interleaved on the stack in the wrong order.  It is
/// refused by name instead, with the path to the offending position.
fn codomain_leaves<P>(module: &Module<P>, ret_value: NodeId) -> Result<Vec<NodeId>, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // Peel the `value_of` extraction first: the checker reaches most values
    // through one, so asking the extraction whether it is a tuple would answer
    // for the *pair* it wraps.
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

/// Whether `node` is (or peels to) a materialized tuple value — the
/// multi-element array value a tuple literal is stored as.  Shared by the
/// codomain walk and the cross-kernel argument walk, which must agree on what
/// counts as a tuple.
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

/// Lower a single-arg `?cfg -> ?write` index function into a **parallel
/// kernel** — a [`KernelFragment`] whose wasm signature is `(n, index)` (the
/// count scalar from `cfg(0)`, then the loop index) and whose body is the
/// index function's body traced with `range`/`read`/`write` host calls.
///
/// The codomain is a `Write` or a **tuple of `Write`s** — one element per
/// output buffer — and the fragment records the arity as its
/// [`KernelFragment::outputs`], so a `plrun` allocates exactly that many
/// buffers.
///
/// `parallel` is the data-parallel lift: running it over the index range
/// `[0, cfg(0))` computes the index function once per index.  The `cfg` is
/// `(n, (buffer…))` — `cfg(0)` is the count `n` (a wasm scalar param), and the
/// buffer tuple at `cfg(1)` is host-side (each buffer read via a `read`
/// import by its position inside the tuple).  The loop index comes from
/// `compute.range n` (a kernel-only op yielding the index param).
///
/// **The every-ordinal-written invariant holds by construction.**  The lowered
/// body is straight-line: the only conditional the kernel-safe subset has is
/// the emitter's 2-element scalar `select`, which is a *value* select and can
/// only skip a *value*, and a `compute.write` has none.  A write reached inside
/// a `select` branch is therefore refused by name rather than emitted (see the
/// `Select` arm of [`emit_node`]), so write `k` runs on every index and each
/// ordinal `0 .. outputs` is always written.
fn compile_parallel_fragment<P>(
    module: &mut Module<P>,
    function: AnyFunctionId,
) -> Result<KernelFragment, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let AnyFunctionId::Dynamic(fid) = function else {
        return Err("static (imported) functions are not kernel-compilable v1".into());
    };
    let cfg_pair = module.functions[fid].parameter;
    let body = module.functions[fid].r#return;
    // The kernel's result is the index function's body value — a `Write`, or a
    // **tuple of `Write`s** (one per output buffer) — through a `[value, type]`
    // pair or a bare value node.
    // SAFETY: `body` is a live node of `module`.
    let ret_value = match unsafe { module.array_items(body) } {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => body,
    };
    // The output count is the **codomain's arity**, read here as the body's own
    // value: a bare value is one output, a materialized tuple value is one
    // output per element.  It is a fact of the *function*, so it is fixed
    // before any index runs — the count is never discovered from which slots
    // happened to be written.  (The kernel struct's `.sig` names the same
    // arity as a type; the body's value is the same fact read where the
    // emitter can count it, without the type encoding.)
    let outputs = parallel_output_nodes(module, ret_value);
    // `cfg = (n, (buffer…))`.  `cfg(0)` is the scalar count `n` — the only
    // scalar wasm param from cfg; the buffer tuple is host-side (read via the
    // `read` import by its position in `cfg(1)`).  Model the cfg's scalar part
    // as a `Tuple([USize])` so the emitter maps `cfg(0)` → `local.get 0`.
    //
    // This seed is the *host ABI's*, not a type fact: the parallel signature is
    // `(n, index)` by construction, whatever the lichen type says, so it is
    // stated here rather than decoded.  The pass then runs as usual, and the
    // slot's shape is read back off the class — the same seed → pass → read
    // chain `compile_fragment` uses.
    let cfg_value = pair_value_node(module, cfg_pair)
        .ok_or_else(|| "parallel cfg parameter is not a [value, type] pair".to_string())?;
    module.seed_class_low_type(cfg_value, LowShape::Tuple(vec![LowShape::USize]));
    module.infer_template_low_types(fid);
    let Some(cfg_shape) = module.low_type_of_node(cfg_value) else {
        return Err(UNDECIDED_DOMAIN.into());
    };
    let cfg_shape = kernel_domain(cfg_shape)?;
    let params = vec![ParamSlot {
        pair: cfg_pair,
        value: cfg_value,
        shape: cfg_shape,
        base: 0,
    }];
    let mut body_instr: Vec<KernelInstr> = Vec::new();
    // One `compute.write` per codomain position, in position order, so write `k`
    // is emitted with `out_pos = k`.  A position is *required* to emit exactly
    // one write: a position that emits none is named, and the total is checked
    // afterwards so a write reached nested inside a position's value (which
    // would consume an ordinal of its own) is caught too.  A conditional write
    // is refused by the emitter, which knows the more specific cause.
    let mut tally = Positions::default();
    for (position, output) in outputs.iter().enumerate() {
        let before = tally.writes;
        emit_node(module, &params, *output, &mut body_instr, &mut tally)?;
        if tally.writes == before {
            return Err(format!(
                "output {position} of the parallel index function is not a `compute.write` \
                 (an index function must write every output it declares)"
            ));
        }
    }
    if tally.writes != outputs.len() {
        return Err(format!(
            "a parallel index function emitted {} write(s) but its codomain names {} \
             output(s): every output must be exactly one `compute.write`",
            tally.writes,
            outputs.len()
        ));
    }
    // The index function writes into the output buffers (side effects); leave a
    // dummy scalar on the stack so the shared `assemble_module` signature holds
    // for the write-only kernel.  That dummy is exactly **one** value, which is
    // what this fragment's `results` records — a parallel kernel's result
    // buffers are its outputs, not its wasm results, and the run reads them out
    // of the buffers the `write` import filled.
    body_instr.push(KernelInstr::Const(0));
    Ok(KernelFragment {
        // `(config, index)` however many buffers the body reads: the buffers are
        // bound rather than passed, so this shape is the parallel signature and
        // says nothing about them. `tally.reads` is what says that.
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body: body_instr.into(),
        inputs: tally.reads,
        outputs: tally.writes,
        results: 1,
        int_width: IntWidth::I64,
    })
}

/// Resolve an index function's return value into its per-position output
/// nodes — the codomain's arity, as a list.
///
/// The return value is the codomain's *value*: a bare value is the
/// single-output form (one output) and a materialized tuple value is the
/// multi-output one (one output per element, in position order).  The elements
/// are handed to the emitter **unresolved**: what a position *is* — a
/// `compute.write`, a conditional, anything else — is the emitter's to
/// determine, which is what keeps the specific cause (a write behind a
/// conditional) with the code that can name it.  A *nested* tuple is refused
/// there as a position that is not a write; the outputs are flat, one buffer
/// per position.
fn parallel_output_nodes<P>(module: &Module<P>, ret_value: NodeId) -> Vec<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // A `value_of` extraction (`Index(pair, 0)`) still carries the pair's
    // value, so peel it the way the emitter's `Index` arm does before asking
    // whether what remains is a tuple.
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
            // A static (imported) element is not a kernel body the emitter can
            // lower; keep it as the single output so the position is named by
            // the emission rather than silently dropped here.
            Err(_) => return vec![ret_value],
        }
    }
    nodes
}

/// Assemble an ordered slice of kernel fragments into a single wasm module.
///
/// `ordered[i]` becomes wasm function index `i`; `index` maps each callee
/// [`KernelId`] to its function index, so a cross-kernel `CallKernel` lowers
/// to an in-module `call`.  The root (index 0) is exported as `main`.  For a
/// single-kernel set this is the degenerate link — one fragment = one module;
/// for a kernel that cross-calls others it is the launch-time assembly that
/// pulls the relative kernel set into one module.
#[allow(clippy::needless_range_loop)]
fn assemble_module(
    ordered: &[KernelFragment],
    index: &HashMap<KernelId, u32>,
) -> Result<Vec<u8>, String> {
    use wasm_encoder::{
        CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection,
        ImportSection, Instruction, Module as WasmModule, TypeSection, ValType,
    };

    // A fragment that lowers buffer `read`/`write` calls declares the two host
    // imports (function indices 0 and 1); the defined functions then start at
    // `base` (2).  A pure scalar kernel has no imports (base 0).
    let uses_imports = ordered.iter().any(|f| {
        f.body.instrs().into_iter().any(|i| {
            matches!(
                i,
                KernelInstr::BufferReadCall | KernelInstr::BufferWriteCall
            )
        })
    });
    let base: u32 = if uses_imports { 2 } else { 0 };

    // Type section: the import signatures (if any), then one
    // `(param-arity) -> (result-arity)` signature per distinct **pair** of
    // arities.
    //
    // Keying on the pair is what a multi-value codomain requires: two fragments
    // can share a parameter arity and differ in result arity (`(i64) -> i64`
    // against `(i64) -> (i64, i64)`), and wasm function types are indexed by
    // type, not by arity — so keying on the parameter arity alone would hand the
    // second fragment the first one's single-`i64` signature and the module
    // would not validate.  Keying on the pair also keeps the shared entry for
    // the two runs that *do* agree: the scalar root and the parallel path's
    // write-only kernel, both `(i64…; n) -> i64`.
    let mut types = TypeSection::new();
    let (mut read_ty, mut write_ty) = (0u32, 0u32);
    if uses_imports {
        read_ty = types.len();
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64], vec![ValType::I64]);
        write_ty = types.len();
        types
            .ty()
            .function(vec![ValType::I64, ValType::I64, ValType::I64], vec![]);
    }
    // Keyed by the **whole** signature, not the parameter arity alone: wasm
    // indexes a type by its whole `(params, results)` pair, and `(i64) -> i64`
    // and `(i64) -> (i64, i64)` share a parameter arity, so keying on one half
    // would hand the second fragment the first one's signature and the module
    // would not validate.
    let mut type_index_by_signature: HashMap<(usize, usize), u32> = HashMap::new();
    let mut func_types: Vec<u32> = Vec::with_capacity(ordered.len());
    for frag in ordered {
        let (params, results) = (frag.param_shape.flat_arity(), frag.results);
        let ti = type_index_by_signature
            .entry((params, results))
            .or_insert_with(|| {
                let id = types.len();
                types
                    .ty()
                    .function(vec![ValType::I64; params], vec![ValType::I64; results]);
                id
            });
        func_types.push(*ti);
    }

    let mut wasm = WasmModule::new();
    wasm.section(&types);
    if uses_imports {
        let mut imports = ImportSection::new();
        imports.import("env", "read", EntityType::Function(read_ty));
        imports.import("env", "write", EntityType::Function(write_ty));
        wasm.section(&imports);
    }
    let mut funcs = FunctionSection::new();
    for &ti in &func_types {
        funcs.function(ti);
    }
    wasm.section(&funcs);
    let mut exports = ExportSection::new();
    exports.export("main", ExportKind::Func, base);
    wasm.section(&exports);

    let mut code = CodeSection::new();
    for frag in ordered {
        let mut body = Function::new([]);
        let instrs = straight_line_body(frag)?;
        lower_body(instrs, index, base, &mut body)?;
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    wasm.section(&code);
    Ok(wasm.finish())
}

/// The straight-line instructions of a fragment's body, or a refusal.
///
/// **A body with a transfer is refused, never emitted straight-line.** The
/// transfer is already in the IR, so this is the emitter's limit rather than
/// something the program could not say — and dropping the branch would compile a
/// fragment that computes a different program than it was lowered from. See
/// `docs/notes/loop-conversion.md` §8.
fn straight_line_body(fragment: &KernelFragment) -> Result<&[KernelInstr], String> {
    fragment
        .body
        .validate()
        .map_err(|broken| format!("compute.wasm: the kernel body is malformed: {broken}"))?;
    fragment.body.straight_line_instrs().ok_or_else(|| {
        format!(
            "compute.wasm: the kernel body allocates {} label(s) and has a transfer, and this \
                 emitter does not yet emit one",
            fragment.body.labels
        )
    })
}

/// Lower a sequence of abstract [`KernelInstr`]s into a wasm function body.
/// `index` resolves each cross-kernel `CallKernel` to the callee's in-module
/// function index (assigned by the launch-time assembly), offset by `base`
/// (the number of leading host-import function indices, so a defined function
/// `i` is wasm index `base + i`).  `BufferReadCall`/`BufferWriteCall` lower to
/// the `read` (index 0) and `write` (index 1) imports.
fn lower_body(
    body: &[KernelInstr],
    index: &HashMap<KernelId, u32>,
    base: u32,
    out: &mut wasm_encoder::Function,
) -> Result<(), String> {
    use wasm_encoder::Instruction;

    for instr in body {
        match instr {
            KernelInstr::Const(n) => {
                out.instruction(&Instruction::I64Const(*n));
            }
            KernelInstr::Bin(op) => match op {
                KernelBin::Add => {
                    out.instruction(&Instruction::I64Add);
                }
                KernelBin::Sub => {
                    out.instruction(&Instruction::I64Sub);
                }
                KernelBin::Mul => {
                    out.instruction(&Instruction::I64Mul);
                }
                // An `Int` is unsigned, so these are the unsigned division,
                // remainder and comparisons (`I64DivS` would agree below 2^63
                // and differ above).  The order comparisons yield an `i32`
                // boolean, which is widened to the `0/1` scalar the language
                // has instead of a `Bool` — the same widening `Eq` needs.
                KernelBin::Div => {
                    out.instruction(&Instruction::I64DivU);
                }
                KernelBin::Rem => {
                    out.instruction(&Instruction::I64RemU);
                }
                KernelBin::Lt => {
                    out.instruction(&Instruction::I64LtU);
                    out.instruction(&Instruction::I64ExtendI32U);
                }
                KernelBin::Gt => {
                    out.instruction(&Instruction::I64GtU);
                    out.instruction(&Instruction::I64ExtendI32U);
                }
                KernelBin::Leq => {
                    out.instruction(&Instruction::I64LeU);
                    out.instruction(&Instruction::I64ExtendI32U);
                }
                KernelBin::Geq => {
                    out.instruction(&Instruction::I64GeU);
                    out.instruction(&Instruction::I64ExtendI32U);
                }
                KernelBin::Eq => {
                    out.instruction(&Instruction::I64Eq);
                    out.instruction(&Instruction::I64ExtendI32U);
                }
                KernelBin::Neq => {
                    out.instruction(&Instruction::I64Ne);
                    out.instruction(&Instruction::I64ExtendI32U);
                }
                KernelBin::BitAnd => {
                    out.instruction(&Instruction::I64And);
                }
                KernelBin::BitOr => {
                    out.instruction(&Instruction::I64Or);
                }
                KernelBin::BitXor => {
                    out.instruction(&Instruction::I64Xor);
                }
            },
            KernelInstr::LocalGet(k) => {
                out.instruction(&Instruction::LocalGet(*k));
            }
            KernelInstr::I32WrapI64 => {
                out.instruction(&Instruction::I32WrapI64);
            }
            KernelInstr::Select => {
                out.instruction(&Instruction::Select);
            }
            KernelInstr::CallKernel(kid) => {
                let target = *index.get(kid).ok_or_else(|| {
                    format!("cross-kernel call to kernel {kid} is not in the assembled set")
                })?;
                out.instruction(&Instruction::Call(base + target));
            }
            KernelInstr::BufferReadCall => {
                // The host `read(cfg_pos, idx)` import — function index 0.
                out.instruction(&Instruction::Call(0));
            }
            KernelInstr::BufferWriteCall => {
                // The host `write(out_pos, idx, val)` import — function index 1.
                out.instruction(&Instruction::Call(1));
            }
        }
    }
    Ok(())
}

/// The reason a `jit` reports when the parameter's domain is not decided at
/// compile time.  Wording matters here: the user can only fix this by
/// annotating the parameter, so the message says what to write.
///
/// This is the hard boundary of the low-type design, not a gap in it — lichen
/// binds names per apply, so a polymorphic template's domain is a per-call-site
/// fact by construction and no pre-apply mechanism can recover it.
const UNDECIDED_DOMAIN: &str = "the kernel parameter's type is not decided when the kernel is compiled; \
annotate it (for example `p : <Int, Int>`) so its domain is known";

/// The reason a `jit` reports when the parameter's domain **holds** a float —
/// anywhere the domain walk reaches, not only when the parameter is one.  A
/// float is a shape that **is** decided, so [`UNDECIDED_DOMAIN`] would
/// misdescribe the program and ask for an annotation the author already wrote.
/// A kernel's ABI is `i64` in five places at once (`lichen_kernel_ir`), so
/// there is no float local to lower the position into
/// (`docs/notes/floating-point.md` §3.8).
const FLOAT_DOMAIN: &str = "the kernel parameter's type contains a float, which a kernel cannot take: \
the kernel ABI is `i64`-only, so a float has no local to be lowered into";

/// A position of a domain shape that the kernel ABI cannot lower to a decided
/// `i64` local.
///
/// **The variant order is the report order**, strongest last, so `max` answers
/// with the cause to name: a `Float` outranks an `Undecided` position because
/// it is the one annotating cannot clear
/// (`docs/notes/floating-point.md` §3.8, §5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DomainObstacle {
    /// An `Unknown` leaf: the shape states nothing.
    Undecided,
    /// A `Float` leaf: decided, but the ABI has no local for it (`i64` in five
    /// places, `lichen_kernel_ir`), so an arity read off it is a mis-encoding.
    Float,
}

/// **The one walk over a domain shape**, read by the gate [`domain_is_known`]
/// and by [`kernel_domain`] for the cause it names, so the two cannot disagree
/// about a position.
///
/// It **recurses through every position**, and that is the load-bearing part: a
/// compound shape reached through an accepted `Tuple` is walked too, because
/// asking a compound shape's own `is_known` answers `true` for `Array(Float, _)`
/// — a float is a decided shape — and `kernel_domain` accepts `Tuple(_)`
/// without re-checking elements.  A function's codomain and a table's value
/// position are walked as well, because they are decided independently of their
/// sibling: a declared `Int -> Float` puts a float in exactly that position.
fn domain_obstacle(shape: &LowShape) -> Option<DomainObstacle> {
    match shape {
        LowShape::Unknown => Some(DomainObstacle::Undecided),
        LowShape::Float => Some(DomainObstacle::Float),
        LowShape::USize => None,
        LowShape::Tuple(items) => items.iter().filter_map(domain_obstacle).max(),
        LowShape::Array(element, _) => domain_obstacle(element),
        LowShape::Function(domain, codomain) => {
            domain_obstacle(domain).max(domain_obstacle(codomain))
        }
        LowShape::Table(key, value) => domain_obstacle(key).max(domain_obstacle(value)),
    }
}

/// A kernel domain must be a decided scalar or a tuple of decided scalars —
/// everything else is a refusal, and each refusal names its own cause rather
/// than falling back to a shape that would compile into a wrong signature.
///
/// A float is named **ahead of** the gate below, at any position the walk
/// reaches: the gate answers `false` for a float because the ABI has no local
/// for it, not because the shape is undecided, so a float reported there would
/// be described as an undecided domain.
fn kernel_domain(domain: LowShape) -> Result<LowShape, String> {
    if let Some(DomainObstacle::Float) = domain_obstacle(&domain) {
        return Err(FLOAT_DOMAIN.into());
    }
    if !domain_is_known(&domain) {
        return Err(UNDECIDED_DOMAIN.into());
    }
    match &domain {
        LowShape::USize | LowShape::Tuple(_) => Ok(domain),
        _ => Err("kernel domain must be a scalar or a tuple of scalars".into()),
    }
}

/// The IR's own expression of a kernel domain — the same structure, in the two
/// variants the lowered-kernel IR has, so that a backend reading a fragment
/// needs no dependency on the host shape lattice.
///
/// **Total, on purpose, and it preserves the arity convention.** A domain
/// `kernel_domain` accepted is a scalar or a tuple, but a tuple's *element* is
/// not itself re-checked, so a shape with no IR counterpart can still arrive
/// here; those fold to [`KernelShape::Scalar`], which flattens to one leaf
/// exactly as the `flat_arity` filler arms do for the same shapes.  A separate
/// refusal arm would be a second failure mode for a case the parameter-count
/// path already tolerates, and would change behaviour rather than preserve it.
fn kernel_shape(domain: &LowShape) -> KernelShape {
    match domain {
        // A float is a **decided scalar shape**, so it is answered as one: this
        // is the *shape* question ("one value, or a tuple of them"), not the
        // dispatchability one, and a float domain is one value.  The refusal
        // that keeps it out of an `i64` wasm signature is `kernel_domain`'s, and
        // it has already run: `domain_obstacle` refuses a float at every
        // position a domain can hold, so this arm states the layout a float
        // would have rather than one anything reaches
        // (`docs/notes/floating-point.md` §3.8, §5).
        LowShape::USize | LowShape::Float => KernelShape::Scalar,
        LowShape::Tuple(items) => KernelShape::Tuple(items.iter().map(kernel_shape).collect()),
        LowShape::Unknown
        | LowShape::Array(_, _)
        | LowShape::Function(..)
        | LowShape::Table(..) => KernelShape::Scalar,
    }
}

/// Whether a shape has no position anywhere in it that the kernel ABI cannot
/// lower to a decided `i64` local — the gate [`kernel_domain`] reads before its
/// own shape match, answered from [`domain_obstacle`] so that there is one walk
/// over a shape rather than two copies of it.
///
/// A domain with a single `Unknown` leaf is as undecided as an all-`Unknown`
/// one: the wasm arity comes from flattening, so one unknown leaf is one
/// unknown local.  A **`Float` leaf gets the same answer for a different
/// reason** — it is decided, but the ABI has no local for it.
fn domain_is_known(shape: &LowShape) -> bool {
    domain_obstacle(shape).is_none()
}

/// The number of scalar `i64` locals a domain shape flattens to — the wasm
/// parameter count.  A scalar is one local; a tuple is the sum of its
/// elements' arities (so `((Int,Int), Int)` is `1 + 1 + 1 = 3`).
///
/// An undecided domain is refused before arity is ever computed — the domain
/// check in `compile_fragment` rejects anything that is not a decided scalar
/// or tuple of decided scalars — so the `Unknown` arm is the total-function
/// filler for a shape that cannot reach a wasm signature, and counts one
/// undecided leaf rather than inventing a parameter count.
fn flat_arity(shape: &LowShape) -> usize {
    match shape {
        LowShape::USize => 1,
        // One leaf, and deliberately the *layout* answer rather than a
        // dispatchability one: a float is one value and [`KernelShape::Scalar`]
        // is also one, so this stays consistent with `kernel_shape`, which is
        // what keeps a domain's locals contiguous.  No float reaches here —
        // every caller flattens a `ParamSlot`'s shape or a sub-shape of one,
        // and `kernel_domain`'s gate reads `domain_obstacle`, which walks a
        // compound shape's element too, so a float is refused at every position
        // a domain can hold.  Zero would be a different claim (that a float
        // occupies no local of the signature `KernelShape` sizes as one) and
        // would put the two arities in disagreement.
        LowShape::Float => 1,
        LowShape::Tuple(items) => items.iter().map(flat_arity).sum(),
        LowShape::Array(_, _) | LowShape::Function(..) | LowShape::Table(..) => 1,
        LowShape::Unknown => 1,
    }
}

/// The disjoint-set representative of `node`, walked without path compression
/// (a `&self` read) — used to compare whether two nodes were unified.  The
/// lowlevel's `equality_representative` needs `&mut`; this is the read-only
/// form for the emitter's `&Module`.
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

/// The member of `node`'s equality class that *defines* its value — a class
/// member carrying a computational operator (anything but a `value_of` index
/// extraction, which is a view of a `[value, type]` pair rather than the
/// computation itself).  The deep pass collapses some values to a bare
/// `Parameterized` cell and unifies that cell with the defining computation
/// (a kernel call's result, a `launch` argument); the emitter reaches the
/// computation through the class.  Returns `None` when the class has no such
/// member — the value is genuinely opaque (an uncomputable leaf).
///
/// The walk is the class's own member list, not a scan of the module's whole
/// node table.  The table holds every kernel's nodes while the emitter is
/// compiling one kernel, and this call is made per kernel that reaches a bare
/// cell, so scanning it made codegen quadratic in the number of kernels.
fn class_computation_node<P>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let root = equality_rep(module, node);
    for member in disjoint::members(&module.nodes, root) {
        if let Some(op) = module.node_operation(member).as_ref()
            && !matches!(
                AsEnum::<LowOperator>::as_enum(&op.operator),
                Some(LowOperator::Index)
            )
        {
            return Some(member);
        }
    }
    None
}

/// A `compute.write` reached inside a conditional.  A write is a side effect
/// with no value, so it cannot sit in a `select` branch (which must leave a
/// value on the stack) — and it would be written on only one of the two paths,
/// which is exactly what the every-ordinal-written invariant forbids.  The one
/// construct that could skip a write is named here rather than emitted.
const CONDITIONAL_WRITE: &str = "a `compute.write` inside a conditional is not supported: that \
     output ordinal would not be written on every index";

/// The buffer positions one kernel body addresses, counted as it is emitted.
///
/// **Two counters, one per buffer space, and they are the two halves of one
/// fact.** A body says which of the buffers it was handed it reads, and which of
/// the buffers it was given to fill it writes; those positions are compile-time
/// constants (`parallel_buffer_pos` refuses anything else by name), and the
/// fragment carries the totals as [`KernelFragment::inputs`] and
/// [`KernelFragment::outputs`]. Counting them here rather than letting a caller
/// discover them is what makes the two numbers impossible to disagree with the
/// body: a caller that is handed the wrong number is refused rather than given a
/// shader that reads a binding nobody bound.
#[derive(Debug, Default, Clone, Copy)]
struct Positions {
    /// The `out_pos` the next `compute.write` is given, and the number of
    /// writes emitted so far once the walk returns — the write's position in
    /// the index function's codomain.
    writes: usize,
    /// One past the highest input position any `compute.read` named, so `0` for
    /// a body that reads no buffer. The max rather than a count, because the
    /// read positions are a sparse space: a body reading only `cfg(1)(1)` still
    /// needs two buffers, or the one at position 1 was never bound.
    reads: usize,
}

/// The kernel-safe reading of a highlevel binary operator — the one conversion
/// between the language's operator vocabulary and the lowered-kernel IR's.
/// `None` for an operator no kernel body can contain (`Fresh` mints a nominal
/// struct id), which the caller reports by name rather than approximating.
///
/// **The arithmetic is unsigned, and that is a fact of the language rather than
/// of this backend:** an `Int` is a machine-sized unsigned integer, so `Div`
/// and `Rem` are the unsigned operations and the order comparisons are the
/// unsigned ones.  See [`lichen_kernel_ir::KernelBin`], which states the choice
/// once for every backend.
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
        TypeOperator::Fresh => return None,
    })
}

/// Emit wasm instructions for one lichen graph node — the scalar kernel-safe
/// subset: integer constants, the [`KernelBin`] arithmetic/comparison/bitwise
/// operators, and parameter reads (`Index(param_pair, 0)` → `local.get k`).
/// `params` is the kernel's
/// parameter-slot list (one for a scalar `jit` kernel, two — config then index
/// — for a parallel kernel).
///
/// `tally` is the emitter's [`Positions`] counter, threaded through the walk so
/// that the two buffer spaces' totals come out of the emission that produced the
/// positions rather than out of a separate reading of the body.
fn emit_node<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: NodeId,
    body: &mut Vec<KernelInstr>,
    tally: &mut Positions,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    if let Some(value) = module.node_value(AnyNodeId::Dynamic(node))
        && let Some(LowValue::USize(n)) = AsEnum::<LowValue>::as_enum(&value)
    {
        body.push(KernelInstr::Const(n as i64));
        return Ok(());
    }
    let Some(operation) = module.node_operation(node) else {
        // A bare value cell (no value specialization, no operator).  If it is
        // in one of the enclosing parameters' equality classes, it is a
        // whole-parameter read: the deep pass's apply-clone *unifies* a
        // substituted parameter with the argument, so a reduced same-module
        // call's parameter reference resolves to this kernel's parameter —
        // emit a `local.get` for it instead of failing.
        for slot in params {
            if equality_rep(module, node) == equality_rep(module, slot.value) {
                let offset = flatten_offset(&slot.shape, &[])?;
                body.push(KernelInstr::LocalGet((slot.base + offset) as u32));
                return Ok(());
            }
        }
        // A value collapsed to a bare `Parameterized` cell resolves through its
        // equality class to the computation that defines it — a kernel call's
        // result, or a `launch` argument (whose cell is *expected* to be
        // parameterized: `launch` is two-step, assemble then call, so the
        // argument is only concrete at run time).  Emit the defining member.
        if let Some(definer) = class_computation_node(module, node) {
            return emit_node(module, params, definer, body, tally);
        }
        // **A node nothing can resolve, described rather than numbered.** This used
        // to report only its `NodeId`, which is a compiler-internal number: the
        // reader learns that something is unresolvable and nothing about what.
        //
        // **It does not claim one cause, because this shape has more than one** and
        // naming the wrong one is worse than naming none. Every way this is reached
        // is the same fact underneath: a kernel is compiled from a template
        // **before any apply**, so a binding the body would have filled in at run
        // time is still empty here. A `let` alias whose value comes from a buffer
        // read is one; a helper defined in the body rather than at module level is
        // another; a `compute.call`'s callee wrapper is a third. The message says
        // so, and the two ways to write past it, without claiming which one this is.
        return Err(format!(
            "a kernel body reached a node with neither a value nor an operation, so there is \
             nothing to emit for it (node={node:?}). A kernel is compiled from a template before \
             any apply, so a binding the body would fill in at run time is still empty here — a \
             `let` alias fed by a buffer read, a helper defined in the body rather than at module \
             level, and a `compute.call`'s wrapper all have this shape. Move the binding to module \
             level, or write what it would have computed directly into the expression the kernel \
             uses"
        ));
    };

    let op = &operation.operator;
    // The structural core: dispatched through `AsEnum<LowOperator>` (the
    // lowlevel never falls through to `run` for these).
    if let Some(low) = AsEnum::<LowOperator>::as_enum(op) {
        match low {
            LowOperator::Index => {
                let (target, index) = operand_pair(module, operation.operand)?;
                // A parameter read at some index path → a wasm `local.get`.
                // (This must run before the value_of defuse: `Index(param_pair,
                // 0)` is a node's value slot, not a general extraction.)
                if let Some(offset) = param_read_offset(module, params, node) {
                    body.push(KernelInstr::LocalGet(offset));
                    return Ok(());
                }
                // A `value_of` extraction — `Index(pair, 0)` with a constant
                // `0` index and `pair` a `[value, type]` pair.  Emit the
                // pair's value slot instead of treating the extraction as a
                // real index.
                if usize_value(module, index) == Some(0)
                    && let Some(value_node) = value_of_node(module, node)
                {
                    return emit_node(module, params, value_node, body, tally);
                }
                // A constant index into a concrete array value selects that
                // element — the wrapper's slot-read destructuring
                // (`read [a, b]` → `x(0)/x(1)`, `write [a, b, c]` →
                // `x(0)/x(1)/x(2)`) leaves `Index(arg_array, k)` ops whose
                // target is a materialized array value.  `value_of` above only
                // peels index 0, so handle every constant `k` here.
                if let Some(k) = usize_value(module, index)
                    && let Some(array_value) = value_of_node(module, target).or(Some(target))
                {
                    // SAFETY: `array_value` is a live node of `module`.
                    if let Some(items) = unsafe { module.array_items(array_value) }
                        && let Some(item) = items.get(k)
                    {
                        return emit_node(module, params, dyn_node(item.node)?, body, tally);
                    }
                }
                // A conditional `if c then a else b` lowers to `[b, a][c]` — a
                // 2-element array value indexed by a *computed* (non-constant)
                // selector, a wasm `select`.  The array may be reached through
                // a value_of extraction; look through it.
                if usize_value(module, index).is_none()
                    && let Some(array_value) = value_of_node(module, target).or(Some(target))
                {
                    // SAFETY: `array_value` is a live node of `module`.
                    if let Some(items) = unsafe { module.array_items(array_value) }
                        && items.len() == 2
                    {
                        let then_node = dyn_node(items[1].node)?;
                        let else_node = dyn_node(items[0].node)?;
                        // Each branch is emitted into its own vector so it can be
                        // inspected before the three parts are concatenated: a
                        // `compute.write` in a branch would be a *statement* in
                        // a position that must hold a *value* (the branch leaves
                        // nothing on the stack, and `select` would read whatever
                        // the branch pushed), and it would run on only one of
                        // the two paths — so that output ordinal would not be
                        // written on every index.  Refuse it by name instead.
                        let mut then_body = Vec::new();
                        let mut else_body = Vec::new();
                        let mut select_body = Vec::new();
                        emit_node(module, params, then_node, &mut then_body, tally)?;
                        emit_node(module, params, else_node, &mut else_body, tally)?;
                        if then_body.contains(&KernelInstr::BufferWriteCall)
                            || else_body.contains(&KernelInstr::BufferWriteCall)
                        {
                            return Err(CONDITIONAL_WRITE.into());
                        }
                        emit_node(module, params, index, &mut select_body, tally)?;
                        body.append(&mut then_body);
                        body.append(&mut else_body);
                        body.append(&mut select_body);
                        body.push(KernelInstr::I32WrapI64);
                        body.push(KernelInstr::Select);
                        return Ok(());
                    }
                }
                return Err(
                    "unsupported index in kernel body (only parameter reads, value_of extractions, and 2-element conditionals)"
                        .into(),
                );
            }
            LowOperator::Apply => {
                let (callee, arg) = apply_pair(module, operation.operand)?;
                // Style 2: a cross-kernel call — the callee is a kernel value
                // (the result of an earlier `jit`).  Emit the (scalar)
                // argument, then a call the launch-time assembler resolves to
                // the callee's function index once the kernel's relative launch
                // set is laid out.
                if kernel_id_of(module, callee).is_some() {
                    return emit_cross_kernel_call(module, params, callee, arg, body, tally);
                }
                // Style 1: a full lichen-function call (inline its body) —
                // deferred.
                return Err(
                    "kernel body Apply is supported only for a cross-kernel (kernel-value) callee v1; inline lichen-function calls are not yet supported"
                        .into(),
                );
            }
            LowOperator::TableGet => return Err(
                "unsupported tableget operator in kernel body (kernel-safe subset is scalar arith)"
                    .into(),
            ),
        }
    }
    // The highlevel's type-level arithmetic over `[left, right]`.
    if let Some(ty_op) = AsEnum::<TypeOperator>::as_enum(op) {
        let Some(bin) = kernel_bin(ty_op) else {
            return Err(format!(
                "unsupported highlevel operator in kernel body: {ty_op:?}"
            ));
        };
        let (left, right) = operand_pair(module, operation.operand)?;
        emit_node(module, params, left, body, tally)?;
        emit_node(module, params, right, body, tally)?;
        body.push(KernelInstr::Bin(bin));
        return Ok(());
    }
    // The compute plugin's own operators: `Launch`/`Call` inside a kernel body
    // are the wrapper cross-kernel call forms (`compute.launch k x` /
    // `call k x`, which lower to a cross-kernel `CallKernel`).
    if let Some(compute_op) = AsEnum::<ComputeOperator>::as_enum(op) {
        match compute_op {
            ComputeOperator::Launch | ComputeOperator::Call => {
                let (kernel, arg) = apply_pair(module, operation.operand)?;
                return emit_cross_kernel_call(module, params, kernel, arg, body, tally);
            }
            // The loop index of the current parallel invocation.  The index is
            // the wasm param immediately after the cfg scalar params.
            ComputeOperator::Range => {
                let index_local: usize = params.iter().map(|p| flat_arity(&p.shape)).sum();
                body.push(KernelInstr::LocalGet(index_local as u32));
                return Ok(());
            }
            // Read an input buffer element: `read [cfg(1)(k), idx]` → the host
            // `read(cfg_pos=k, idx)` import.  The buffer node is a cfg buffer
            // tuple slot; its position is the compile-time cfg_pos.
            ComputeOperator::Read => {
                let (buf, idx) = operand_pair(module, operation.operand)?;
                // The buffer operand comes through the wrapper's slot-read
                // destructuring: `read = x => $read(x(0), x(1))` applied to
                // `[cfg(1)(k), idx]` leaves `Index(arg_array, 0)` where
                // `arg_array` is the materialized argument array.  Resolve
                // that to the actual buffer node (the cfg buffer-tuple slot)
                // so `parallel_buffer_pos` recognizes it, exactly like the
                // `Index` emitter peels a constant array element.
                let mut buf = buf;
                for _ in 0..8 {
                    let target_oi = match module.node_operation(buf).as_ref() {
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
                    let Some(array_value) = value_of_node(module, target).or(Some(target)) else {
                        break;
                    };
                    // SAFETY: `array_value` is a live node of `module`.
                    let Some(items) = (unsafe { module.array_items(array_value) }) else {
                        break;
                    };
                    let Some(item) = items.get(k) else { break };
                    buf = dyn_node(item.node)?;
                }
                let pos = parallel_buffer_pos(module, params, buf).ok_or_else(|| {
                    "read's buffer argument is not a cfg buffer tuple slot (cfg(1)(k))".to_string()
                })?;
                // The input count is a **max**, not a tally: the read positions are
                // a sparse space, and a body that reads only `cfg(1)(1)` still
                // needs two buffers bound or the one it read was never bound.
                tally.reads = tally.reads.max(pos + 1);
                body.push(KernelInstr::Const(pos as i64));
                emit_node(module, params, idx, body, tally)?;
                body.push(KernelInstr::BufferReadCall);
                return Ok(());
            }
            // A pending write: `write [n, idx, val]` → the host
            // `write(out_pos, idx, val)` import, with `out_pos` this write's
            // **emission ordinal** — its position in the index function's
            // codomain, which is a compile-time constant exactly as `read`'s
            // `cfg_pos` is.  The ordinal is taken (and the counter advanced)
            // before the operands are emitted, so a write nested inside another
            // write's value would still consume an ordinal of its own — which
            // is what `compile_parallel_fragment`'s count check refuses.
            ComputeOperator::Write => {
                let operand = operation
                    .operand
                    .ok_or_else(|| "write operand array is missing".to_string())?;
                let items = operand_items(module, operand)?;
                let idx = dyn_node(items[1].node)?;
                let val = dyn_node(items[2].node)?;
                let out_pos = tally.writes;
                tally.writes += 1;
                body.push(KernelInstr::Const(out_pos as i64));
                emit_node(module, params, idx, body, tally)?;
                emit_node(module, params, val, body, tally)?;
                body.push(KernelInstr::BufferWriteCall);
                return Ok(());
            }
            // Jitting another function from *inside* a kernel body is not a v1
            // cross-kernel call; launching a parallel kernel from inside a body
            // is likewise deferred.
            other => {
                return Err(format!(
                    "unsupported compute operator in kernel body: {other:?}"
                ));
            }
        }
    }
    Err(format!(
        "unsupported operation in kernel body: {op:?} (kernel-safe subset is scalar arith)"
    ))
}

/// The buffer's cfg position, if `node` is a cfg buffer-tuple slot
/// `cfg(1)(k)` in a parallel kernel — the position `k` the host `read` import
/// reads.  `params[0]` is the cfg parameter slot; its `.value` is the cfg tuple
/// value node, and the buffer tuple lives at `cfg(1)`.
fn parallel_buffer_pos<P>(module: &Module<P>, params: &[ParamSlot], node: NodeId) -> Option<usize>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let cfg_value = params.first()?.value;
    let operation = module.node_operation(node)?;
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&operation.operator),
        Some(LowOperator::Index)
    ) {
        return None;
    }
    let (target, index) = operand_pair(module, operation.operand).ok()?;
    let k = usize_value(module, index)?;
    let target_op = module.node_operation(target)?;
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&target_op.operator),
        Some(LowOperator::Index)
    ) {
        return None;
    }
    let (tt, ti) = operand_pair(module, target_op.operand).ok()?;
    // The body's cfg reads reference the cfg value through `Index(cfg_pair, 0)`
    // (a read node) rather than the value node itself, so compare the two cfg
    // value slots by *equality class* (as the `emit_node` parameter-read path
    // does) instead of node identity.
    if equality_rep(module, tt) != equality_rep(module, cfg_value)
        || usize_value(module, ti) != Some(1)
    {
        return None;
    }
    Some(k)
}

/// A cross-kernel call to a callee that returns **more than one value**.
///
/// The emitter's contract is that every `emit_node` leaves exactly one value on
/// the stack, and a wasm `call` pushes one value per result the callee declares.
/// So an `N`-result callee inside a body would push `N` values where the body
/// expects one — which is exactly the silent miscompilation this crate never
/// allows (the same reason a conditional write is refused rather than emitted).
///
/// Re-materialising a tuple would mean spilling those `N` values into locals and
/// reading one back for a further `local.get`, a spilling primitive
/// [`KernelInstr`] has no encoding for; until it does, the call is **refused by
/// name**, saying the callee and its arity, rather than truncated to its first
/// value.
const CROSS_KERNEL_RESULT_ARITY: &str = "a cross-kernel call to a kernel that returns more than \
one value is not supported: a kernel body reads a callee result as a single value, and \
re-materialising a tuple result needs local slots the kernel instruction set does not yet \
have";

/// Emit a cross-kernel call (style 2): the (scalar) argument expression, then
/// a [`KernelInstr::CallKernel`] the launch-time assembler resolves.  Both a
/// direct kernel `Apply` (`k x`) and the wrapper's `launch`/`$launch`
/// (`compute.launch k x`) lower here — the latter is the typed form (its
/// codomain is resolved by [`LaunchOp`]), the former the untyped-form gap.
///
/// **The callee must return exactly one value.**  A multi-value callee is
/// refused by name (see [`CROSS_KERNEL_RESULT_ARITY`]) rather than truncated to
/// its first result.
fn emit_cross_kernel_call<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    kernel: NodeId,
    arg: NodeId,
    body: &mut Vec<KernelInstr>,
    tally: &mut Positions,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let kid = kernel_id_of(module, kernel)
        .ok_or_else(|| "cross-kernel call target is not a kernel value".to_string())?;
    // The callee's domain **and** result arity are facts of the *callee's*
    // registration, read here and released before any emission: emitting can
    // reach a further cross-kernel call, which locks the same registry again,
    // and the lock is not reentrant.
    let (shape, results) = {
        let fragments = kernels().lock().unwrap();
        let fragment = fragments
            .get(&kid)
            .ok_or_else(|| "cross-kernel callee is not a registered kernel".to_string())?;
        (fragment.param_shape.clone(), fragment.results)
    };
    if results != 1 {
        return Err(format!(
            "cross-kernel call to kernel {kid}, which returns {results} value(s): {}",
            CROSS_KERNEL_RESULT_ARITY
        ));
    }
    if shape.flat_arity() == 1 {
        // A scalar-domain callee takes one i64, and the argument is peeled
        // once and emitted once — the pre-existing path, kept exactly as it was.
        // (A *tuple* domain has to resolve its own encoding; see
        // `emit_callee_args`.)
        let arg = pair_value_node(module, arg).unwrap_or(arg);
        emit_node(module, params, arg, body, tally)?;
    } else {
        emit_callee_args(module, params, arg, &shape, body, tally)?;
    }
    body.push(KernelInstr::CallKernel(kid));
    Ok(())
}

/// A cross-kernel call's tuple argument that is neither a whole-parameter read
/// nor a concrete tuple value, under any encoding.  The flattened layout is
/// what makes the other cases work, so a wrong one would read a local the
/// argument does not own.
const CALLEE_ARGUMENT: &str = "a cross-kernel call's argument must be a concrete tuple value or a \
whole parameter read; build the argument from its elements (or pass the parameter through)";

/// Emit a cross-kernel call's tuple argument as the callee domain's scalar
/// leaves, in callee parameter order — the stack values the wasm `call`
/// consumes.  The caller pushes one `i64` per leaf and the callee's signature
/// is `(i64) * flat_arity`, so the count is a correctness requirement rather
/// than a lowering choice.
///
/// The argument reaches a call in one of two **encodings**, and they cannot be
/// told apart by shape: a bare kernel apply carries the `[value, type]` pair
/// whose element 0 is the argument, while a `launch` argument arrives as a
/// bare `Parameterized` cell (concrete only at run time) — and a pair has
/// exactly as many elements as the two-element tuple it wraps.  So each
/// encoding is *emitted* and the first that produces one leaf per domain
/// element is kept.  That is not a guess: the leaves have to emit anyway, and a
/// pair read as a tuple fails here on its second element, which is a type cell.
fn emit_callee_args<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    arg: NodeId,
    shape: &KernelShape,
    body: &mut Vec<KernelInstr>,
    tally: &mut Positions,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let KernelShape::Tuple(items) = shape else {
        return emit_node(module, params, arg, body, tally);
    };
    // A candidate that reads as a tuple but disagrees with the domain is a
    // cause worth reporting; one that simply is not a tuple only says the
    // encoding was wrong, which the next candidate may still fix.
    let mut cause: Option<String> = None;
    for candidate in callee_arg_encodings(module, arg) {
        let mut leaves: Vec<KernelInstr> = Vec::new();
        // Each candidate is emitted from the same `tally` counts, so a failed
        // attempt cannot leave the position counters advanced by instructions
        // that are then thrown away.
        let mut candidate_tally = *tally;
        match emit_tuple_leaves(
            module,
            params,
            candidate,
            items,
            &mut leaves,
            &mut candidate_tally,
        ) {
            Ok(()) => {
                *tally = candidate_tally;
                body.extend(leaves);
                return Ok(());
            }
            Err(reason) => {
                if reason != CALLEE_ARGUMENT {
                    cause = Some(reason);
                }
            }
        }
    }
    Err(cause.unwrap_or_else(|| CALLEE_ARGUMENT.to_string()))
}

/// The ways one call argument can be encoded, in the order they are tried — the
/// same peel order the rest of the emitter resolves through: the `[value, type]`
/// pair, a `value_of` extraction, the value a `Parameterized` class committed
/// to, then the node itself.
fn callee_arg_encodings<P>(module: &Module<P>, arg: NodeId) -> Vec<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    [
        pair_value_node(module, arg),
        value_of_node(module, arg),
        class_value_node(module, arg),
        Some(arg),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Emit the leaves of one tuple value against the domain elements `items`.
///
/// Two argument shapes cover it:
///
/// - A **whole-parameter read** is passed through as the parameter's own
///   locals.  A domain's leaves are contiguous in the flattened layout (that is
///   what [`flatten_offset`] counts), so a sub-tuple read has a base local and
///   its leaves follow it.  The read's own sub-shape must flatten to exactly
///   the arity `items` flattens to — a read that is *shorter* would push the
///   next parameter's local as if it were the callee's last argument, so a
///   mismatch is refused by arity rather than trusted.
/// - Anything else must be a **concrete tuple value** of exactly `items.len()`
///   elements, each emitted against its own element shape (recursively, for a
///   nested domain).  A scalar element goes through [`emit_node`], so a
///   constant, a parameter read, a call result, and a `Parameterized` cell
///   resolved through its class all keep working inside a tuple argument.
fn emit_tuple_leaves<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: NodeId,
    items: &[KernelShape],
    out: &mut Vec<KernelInstr>,
    tally: &mut Positions,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let arity: usize = items.iter().map(KernelShape::flat_arity).sum();
    if let Some((base, read)) = param_pass_through(module, params, node) {
        if flat_arity(&read) == arity {
            for offset in 0..arity {
                out.push(KernelInstr::LocalGet((base + offset) as u32));
            }
            return Ok(());
        }
        // A parameter read whose own shape is not the callee's domain: taking
        // its locals anyway would read past the read into the next parameter's,
        // so it is named rather than padded or truncated.
        return Err(format!(
            "cross-kernel call passes a {}-element parameter read to a domain of {arity} \
scalar(s)",
            flat_arity(&read)
        ));
    }
    let elements = tuple_elements(module, node).ok_or(CALLEE_ARGUMENT)?;
    if elements.len() != items.len() {
        return Err(format!(
            "cross-kernel call passes {} element(s) to a {}-element tuple domain",
            elements.len(),
            items.len()
        ));
    }
    for (element, element_shape) in elements.iter().zip(items) {
        match element_shape {
            KernelShape::Tuple(nested) => {
                emit_tuple_leaves(module, params, *element, nested, out, tally)?
            }
            KernelShape::Scalar => emit_node(module, params, *element, out, tally)?,
        }
    }
    Ok(())
}

/// The wasm local a whole-parameter read of `node` starts at, with the domain
/// sub-shape that read covers — the base a multi-arity cross-kernel argument
/// is passed through from.  `None` when `node` is not a parameter read.
///
/// Both read forms count: the `Index` chain [`param_path`] recognises, and the
/// bare value cell a reduced call's argument unification leaves behind (the
/// same read with the empty path, as `emit_node` also accepts).
fn param_pass_through<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: NodeId,
) -> Option<(usize, LowShape)>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    for slot in params {
        let path = match param_path(module, slot.pair, node) {
            Some(path) => path,
            None if module.node_operation(node).is_none()
                && equality_rep(module, node) == equality_rep(module, slot.value) =>
            {
                Vec::new()
            }
            None => continue,
        };
        let Some(read) = sub_shape(&slot.shape, &path) else {
            continue;
        };
        let Some(base) = flatten_offset(&slot.shape, &path).ok() else {
            continue;
        };
        return Some((slot.base + base, read.clone()));
    }
    None
}

/// The elements of a concrete tuple value, as dynamic node ids.
///
/// Three ways a tuple argument reaches its array: it is the array value
/// itself, it is a `value_of` extraction over one, or it is a
/// `Parameterized` cell whose class is committed to the tuple that defines it
/// (a `launch` argument, which is only concrete at run time).  The third
/// resolves through the class's *value* members, because a materialized tuple
/// is a value node and so states no operation for the emitter to trace.
fn tuple_elements<P>(module: &Module<P>, node: NodeId) -> Option<Vec<NodeId>>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let candidates = [
        value_of_node(module, node),
        class_value_node(module, node),
        Some(node),
    ];
    for candidate in candidates.into_iter().flatten() {
        // SAFETY: each candidate is a live node of `module`; nothing in this
        // crate calls `Module::drop_block`.
        if let Some(items) = unsafe { module.array_items(candidate) } {
            return items
                .iter()
                .map(|item| dyn_node(item.node))
                .collect::<Result<Vec<_>, _>>()
                .ok();
        }
    }
    None
}

/// A member of `node`'s equality class that holds a **value** — a node the deep
/// pass has already evaluated.  The counterpart of [`class_computation_node`]
/// for a class whose defining member is a materialized value (a tuple literal,
/// a string) rather than an operator, which states no operation to trace.
fn class_value_node<P>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let root = equality_rep(module, node);
    disjoint::members(&module.nodes, root)
        .into_iter()
        .find(|member| module.node_value(AnyNodeId::Dynamic(*member)).is_some())
}

/// Is `node` the parameter's *value* node — `Index(param_pair, 0)`?  The
/// scalar `Int -> Int` kernel reads the value directly; the tuple-domain
/// kernel reads element `k` through `Index(Index(param_pair, 0), k)`, whose
/// target is this node.
fn is_param_value<P>(module: &Module<P>, param_pair: NodeId, node: NodeId) -> bool
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(operation) = module.node_operation(node) else {
        return false;
    };
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&operation.operator),
        Some(LowOperator::Index)
    ) {
        return false;
    }
    let Ok((target, index)) = operand_pair(module, operation.operand) else {
        return false;
    };
    if target != param_pair {
        return false;
    }
    module
        .node_value(AnyNodeId::Dynamic(index))
        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
        .is_some_and(|v| matches!(v, LowValue::USize(0)))
}

/// The constant `USize` value behind `node`, if it is one (an `Index`'s
/// selector must be a compile-time constant in a kernel body).
fn usize_value<P>(module: &Module<P>, node: NodeId) -> Option<usize>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    match module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
    {
        Some(LowValue::USize(n)) => Some(n),
        _ => None,
    }
}

/// Follow a `value_of` extraction — `Index(pair, 0)`, where `pair` is a
/// `[value, type]` pair and the index is the constant `0` — to the pair's
/// value slot.  The checker accesses most values through such an extraction,
/// so the JIT must look through it to reach the actual value (a constant, a
/// parameter read, or a computation).
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
    // A concrete `[value, type]` pair value → its value slot (element 0).
    // SAFETY: `target` is a live node of `module`.
    if let Some(items) = unsafe { module.array_items(target) } {
        return dyn_node(items.first()?.node).ok();
    }
    // An *operator* node as the target — e.g. `Index(apply_op, 0)` where the
    // checker peels a call result (`value_of` over an `Apply` expression).  The
    // operator's result is the pair's value, so emit the operator directly; its
    // codegen produces the scalar (a cross-kernel call, an arithmetic op, ...).
    if module.node_operation(target).is_some() {
        return Some(target);
    }
    None
}

/// The [`KernelId`] of a kernel-value node (`ComputeValue::Kernel`), looking
/// through any `value_of` extractions and a kernel struct's `.native` field read
/// (`Index(struct, 0)`).  Returns `None` for a node that is not (or does not
/// reach) a kernel value — e.g. a lichen function, used by the style-1 inline
/// path instead.
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
    // A kernel *struct value* `[native, sig]` reached by value (not through an
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
fn pair_value_node<P>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    // SAFETY: `node` is a live node of `module`.
    let items = unsafe { module.array_items(node) }?;
    dyn_node(items.first()?.node).ok()
}

/// The index *path* from the parameter to the value `node` reads, if `node` is
/// a parameter read:
/// - `Index(param_pair, 0)` (a scalar domain value) → `[]`,
/// - `Index(param_value, k)` (a flat tuple element) → `[k]`,
/// - `Index(Index(param_value, a), b)` (a nested tuple element) → `[a, b]`.
///
/// Any other `Index` (a structured-array conditional, an out-of-domain
/// index) is `None`.
fn param_path<P>(module: &Module<P>, param_pair: NodeId, node: NodeId) -> Option<Vec<usize>>
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
    if target == param_pair {
        // `Index(param_pair, k)`: the parameter's value node.  Read directly
        // only for a scalar domain (`k == 0`), i.e. the empty path.
        return Some(if k == 0 { vec![] } else { vec![k] });
    }
    if is_param_value(module, param_pair, target) {
        // `Index(param_value, k)` — a direct tuple element read.
        return Some(vec![k]);
    }
    // `target` is itself a deeper parameter read (a nested tuple element).
    let mut path = param_path(module, param_pair, target)?;
    path.push(k);
    Some(path)
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

/// The domain shape a parameter index `path` reads — the mirror of
/// [`flatten_offset`], which answers the same path with a local offset instead
/// of a shape.  A path into a scalar is `None`: a scalar domain is only ever
/// read whole, as the empty path.
fn sub_shape<'a>(domain: &'a LowShape, path: &[usize]) -> Option<&'a LowShape> {
    let mut shape = domain;
    for &i in path {
        let LowShape::Tuple(items) = shape else {
            return None;
        };
        shape = items.get(i)?;
    }
    Some(shape)
}

/// Flatten a kernel argument value (a scalar `USize` leaf, or a possibly
/// nested `Array` of them, as a tuple-of-tuples domain needs) into the wasm
/// argument vector.  Returns `Err` naming the first element that is not a
/// scalar `USize` leaf — the definition pass reports the unbound result, but
/// only this says *which* element was unusable.
///
/// `path` is the offending element's position in the argument: `""` at the
/// root, then `"1"`, `"1.0"`, …  It is built as the walk descends, because for
/// a tuple-of-tuples argument a position is the only way to point at the one
/// element that could not be lowered; an empty `path` is the root, whose
/// refusal names the argument as a whole.
fn collect_args<P>(
    module: &Module<P>,
    node: AnyNodeId,
    out: &mut Vec<i64>,
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
            out.push(n as i64);
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
                "{refusal}, not a concrete Int, so the kernel's parameters could not be filled"
            ))
        }
    }
}

/// What a launch argument that is not a kernel parameter vector at all looks
/// like, in the terms a lichen program is written in: a refusal that names the
/// runtime variant instead names the value the user wrote.  `USize` and
/// `Array` are the two shapes a parameter vector may take and are recognised
/// before this is asked, so the last arm covers a node that carries no value at
/// all.
fn argument_kind(value: Option<&LowValue>) -> &'static str {
    match value {
        // A float is a concrete scalar, so it is named as one rather than left
        // to the "no value" catch-all: the refusal this feeds has to say what
        // the user actually wrote, and a float argument is the phase-0 case a
        // kernel must refuse (`docs/notes/floating-point.md` §3.8, §5).  The
        // `i64` vector `collect_args` builds has no element for it.
        Some(LowValue::Float(_)) => "a float",
        Some(LowValue::Str(_)) => "a string",
        Some(LowValue::Table(_)) => "a table",
        Some(LowValue::Function(_)) => "a function",
        Some(LowValue::None) => "the unit value",
        Some(LowValue::Void) => "nothing (the empty value of a failed read)",
        Some(LowValue::Parameterized) => "a value that is not decided yet",
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

/// A buffer position holding something that is not a buffer.
///
/// **This is a refusal, and the `Parameterized` it replaces was a silent
/// no-op.** The lazy cell is what makes a kernel's own read deferrable and what
/// makes an undecided argument stay undecided — but a program array *is*
/// decided, it is an ordinary lichen value with ordinary elements, and
/// answering `parameterized` for it made `compute.read [data, i]` a
/// plausible-looking program that computed nothing while still printing
/// `array<?a, ?b>`. There is no way to make a buffer out of a program value, so
/// the honest answer names that rather than waiting for a buffer that will not
/// arrive.
///
/// `subject` says what the position is *for* and `at` names it, so the three
/// sites that reach this describe their own mistake rather than sharing one
/// sentence.
fn not_a_buffer<P>(module: &mut Module<P>, node: AnyNodeId, subject: &str, at: &str) -> P::Value
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
    <P::Value as From<LowValue>>::from(LowValue::Parameterized)
}

/// The value a completed kernel run produces: a bare `USize` for a
/// single-result kernel, and the **tuple** of them for a multi-result one.
///
/// This is the same shape `ParLaunch` builds for its several output buffers —
/// each result becomes a node of `block` first, so the array holds live nodes
/// rather than detached values — and it is the direct counterpart of
/// [`codomain_leaves`], which is what decided how many results there are.  A
/// lichen tuple is an ordinary array value, so `r(0)`/`r(1)` index it with no
/// special case, exactly as the codomain's leaves were ordinary `i64`s.
fn kernel_results_value<P>(module: &mut Module<P>, block: BlockId, results: Vec<usize>) -> P::Value
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some((first, rest)) = results.split_first() else {
        // A fragment always leaves at least one value, so an empty run is not
        // reachable; stay lazy rather than fabricating a value for it.
        return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
    };
    if rest.is_empty() {
        // The single-result form is a bare `USize`, exactly what it always was.
        return <P::Value as From<LowValue>>::from(LowValue::USize(*first));
    }
    let scalar = |value: &usize| <P::Value as From<LowValue>>::from(LowValue::USize(*value));
    let items: Vec<ArrayItem> = results
        .iter()
        .map(|value| {
            let node = module.add_node(block, None, Some(scalar(value)));
            ArrayItem::new(AnyNodeId::Dynamic(node))
        })
        .collect();
    let handle = module.alloc_array(&items, block);
    <P::Value as From<LowValue>>::from(LowValue::Array(handle))
}

/// Read a binary op/Index operand array `[a, b]` as two dynamic node ids.
fn operand_pair<P>(module: &Module<P>, operand: Option<NodeId>) -> Result<(NodeId, NodeId), String>
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
    Ok((dyn_node(items[0].node)?, dyn_node(items[1].node)?))
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

/// The `[function, argument]` of an `Apply` operand array.  The checker's
/// apply operands are `[function, argument, result_cell]` (the result cell is
/// a checker-wired value that does not participate in codegen), so unlike
/// [`operand_pair`] this tolerates extra elements and takes the first two.
fn apply_pair<P>(module: &Module<P>, operand: Option<NodeId>) -> Result<(NodeId, NodeId), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let Some(operand) = operand else {
        return Err("Apply operand is missing".into());
    };
    let items = operand_items(module, operand)?;
    if items.len() < 2 {
        return Err("Apply operand array must have at least two elements".into());
    }
    Ok((dyn_node(items[0].node)?, dyn_node(items[1].node)?))
}

/// Record one dispatch into the graph being built, in place of running it.
///
/// **The cfg is read for its placeholders, not for its data.** `cfg(0)` is the
/// extent and `cfg(1)` the buffer tuple, the same positions a real launch reads,
/// so the body is walked by exactly the path a run would take and the only thing
/// that differs is what comes back. A value that is neither a placeholder nor an
/// input the parameter supplied is refused here by name rather than coerced: this
/// is the filter, and it is where a jit'd function's arbitrary values are sorted
/// into the two roles a graph's value table has.
fn record_launch<P>(module: &mut Module<P>, block: BlockId, operand: P::Value) -> P::Value
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + From<LowValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let refuse = |module: &mut Module<P>, reason: String| {
        module.record_extension_diagnostic(GRAPH_DIAGNOSTIC, None, reason);
        <P::Value as From<LowValue>>::from(LowValue::Parameterized)
    };
    let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
        unreachable!("ParLaunch expects an operand array of [kernel, cfg]")
    };
    // SAFETY: `operands` is the operand array the VM just evaluated for this
    // operation, and every walk below stays inside this borrow of `module`.
    let operands = unsafe { operands.items() };
    if operands.len() < 2 {
        return refuse(
            module,
            "a parallel launch's operand array is [kernel, cfg]".into(),
        );
    }
    let Some(ComputeValue::ParKernel(id, backend)) = module
        .node_value(operands[0].node)
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
    else {
        return refuse(
            module,
            "a recorded dispatch has to name a parallel kernel, and this one does not".into(),
        );
    };
    let fragment = {
        let kernels = kernels().lock().unwrap();
        match kernels.get(&id) {
            Some(fragment) => fragment.clone(),
            None => {
                return refuse(
                    module,
                    format!(
                        "parallel kernel {id} is not registered, so there is no fragment to record"
                    ),
                );
            }
        }
    };
    let Ok(cfg) = dyn_node(operands[1].node) else {
        return refuse(module, "a recorded dispatch's cfg is not a node".into());
    };
    // SAFETY: `cfg` names a live node of `module`, and the items are read only.
    let Some(cfg_items) = (unsafe { module.array_items(cfg) }) else {
        return refuse(module, "a recorded dispatch's cfg is not a tuple".into());
    };
    // The extent, read on its own: a count is a different role from a buffer, and
    // a caller who swapped the two deserves to be told which was wrong rather
    // than that a shape did not match.
    let extent = match cfg_items.first() {
        Some(item) => {
            let literal = module
                .node_value(item.node)
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
                    .node_value(item.node)
                    .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
                    .and_then(|value| match value {
                        ComputeValue::GraphInput(slot) => Some(Extent::Value(Placed::Input(slot))),
                        ComputeValue::GraphValue(id) => Some(Extent::Value(Placed::Value(id))),
                        _ => None,
                    }) {
                    Some(extent) => extent,
                    None => {
                        let found = module
                            .node_value(item.node)
                            .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
                            .map(|value| format!("{value:?}"))
                            .unwrap_or_else(|| {
                                module
                                    .node_value(item.node)
                                    .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
                                    .map(|value| graph::describe(&value).to_string())
                                    .unwrap_or_else(|| "not a value at all".to_string())
                            });
                        return refuse(
                            module,
                            format!(
                                "a dispatch's count is {found}, and a count has to be a literal or \
                                 one of this function's arguments: a graph's extent is either known \
                                 while it is built or read from its own value table at run time"
                            ),
                        );
                    }
                },
            }
        }
        None => {
            return refuse(
                module,
                "a recorded dispatch's cfg has no count at position 0".into(),
            );
        }
    };
    let mut inputs: Vec<Placed> = Vec::new();
    if let Some(tuple) = cfg_items.get(1)
        && let Ok(tuple_node) = dyn_node(tuple.node)
        // SAFETY: `tuple_node` names a live node of `module`.
        && let Some(items) = (unsafe { module.array_items(tuple_node) })
    {
        for (position, item) in items.iter().enumerate() {
            let Some(value) = module
                .node_value(item.node)
                .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
            else {
                return refuse(
                    module,
                    format!("argument {position} of this dispatch is not a compute value at all"),
                );
            };
            match graph::place(&value, position) {
                Ok(placed) => inputs.push(placed),
                Err(reason) => return refuse(module, reason),
            }
        }
    }
    let placed = match graph::record_dispatch(fragment, backend, extent, &inputs) {
        Ok(placed) => placed,
        Err(reason) => return refuse(module, reason),
    };
    // **The same shape a real launch produces**: a bare value for one output, the
    // tuple of them for several. Each placeholder becomes a node of this block
    // first, so the tuple holds live nodes rather than detached values, and the
    // body downstream reads a result exactly as it reads a run's.
    let placeholder =
        |id: usize| <P::Value as From<ComputeValue>>::from(ComputeValue::GraphValue(id));
    if placed.len() == 1 {
        return placeholder(placed[0].edge());
    }
    let items: Vec<ArrayItem> = placed
        .iter()
        .map(|placed| {
            let node = module.add_node(block, None, Some(placeholder(placed.edge())));
            ArrayItem::new(AnyNodeId::Dynamic(node))
        })
        .collect();
    let handle = module.alloc_array(&items, block);
    <P::Value as From<LowValue>>::from(LowValue::Array(handle))
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
fn build_graph<P>(module: &mut Module<P>, block: BlockId, function_node: AnyNodeId) -> P::Value
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + From<LowValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator> + From<LowOperator>,
{
    let refuse = |module: &mut Module<P>, reason: String| {
        module.record_extension_diagnostic(GRAPH_DIAGNOSTIC, None, reason);
        <P::Value as From<LowValue>>::from(LowValue::Parameterized)
    };
    // A **check, not a resolution.** The apply below consumes the operand *node*,
    // because a function id and a node id are different id spaces and `Apply`
    // addresses its callee as a node. So this only has to answer "is this
    // operand a function at all" — and it answers it by reading the node's own
    // value, which is the same read the apply's callee extraction will do.
    let is_function = matches!(
        module.evaluate_node(function_node, Some(block)).as_enum(),
        Some(LowValue::Function(_))
    );
    if !is_function {
        return refuse(
            module,
            "a graph is recorded from a function, and this operand is not one".into(),
        );
    }
    // **The arity is the parameter tuple's length, read without evaluating
    // anything.** A read of `ins(i)` compiles to a bare cell with no operation
    // and no subscript, so the body's read positions are not visible before the
    // apply and cannot be used to size the placeholder tuple. The parameter *is*
    // a tuple with one cell per read, though, and its length is the answer.
    // **The arity is not knowable here, and that is the finding.** A read of
    // `ins(i)` compiles to a bare cell with no operation and no subscript, so
    // the unapplied body contains nothing that says which slot a read wants; and
    // the parameter's own value node cannot answer it either, because in a
    // template that value is an undecided cell rather than a tuple — so there is
    // no length to read anywhere before the apply. The tuple is therefore built
    // at a ceiling and the graph is trimmed to the slots the body actually read.
    let arity = graph::MAX_GRAPH_INPUTS;
    // One placeholder per slot, **and the slot is the number**, so the apply's
    // tuple walk binds the `k`-th argument to the `k`-th cell and `ins(k)` is the
    // `k`-th argument with no ordering to guess.
    let cells: Vec<ArrayItem> = (0..arity)
        .map(|slot| {
            let value = <P::Value as From<ComputeValue>>::from(ComputeValue::GraphInput(slot));
            let node = module.add_node(block, None, Some(value));
            ArrayItem::new(AnyNodeId::Dynamic(node))
        })
        .collect();
    let placeholders = array_node::<P>(module, block, &cells);
    // The argument is a **pair**, because a parameter is one: the value side is
    // the placeholder tuple and the type side is left undecided. The type side
    // has to stay undecided rather than be invented, because a parameter's type
    // is what says which role each argument has — and that is precisely the
    // question this recording is going to answer by looking at the values.
    let undecided = module.add_node(
        block,
        None,
        Some(<P::Value as From<LowValue>>::from(LowValue::Parameterized)),
    );
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
    // `Module::evaluate_node_forced` performs every statement, and it also leaves
    // the function's return slot empty, so the reader that has to name the return
    // finds no value and *every* recording refuses — including bodies with no
    // unread statement at all. The empty slot was isolated to the operand forcing
    // rather than the shallow descent: a walk that descends every position in
    // order (`skip_shallow` off, `force_operand` off) records the same two
    // dispatches, and turning `force_operand` on alone empties the return slot
    // with the shallow mask untouched. See the landmine.
    let result = module.evaluate_node_deep(apply, Some(block));
    if matches!(
        AsEnum::<LowValue>::as_enum(&result),
        Some(LowValue::Parameterized)
    ) {
        return refuse(
            module,
            format!(
                "applying this function to its own input placeholders stayed undecided, so \
                 nothing was recorded. A function whose parameter is not a plain tuple of cells \
                 cannot be recorded this way, and the parameter here has {} cell(s)",
                arity
            ),
        );
    }
    let recorded = returned_value_ids::<P>(module, &result);
    // **An unreadable return is a refusal, not an unrecorded one.** A graph with
    // no recorded return answers with its whole value table, so treating "I
    // could not read what this function returns" as "it returns everything" hands
    // the caller an extent and a buffer nobody asked to have returned — a wrong
    // answer that runs. The permissive reading of an unrecorded return belongs to
    // a graph nobody recorded, and the language path has just recorded one.
    let Some(values) = recorded else {
        return refuse(
            module,
            "this function's return is not a value a graph can hand back, so there is no \
             graph to record: a graph's value table holds what its dispatches produced and the \
             arguments it was run with, and this body's own value is not one of those"
                .into(),
        );
    };
    if let Err(reason) = graph::record_return(values) {
        return refuse(module, reason);
    }
    match graph::finish() {
        Ok((graph, backend)) => {
            let id = graph::intern(graph);
            <P::Value as From<ComputeValue>>::from(ComputeValue::Graph(id, backend))
        }
        Err(reason) => {
            // **An empty recording with a decided result is a different mistake
            // from an empty one with no result**, and only this one says the body
            // *ran* — so the two refusals have to name different causes or a
            // caller will go looking in the wrong place.
            let reason = if matches!(
                AsEnum::<LowValue>::as_enum(&result),
                Some(LowValue::Parameterized)
            ) {
                reason
            } else {
                format!(
                    "{reason} (the body's own value came back as a decided value, not a lazy one)"
                )
            };
            refuse(module, reason)
        }
    }
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
    P::Value: AsEnum<ComputeValue> + AsEnum<LowValue>,
{
    // The items of a result are **nodes**, not values, so each one is read back
    // through the module rather than matched in place. That is the same reason a
    // multi-output launch gives every buffer a node of its own before wrapping
    // them in a tuple.
    let place_of = |node: AnyNodeId| {
        module
            .node_value(node)
            .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
            .and_then(|value| match value {
                ComputeValue::GraphValue(id) => Some(Placed::Value(id)),
                ComputeValue::GraphInput(slot) => Some(Placed::Input(slot)),
                _ => None,
            })
    };
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
        items.iter().map(|item| place_of(item.node)).collect()
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
) -> P::Value
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + From<LowValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let refuse = |module: &mut Module<P>, reason: String| {
        module.record_extension_diagnostic(GRAPH_DIAGNOSTIC, None, reason);
        <P::Value as From<LowValue>>::from(LowValue::Parameterized)
    };
    let (id, backend) = match module
        .node_value(graph_node)
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
    {
        Some(ComputeValue::Graph(id, backend)) => (id, backend),
        _ => {
            return refuse(
                module,
                "a graph run has to be given a graph, and this is not one".into(),
            );
        }
    };
    // SAFETY: a live node of `module`, read only.
    let argument_items = match arguments.map(|node| dyn_node(node)).transpose() {
        Ok(Some(node)) => match unsafe { module.array_items(node) } {
            Some(items) => items.to_vec(),
            None => {
                return refuse(
                    module,
                    "a graph's arguments have to be a tuple, because the function that recorded \
                     the graph took one parameter"
                        .into(),
                );
            }
        },
        Ok(None) => Vec::new(),
        Err(reason) => return refuse(module, reason),
    };
    let mut run_arguments: Vec<RunArgument> = Vec::new();
    for (position, item) in argument_items.iter().enumerate() {
        let Some(value) = module
            .node_value(item.node)
            .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value))
        else {
            // Not a compute value at all, so it is a number — the only other role
            // a dispatch can read. Asked on its own terms rather than through the
            // compute vocabulary, because a count is a lichen `Int` and not a
            // compute leaf.
            match module
                .node_value(item.node)
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
            {
                Some(LowValue::USize(count)) => {
                    run_arguments.push(RunArgument::Count(count as i64));
                    continue;
                }
                _ => {
                    return refuse(
                        module,
                        format!(
                            "argument {position} is {}, and a graph run reads a buffer or a number \
                             — the two roles a dispatch has",
                            graph::describe(&ComputeValue::TypeWrite)
                        ),
                    );
                }
            }
        };
        run_arguments.push(match value {
            ComputeValue::Buffer(payload) => {
                // SAFETY: read out of `module` on this borrow, so the payload's
                // home block is alive while the data is copied out.
                match buffer_items(&payload) {
                    Some(data) => RunArgument::Buffer(data.to_vec()),
                    None => {
                        return refuse(
                            module,
                            format!("argument {position} is a buffer this process cannot read"),
                        );
                    }
                }
            }
            ComputeValue::DeviceBuffer(resident) => RunArgument::Resident(resident),
            other => {
                return refuse(
                    module,
                    format!(
                        "argument {position} is {}, and a graph run reads a buffer or a number — \
                         the two roles a dispatch has",
                        graph::describe(&other)
                    ),
                );
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
        Err(reason) => return refuse(module, reason),
    };
    let produced = values;
    // **Left where they are.** A value the device wrote stays a resident id and
    // crosses the bus when the language asks for host data, which is the same
    // discipline a single launch follows; a run that fetched everything on the
    // way out would put a download on the path of every result.
    if produced.len() == 1 {
        return match &produced[0] {
            RunResult::Buffer(data) => <P::Value as From<ComputeValue>>::from(
                ComputeValue::Buffer(module.alloc_payload(data, block)),
            ),
            RunResult::Resident(resident) => {
                <P::Value as From<ComputeValue>>::from(ComputeValue::DeviceBuffer(*resident))
            }
            RunResult::Count(count) => <P::Value as From<LowValue>>::from(LowValue::USize(*count)),
        };
    }
    let items: Vec<ArrayItem> = produced
        .iter()
        .map(|result| {
            let value = match result {
                RunResult::Buffer(data) => <P::Value as From<ComputeValue>>::from(
                    ComputeValue::Buffer(module.alloc_payload(data, block)),
                ),
                RunResult::Resident(resident) => {
                    <P::Value as From<ComputeValue>>::from(ComputeValue::DeviceBuffer(*resident))
                }
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
}

fn dyn_node(id: AnyNodeId) -> Result<NodeId, String> {
    match id {
        AnyNodeId::Dynamic(n) => Ok(n),
        AnyNodeId::Static(_) => Err("static refs are not kernel-compilable v1".into()),
    }
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

/// Which launch assembled a cached module.  The fragment set is the mode's, so
/// the mode is part of the cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LaunchMode {
    /// [`run_kernel`]: the root's relative launch set.
    Kernel,
    /// [`run_parallel_kernel`]: the single parallel fragment.
    Parallel,
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
    entries: HashMap<(LaunchMode, KernelId), CachedModule>,
    next_stamp: u64,
}

impl ModuleCache {
    /// A resident module, cloned out (both are `Arc` handles) so the caller
    /// does not hold the lock while instantiating.
    fn get(&self, key: (LaunchMode, KernelId)) -> Option<(wasmi::Engine, wasmi::Module)> {
        self.entries
            .get(&key)
            .map(|cached| (cached.engine.clone(), cached.module.clone()))
    }

    /// Insert a freshly compiled module, evicting the oldest entry once the
    /// bound is reached.  Replacing a resident key evicts nothing: the assembly
    /// for a key is deterministic, so the entry is the same module.
    fn insert(
        &mut self,
        key: (LaunchMode, KernelId),
        engine: wasmi::Engine,
        module: wasmi::Module,
    ) {
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

/// The compiled module for `(mode, root)`, assembling and compiling it on a
/// miss.  `assemble` is the one half the launch paths do differently.
fn cached_module(
    mode: LaunchMode,
    root: KernelId,
    assemble: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<(wasmi::Engine, wasmi::Module), String> {
    let key = (mode, root);
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
/// results — one `usize` per value the fragment declares
/// ([`KernelFragment::results`]).  The dynamic [`wasmi::Func::call`] API accepts
/// any number of `i64` inputs, so a tuple-domain kernel (arity N) launches with
/// N arguments and a scalar kernel (arity 1) with one.
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
/// The kernel's **relative launch set** — the kernel itself plus every kernel
/// it (transitively) cross-calls, discovered by scanning each fragment's
/// cross-kernel instructions — is assembled into one wasm module (launch-time
/// assembly, the deferred linker), the root exported as `main`.  The module is
/// fetched through [`cached_module`], so a repeat launch of the same kernel
/// reuses it (`P1-18`).
fn run_kernel(id: KernelId, args: &[i64]) -> Result<Vec<usize>, String> {
    // Both counts are read under one lock and released before assembly, which
    // locks the same registry again through [`assemble_launch_set`].
    let (expected, results) = {
        let fragments = kernels().lock().unwrap();
        let fragment = fragments
            .get(&id)
            .ok_or_else(|| format!("kernel {id} is not registered"))?;
        (fragment.param_shape.flat_arity(), fragment.results)
    };
    if args.len() != expected {
        return Err(format!(
            "the callee kernel {id} takes {expected} {}, but this call supplied {}",
            if expected == 1 {
                "argument"
            } else {
                "arguments"
            },
            args.len()
        ));
    }
    // A fragment's result arity is at least one (a body always leaves a value),
    // so the buffer is never empty; the guard keeps the invariant stated here
    // rather than resting on the emitter alone.
    let outputs = results.max(1);
    let (engine, module) = cached_module(LaunchMode::Kernel, id, || assemble_launch_set(id))?;
    let mut store = wasmi::Store::new(&engine, ());
    let linker = wasmi::Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_func(&store, "main")
        .ok_or_else(|| "kernel has no export `main`".to_string())?;
    let inputs: Vec<wasmi::Val> = args.iter().map(|&a| wasmi::Val::I64(a)).collect();
    let mut results: Vec<wasmi::Val> = vec![wasmi::Val::I64(0); outputs];
    main.call(&mut store, &inputs, &mut results)
        .map_err(|e| e.to_string())?;
    results
        .iter()
        .map(|value| {
            value
                .i64()
                .map(|result| result as usize)
                .ok_or_else(|| "kernel `main` returned a non-i64".to_string())
        })
        .collect()
}

/// Assemble the wasm bytes of the root kernel's **relative launch set** — the
/// root plus every kernel it (transitively) cross-calls.
///
/// The set is discovered in BFS order: `ordered[i]` becomes wasm function
/// index `i`; `index` maps a callee kernel-id to that index.  The result is a
/// function of the root id alone (the registry's fragments are immutable and
/// ids are never reused), which is what makes the id a sufficient cache key
/// for [`cached_module`].
fn assemble_launch_set(id: KernelId) -> Result<Vec<u8>, String> {
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
            if let KernelInstr::CallKernel(kid) = instr {
                let kid = *kid;
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
    /// The input buffers (the cfg buffer tuple), indexed by cfg position.  They
    /// are never partitioned: a read is by a **global** index, so every worker
    /// reads the whole buffer.
    inputs: &'a [Vec<i64>],
    /// This worker's span of each output buffer, indexed by the write's
    /// `out_pos` ordinal.  There is one span per output the index function
    /// declares — the count comes from the compiled fragment
    /// ([`KernelFragment::outputs`]), never from which slots happened to be
    /// written.
    outputs: Vec<&'a mut [i64]>,
    /// The global index of this partition's first element, which the `write`
    /// import adds to the index the kernel passes.
    base: usize,
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
/// The fragment is cloned out of the registry and the lock released before the
/// call, because a backend that emitted a cross-kernel call would reach the
/// registry again — the lock is not reentrant.
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
    let fragment = {
        let fragments = kernels().lock().unwrap();
        fragments
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("parallel kernel {id} is not registered"))?
    };
    if fragment.outputs != outputs {
        return Err(format!(
            "parallel kernel {id} declares {} output(s) here and {} in the fragment it names",
            outputs, fragment.outputs
        ));
    }
    // The one place a run can be wired to leave its inputs where they are: an
    // input a previous run left on the device is handed back as the id it already
    // has, so a chain of kernels pays one upload for the whole chain rather than
    // one per link.
    let slots: Vec<BufferSlot> = inputs
        .iter()
        .map(|input| match input {
            RunInput::Host(data) => BufferSlot::Host(data),
            RunInput::Resident(resident) => BufferSlot::Resident(resident.id),
        })
        .collect();
    let resident = backend.run(&fragment, &slots, count).map_err(|reason| {
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
    Ok(RunOutcome::Resident(
        resident
            .into_iter()
            .map(|id| ResidentBuffer { id, count })
            .collect(),
    ))
}

/// Assemble the wasm bytes of one **parallel** fragment — the degenerate
/// single-fragment link (`assemble_module`, which `run_kernel` uses for a whole
/// relative launch set).  Like [`assemble_launch_set`] this is a function of
/// the kernel id alone, and it is the other half of [`cached_module`]'s key:
/// the two modes assemble different fragment sets for one id.
fn assemble_parallel_fragment(id: KernelId) -> Result<Vec<u8>, String> {
    let fragment = kernels()
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("parallel kernel {id} is not registered"))?;
    let index: HashMap<KernelId, u32> = [(id, 0)].into();
    assemble_module(&[fragment], &index)
}

/// The most elements one `plrun` may collect — the bound on the count the
/// program controls (the `cfg(0)` the index function is run over).
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
/// index function once per index with `cfg(0) = count` and the cfg input
/// buffers fixed, and collecting the writes into the output buffers.
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
/// that writes a slot that is not its own (`compute.write [n, i - i, v]`,
/// whose index is `0` for every `i`, so all of them collide) already had an
/// order-dependent winner sequentially — the partition, not the launch, is then
/// what decides it.
///
/// `count > `[`MAX_PARALLEL_ELEMENTS`] is refused with an `Err` before the
/// buffers are allocated.  The caller records the reason through
/// [`Module::record_extension_diagnostic`] and returns the lazy
/// (`Parameterized`) marker, which is this plugin's channel for every runtime
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
    count: usize,
    inputs: Vec<RunInput>,
) -> Result<RunOutcome, String> {
    if count > MAX_PARALLEL_ELEMENTS {
        return Err(format!(
            "parallel launch count {count} exceeds the limit of {MAX_PARALLEL_ELEMENTS} elements"
        ));
    }
    // The output count is a property of the *registered fragment*, so it is read
    // here rather than carried in: a kernel id is content-addressed, so the
    // fragment it names cannot be a different one.  The lock is released before
    // any emission or assembly, which locks the same registry again.
    let outputs = {
        let fragments = kernels().lock().unwrap();
        fragments
            .get(&id)
            .map(|fragment| fragment.outputs)
            .ok_or_else(|| format!("parallel kernel {id} is not registered"))?
    };
    if let Backend::Gpu = backend {
        return run_on_installed_backend(id, count, &inputs, outputs);
    }
    // A `"cpu"` run has no device, so an input a `"gpu"` run left there is
    // brought home before the run starts.  This is the one place the two
    // backends meet, and it is a *fetch* rather than a silent refusal: the
    // program's data is on the device and the CPU has no way to reach it, so
    // moving it is what running on the CPU means — not a change in what is
    // computed.
    let mut host_inputs: Vec<Vec<i64>> = Vec::with_capacity(inputs.len());
    for (position, input) in inputs.into_iter().enumerate() {
        match input {
            RunInput::Host(data) => host_inputs.push(data),
            RunInput::Resident(resident) => {
                host_inputs.push(fetch_resident(resident, position)?);
            }
        }
    }
    let inputs = host_inputs;
    let (engine, module) =
        cached_module(LaunchMode::Parallel, id, || assemble_parallel_fragment(id))?;
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
        };
        run_parallel_range(&engine, &module, count, state, 0, count)?;
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
                let run = move || {
                    run_parallel_range(
                        engine,
                        module,
                        count,
                        ParallelState {
                            inputs,
                            outputs: partition,
                            base,
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
    Ok(RunOutcome::Host(outputs))
}

/// Bring one resident buffer home, naming the position it was read at.
///
/// The count the buffer holds travels with the value, so a fetch asks for what
/// the run actually produced rather than for the device's padded allocation.
fn fetch_resident(resident: ResidentBuffer, position: usize) -> Result<Vec<i64>, String> {
    let Some(backend) = lichen_kernel_ir::parallel_backend() else {
        return Err(format!(
            "input buffer {position} of this run is still on a device, but no compute backend \
             is installed any more, so there is nothing left that can bring it back"
        ));
    };
    backend
        .fetch(resident.id, resident.count)
        .map_err(|reason| {
            format!(
                "the {:?} backend declined to return input buffer {position} (buffer {}): {reason}",
                backend.name(),
                resident.id.0
            )
        })
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
/// `count` is passed whole because `cfg(0)` is the whole launch's count: the
/// index function is a function of the full extent, not of the chunk, so a
/// worker must not see a narrowed one.
fn run_parallel_range(
    engine: &wasmi::Engine,
    module: &wasmi::Module,
    count: usize,
    state: ParallelState<'_>,
    base: usize,
    end: usize,
) -> Result<(), String> {
    let mut store = wasmi::Store::new(engine, state);
    let mut linker = wasmi::Linker::<ParallelState<'_>>::new(engine);

    let read_ty = wasmi::FuncType::new(
        [wasmi::ValType::I64, wasmi::ValType::I64],
        [wasmi::ValType::I64],
    );
    let write_ty = wasmi::FuncType::new(
        [
            wasmi::ValType::I64,
            wasmi::ValType::I64,
            wasmi::ValType::I64,
        ],
        [],
    );
    linker
        .func_new(
            "env",
            "read",
            read_ty,
            |caller: wasmi::Caller<'_, ParallelState<'_>>,
             params: &[wasmi::Val],
             results: &mut [wasmi::Val]| {
                let pos = params.first().and_then(|v| v.i64()).unwrap_or(0) as usize;
                let idx = params.get(1).and_then(|v| v.i64()).unwrap_or(0) as usize;
                // The index is **global** — inputs are never partitioned — so
                // no rebase here; a worker reads the whole input buffer.
                let value = caller
                    .data()
                    .inputs
                    .get(pos)
                    .and_then(|v| v.get(idx))
                    .copied()
                    .unwrap_or(0);
                results[0] = wasmi::Val::I64(value);
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
    linker
        .func_new(
            "env",
            "write",
            write_ty,
            |mut caller: wasmi::Caller<'_, ParallelState<'_>>,
             params: &[wasmi::Val],
             _results: &mut [wasmi::Val]| {
                let out_pos = params.first().and_then(|v| v.i64()).unwrap_or(0) as usize;
                let idx = params.get(1).and_then(|v| v.i64()).unwrap_or(0) as usize;
                let value = params.get(2).and_then(|v| v.i64()).unwrap_or(0);
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

    let instance = linker
        .instantiate_and_start(&mut store, module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_func(&store, "main")
        .ok_or_else(|| "parallel kernel has no export `main`".to_string())?;
    for i in base..end {
        let args = [wasmi::Val::I64(count as i64), wasmi::Val::I64(i as i64)];
        let mut results = [wasmi::Val::I64(0)];
        main.call(&mut store, &args, &mut results)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// --- The parallel run: the worker rule, the partition, the fan-out ---

#[cfg(test)]
mod parallel_launch_tests {
    use super::*;
    use lichen_kernel_ir::ResidentId;

    /// A two-output parallel fragment over `(n, i)`: `out0[i] = i + 1` and
    /// `out1[i] = i + i`, with each `BufferWriteCall` fed the
    /// `[out_pos, idx, val]` stack its host import takes.  The trailing
    /// `Const(0)` is what `compile_parallel_fragment` appends: the index
    /// function only has side effects, and the shared assembler's `-> i64`
    /// signature needs one value left on the stack.
    fn two_outputs() -> KernelFragment {
        KernelFragment {
            param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
            body: vec![
                KernelInstr::Const(0),
                KernelInstr::LocalGet(1),
                KernelInstr::LocalGet(1),
                KernelInstr::Const(1),
                KernelInstr::Bin(KernelBin::Add),
                KernelInstr::BufferWriteCall,
                KernelInstr::Const(1),
                KernelInstr::LocalGet(1),
                KernelInstr::LocalGet(1),
                KernelInstr::LocalGet(1),
                KernelInstr::Bin(KernelBin::Add),
                KernelInstr::BufferWriteCall,
                KernelInstr::Const(0),
            ]
            .into(),
            inputs: 0,
            outputs: 2,
            results: 1,
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
                _fragment: &KernelFragment,
                _inputs: &[BufferSlot],
                _count: usize,
            ) -> Result<Vec<ResidentId>, String> {
                self.seen.fetch_add(1, Ordering::SeqCst);
                Ok(vec![ResidentId(41)])
            }
            fn fetch(&self, _id: ResidentId, count: usize) -> Result<Vec<i64>, String> {
                self.fetched.fetch_add(1, Ordering::SeqCst);
                Ok(vec![7; count])
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
        let outputs =
            run_parallel_kernel(id, Backend::Gpu, 8, vec![]).expect("the installed backend runs");
        lichen_kernel_ir::clear_parallel_backend();

        assert_eq!(
            outputs,
            RunOutcome::Resident(vec![ResidentBuffer {
                id: ResidentId(41),
                count: 8,
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
            _fragment: &KernelFragment,
            inputs: &[BufferSlot],
            _count: usize,
        ) -> Result<Vec<ResidentId>, String> {
            self.seen.lock().unwrap().push(
                inputs
                    .iter()
                    .map(|slot| match slot {
                        BufferSlot::Host(data) => Slot::Host(data.len()),
                        BufferSlot::Resident(id) => Slot::Resident(id.0),
                    })
                    .collect(),
            );
            Ok(vec![ResidentId(41)])
        }
        fn fetch(&self, _id: ResidentId, count: usize) -> Result<Vec<i64>, String> {
            self.fetched.fetch_add(1, Ordering::SeqCst);
            Ok(vec![0; count])
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
            count,
            vec![RunInput::Host((0..count as i64).collect())],
        )
        .expect("the first run completes");
        let RunOutcome::Resident(resident) = first else {
            panic!("a \"gpu\" run leaves its results on the device");
        };
        let second = run_parallel_kernel(
            id,
            Backend::Gpu,
            count,
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
            count,
            vec![RunInput::Resident(ResidentBuffer {
                id: ResidentId(41),
                count,
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
        let refusal = run_parallel_kernel(id, Backend::Gpu, 8, vec![])
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
                _fragment: &KernelFragment,
                _inputs: &[BufferSlot],
                _count: usize,
            ) -> Result<Vec<ResidentId>, String> {
                Err("this device has no compute queue".to_string())
            }

            fn fetch(&self, _id: ResidentId, _count: usize) -> Result<Vec<i64>, String> {
                unreachable!("a run that declined never hands back an id to fetch")
            }

            fn release(&self, _id: ResidentId) {
                unreachable!("a run that declined never hands back an id to release")
            }
        }
        lichen_kernel_ir::install_parallel_backend(std::sync::Arc::new(Stub));
        let id = intern_kernel(two_outputs());
        let refusal =
            run_parallel_kernel(id, Backend::Gpu, 8, vec![]).expect_err("the backend declined");
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
    fn host_outputs(id: KernelId, count: usize) -> Vec<Vec<i64>> {
        match run_parallel_kernel(id, Backend::Cpu, count, vec![]).expect("the run must succeed") {
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
            assert_eq!(outputs[0][index], index as i64 + 1, "out0[{index}]");
            assert_eq!(outputs[1][index], index as i64 * 2, "out1[{index}]");
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
            assert_eq!(outputs[0][index], index as i64 + 1, "out0[{index}]");
            assert_eq!(outputs[1][index], index as i64 * 2, "out1[{index}]");
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
/// it (`.native` = the artifact, `.sig` = `type_of f`).
///
/// The program marker is generic: a host composes this op into its own
/// `NativeOps` registry (a `&'static [(&str, &dyn NativeOp<P>)]`), so the
/// `$jit`/`$launch` names stay private to the plugin's own embedded source.
pub struct JitOp;

/// `$launch(native, sig, a)` — run kernel `native` on `a`.  The signature gate
/// unifies `sig`'s type with a function type, reading the domain/codomain
/// *lazily* out of the signature; the argument is unified against the domain and
/// the result typed as the codomain.
pub struct LaunchOp;

impl<P> NativeOp<P> for JitOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let f = &args[0];
        // Function-ness gate: the argument's type must be a function (an arrow),
        // binding the domain/codomain the signature value carries.
        let d = ctx.fresh();
        let c = ctx.fresh();
        // Built through the highlevel's single arrow construction point: the
        // three nodes it allocates (shape, kind, pair) are the same three this
        // site allocated before, in the same order.
        let fn_ty = ctx.arrow(d, c);
        ctx.check_unify(f.ty, fn_ty, loc, DiagKind::Guard);

        // The bare native kernel artifact — the lichen wrapper wraps this value
        // into a `kernel` struct (`.native`).  It is opaque: its type is a fresh
        // cell (the signature rides in the struct's `.sig` field, not here).
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Jit), Some(f.value));
        let kernel_ty = ctx.fresh();
        // The expression's `term` must evaluate to a `[value, type]` pair, so
        // `value_of` can `Index` it; the op's own result is the bare `Kernel`.
        let pair = ctx.array_node(&[op, kernel_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: kernel_ty,
        }
    }
}

impl<P> NativeOp<P> for LaunchOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator> + From<LowOperator>,
{
    /// `$launch(native, sig, a)` — run kernel `native` on `a`.  The lichen
    /// wrapper extracts `.native` (the bare kernel) and `.sig` (the signature
    /// type) out of the kernel struct; the native op only gates the signature
    /// (a function type, binding the domain/codomain) and the argument, and
    /// never re-parses the struct.
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let native = &args[0];
        let sig = &args[1];
        let a = &args[2];
        // The signature value is a function type `d -> c`: gate it, binding the
        // domain/codomain to *lazy reads* of the signature — `Index(sig.ty, 0)`
        // is the domain/codomain pair, so `d` = element 0 and `c` = element 1.
        // Reading them lazily (rather than a fresh cell) means an unbound
        // signature (the frozen `launch` template's generic kernel) resolves the
        // domain/codomain once a concrete kernel struct binds it at apply time,
        // while a concrete signature resolves them immediately — so the argument
        // gate (`a.ty ~ d`) and the result type (`c`) are both checked against
        // the *actual* signature, not an unbound cell.
        let zero = ctx.value_node(<P::Value as From<LowValue>>::from(LowValue::USize(0)));
        let one = ctx.value_node(<P::Value as From<LowValue>>::from(LowValue::USize(1)));
        let sig_shape_ops = ctx.array_node(&[sig.ty, zero]);
        let sig_shape = ctx.op_node(P::Operator::from(LowOperator::Index), Some(sig_shape_ops));
        let d_ops = ctx.array_node(&[sig_shape, zero]);
        let d = ctx.op_node(P::Operator::from(LowOperator::Index), Some(d_ops));
        let c_ops = ctx.array_node(&[sig_shape, one]);
        let c = ctx.op_node(P::Operator::from(LowOperator::Index), Some(c_ops));
        let fn_ty = ctx.arrow(d, c);
        ctx.check_unify(sig.ty, fn_ty, loc.clone(), DiagKind::Guard);
        // Unify the argument against the kernel's domain.
        ctx.check_unify(a.ty, d, loc.clone(), DiagKind::Guard);
        // Emit the `Launch` operator over `[native, a]`, typed as the codomain.
        // The domain read `d` rides along as a third (inert) operand element so
        // it is reachable from the application's return graph and therefore
        // cloned + resolved at apply time; the operator itself only reads
        // elements 0 and 1.
        let operands = ctx.array_node(&[native.value, a.value, d]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Launch), Some(operands));
        let pair = ctx.array_node(&[op, c]);
        NativeApply {
            node: pair,
            val: None,
            ty: c,
        }
    }
}

/// `$call(k, a)` — a **cross-kernel call** on native kernel `k`.  The lichen
/// wrapper extracts `.native` out of the kernel struct (`call = k => a =>
/// $call(k.native, a)`); the native op only gates the *argument* against a
/// fresh domain cell and types the result as a fresh codomain cell (the callee
/// signature is read at launch-time assembly).  It never re-parses the struct.
pub struct CallOp;

impl<P> NativeOp<P> for CallOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let k = &args[0];
        let a = &args[1];
        let d = ctx.fresh();
        let c = ctx.fresh();
        ctx.check_unify(a.ty, d, loc.clone(), DiagKind::Guard);
        let operands = ctx.array_node(&[k.value, a.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Call), Some(operands));
        let pair = ctx.array_node(&[op, c]);
        NativeApply {
            node: pair,
            val: None,
            ty: c,
        }
    }
}

/// `$parallel(f, backend)` — compile a single-arg `?cfg -> ?write` index function
/// into a parallel kernel, and record the backend its runs are dispatched to.
/// The function-ness gate verifies `f` is a function; the body is lowered over the
/// loop index (from `compute.range n`) and the cfg buffers (read via
/// `compute.read`).  A **tuple** codomain of `Write`s is the multi-output form;
/// which position a write is becomes its output ordinal at emission time, and the
/// codomain's arity becomes the launch's output count.
pub struct ParallelOp;

/// `$plrun(pk, cfg)` — run a parallel kernel over the index range `[0, cfg(0))`
/// with the input buffers from `cfg(1)` fixed, collecting the writes into one
/// `Buffer` per output (a bare `Buffer` for a single output, their tuple for
/// several).  The count is `cfg(0)`.
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
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let f = &args[0];
        let backend = &args[1];
        // Function-ness gate: `f : ?cfg -> ?write` (a single-arg index function;
        // the loop index comes from `compute.range n` inside the body, not a
        // second function parameter).
        let d0 = ctx.fresh();
        let c0 = ctx.fresh();
        let fn_ty = ctx.arrow(d0, c0);
        ctx.check_unify(f.ty, fn_ty, loc.clone(), DiagKind::Guard);
        // The backend is gated as a **string** here, and that is all this gate
        // can be: the language has no enum type yet, so which strings are
        // backends is the runtime parse's authority. Naming the shape here still
        // moves a wrong-shaped argument from a run-time refusal to a check
        // error, and an unknown *name* is reported by that parse, by name.
        ctx.check_unify(backend.ty, ctx.string_type(), loc, DiagKind::Guard);
        // The bare native parallel kernel artifact — the lichen wrapper wraps
        // this value into a `kernel` struct (`.native`).  Opaque: typed `_`.
        let operands = ctx.array_node(&[f.value, backend.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Parallel), Some(operands));
        let par_ty = ctx.fresh();
        let pair = ctx.array_node(&[op, par_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: par_ty,
        }
    }
}

impl<P> NativeOp<P> for ParLaunchOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$plrun(native, sig, a)` — run parallel kernel `native` over `a` (the
    /// `cfg`).  The lichen wrapper extracts `.native`/`.sig` out of the kernel
    /// struct; the native op gates the signature as a single-arg function and
    /// the `cfg` argument against its domain.
    ///
    /// **The result type is a fresh cell**, and that is a deliberate limit, not
    /// an oversight.  The signature's *arity* is what decides the result's
    /// shape — a bare `Buffer` for a one-write index function, a tuple of
    /// buffers for a several-write one — and the arity cannot be read here:
    /// `build` runs once, on the frozen `plrun` template, where `.sig` is an
    /// unbound cell that only resolves at run time.  A tuple type is a value
    /// node with one element per position, so no check-time node can name a
    /// tuple whose arity is not known until the run.  Naming it `[?b,
    /// BufferKind]` instead (what the single-output form used to do) would be
    /// check-time *decided*, and a decided non-positional type is exactly what
    /// the checker's field-read guard refuses — a single output's `out(1)`
    /// would not check at all, which would leave the one-output kernel unable
    /// to be read positionally.
    ///
    /// What the fresh cell costs is **static precision, not safety**: the
    /// element type is no longer named by the signature, so `read` on a `plrun`
    /// result binds its own element cell and resolves it from the value the
    /// run produces (always `Int`, since `WriteOp` pins the written value to
    /// `Int`).  An ordinal that does not exist is still **refused, at check
    /// time, with a span** — the checker's evaluation pass reconciles the
    /// constant index against the tuple the launch produced and records an
    /// out-of-bounds `Index` — so the two shapes stay distinguishable exactly
    /// where it matters.
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let native = &args[0];
        let sig = &args[1];
        let a = &args[2];
        // The signature is a single-arg function `?cfg -> ?codomain`, where the
        // codomain is a `Write` or a tuple of `Write`s.  Both halves are fresh
        // cells: the frozen `$plrun` template's generic kernel binds them when
        // a concrete kernel struct arrives at run time, and the *shape* of the
        // codomain is the emitter's fact to check (it is the one that can count
        // the writes), not this gate's.
        let d0 = ctx.fresh();
        let c0 = ctx.fresh();
        let sig_ty = ctx.arrow(d0, c0);
        ctx.check_unify(sig.ty, sig_ty, loc.clone(), DiagKind::Guard);
        // The argument is the `cfg = (n, (buffer…))`; unify it against the
        // kernel's domain.
        ctx.check_unify(a.ty, d0, loc.clone(), DiagKind::Guard);
        // The result — one `Buffer` or a tuple of them — is a fresh cell; see
        // the note above on why the arity cannot be named here.
        let out_ty = ctx.fresh();
        let operands = ctx.array_node(&[native.value, a.value]);
        let op = ctx.op_node(
            P::Operator::from(ComputeOperator::ParLaunch),
            Some(operands),
        );
        let pair = ctx.array_node(&[op, out_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: out_ty,
        }
    }
}

impl<P> NativeOp<P> for RangeOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$range(n)` — the loop index of the current parallel invocation.  Gate
    /// `n` as `Int`; the operation is kernel-only (lowers to the index param),
    /// so the result type is `Int`.
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let n = &args[0];
        ctx.check_unify(n.ty, ctx.int_type(), loc.clone(), DiagKind::Guard);
        let operands = ctx.array_node(&[n.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Range), Some(operands));
        let pair = ctx.array_node(&[op, ctx.int_type()]);
        NativeApply {
            node: pair,
            val: None,
            ty: ctx.int_type(),
        }
    }
}

impl<P> NativeOp<P> for ReadOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    /// `$read(buf, i)` — read one buffer element.  Buffer gate
    /// `buf : [?b, [TypeBuffer, Type]]` (binding the element type), index gate
    /// `Int`; the result is the element type `?b`.  In-kernel this lowers to the
    /// host `read` import; at the VM it reads a buffer value's element.
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let b = &args[0];
        let i = &args[1];
        let elem = ctx.fresh();
        let buf_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeBuffer,
        ));
        let buf_kind = ctx.kind_expr(buf_marker);
        let buf_ty = ctx.array_node(&[elem, buf_kind]);
        ctx.check_unify(b.ty, buf_ty, loc.clone(), DiagKind::Guard);
        ctx.check_unify(i.ty, ctx.int_type(), loc.clone(), DiagKind::Guard);
        let operands = ctx.array_node(&[b.value, i.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Read), Some(operands));
        let pair = ctx.array_node(&[op, elem]);
        NativeApply {
            node: pair,
            val: None,
            ty: elem,
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
    /// index `i` (length `n`).  Gate `n` and `i` as `Int`, `val` as the element
    /// type `?b`; the result is a `Write` type `[?b, [TypeWrite, Type]]`.
    /// Kernel-only (lowers to the host `write` import).
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let n = &args[0];
        let i = &args[1];
        let val = &args[2];
        ctx.check_unify(n.ty, ctx.int_type(), loc.clone(), DiagKind::Guard);
        ctx.check_unify(i.ty, ctx.int_type(), loc.clone(), DiagKind::Guard);
        // The `Write`'s element type is pinned to the canonical `Int` (v1: the
        // val must be an `Int`), so the kernel signature's codomain carries a
        // *concrete* element type — `compute.read` then returns `Int` and a
        // dependent index function (`a + a`) typechecks.
        ctx.check_unify(val.ty, ctx.int_type(), loc.clone(), DiagKind::Guard);
        let write_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeWrite,
        ));
        let write_kind = ctx.kind_expr(write_marker);
        let write_ty = ctx.array_node(&[ctx.int_type(), write_kind]);
        let operands = ctx.array_node(&[n.value, i.value, val.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Write), Some(operands));
        let pair = ctx.array_node(&[op, write_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: write_ty,
        }
    }
}

impl<P> NativeOp<P> for BufferCollectOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let b = &args[0];
        // Buffer gate: `b : [?b, [TypeBuffer, Type]]`, binding the element type.
        let elem = ctx.fresh();
        let buf_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeBuffer,
        ));
        let buf_kind = ctx.kind_expr(buf_marker);
        let buf_ty = ctx.array_node(&[elem, buf_kind]);
        ctx.check_unify(b.ty, buf_ty, loc.clone(), DiagKind::Guard);
        // Array result type: `[[?b, len], [TypeArray, Type]]` with a fresh
        // length cell (the array's length is a runtime count, so it stays a
        // `?`-length type until observed).
        let len = ctx.fresh();
        let arr_shape = ctx.array_node(&[elem, len]);
        let arr_kind = ctx.kind_expr(ctx.array_type_marker_node());
        let arr_ty = ctx.array_node(&[arr_shape, arr_kind]);
        let operands = ctx.array_node(&[b.value]);
        let op = ctx.op_node(
            P::Operator::from(ComputeOperator::BufferCollect),
            Some(operands),
        );
        let pair = ctx.array_node(&[op, arr_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: arr_ty,
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
    /// The gate is only a function-ness gate. **The arity is deliberately not
    /// checked here**: it is the length of `f`'s parameter tuple, which is a
    /// *runtime* fact of a value the checker has not cloned yet, and a gate that
    /// named an arity it cannot read would refuse programs the recording accepts.
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let f = &args[0];
        let domain = ctx.fresh();
        let codomain = ctx.fresh();
        let fn_ty = ctx.arrow(domain, codomain);
        ctx.check_unify(f.ty, fn_ty, loc, DiagKind::Guard);
        // The graph itself is host-owned and opaque: what a graph may be run over
        // is a question about the function it was recorded from, and that
        // function is not available to the checker — a run is what finds out.
        let graph_ty = ctx.fresh();
        let operands = ctx.array_node(&[f.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Graph), Some(operands));
        let pair = ctx.array_node(&[op, graph_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: graph_ty,
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
    /// **The arguments unify against a fresh cell, and that is what makes a graph
    /// reusable across runs.** A graph's parameter tuple is a *runtime* shape —
    /// how many arguments it takes is the length of the tuple the recording read
    /// off a function value — so a fixed domain type would name an arity the
    /// checker cannot know, and would refuse exactly the programs a recording
    /// accepts. The result is a fresh cell for the same reason `plrun`'s is.
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let g = &args[0];
        let a = &args[1];
        let graph_ty = ctx.fresh();
        ctx.check_unify(g.ty, graph_ty, loc.clone(), DiagKind::Guard);
        let arguments_ty = ctx.fresh();
        ctx.check_unify(a.ty, arguments_ty, loc, DiagKind::Guard);
        let out_ty = ctx.fresh();
        let operands = ctx.array_node(&[g.value, a.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::GraphRun), Some(operands));
        let pair = ctx.array_node(&[op, out_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: out_ty,
        }
    }
}

/// The `lichen-compute` plugin's embedded lichen source — the real `compute`
/// plugin file, kept as a `.lichen` source file and embedded with
/// [`include_str!`].  It defines the user-facing `jit`/`launch` functions as
/// ordinary typed lichen (whose bodies call the native `$jit`/`$launch`), and
/// exports them as a **named struct** (`compute.jit`, `compute.launch`).
pub const WRAPPER_SOURCE: &str = include_str!("compute.lichen");
