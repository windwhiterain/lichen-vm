//! The `lichen-compute` extension: a native "compute" wrapper package.
//!
//! The native part injects a [`ComputeValue`] vocabulary — the **`Kernel`**
//! value (a compiled, runnable wasm artifact), the **`TypeKernel`** kind
//! marker (a kernel's type is `[signature, [TypeKernel, Type]]`, mirroring a
//! function type), the first-class **`Native` operator values** (`jit`,
//! `launch`), and the **`LaunchTarget`** intermediate of a curried `launch`.
//! It also injects the [`ComputeOperator`]s `Jit`/`Launch`, whose
//! [`OperatorExt::run`] does the wasm compile/execute and the global kernel
//! registry for the compiled artifacts.
//!
//! Operators are bound to source as **values** (Option B): `jit` and `launch`
//! are values whose *type* tags them; when the checker sees an `Apply` whose
//! callee has one of those types, it delegates to the matching
//! [`NativeExt`] (see [`native_registry`]), which emits the
//! [`ComputeOperator`] and does that operator's type check.  The runtime
//! `LowOperator::Apply` never sees them.
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
//! - `jit f` requires `f` to be a *function* (function-ness gate) and gives
//!   the result the type `Kernel<Int -> Int>` (scalar v1).
//! - `launch k` requires `k` to be a *kernel* (kernel-ness gate) and produces
//!   a `LaunchTarget` typed as a function `domain -> codomain`.
//! - `(launch k) a` unifies `a` against the kernel's domain and its result is
//!   the kernel's codomain — a function-style apply over a kernel.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp};
use lichen_highlevel::program::{Ctx, HighProgram, TypeOperator, ValueType};
use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, ArrayItem, BlockId, FunctionId, LowOperator, LowShape, LowValue,
    Module, NodeId, OperatorExt, Program,
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
    /// The kind marker of kernel types — a kernel's type is
    /// `[signature, [TypeKernel, Type]]`.
    TypeKernel,
    /// A compiled **parallel** kernel artifact: a curried `?a -> USize -> ?b`
    /// function flattened to a `(?a, USize) -> ?b` wasm function (the config
    /// is the first group of parameters, the *index* the last scalar).
    ParKernel(KernelId),
    /// The kind marker of parallel-kernel types — a parallel kernel's type is
    /// `[signature, [TypeParKernel, Type]]`.
    TypeParKernel,
    /// A runtime results **buffer**: `plrun` ran the parallel kernel over the
    /// index range `[0, n)` and collected the `n` `?b` results here, host-owned
    /// (a `Copy` scalar into the process buffer registry, exactly like
    /// [`Kernel`]'s [`KernelId`]).
    Buffer(BufferId),
    /// The kind marker of buffer types — a buffer's type is
    /// `[element_type, [TypeBuffer, Type]]`.
    TypeBuffer,
}

impl lichen_utils::extend::FunctionKind for ComputeValue {
    fn is_function_kind(&self) -> bool {
        // `TypeKernel` (and its parallel twin `TypeParKernel`) re-head the
        // universe to form a kernel type `[signature, [TypeKernel, Type]]`,
        // which mirrors a function type — the generic renderer spells that
        // kind as `in -> out`.  A parallel kernel mirrors the lifted
        // `?a -> USize -> ?b` the same way.
        matches!(self, ComputeValue::TypeKernel | ComputeValue::TypeParKernel)
    }
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
    /// Compile a curried `?a -> USize -> ?b` function to a parallel kernel
    /// (flattened `(?a, USize) -> ?b`) → a `ParKernel` value.
    Parallel,
    /// `[parallel_kernel, cfg, count]` operand — run the parallel kernel over
    /// the index range `[0, count)` → a `Buffer` value.
    ParLaunch,
    /// `[buffer, index]` operand — read one buffer element → `?b`.
    BufferGet,
    /// `[buffer]` operand — collect the whole buffer into a lichen array `[?b]`.
    BufferCollect,
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
                    unreachable!("ParLaunch expects an operand array of [kernel, (cfg, count)]")
                };
                let operands = operands.items();
                let Some(ComputeValue::ParKernel(id)) = module
                    .node_value(operands[0].node)
                    .and_then(|v| AsEnum::<ComputeValue>::as_enum(&v))
                else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                // The tuple argument `(cfg, count)`: element 0 is the config
                // (a scalar `USize` or a (nested) tuple of scalars, flattened
                // like a `Launch` argument), element 1 is the `USize` count.
                let Ok(tuple_node) = dyn_node(operands[1].node) else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                let Some(tuple_items) = module.array_items(tuple_node) else {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                };
                let mut cfg_args: Vec<i64> = Vec::new();
                match tuple_items
                    .first()
                    .and_then(|item| module.node_value(item.node))
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                {
                    Some(LowValue::USize(n)) => cfg_args.push(n as i64),
                    Some(LowValue::Array(_)) => {
                        if !collect_args(module, tuple_items[0].node, &mut cfg_args) {
                            return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                        }
                    }
                    _ => return <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                };
                let count = match tuple_items
                    .get(1)
                    .and_then(|item| module.node_value(item.node))
                    .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
                {
                    Some(LowValue::USize(n)) => n,
                    _ => return <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                };
                match run_parallel_kernel(id, &cfg_args, count) {
                    Ok(results) => {
                        let bid = alloc_buffer_id();
                        buffers().lock().unwrap().insert(bid, results);
                        <P::Value as From<ComputeValue>>::from(ComputeValue::Buffer(bid))
                    }
                    Err(..) => <P::Value as From<LowValue>>::from(LowValue::Parameterized),
                }
            }
            ComputeOperator::BufferGet => {
                if matches!(
                    AsEnum::<LowValue>::as_enum(&operand),
                    Some(LowValue::Parameterized)
                ) {
                    return <P::Value as From<LowValue>>::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = AsEnum::<LowValue>::as_enum(&operand) else {
                    unreachable!("BufferGet expects an operand array of [buffer, index]")
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

/// The [`FunctionId`] of the value `node` denotes, if it is (or reaches) a
/// function value.  Looks through a `[value, type]` pair and a `value_of`
/// extraction before reading the `LowValue::Function`.
fn inner_function_id<P>(module: &Module<P>, node: NodeId) -> Result<FunctionId, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    let value_node = pair_value_node(module, node)
        .or_else(|| value_of_node(module, node))
        .unwrap_or(node);
    match module
        .node_value(AnyNodeId::Dynamic(value_node))
        .and_then(|v| AsEnum::<LowValue>::as_enum(&v))
    {
        Some(LowValue::Function(AnyFunctionId::Dynamic(fid))) => Ok(fid),
        _ => Err("parallel kernel's outer body must be an index function `i => ?b`".into()),
    }
}

/// Lower a curried `?a -> USize -> ?b` function into a **parallel kernel** — a
/// [`KernelFragment`] whose domain is the flattened tuple `(?a, USize)` and
/// whose body is the inner function's body, traced with two parameter slots
/// (the config group then the index scalar).
///
/// `parallel` is the data-parallel lift: running it over the index range
/// `[0, n)` computes `f cfg i` for each `i` into a [`Buffer`], so the index
/// (the inner function's parameter) is the *last* scalar of the domain and the
/// config `?a` is the first group.
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
    let cfg_shape = kernel_param_shape(module, cfg_pair)?;
    match &cfg_shape {
        LowShape::USize | LowShape::Tuple(_) => {}
        _ => {
            return Err(
                "parallel kernel config domain must be a scalar or a tuple of scalars".into(),
            );
        }
    }
    // The outer body is the inner index function `i => ?b`.
    let inner_fid = inner_function_id(module, module.functions[fid].r#return)?;
    let inner = &module.functions[inner_fid];
    let i_pair = inner.parameter;
    let body = inner.r#return;
    let i_shape = kernel_param_shape(module, i_pair)?;
    match &i_shape {
        LowShape::USize => {}
        _ => return Err("parallel kernel index parameter must be a scalar USize".into()),
    }
    // The kernel's result is the inner body's value (through a `[value,type]`
    // pair, or a bare value node).
    let ret_value = match module.array_items(body) {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => body,
    };
    let cfg_value = pair_value_node(module, cfg_pair)
        .ok_or_else(|| "parallel config parameter is not a [value, type] pair".to_string())?;
    let i_value = pair_value_node(module, i_pair)
        .ok_or_else(|| "parallel index parameter is not a [value, type] pair".to_string())?;
    module.set_node_shape(cfg_value, Some(cfg_shape.clone()));
    module.set_node_shape(i_value, Some(i_shape.clone()));
    let cfg_arity = flat_arity(&cfg_shape);
    let params = vec![
        ParamSlot {
            pair: cfg_pair,
            value: cfg_value,
            shape: cfg_shape.clone(),
            base: 0,
        },
        ParamSlot {
            pair: i_pair,
            value: i_value,
            shape: i_shape.clone(),
            base: cfg_arity,
        },
    ];
    let mut body_instr: Vec<KernelInstr> = Vec::new();
    emit_node(module, &params, ret_value, &mut body_instr)?;
    Ok(KernelFragment {
        param_shape: LowShape::Tuple(vec![cfg_shape, i_shape]),
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
fn assemble_module(
    ordered: &[KernelFragment],
    index: &HashMap<KernelId, u32>,
) -> Result<Vec<u8>, String> {
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction,
        Module as WasmModule, TypeSection, ValType,
    };

    // Type section: one (param-arity) -> i64 signature per distinct arity.
    let mut types = TypeSection::new();
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
    let mut funcs = FunctionSection::new();
    for &ti in &func_types {
        funcs.function(ti);
    }
    wasm.section(&funcs);
    let mut exports = ExportSection::new();
    exports.export("main", ExportKind::Func, 0);
    wasm.section(&exports);

    let mut code = CodeSection::new();
    for frag in ordered {
        let mut body = Function::new([]);
        lower_body(&frag.body, index, &mut body)?;
        body.instruction(&Instruction::End);
        code.function(&body);
    }
    wasm.section(&code);
    Ok(wasm.finish())
}

/// Lower a sequence of abstract [`KernelInstr`]s into a wasm function body.
/// `index` resolves each cross-kernel `CallKernel` to the callee's in-module
/// function index (assigned by the launch-time assembly).
fn lower_body(
    body: &[KernelInstr],
    index: &HashMap<KernelId, u32>,
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
                out.instruction(&Instruction::Call(target));
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
    // The compute plugin's own operators: `Launch` inside a kernel body is the
    // wrapper/`$launch` cross-kernel call form.
    if let Some(compute_op) = AsEnum::<ComputeOperator>::as_enum(op) {
        match compute_op {
            ComputeOperator::Launch => {
                let (kernel, arg) = apply_pair(module, operation.operand)?;
                return emit_cross_kernel_call(module, params, kernel, arg, body);
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
/// through any `value_of` extractions.  Returns `None` for a node that is not
/// (or does not reach) a kernel value — e.g. a lichen function, used by the
/// style-1 inline path instead.
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
    let inner = value_of_node(module, node)?;
    kernel_id_of(module, inner)
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

/// Run a **parallel** kernel over the index range `[0, count)`, computing the
/// flattened-kernel `(?a, USize) -> ?b` once per index with the (already
/// flattened) config argument vector fixed, and collect the `?b` results into a
/// `Vec<i64>`.
///
/// The indices are distributed across a bounded pool of scoped threads (one
/// chunk per worker, so a large range is genuinely concurrent — the "parallel"
/// of the data-parallel lift), each worker running its own [`run_kernel`] on the
/// kernel's relative launch set.  Results are placed back in index order, so the
/// buffer is deterministic regardless of scheduling.
fn run_parallel_kernel(id: KernelId, cfg_args: &[i64], count: usize) -> Result<Vec<i64>, String> {
    // Warm the kernel once: this validates the fragment, the assembly, and the
    // config arity, so a broken parallel kernel fails fast (before threads).
    let _ = run_kernel(id, &[cfg_args, &[0]].concat())?;
    let mut results = vec![0i64; count];
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(count.max(1));
    if workers <= 1 || count <= 1 {
        for i in 0..count {
            let args = [cfg_args, &[i as i64]].concat();
            results[i] = run_kernel(id, &args)? as i64;
        }
        return Ok(results);
    }
    std::thread::scope(|scope| {
        let chunk = count.div_ceil(workers);
        let cfg = cfg_args.to_vec();
        let mut handles: Vec<(usize, std::thread::ScopedJoinHandle<'_, Vec<i64>>)> = Vec::new();
        for w in 0..workers {
            let lo = w * chunk;
            if lo >= count {
                break;
            }
            let hi = (lo + chunk).min(count);
            let cfg = cfg.clone();
            handles.push((
                lo,
                scope.spawn(move || {
                    let mut out = Vec::with_capacity(hi - lo);
                    for i in lo..hi {
                        let args = [cfg.as_slice(), &[i as i64]].concat();
                        match run_kernel(id, &args) {
                            Ok(value) => out.push(value as i64),
                            Err(e) => eprintln!("parallel kernel index {i} failed: {e}"),
                        }
                    }
                    out
                }),
            ));
        }
        // Join in index order and copy into the result vector.
        for (lo, handle) in handles {
            let vals = handle.join().expect("parallel worker panicked");
            results[lo..lo + vals.len()].copy_from_slice(&vals);
        }
    });
    Ok(results)
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
        static PARALLEL: $crate::ParallelOp = $crate::ParallelOp;
        static PARLAUNCH: $crate::ParLaunchOp = $crate::ParLaunchOp;
        static PGET: $crate::BufferGetOp = $crate::BufferGetOp;
        static PCOLLECT: $crate::BufferCollectOp = $crate::BufferCollectOp;
        let ops: Vec<(&'static str, &'static dyn $crate::NativeOp<$program>)> = vec![
            ("jit", &JIT as &dyn $crate::NativeOp<$program>),
            ("launch", &LAUNCH as &dyn $crate::NativeOp<$program>),
            ("parallel", &PARALLEL as &dyn $crate::NativeOp<$program>),
            ("plrun", &PARLAUNCH as &dyn $crate::NativeOp<$program>),
            ("pget", &PGET as &dyn $crate::NativeOp<$program>),
            ("pcollect", &PCOLLECT as &dyn $crate::NativeOp<$program>),
        ];
        Box::leak(ops.into_boxed_slice()) as $crate::NativeOps<$program>
    }};
}

// --- Native operators: the private contract with the plugin's source -------

/// `$jit(f)` — compile a function to a kernel.  The function-ness gate unifies
/// the argument's type with an arrow shape (the *gate*); the kernel type
/// `[sig, [TypeKernel, Type]]` carries the arrow's `[in, out]` shape as its
/// signature, so `launch` reads the domain/codomain out of the type.
///
/// The program marker is generic: a host composes this op into its own
/// `NativeOps` registry (a `&'static [(&str, &dyn NativeOp<P>)]`), so the
/// `$jit`/`$launch` names stay private to the plugin's own embedded source.
pub struct JitOp;

/// `$launch(k, a)` — run kernel `k` on `a`.  The kernel-ness gate unifies `k`'s
/// type with a kernel type (binding the domain/codomain to fresh cells); the
/// argument is unified against the domain and the result typed as the codomain.
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
        // binding the domain/codomain to the fresh cells below.
        let d = ctx.fresh();
        let c = ctx.fresh();
        let shape = ctx.array_node(&[d, c]);
        let fn_marker = ctx.value_node(P::Value::function_type_marker());
        let universe = ctx.universe();
        let kind = ctx.array_node(&[fn_marker, universe]);
        let fn_ty = ctx.array_node(&[shape, kind]);
        ctx.check_unify(f.ty, fn_ty, loc, DiagKind::Guard);

        // Kernel type: `[sig, [TypeKernel, Type]]` where `sig = [d, c]` is the
        // arrow's signature — this is what `launch` reads the domain/codomain
        // from.  It references the cells the gate just bound, so a concrete
        // function signature flows into the kernel's type.
        let sig = ctx.array_node(&[d, c]);
        let k_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeKernel,
        ));
        let universe = ctx.universe();
        let k_kind = ctx.array_node(&[k_marker, universe]);
        let kernel_ty = ctx.array_node(&[sig, k_kind]);

        let op = ctx.op_node(P::Operator::from(ComputeOperator::Jit), Some(f.value));
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
    P::Operator: From<ComputeOperator>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let k = &args[0];
        let a = &args[1];
        // Kernel-ness gate: `k`'s type must be a kernel type, binding the
        // domain/codomain to the fresh signature cells.
        let d = ctx.fresh();
        let c = ctx.fresh();
        let sig = ctx.array_node(&[d, c]);
        let k_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeKernel,
        ));
        let universe = ctx.universe();
        let k_kind = ctx.array_node(&[k_marker, universe]);
        let kernel_ty = ctx.array_node(&[sig, k_kind]);
        ctx.check_unify(k.ty, kernel_ty, loc.clone(), DiagKind::Guard);
        // Unify the argument against the kernel's domain.
        ctx.check_unify(a.ty, d, loc.clone(), DiagKind::Guard);
        // Emit the `Launch` operator over `[k, a]`, typed as the codomain.
        let operands = ctx.array_node(&[k.value, a.value]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Launch), Some(operands));
        let pair = ctx.array_node(&[op, c]);
        NativeApply {
            node: pair,
            val: None,
            ty: c,
        }
    }
}

/// `$parallel(f)` — compile a curried `?a -> USize -> ?b` index function into a
/// parallel kernel.  The curried-arrow gate verifies `f` is a two-argument
/// function whose first argument is the config and whose second is a `USize`
/// index (bound through the `Int` type cell), binding the config `?a` and
/// element `?b` to fresh cells for the buffer ops to read.
pub struct ParallelOp;

/// `$plrun(pk, cfg, n)` — run a parallel kernel over the index range `[0, n)`
/// with config `cfg` fixed, collecting the `n` element results into a `Buffer`.
pub struct ParLaunchOp;

/// `$pget(buf, i)` — read element `i` of a buffer.
pub struct BufferGetOp;

/// `$pcollect(buf)` — collect a whole buffer into a lichen array `[?b]`.
pub struct BufferCollectOp;

impl<P> NativeOp<P> for ParallelOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let f = &args[0];
        // Curried-arrow gate: `f : ?a -> r0`.
        let d0 = ctx.fresh();
        let r0 = ctx.fresh();
        let outer_shape = ctx.array_node(&[d0, r0]);
        let fn_kind = ctx.kind_expr(ctx.function_type_marker_node());
        let fn_ty = ctx.array_node(&[outer_shape, fn_kind]);
        ctx.check_unify(f.ty, fn_ty, loc.clone(), DiagKind::Guard);
        // Inner-arrow gate: `r0 : Int -> ?b` — the index is a `USize`/`Int`.
        let b = ctx.fresh();
        let inner_shape = ctx.array_node(&[ctx.int_type(), b]);
        let inner_kind = ctx.kind_expr(ctx.function_type_marker_node());
        let inner_ty = ctx.array_node(&[inner_shape, inner_kind]);
        ctx.check_unify(r0, inner_ty, loc.clone(), DiagKind::Guard);
        // ParKernel type: `[sig, [TypeParKernel, Type]]`, `sig = [?a, Int -> ?b]` —
        // the lifted `?a -> USize -> ?b` signature `plrun` reads the config and
        // element type from.  The codomain is the *full* inner function type, so
        // the generic renderer spells the whole thing as `?a -> Int -> ?b`.
        let sig = ctx.array_node(&[d0, inner_ty]);
        let par_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeParKernel,
        ));
        let par_kind = ctx.kind_expr(par_marker);
        let par_ty = ctx.array_node(&[sig, par_kind]);
        let op = ctx.op_node(P::Operator::from(ComputeOperator::Parallel), Some(f.value));
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
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let k = &args[0];
        let a = &args[1];
        // ParKernel gate: `k : [?a, Int -> ?b] → [sig, [TypeParKernel, Type]]`.
        // The sig's codomain is the inner *function* type; its shape gives the
        // element cell `?b`.
        let d0 = ctx.fresh();
        let b = ctx.fresh();
        let mid = ctx.fresh();
        let inner_shape = ctx.array_node(&[mid, b]);
        let inner_kind = ctx.kind_expr(ctx.function_type_marker_node());
        let inner_ty = ctx.array_node(&[inner_shape, inner_kind]);
        let sig = ctx.array_node(&[d0, inner_ty]);
        let par_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeParKernel,
        ));
        let par_kind = ctx.kind_expr(par_marker);
        let par_ty = ctx.array_node(&[sig, par_kind]);
        ctx.check_unify(k.ty, par_ty, loc.clone(), DiagKind::Guard);
        ctx.check_unify(mid, ctx.int_type(), loc.clone(), DiagKind::Guard);
        // The argument is a `(config, count)` tuple: unify `a`'s type with a
        // 2-tuple, binding the config cell to the domain `?a` and the count to a
        // `USize`.
        let cfg_cell = ctx.fresh();
        let n_cell = ctx.fresh();
        let tuple_shape = ctx.array_node(&[cfg_cell, n_cell]);
        let tuple_kind = ctx.kind_expr(ctx.tuple_type_marker_node());
        let tuple_ty = ctx.array_node(&[tuple_shape, tuple_kind]);
        ctx.check_unify(a.ty, tuple_ty, loc.clone(), DiagKind::Guard);
        ctx.check_unify(cfg_cell, d0, loc.clone(), DiagKind::Guard);
        ctx.check_unify(n_cell, ctx.int_type(), loc.clone(), DiagKind::Guard);
        // Buffer result type: `[?b, [TypeBuffer, Type]]`.
        let buf_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeBuffer,
        ));
        let buf_kind = ctx.kind_expr(buf_marker);
        let buf_ty = ctx.array_node(&[b, buf_kind]);
        let operands = ctx.array_node(&[k.value, a.value]);
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

impl<P> NativeOp<P> for BufferGetOp
where
    P: HighProgram,
    P::Value: ValueType + From<ComputeValue>,
    P::Operator: From<ComputeOperator>,
{
    fn build(&self, ctx: &mut dyn Ctx<P>, _e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply {
        let b = &args[0];
        let i = &args[1];
        // Buffer gate: `b : [?b, [TypeBuffer, Type]]`, binding the element type.
        let elem = ctx.fresh();
        let buf_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(
            ComputeValue::TypeBuffer,
        ));
        let buf_kind = ctx.kind_expr(buf_marker);
        let buf_ty = ctx.array_node(&[elem, buf_kind]);
        ctx.check_unify(b.ty, buf_ty, loc.clone(), DiagKind::Guard);
        ctx.check_unify(i.ty, ctx.int_type(), loc.clone(), DiagKind::Guard);
        let operands = ctx.array_node(&[b.value, i.value]);
        let op = ctx.op_node(
            P::Operator::from(ComputeOperator::BufferGet),
            Some(operands),
        );
        let pair = ctx.array_node(&[op, elem]);
        NativeApply {
            node: pair,
            val: None,
            ty: elem,
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
