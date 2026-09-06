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
//! and wires the plugin's native registry itself (see the `liche-language`
//! crate's `package.rs`).
//!
//! ## Type-checking coverage
//!
//! - `jit f` requires `f` to be a *function* (function-ness gate) and wraps the
//!   bare artifact into a kernel struct `struct<.native _, .sig (type_of f)>`.
//! - `launch k a` reads `k.native`/`k.sig`, gates the `.sig` (a function type,
//!   binding the domain/codomain lazily), unifies `a` against the domain, and
//!   its result is the kernel's codomain — a function-style apply over a kernel.
//! - `parallel f` lifts a single-arg `?cfg -> Write` index function into a
//!   parallel kernel struct (`cfg = (n, (buffer…))` — the count is `cfg(0)`,
//!   the input buffers a tuple at `cfg(1)`); `plrun k cfg` runs it over
//!   `[0, cfg(0))` into a `Buffer`, the index function reading inputs via
//!   `compute.read [cfg(1)(k), i]` and writing via `compute.write [n, i, val]`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp};
use lichen_highlevel::program::{Ctx, HighProgram, TypeOperator, ValueType};
use lichen_lowlevel::codec::{OperatorCodec, Reader, ValueCodec, Writer};
use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, ArrayItem, BlockId, LowOperator, LowShape, LowValue, Module,
    ModuleKey, NodeId, OperatorExt, Program, StaticModule,
};
use lichen_utils::extend::AsEnum;

/// The program-generic bounds the kernel-safe JIT requires.
///
/// A value vocabulary that composes [`ComputeValue`] (so a `Kernel` value
/// fits as a sibling leaf) and an operator vocabulary that composes the
/// structural [`LowOperator`], the highlevel's [`TypeOperator`] (the scalar
/// arithmetic the kernel-safe subset lowers), and [`ComputeOperator`]
/// (`Jit`/`Launch`).
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
pub type KernelId = usize;

/// A runtime parallel-buffer artifact's identity — a compact index into the
/// process buffer registry (the `n` collected element results).  Like
/// [`KernelId`], it is a `Copy` host-owned scalar, never an arena payload.
pub type BufferId = usize;

/// The process kernel registry: compiled kernel **fragments** (bytecode units),
/// keyed by [`KernelId`].  Kernels are immutable artifacts shared across
/// modules in the process.  The fragment is the durable JIT output; the module
/// bytes are derived on demand by [`assemble_module`] at launch.
static KERNELS: OnceLock<Mutex<HashMap<KernelId, KernelFragment>>> = OnceLock::new();
fn kernels() -> &'static Mutex<HashMap<KernelId, KernelFragment>> {
    KERNELS.get_or_init(Default::default)
}
/// The next kernel id — process-global, so ids never collide across modules.
static NEXT_KERNEL_ID: AtomicUsize = AtomicUsize::new(0);
fn alloc_kernel_id() -> KernelId {
    NEXT_KERNEL_ID.fetch_add(1, Ordering::Relaxed)
}

/// The process buffer registry: the element results a `plrun` collected,
/// keyed by [`BufferId`].  A buffer is an immutable, host-owned vector of
/// scalar `i64` results (the `?b` values of a `?a -> USize -> ?b` kernel).
/// It lives process-global like the kernel registry, so a `Buffer` value is a
/// small `Copy` scalar and reads/collects stay arena-free.
static BUFFERS: OnceLock<Mutex<HashMap<BufferId, Vec<i64>>>> = OnceLock::new();
fn buffers() -> &'static Mutex<HashMap<BufferId, Vec<i64>>> {
    BUFFERS.get_or_init(Default::default)
}
/// The next buffer id — process-global, so ids never collide across modules.
static NEXT_BUFFER_ID: AtomicUsize = AtomicUsize::new(0);
fn alloc_buffer_id() -> BufferId {
    NEXT_BUFFER_ID.fetch_add(1, Ordering::Relaxed)
}

/// The element results of a buffer — a cloned snapshot, so the caller does not
/// hold the buffer registry lock while materializing nodes.
fn buffer_results(id: BufferId) -> Option<Vec<i64>> {
    buffers().lock().unwrap().get(&id).cloned()
}

/// A binary arithmetic/comparison operator of the kernel-safe subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KernelBin {
    Add,
    Sub,
    Leq,
    Eq,
}

/// One abstract instruction in a lowered kernel body.
///
/// A [`KernelFragment`] stores a `Vec<KernelInstr>` — **not** raw wasm — so the
/// launcher can lower cross-kernel calls with indices resolved *after* the
/// kernel's relative launch set is laid out (the deferred-linker condition
/// style-2 `k x` calls need).  Style-1 inline lichen-function calls and the
/// scalar-arithmetic subset lower directly; a later variant carries a
/// cross-kernel call by callee [`KernelId`].
#[derive(Debug, Clone)]
enum KernelInstr {
    /// Push an `i64` constant.
    Const(i64),
    /// A binary `add/sub/leq/eq` over the top two stack values.
    Bin(KernelBin),
    /// Read a parameter local (a flattened scalar offset in the domain).
    LocalGet(u32),
    /// Convert the top stack value `i64 -> i32` (a `select` condition).
    I32WrapI64,
    /// A `if c then a else b` — emitted as then/else values, the selector,
    /// `I32WrapI64`, then this `select`.
    Select,
    /// A cross-kernel call (style 2): the top `arity` stack values are the
    /// argument; the caller's launch-time assembler resolves this to an
    /// in-module `call` to the callee kernel's assembled function index.
    CallKernel(KernelId),
    /// Call the host `read(cfg_pos, idx)` import — the stack holds
    /// `[cfg_pos, idx]`.
    BufferReadCall,
    /// Call the host `write(out_pos, idx, val)` import — the stack holds
    /// `[out_pos, idx, val]`.
    BufferWriteCall,
}

/// A compiled kernel-callable unit — the JIT's **bytecode** output, not a
/// module.
///
/// `jit` lowers one lichen function to a [`KernelFragment`]: the function's
/// body as abstract instructions, plus the domain shape signature a linker
/// needs.  The fragment is stored (not the whole module); `launch` assembles
/// the reachable fragment set into one module ([`assemble_module`]) and runs
/// it.  Splitting "emit bytecode" from "assemble a module" is what lets a
/// later step link many fragments together (helper sharing, recursion) and
/// emit cross-module imports for callees compiled elsewhere.
#[derive(Debug, Clone)]
struct KernelFragment {
    /// The parameter domain shape — the wasm parameter types and the layout
    /// the body emitter used for parameter reads.
    param_shape: LowShape,
    /// The lowered function body as abstract instructions.  The launcher
    /// lowers these to wasm with any cross-kernel call indices resolved.
    body: Vec<KernelInstr>,
}

/// The compute value vocabulary — injected as a sibling leaf into a host's
/// value union (see a host `program` module).  A plain enum of exactly this
/// extension's variants, composed with [`lichen_utils::enum_ext!`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComputeValue {
    /// A compiled, runnable kernel artifact.
    Kernel(KernelId),
    /// A compiled **parallel** kernel artifact: a curried `?a -> USize -> ?b`
    /// function flattened to a `(?a, USize) -> ?b` wasm function (the config
    /// is the first group of parameters, the *index* the last scalar).
    ParKernel(KernelId),
    /// A runtime results **buffer**: `plrun` ran the parallel kernel over the
    /// index range `[0, n)` and collected the `n` `?b` results here, host-owned
    /// (a `Copy` scalar into the process buffer registry, exactly like
    /// [`Kernel`]'s [`KernelId`]).
    Buffer(BufferId),
    /// The kind marker of buffer types — a buffer's type is
    /// `[element_type, [TypeBuffer, Type]]`.
    TypeBuffer,
    /// The kind marker of write types — a `Write`'s type is
    /// `[element_type, [TypeWrite, Type]]`.
    TypeWrite,
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
    /// (the kernel body is lowered over the loop index) → a `ParKernel` value.
    Parallel,
    /// `[parallel_kernel, cfg]` operand — run the parallel kernel over the
    /// index range `[0, cfg(0))` (the count is fixed at cfg position 0) →
    /// a `Buffer` value.
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
}

// --- the compute leaves' per-leaf artifact codec ----------------------------
//
// A kernel/par-kernel/buffer value and every compute operator are **runtime
// only**: they are process-local registry handles/operations with no stable
// on-disk identity, so a persistent artifact must never carry them (a frozen
// module is a *type* artifact, not a runnable kernel).  `TypeBuffer` is a pure
// type-constant marker and is serializable like the other kind markers.  The
// panic arms keep the byte format total while staying honest: a compute value
// in a frozen module is an invariant violation.

impl ValueCodec for ComputeValue {
    fn write_value<P: Program>(
        w: &mut Writer,
        value: Self,
        _modules: &HashMap<ModuleKey, Arc<StaticModule<P>>>,
    ) {
        match value {
            ComputeValue::TypeBuffer | ComputeValue::TypeWrite => w.u8(0),
            ComputeValue::Kernel(_) | ComputeValue::ParKernel(_) | ComputeValue::Buffer(_) => {
                panic!("serializing a compute value (Kernel/ParKernel/Buffer are runtime-only)")
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
        Ok(match r.u8()? {
            0 => ComputeValue::TypeBuffer,
            1 => ComputeValue::TypeWrite,
            tag => return Err(format!("unknown compute-value tag {tag}")),
        })
    }
}

impl OperatorCodec for ComputeOperator {
    fn write_operator(_w: &mut Writer, op: Self) {
        match op {
            ComputeOperator::Jit
            | ComputeOperator::Launch
            | ComputeOperator::Call
            | ComputeOperator::Parallel
            | ComputeOperator::ParLaunch
            | ComputeOperator::Range
            | ComputeOperator::Read
            | ComputeOperator::Write
            | ComputeOperator::BufferCollect => {
                panic!("serializing a compute operator (Jit/Launch/... are runtime-only)")
            }
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
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> P::Value {
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
                        let id = alloc_kernel_id();
                        kernels().lock().unwrap().insert(id, fragment);
                        <P::Value as From<ComputeValue>>::from(ComputeValue::Kernel(id))
                    }
                    Err(err) => {
                        // The body uses an operator outside the kernel-safe
                        // subset — record nothing and stay lazy; the definition
                        // pass's error channel reports the unbound result.
                        let _ = err;
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
                let operands = operands.items();
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
                // the unbound result.
                let mut args: Vec<i64> = Vec::new();
                match module
                    .node_value(operands[1].node)
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                {
                    Some(LowValue::USize(n)) => args.push(n as i64),
                    Some(LowValue::Array(_)) => {
                        if !collect_args(module, operands[1].node, &mut args) {
                            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                        }
                    }
                    _ => return <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                };
                match run_kernel(id, &args) {
                    Ok(result) => <P::Value as From<LowValue>>::from(LowValue::USize(result)),
                    Err(..) => <P::Value as From<LowValue>>::from(LowValue::Parameterized),
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
                let operands = operands.items();
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
                        if !collect_args(module, operands[1].node, &mut args) {
                            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                        }
                    }
                    _ => return <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                };
                match run_kernel(id, &args) {
                    Ok(result) => <P::Value as From<LowValue>>::from(LowValue::USize(result)),
                    Err(..) => <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                }
            }
            ComputeOperator::Parallel => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Function(function)) = AsEnum::<LowValue>::as_enum(&operand)
                else {
                    // A non-function parallel target is the checker's
                    // function-ness gate; stay lazy rather than panicking.
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                match compile_parallel_fragment(module, function) {
                    Ok(fragment) => {
                        let id = alloc_kernel_id();
                        kernels().lock().unwrap().insert(id, fragment);
                        <P::Value as From<ComputeValue>>::from(ComputeValue::ParKernel(id))
                    }
                    Err(err) => {
                        let _ = err;
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
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("ParLaunch expects an operand array of [kernel, cfg]")
                };
                let operands = operands.items();
                let Some(ComputeValue::ParKernel(id)) = module
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
                let Some(cfg_items) = module.array_items(cfg_node) else {
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
                let mut inputs: Vec<Vec<i64>> = Vec::new();
                if let Some(buf_tuple) = cfg_items.get(1)
                    && let Ok(buf_tuple_node) = dyn_node(buf_tuple.node)
                    && let Some(buf_items) = module.array_items(buf_tuple_node)
                {
                    for item in buf_items {
                        match module
                            .node_value(item.node)
                            .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                        {
                            Some(ComputeValue::Buffer(bid)) => {
                                if let Some(data) = buffer_results(bid) {
                                    inputs.push(data);
                                } else {
                                    return <P::Value as From<LowValue>>::from(
                                        LowValue::Parameterized,
                                    );
                                }
                            }
                            _ => {
                                return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                            }
                        }
                    }
                }
                match run_parallel_kernel(id, count, inputs) {
                    Ok(results) => {
                        let bid = alloc_buffer_id();
                        buffers().lock().unwrap().insert(bid, results);
                        <P::Value as From<ComputeValue>>::from(ComputeValue::Buffer(bid))
                    }
                    Err(..) => <P::Value as From<LowValue>>::from(LowValue::Parameterized),
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
                let operands = operands.items();
                let Some(ComputeValue::Buffer(id)) = module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                let index = match module
                    .node_value(operands[1].node)
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                {
                    Some(LowValue::USize(n)) => n,
                    _ => return <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                };
                let results = buffers().lock().unwrap();
                match results.get(&id).and_then(|v| v.get(index)) {
                    Some(&value) => {
                        <P::Value as From<LowValue>>::from(LowValue::USize(value as usize))
                    }
                    None => <P::Value as From<LowValue>>::from(LowValue::Parameterized),
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
                let operands = operands.items();
                let Some(ComputeValue::Buffer(id)) = module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                let results = buffer_results(id);
                let Some(results) = results else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                // Materialize each element as a fresh scalar node and build a
                // real lichen array value over them, so `collect` yields an
                // ordinary array the user can index/treat as `Int<n>`.
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
        if let Some(path) = param_path(module, slot.pair, node) {
            if let Ok(offset) = flatten_offset(&slot.shape, &path) {
                return Some((slot.base + offset) as u32);
            }
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
/// The parameter's domain shape (scalar vs tuple of scalars) is derived from
/// its *type* and recorded on the parameter's value cell — the level-3 shape
/// marker a backend reads instead of re-deriving the type half.  The body
/// emitter then reads that shape to distinguish a scalar parameter read
/// (`local.get 0`) from a tuple-element read (`local.get k`).
fn compile_fragment<P>(
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
    let param_pair = module.functions[fid].parameter;
    let ret = module.functions[fid].r#return;
    // The function's `return` is the `[value, type]` pair node; the kernel's
    // result is the pair's *value* (element 0).  A body whose return is a bare
    // value node (the checker leaves a direct kernel-apply's codomain unbound,
    // so it stores the body's value node directly instead of a pair) is used
    // as the value itself.
    let ret_value = match module.array_items(ret) {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => ret,
    };

    // The domain shape from the parameter's type cell.
    let param_shape = kernel_param_shape(module, param_pair)?;
    match &param_shape {
        LowShape::USize | LowShape::Tuple(_) => {}
        _ => {
            return Err("kernel domain must be a scalar or a tuple of scalars".into());
        }
    }
    // The parameter's value cell (element 0 of the `[value, type]` pair) is
    // where the domain shape is stored — the node the body emitter consults.
    let param_value = match module
        .array_items(param_pair)
        .and_then(|items| items.first())
    {
        Some(first) => dyn_node(first.node)?,
        None => return Err("parameter is not a [value, type] pair".into()),
    };
    module.set_node_shape(param_value, Some(param_shape.clone()));

    let params = vec![ParamSlot {
        pair: param_pair,
        value: param_value,
        shape: param_shape.clone(),
        base: 0,
    }];

    let mut body: Vec<KernelInstr> = Vec::new();
    emit_node(module, &params, ret_value, &mut body)?;

    Ok(KernelFragment { param_shape, body })
}

/// Lower a single-arg `?cfg -> ?write` index function into a **parallel
/// kernel** — a [`KernelFragment`] whose wasm signature is `(n, index)` (the
/// count scalar from `cfg(0)`, then the loop index) and whose body is the
/// index function's body traced with `range`/`read`/`write` host calls.
///
/// `parallel` is the data-parallel lift: running it over the index range
/// `[0, cfg(0))` computes the index function once per index.  The `cfg` is
/// `(n, (buffer…))` — `cfg(0)` is the count `n` (a wasm scalar param), and the
/// buffer tuple at `cfg(1)` is host-side (each buffer read via a `read`
/// import by its position inside the tuple).  The loop index comes from
/// `compute.range n` (a kernel-only op yielding the index param).
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
    // The kernel's result is the index function's body value (a `Write`, v1
    // single output), through a `[value, type]` pair or a bare value node.
    let ret_value = match module.array_items(body) {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => body,
    };
    // `cfg = (n, (buffer…))`.  `cfg(0)` is the scalar count `n` — the only
    // scalar wasm param from cfg; the buffer tuple is host-side (read via the
    // `read` import by its position in `cfg(1)`).  Model the cfg's scalar part
    // as a `Tuple([USize])` so the emitter maps `cfg(0)` → `local.get 0`.
    let cfg_value = pair_value_node(module, cfg_pair)
        .ok_or_else(|| "parallel cfg parameter is not a [value, type] pair".to_string())?;
    let cfg_shape = LowShape::Tuple(vec![LowShape::USize]);
    module.set_node_shape(cfg_value, Some(cfg_shape.clone()));
    let params = vec![ParamSlot {
        pair: cfg_pair,
        value: cfg_value,
        shape: cfg_shape,
        base: 0,
    }];
    let mut body_instr: Vec<KernelInstr> = Vec::new();
    emit_node(module, &params, ret_value, &mut body_instr)?;
    // The index function writes into the output buffer (side effects); leave a
    // dummy scalar on the stack so the shared `assemble_module`'s `-> i64`
    // signature holds for the write-only kernel.
    body_instr.push(KernelInstr::Const(0));
    Ok(KernelFragment {
        param_shape: LowShape::Tuple(vec![LowShape::USize, LowShape::USize]),
        body: body_instr,
    })
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
        f.body.iter().any(|i| {
            matches!(
                i,
                KernelInstr::BufferReadCall | KernelInstr::BufferWriteCall
            )
        })
    });
    let base: u32 = if uses_imports { 2 } else { 0 };

    // Type section: the import signatures (if any), then one
    // `(param-arity) -> i64` signature per distinct arity.
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
    let mut type_index_by_arity: HashMap<usize, u32> = HashMap::new();
    let mut func_types: Vec<u32> = Vec::with_capacity(ordered.len());
    for frag in ordered {
        let arity = flat_arity(&frag.param_shape);
        let ti = *type_index_by_arity.entry(arity).or_insert_with(|| {
            let id = types.len();
            types
                .ty()
                .function(vec![ValType::I64; arity], vec![ValType::I64]);
            id
        });
        func_types.push(ti);
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
        lower_body(&frag.body, index, base, &mut body)?;
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    wasm.section(&code);
    Ok(wasm.finish())
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
                KernelBin::Leq => {
                    out.instruction(&Instruction::I64LeS);
                    out.instruction(&Instruction::I64ExtendI32U);
                }
                KernelBin::Eq => {
                    out.instruction(&Instruction::I64Eq);
                    out.instruction(&Instruction::I64ExtendI32U);
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

/// The [`LowShape`] of a function's parameter, from its type cell: a tuple
/// parameter `(T0, .., Tn)` yields `Tuple(..)` with arity `n + 1`; an
/// (annotated or unannotated) scalar `Int` yields `USize`.  This is the one
/// place the codegen reads the type half — once, to seed the parameter's
/// shape marker; afterwards the body emitter reads only [`LowShape`]s.
fn kernel_param_shape<P>(module: &Module<P>, param_pair: NodeId) -> Result<LowShape, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let pair = module
        .array_items(param_pair)
        .ok_or_else(|| "parameter is not a [value, type] pair".to_string())?;
    let Some(type_cell) = pair.get(1) else {
        return Ok(LowShape::USize);
    };
    element_shape(module, type_cell.node)
}

/// The [`LowShape`] of a type value node — recursive, so a tuple whose
/// element is itself a tuple yields a nested [`LowShape::Tuple`].  A type's
/// value is `[shape, kind]`; a tuple type's `shape` is an array of element
/// types (each recursed), a scalar `Int`'s `shape` is the `Int` marker (a
/// leaf → `USize`).  The `_` fallback keeps an unannotated `x => x + 1`
/// compiling (its type cell ends up `[Int, k]`, element 0 a leaf).
fn element_shape<P>(module: &Module<P>, type_node: AnyNodeId) -> Result<LowShape, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let type_node = dyn_node(type_node)?;
    let Some(type_items) = module.array_items(type_node) else {
        return Ok(LowShape::USize);
    };
    if type_items.len() < 2 {
        return Ok(LowShape::USize);
    }
    let shape = dyn_node(type_items[0].node)?;
    match module
        .node_value(AnyNodeId::Dynamic(shape))
        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
    {
        Some(LowValue::Array(shape_array)) => {
            let mut items = Vec::with_capacity(shape_array.items().len());
            for item in shape_array.items() {
                items.push(element_shape(module, item.node)?);
            }
            Ok(LowShape::Tuple(items))
        }
        _ => Ok(LowShape::USize),
    }
}

/// The number of scalar `i64` locals a domain shape flattens to — the wasm
/// parameter count.  A scalar is one local; a tuple is the sum of its
/// elements' arities (so `((Int,Int), Int)` is `1 + 1 + 1 = 3`).
fn flat_arity(shape: &LowShape) -> usize {
    match shape {
        LowShape::USize => 1,
        LowShape::Tuple(items) => items.iter().map(flat_arity).sum(),
        LowShape::Array(_, _) | LowShape::Function(..) | LowShape::Table(..) => 1,
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
    while let Some(parent) = module.nodes[root].equality.parent {
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
fn class_computation_node<P>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let root = equality_rep(module, node);
    for (n, nd) in &module.nodes {
        if equality_rep(module, n) != root {
            continue;
        }
        if let Some(op) = nd.operation.as_ref() {
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&op.operator),
                Some(LowOperator::Index)
            ) {
                return Some(n);
            }
        }
    }
    None
}

/// Emit wasm instructions for one lichen graph node — the scalar kernel-safe
/// subset: integer constants, `Add`/`Sub`/`Leq`/`Eq`, and parameter reads
/// (`Index(param_pair, 0)` → `local.get k`).  `params` is the kernel's
/// parameter-slot list (one for a scalar `jit` kernel, two — config then index
/// — for a parallel kernel).
fn emit_node<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    node: NodeId,
    body: &mut Vec<KernelInstr>,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    if let Some(value) = module.node_value(AnyNodeId::Dynamic(node)) {
        match AsEnum::<LowValue>::as_enum(&value) {
            Some(LowValue::USize(n)) => {
                body.push(KernelInstr::Const(n as i64));
                return Ok(());
            }
            _ => {}
        }
    }
    let Some(operation) = module.nodes[node].operation else {
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
            return emit_node(module, params, definer, body);
        }
        return Err(format!(
            "kernel body hits a node with neither value nor operation (node={node:?})"
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
                if usize_value(module, index) == Some(0) {
                    if let Some(value_node) = value_of_node(module, node) {
                        return emit_node(module, params, value_node, body);
                    }
                }
                // A constant index into a concrete array value selects that
                // element — the wrapper's slot-read destructuring
                // (`read [a, b]` → `x(0)/x(1)`, `write [a, b, c]` →
                // `x(0)/x(1)/x(2)`) leaves `Index(arg_array, k)` ops whose
                // target is a materialized array value.  `value_of` above only
                // peels index 0, so handle every constant `k` here.
                if let Some(k) = usize_value(module, index) {
                    if let Some(array_value) = value_of_node(module, target).or(Some(target)) {
                        if let Some(items) = module.array_items(array_value) {
                            if let Some(item) = items.get(k) {
                                return emit_node(module, params, dyn_node(item.node)?, body);
                            }
                        }
                    }
                }
                // A conditional `if c then a else b` lowers to `[b, a][c]` — a
                // 2-element array value indexed by a *computed* (non-constant)
                // selector, a wasm `select`.  The array may be reached through
                // a value_of extraction; look through it.
                if usize_value(module, index).is_none() {
                    if let Some(array_value) = value_of_node(module, target).or(Some(target)) {
                        if let Some(items) = module.array_items(array_value) {
                            if items.len() == 2 {
                                let then_node = dyn_node(items[1].node)?;
                                let else_node = dyn_node(items[0].node)?;
                                emit_node(module, params, then_node, body)?;
                                emit_node(module, params, else_node, body)?;
                                emit_node(module, params, index, body)?;
                                body.push(KernelInstr::I32WrapI64);
                                body.push(KernelInstr::Select);
                                return Ok(());
                            }
                        }
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
                    return emit_cross_kernel_call(module, params, callee, arg, body);
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
    // The highlevel's type-level arithmetic: `Add`/`Sub`/`Leq`/`Eq` over
    // `[left, right]`.
    if let Some(ty_op) = AsEnum::<TypeOperator>::as_enum(op) {
        match ty_op {
            TypeOperator::Add | TypeOperator::Sub | TypeOperator::Leq | TypeOperator::Eq => {
                let (left, right) = operand_pair(module, operation.operand)?;
                emit_node(module, params, left, body)?;
                emit_node(module, params, right, body)?;
                let bin = match ty_op {
                    TypeOperator::Add => KernelBin::Add,
                    TypeOperator::Sub => KernelBin::Sub,
                    TypeOperator::Leq => KernelBin::Leq,
                    TypeOperator::Eq => KernelBin::Eq,
                    _ => unreachable!(),
                };
                body.push(KernelInstr::Bin(bin));
                return Ok(());
            }
            _ => {
                return Err(format!(
                    "unsupported highlevel operator in kernel body: {ty_op:?}"
                ));
            }
        }
    }
    // The compute plugin's own operators: `Launch`/`Call` inside a kernel body
    // are the wrapper cross-kernel call forms (`compute.launch k x` /
    // `call k x`, which lower to a cross-kernel `CallKernel`).
    if let Some(compute_op) = AsEnum::<ComputeOperator>::as_enum(op) {
        match compute_op {
            ComputeOperator::Launch | ComputeOperator::Call => {
                let (kernel, arg) = apply_pair(module, operation.operand)?;
                return emit_cross_kernel_call(module, params, kernel, arg, body);
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
                    let target_oi = match module.nodes[buf].operation.as_ref() {
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
                    let Some(items) = module.array_items(array_value) else {
                        break;
                    };
                    let Some(item) = items.get(k) else { break };
                    buf = dyn_node(item.node)?;
                }
                let pos = parallel_buffer_pos(module, params, buf).ok_or_else(|| {
                    "read's buffer argument is not a cfg buffer tuple slot (cfg(1)(k))".to_string()
                })?;
                body.push(KernelInstr::Const(pos as i64));
                emit_node(module, params, idx, body)?;
                body.push(KernelInstr::BufferReadCall);
                return Ok(());
            }
            // A pending write: `write [n, idx, val]` → the host
            // `write(out_pos=0, idx, val)` import (v1 single output buffer).
            ComputeOperator::Write => {
                let operand = operation
                    .operand
                    .ok_or_else(|| "write operand array is missing".to_string())?;
                let items = operand_items(module, operand)?;
                let idx = dyn_node(items[1].node)?;
                let val = dyn_node(items[2].node)?;
                body.push(KernelInstr::Const(0)); // out_pos
                emit_node(module, params, idx, body)?;
                emit_node(module, params, val, body)?;
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
    let operation = module.nodes[node].operation?;
    if !matches!(
        AsEnum::<LowOperator>::as_enum(&operation.operator),
        Some(LowOperator::Index)
    ) {
        return None;
    }
    let (target, index) = operand_pair(module, operation.operand).ok()?;
    let k = usize_value(module, index)?;
    let target_op = module.nodes[target].operation?;
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

/// Emit a cross-kernel call (style 2): the (scalar) argument expression, then
/// a [`KernelInstr::CallKernel`] the launch-time assembler resolves.  Both a
/// direct kernel `Apply` (`k x`) and the wrapper's `launch`/`$launch`
/// (`compute.launch k x`) lower here — the latter is the typed form (its
/// codomain is resolved by [`LaunchOp`]), the former the untyped-form gap.
fn emit_cross_kernel_call<P>(
    module: &Module<P>,
    params: &[ParamSlot],
    kernel: NodeId,
    arg: NodeId,
    body: &mut Vec<KernelInstr>,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let kid = kernel_id_of(module, kernel)
        .ok_or_else(|| "cross-kernel call target is not a kernel value".to_string())?;
    // v1 restricts the callee domain to a scalar (arity 1): the argument is one
    // i64 on the stack.
    let arity = kernels()
        .lock()
        .unwrap()
        .get(&kid)
        .map(|f| flat_arity(&f.param_shape))
        .ok_or_else(|| "cross-kernel callee is not a registered kernel".to_string())?;
    if arity != 1 {
        return Err("cross-kernel call supports only a scalar-domain callee in v1".into());
    }
    let arg = pair_value_node(module, arg).unwrap_or(arg);
    emit_node(module, params, arg, body)?;
    body.push(KernelInstr::CallKernel(kid));
    Ok(())
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
    let Some(operation) = module.nodes[node].operation else {
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
    let operation = module.nodes[node].operation?;
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
    if let Some(items) = module.array_items(target) {
        return dyn_node(items.first()?.node).ok();
    }
    // An *operator* node as the target — e.g. `Index(apply_op, 0)` where the
    // checker peels a call result (`value_of` over an `Apply` expression).  The
    // operator's result is the pair's value, so emit the operator directly; its
    // codegen produces the scalar (a cross-kernel call, an arithmetic op, ...).
    if module.nodes[target].operation.is_some() {
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
    if let Some(value) = module.node_value(AnyNodeId::Dynamic(node)) {
        if let Some(ComputeValue::Kernel(kid)) = AsEnum::<ComputeValue>::as_enum(&value) {
            return Some(kid);
        }
    }
    if let Some(inner) = value_of_node(module, node) {
        if let Some(kid) = kernel_id_of(module, inner) {
            return Some(kid);
        }
    }
    // A kernel *struct value* `[native, sig]` reached by value (not through an
    // `Index` op): its element 0 is the bare `.native` kernel artifact.
    if let Some(items) = module.array_items(node)
        && let Some(first) = items.first()
        && let Ok(first) = dyn_node(first.node)
        && let Some(kid) = kernel_id_of(module, first)
    {
        return Some(kid);
    }
    // A kernel struct `.native` field read: `Index(struct, 0)`, where the
    // struct value's element 0 is the bare kernel artifact.
    if let Some(operation) = module.nodes[node].operation {
        if let Some(LowOperator::Index) = AsEnum::<LowOperator>::as_enum(&operation.operator)
            && let Ok((target, index)) = operand_pair(module, operation.operand)
            && usize_value(module, index) == Some(0)
            && let Some(items) = module.array_items(target)
            && let Ok(first) = dyn_node(items.first()?.node)
        {
            return kernel_id_of(module, first);
        }
    }
    None
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
    let items = module.array_items(node)?;
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
    let operation = module.nodes[node].operation?;
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

/// Flatten a kernel argument value (a scalar `USize` leaf, or a possibly
/// nested `Array` of them, as a tuple-of-tuples domain needs) into the wasm
/// argument vector.  Returns `false` if any element is not a scalar `USize`
/// leaf — the definition pass reports the unbound result.
fn collect_args<P>(module: &Module<P>, node: AnyNodeId, out: &mut Vec<i64>) -> bool
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
            true
        }
        Some(LowValue::Array(arr)) => {
            for item in arr.items() {
                if !collect_args(module, item.node, out) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
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
    module
        .array_items(node)
        .ok_or_else(|| "operand is not an array value".into())
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

fn dyn_node(id: AnyNodeId) -> Result<NodeId, String> {
    match id {
        AnyNodeId::Dynamic(n) => Ok(n),
        AnyNodeId::Static(_) => Err("static refs are not kernel-compilable v1".into()),
    }
}

/// Execute a compiled kernel on an argument vector with wasmi, returning the
/// `usize` result.  The dynamic [`wasmi::Func::call`] API accepts any number of
/// `i64` inputs, so a tuple-domain kernel (arity N) launches with N arguments
/// and a scalar kernel (arity 1) with one.
///
/// The kernel's **relative launch set** — the kernel itself plus every kernel
/// it (transitively) cross-calls, discovered by scanning each fragment's
/// cross-kernel instructions — is assembled into one wasm module (launch-time
/// assembly, the deferred linker), the root exported as `main`.
fn run_kernel(id: KernelId, args: &[i64]) -> Result<usize, String> {
    // Discover the relative kernel set in BFS order: `ordered[i]` becomes wasm
    // function index `i`; `index` maps a callee kernel-id to that index.
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
        for instr in &frag.body {
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

    let bytes = assemble_module(&ordered, &index)?;
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes).map_err(|e| e.to_string())?;
    let mut store = wasmi::Store::new(&engine, ());
    let linker = wasmi::Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_func(&store, "main")
        .ok_or_else(|| "kernel has no export `main`".to_string())?;
    let inputs: Vec<wasmi::Val> = args.iter().map(|&a| wasmi::Val::I64(a as i64)).collect();
    let mut outputs = [wasmi::Val::I64(0)];
    main.call(&mut store, &inputs, &mut outputs)
        .map_err(|e| e.to_string())?;
    let result = outputs[0]
        .i64()
        .ok_or_else(|| "kernel `main` returned a non-i64".to_string())?;
    Ok(result as usize)
}

/// The execution state a parallel kernel's host imports read/write against:
/// the input buffers (indexed by cfg position) and the single output buffer
/// (indexed by element).  Carried as the wasmi [`wasmi::Store`] data, so the
/// `read`/`write` imports reach it through `Caller::data`/`data_mut`.
struct ParallelState {
    /// The input buffers (the cfg buffer tuple), indexed by cfg position.
    inputs: Vec<Vec<i64>>,
    /// The output buffer being written (v1: a single buffer, length `count`).
    output: Vec<i64>,
}

/// Run a **parallel** kernel over the index range `[0, count)`, computing the
/// index function once per index with `cfg(0) = count` and the cfg input
/// buffers fixed, and collecting the writes into the output buffer.
///
/// The kernel is a wasm function with two host imports — `read(cfg_pos, idx)`
/// reads an input buffer element, `write(out_pos, idx, val)` writes an output
/// buffer element — wired to the host-side input/output buffers through the
/// [`ParallelState`] the store carries.  The kernel is called once per index;
/// the writes accumulate into the output buffer (last write to a slot wins, a
/// scatter).  v1 runs sequentially (the data-parallelism is logical); a worker
/// pool is future work.
fn run_parallel_kernel(
    id: KernelId,
    count: usize,
    inputs: Vec<Vec<i64>>,
) -> Result<Vec<i64>, String> {
    let fragment = kernels()
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("parallel kernel {id} is not registered"))?;
    let ordered = vec![fragment];
    let index: HashMap<KernelId, u32> = [(id, 0)].into();

    let bytes = assemble_module(&ordered, &index)?;
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes).map_err(|e| e.to_string())?;
    let output = vec![0i64; count];
    let mut store = wasmi::Store::new(&engine, ParallelState { inputs, output });
    let mut linker = wasmi::Linker::new(&engine);

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
            |caller: wasmi::Caller<'_, ParallelState>,
             params: &[wasmi::Val],
             results: &mut [wasmi::Val]| {
                let pos = params.first().and_then(|v| v.i64()).unwrap_or(0) as usize;
                let idx = params.get(1).and_then(|v| v.i64()).unwrap_or(0) as usize;
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
            |mut caller: wasmi::Caller<'_, ParallelState>,
             params: &[wasmi::Val],
             _results: &mut [wasmi::Val]| {
                let idx = params.get(1).and_then(|v| v.i64()).unwrap_or(0) as usize;
                let value = params.get(2).and_then(|v| v.i64()).unwrap_or(0);
                if let Some(slot) = caller.data_mut().output.get_mut(idx) {
                    *slot = value;
                }
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;

    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_func(&store, "main")
        .ok_or_else(|| "parallel kernel has no export `main`".to_string())?;
    for i in 0..count {
        let args = [wasmi::Val::I64(count as i64), wasmi::Val::I64(i as i64)];
        let mut outputs = [wasmi::Val::I64(0)];
        main.call(&mut store, &args, &mut outputs)
            .map_err(|e| e.to_string())?;
    }
    Ok(store.into_data().output)
}

// --- Native-op registry: the plugin's opt-in to the native-plugin contract --

/// The `lichen-compute` native plugin marker — the nominal opt-in to the
/// native-plugin contract ([`lichen_highlevel::plugin::NativePlugin`]).
///
/// A unit marker: the plugin contributes its [`ComputeValue`] /
/// [`ComputeOperator`] leaves and its native op registry (via
/// [`compute_native_ops!`]), and never names a concrete host program.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComputePlugin;

impl lichen_highlevel::plugin::NativePlugin for ComputePlugin {}

/// Assemble `lichen-compute`'s private native-operator registry for a host
/// program `$program`, expanding to a `&'static` [`NativeOps`].
///
/// Invoked by a host that composes the plugin (see `liche-language`'s
/// `package.rs`), so the `$jit`/`$launch` names stay private to the plugin's
/// own embedded source.  The host names only the plugin crate and its program
/// marker — never the plugin's op structs — so this is the composition point a
/// package manager would generate.
#[macro_export]
macro_rules! compute_native_ops {
    ($program:ty) => {{
        // The registry is a `&'static [(&str, &dyn NativeOp<P>)]`.  A `static`
        // of that type cannot reference a *generic* `$program` (statics are
        // never generic), so build it per call and leak it — once per host
        // `register_compute`, a handful of small allocations.
        static JIT: $crate::JitOp = $crate::JitOp;
        static LAUNCH: $crate::LaunchOp = $crate::LaunchOp;
        static CALL: $crate::CallOp = $crate::CallOp;
        static PARALLEL: $crate::ParallelOp = $crate::ParallelOp;
        static PARLAUNCH: $crate::ParLaunchOp = $crate::ParLaunchOp;
        static RANGE: $crate::RangeOp = $crate::RangeOp;
        static READ: $crate::ReadOp = $crate::ReadOp;
        static WRITE: $crate::WriteOp = $crate::WriteOp;
        static COLLECT: $crate::BufferCollectOp = $crate::BufferCollectOp;
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
        let shape = ctx.array_node(&[d, c]);
        let fn_marker = ctx.value_node(P::Value::function_type_marker());
        let universe = ctx.universe();
        let kind = ctx.array_node(&[fn_marker, universe]);
        let fn_ty = ctx.array_node(&[shape, kind]);
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
        let shape = ctx.array_node(&[d, c]);
        let fn_marker = ctx.value_node(P::Value::function_type_marker());
        let universe = ctx.universe();
        let kind = ctx.array_node(&[fn_marker, universe]);
        let fn_ty = ctx.array_node(&[shape, kind]);
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

/// `$parallel(f)` — compile a single-arg `?cfg -> ?write` index function into a
/// parallel kernel.  The function-ness gate verifies `f` is a function; the
/// body is lowered over the loop index (from `compute.range n`) and the cfg
/// buffers (read via `compute.read`).
pub struct ParallelOp;

/// `$plrun(pk, cfg)` — run a parallel kernel over the index range `[0, cfg(0))`
/// with the input buffers from `cfg(1)` fixed, collecting the writes into a
/// `Buffer`.  The count is `cfg(0)`.
pub struct ParLaunchOp;

/// `$range(n)` — the loop index `i ∈ [0, n)` of the current parallel
/// invocation.  Kernel-only (lowers to the index parameter).
pub struct RangeOp;

/// `$read(buf, i)` — read one buffer element → `?b`.  In-kernel this lowers to
/// the host `read` import; at the VM it reads a `Buffer` value's element.
pub struct ReadOp;

/// `$write(n, i, val)` — a pending parallel write into the output buffer at `i`
/// (length `n`).  Kernel-only (lowers to the host `write` import).
pub struct WriteOp;

/// `$collect(buf)` — collect a whole buffer into a lichen array `[?b]`.
pub struct BufferCollectOp;

impl<P> NativeOp<P> for ParallelOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let f = &args[0];
        // Function-ness gate: `f : ?cfg -> ?write` (a single-arg index function;
        // the loop index comes from `compute.range n` inside the body, not a
        // second function parameter).
        let d0 = ctx.fresh();
        let c0 = ctx.fresh();
        let outer_shape = ctx.array_node(&[d0, c0]);
        let fn_kind = ctx.kind_expr(ctx.function_type_marker_node());
        let fn_ty = ctx.array_node(&[outer_shape, fn_kind]);
        ctx.check_unify(f.ty, fn_ty, loc.clone(), DiagKind::Guard);
        // The bare native parallel kernel artifact — the lichen wrapper wraps
        // this value into a `kernel` struct (`.native`).  Opaque: typed `_`.
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Parallel), Some(f.value));
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
    P::Operator: From<ComputeOperator> + From<LowOperator>,
{
    /// `$plrun(native, sig, a)` — run parallel kernel `native` over `a` (the
    /// `cfg`).  The lichen wrapper extracts `.native`/`.sig` out of the kernel
    /// struct; the native op gates the signature as `?cfg -> Write` and the
    /// `cfg` argument against the domain, and types the result as a `Buffer`
    /// whose element type is the `Write`'s element type.
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let native = &args[0];
        let sig = &args[1];
        let a = &args[2];
        // The signature is a single-arg function `?cfg -> Write`.  Read the
        // signature *lazily* (like `LaunchOp`) so the frozen `$plrun` template's
        // generic kernel resolves its codomain once a concrete kernel struct
        // binds at apply time:
        //   sig_shape = Index(sig.ty, 0)   → [?cfg, write_ty]
        //   write_ty  = Index(sig_shape, 1) → the Write type
        //   b         = Index(write_ty, 0)  → the output element type `?b`
        let zero = ctx.value_node(<P::Value as From<LowValue>>::from(LowValue::USize(0)));
        let one = ctx.value_node(<P::Value as From<LowValue>>::from(LowValue::USize(1)));
        let sig_shape_ops = ctx.array_node(&[sig.ty, zero]);
        let sig_shape = ctx.op_node(P::Operator::from(LowOperator::Index), Some(sig_shape_ops));
        let write_ty_ops = ctx.array_node(&[sig_shape, one]);
        let write_ty = ctx.op_node(P::Operator::from(LowOperator::Index), Some(write_ty_ops));
        let b_ops = ctx.array_node(&[write_ty, zero]);
        let b = ctx.op_node(P::Operator::from(LowOperator::Index), Some(b_ops));
        // Gate the signature as `[?cfg, write_ty]` function type and the
        // codomain as a `Write` type `[b, [TypeWrite, Type]]`, so `b` resolves
        // to the actual output element type.
        let d0 = ctx.fresh();
        let sig_shape_pat = ctx.array_node(&[d0, write_ty]);
        let sig_kind = ctx.kind_expr(ctx.function_type_marker_node());
        let sig_ty = ctx.array_node(&[sig_shape_pat, sig_kind]);
        ctx.check_unify(sig.ty, sig_ty, loc.clone(), DiagKind::Guard);
        let write_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeWrite,
        ));
        let write_kind = ctx.kind_expr(write_marker);
        let write_ty_pat = ctx.array_node(&[b, write_kind]);
        ctx.check_unify(write_ty, write_ty_pat, loc.clone(), DiagKind::Guard);
        // The argument is the `cfg = (n, (buffer…))`; unify it against the
        // kernel's domain.
        ctx.check_unify(a.ty, d0, loc.clone(), DiagKind::Guard);
        // Buffer result type: `[?b, [TypeBuffer, Type]]`.
        let buf_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeBuffer,
        ));
        let buf_kind = ctx.kind_expr(buf_marker);
        let buf_ty = ctx.array_node(&[b, buf_kind]);
        let operands = ctx.array_node(&[native.value, a.value]);
        let op = ctx.op_node(
            P::Operator::from(ComputeOperator::ParLaunch),
            Some(operands),
        );
        let pair = ctx.array_node(&[op, buf_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: buf_ty,
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

/// The `lichen-compute` plugin's embedded lichen source — the real `compute`
/// plugin file, kept as a `.lichen` source file and embedded with
/// [`include_str!`].  It defines the user-facing `jit`/`launch` functions as
/// ordinary typed lichen (whose bodies call the native `$jit`/`$launch`), and
/// exports them as a **named struct** (`compute.jit`, `compute.launch`).
pub const WRAPPER_SOURCE: &str = include_str!("compute.lichen");
