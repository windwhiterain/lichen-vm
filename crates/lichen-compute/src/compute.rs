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
    AnyFunctionId, AnyNodeId, ArrayItem, BlockId, LowOperator, LowShape, LowValue, Module, NodeId,
    OperatorExt, Program,
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
}

// --- OperatorExt::run (the VM dispatch for the injected operators) ---------

impl<P> OperatorExt<P> for ComputeOperator
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
{
    fn run(&self, operand: P::Value, _block: BlockId, module: &mut Module<P>) -> P::Value {
        match self {
            ComputeOperator::Jit => {
                if matches!(AsEnum::<LowValue>::as_enum(&operand), Some(LowValue::Parameterized)) {
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
                if matches!(AsEnum::<LowValue>::as_enum(&operand), Some(LowValue::Parameterized)) {
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
                match module.node_value(operands[1].node).and_then(|v| AsEnum::<LowValue>::as_enum(&v)) {
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
        }
    }
}

// --- Codegen: lichen graph → a scalar `(i64) -> i64` wasm function body -----

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
fn compile_fragment<P>(module: &mut Module<P>, function: AnyFunctionId) -> Result<KernelFragment, String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    let param_value = match module.array_items(param_pair).and_then(|items| items.first()) {
        Some(first) => dyn_node(first.node)?,
        None => return Err("parameter is not a [value, type] pair".into()),
    };
    module.set_node_shape(param_value, Some(param_shape.clone()));

    let mut body: Vec<KernelInstr> = Vec::new();
    emit_node(module, param_pair, param_value, ret_value, &mut body)?;

    Ok(KernelFragment {
        param_shape,
        body,
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
            types.ty().function(vec![ValType::I64; arity], vec![ValType::I64]);
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
{
    let root = equality_rep(module, node);
    for (n, nd) in &module.nodes {
        if equality_rep(module, n) != root {
            continue;
        }
        if let Some(op) = nd.operation.as_ref() {
            if !matches!(AsEnum::<LowOperator>::as_enum(&op.operator), Some(LowOperator::Index)) {
                return Some(n);
            }
        }
    }
    None
}

/// Emit wasm instructions for one lichen graph node — the scalar kernel-safe
/// subset: integer constants, `Add`/`Sub`/`Leq`/`Eq`, and the parameter value
/// (`Index(param_pair, 0)` → `local.get 0`).
fn emit_node<P>(
    module: &Module<P>,
    param_pair: NodeId,
    param_value: NodeId,
    node: NodeId,
    body: &mut Vec<KernelInstr>,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
        // in the enclosing parameter's equality class, it is a whole-parameter
        // read: the deep pass's apply-clone *unifies* a substituted parameter
        // with the argument, so a reduced same-module call's parameter
        // reference resolves to this kernel's parameter — emit a `local.get`
        // for it instead of failing.
        if equality_rep(module, node) == equality_rep(module, param_value) {
            let domain = module
                .node_shape(param_value)
                .ok_or_else(|| "kernel parameter has no domain shape".to_string())?;
            let offset = flatten_offset(domain, &[])?;
            body.push(KernelInstr::LocalGet(offset as u32));
            return Ok(());
        }
        // A value collapsed to a bare `Parameterized` cell resolves through its
        // equality class to the computation that defines it — a kernel call's
        // result, or a `launch` argument (whose cell is *expected* to be
        // parameterized: `launch` is two-step, assemble then call, so the
        // argument is only concrete at run time).  Emit the defining member.
        if let Some(definer) = class_computation_node(module, node) {
            return emit_node(module, param_pair, param_value, definer, body);
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
                if let Some(path) = param_path(module, param_pair, node) {
                    let domain = module
                        .node_shape(param_value)
                        .ok_or_else(|| "kernel parameter has no domain shape".to_string())?;
                    let offset = flatten_offset(domain, &path)?;
                    body.push(KernelInstr::LocalGet(offset as u32));
                    return Ok(());
                }
                // A `value_of` extraction — `Index(pair, 0)` with a constant
                // `0` index and `pair` a `[value, type]` pair.  Emit the
                // pair's value slot instead of treating the extraction as a
                // real index.
                if usize_value(module, index) == Some(0) {
                    if let Some(value_node) = value_of_node(module, node) {
                        return emit_node(module, param_pair, param_value, value_node, body);
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
                                emit_node(module, param_pair, param_value, then_node, body)?;
                                emit_node(module, param_pair, param_value, else_node, body)?;
                                emit_node(module, param_pair, param_value, index, body)?;
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
                    return emit_cross_kernel_call(
                        module,
                        param_pair,
                        param_value,
                        callee,
                        arg,
                        body,
                    );
                }
                // Style 1: a full lichen-function call (inline its body) —
                // deferred.
                return Err(
                    "kernel body Apply is supported only for a cross-kernel (kernel-value) callee v1; inline lichen-function calls are not yet supported"
                        .into(),
                );
            }
            LowOperator::TableGet => {
                return Err(
                    "unsupported tableget operator in kernel body (kernel-safe subset is scalar arith)"
                        .into(),
                )
            }
        }
    }
    // The highlevel's type-level arithmetic: `Add`/`Sub`/`Leq`/`Eq` over
    // `[left, right]`.
    if let Some(ty_op) = AsEnum::<TypeOperator>::as_enum(op) {
        match ty_op {
            TypeOperator::Add | TypeOperator::Sub | TypeOperator::Leq | TypeOperator::Eq => {
                let (left, right) = operand_pair(module, operation.operand)?;
                emit_node(module, param_pair, param_value, left, body)?;
                emit_node(module, param_pair, param_value, right, body)?;
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
                ))
            }
        }
    }
    // The compute plugin's own operators: `Launch` inside a kernel body is the
    // wrapper/`$launch` cross-kernel call form.
    if let Some(compute_op) = AsEnum::<ComputeOperator>::as_enum(op) {
        match compute_op {
            ComputeOperator::Launch => {
                let (kernel, arg) = apply_pair(module, operation.operand)?;
                return emit_cross_kernel_call(module, param_pair, param_value, kernel, arg, body);
            }
            // Jitting another function from *inside* a kernel body is not a v1
            // cross-kernel call.
            other => {
                return Err(format!(
                    "unsupported compute operator in kernel body: {other:?}"
                ))
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
    param_pair: NodeId,
    param_value: NodeId,
    kernel: NodeId,
    arg: NodeId,
    body: &mut Vec<KernelInstr>,
) -> Result<(), String>
where
    P: Program,
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    emit_node(module, param_pair, param_value, arg, body)?;
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
{
    let Some(operation) = module.nodes[node].operation else {
        return false;
    };
    if !matches!(AsEnum::<LowOperator>::as_enum(&operation.operator), Some(LowOperator::Index)) {
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
{
    let operation = module.nodes[node].operation?;
    if !matches!(AsEnum::<LowOperator>::as_enum(&operation.operator), Some(LowOperator::Index)) {
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
{
    let operation = module.nodes[node].operation?;
    if !matches!(AsEnum::<LowOperator>::as_enum(&operation.operator), Some(LowOperator::Index)) {
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
{
    match module.node_value(node).and_then(|v| AsEnum::<LowValue>::as_enum(&v)) {
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>
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
                        return Err(format!("cross-kernel callee kernel {kid} is not registered"));
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
        static JIT: $crate::JitOp = $crate::JitOp;
        static LAUNCH: $crate::LaunchOp = $crate::LaunchOp;
        static OPS: [(&str, &dyn $crate::NativeOp<$program>); 2] = [
            ("jit", &JIT),
            ("launch", &LAUNCH),
        ];
        &OPS[..] as $crate::NativeOps<$program>
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
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        loc: Loc,
    ) -> NativeApply {
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
        let k_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(ComputeValue::TypeKernel));
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
    fn build(
        &self,
        ctx: &mut dyn Ctx<P>,
        _e: ExprId,
        args: &[NativeArg],
        loc: Loc,
    ) -> NativeApply {
        let k = &args[0];
        let a = &args[1];
        // Kernel-ness gate: `k`'s type must be a kernel type, binding the
        // domain/codomain to the fresh signature cells.
        let d = ctx.fresh();
        let c = ctx.fresh();
        let sig = ctx.array_node(&[d, c]);
        let k_marker = ctx.value_node(<P::Value as From<ComputeValue>>::from(ComputeValue::TypeKernel));
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

/// The `lichen-compute` plugin's embedded lichen source — the real `compute`
/// plugin file, kept as a `.lichen` source file and embedded with
/// [`include_str!`].  It defines the user-facing `jit`/`launch` functions as
/// ordinary typed lichen (whose bodies call the native `$jit`/`$launch`), and
/// exports them as a **named struct** (`compute.jit`, `compute.launch`).
pub const WRAPPER_SOURCE: &str = include_str!("compute.lichen");
