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
//! [`NativeExt`] (see [`native`](crate::compute::native_registry)), which
//! emits the [`ComputeOperator`] and does that operator's type check.  The
//! runtime `LowOperator::Apply` never sees them.
//!
//! ## Type-checking coverage
//!
//! - `jit f` requires `f` to be a *function* (function-ness gate) and gives
//!   the result the type `Kernel<Int -> Int>` (scalar v1).
//! - `launch k` requires `k` to be a *kernel* (kernel-ness gate) and produces
//!   a `LaunchTarget` typed as a function `domain -> codomain`.
//! - `(launch k) a` unifies `a` against the kernel's domain and its result is
//!   the kernel's codomain — a function-style apply over a kernel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprId, Loc};
use lichen_highlevel::native::{NativeApply, NativeArg, NativeOp, NativeOps};
use lichen_highlevel::program::{Ctx, TypeOperator, ValueType};
use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, ArrayItem, BlockId, LowOperator, LowShape, LowValue, Module, NodeId,
    OperatorExt,
};
use lichen_utils::extend::AsEnum;

use crate::program::{LangOperator, LangProgram, LangValue};

/// A compiled kernel artifact's identity — a compact index into the process
/// kernel registry (the compiled wasm bytes).  A kernel value is host-owned
/// (a small `Copy` scalar), so it is never an arena payload and never needs GC
/// re-homing or static freeze.
pub type KernelId = usize;

/// The process kernel registry: compiled kernel **fragments** (bytecode units),
/// keyed by [`KernelId`].  Kernels are immutable artifacts shared across
/// modules in the process.  The fragment is the durable JIT output; the module
/// bytes are derived on demand by [`link_fragment`].
static KERNELS: OnceLock<Mutex<HashMap<KernelId, KernelFragment>>> = OnceLock::new();
fn kernels() -> &'static Mutex<HashMap<KernelId, KernelFragment>> {
    KERNELS.get_or_init(Default::default)
}
/// The next kernel id — process-global, so ids never collide across modules.
static NEXT_KERNEL_ID: AtomicUsize = AtomicUsize::new(0);
fn alloc_kernel_id() -> KernelId {
    NEXT_KERNEL_ID.fetch_add(1, Ordering::Relaxed)
}

/// A compiled kernel-callable unit — the JIT's **bytecode** output, not a
/// module.
///
/// `jit` lowers one lichen function to a [`KernelFragment`]: the function's
/// body as raw wasm, plus the domain shape signature a linker needs.  The
/// fragment is stored (not the whole module); `launch` links the reachable
/// fragment set into a module (today [`link_fragment`]'s degenerate
/// single-fragment link) and runs it.  Splitting "emit bytecode" from "assemble
/// a module" is what lets a later step link many fragments together (helper
/// sharing, recursion) and emit cross-module imports for callees compiled
/// elsewhere.
#[derive(Debug, Clone)]
struct KernelFragment {
    /// The parameter domain shape — the wasm parameter types and the layout
    /// the body emitter used for parameter reads.
    param_shape: LowShape,
    /// The lowered wasm function body: raw bytes (locals + opcodes, no
    /// leading size prefix), as produced by
    /// [`wasm_encoder::Function::into_raw_body`].
    body: Vec<u8>,
}

/// The compute value vocabulary — injected as a sibling leaf into the
/// language's value union (see [`crate::program`]).  A plain enum of exactly
/// this extension's variants, composed with [`lichen_utils::enum_ext!`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComputeValue {
    /// A compiled, runnable kernel artifact.
    Kernel(KernelId),
    /// The kind marker of kernel types — a kernel's type is
    /// `[signature, [TypeKernel, Type]]`.
    TypeKernel,
}

/// The compute operator vocabulary — the `Jit`/`Launch` operations dispatched
/// by the VM through [`OperatorExt::run`].  Composed into the language's
/// operator union (see [`crate::program`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComputeOperator {
    /// Compile a function value to wasm bytecode → a `Kernel` value.
    Jit,
    /// `[kernel, arg]` operand — run the kernel on the arg → the result.
    Launch,
}

// --- OperatorExt::run (the VM dispatch for the injected operators) ---------

impl OperatorExt<LangProgram> for ComputeOperator {
    fn run(
        &self,
        operand: LangValue,
        _block: BlockId,
        module: &mut Module<LangProgram>,
    ) -> LangValue {
        match self {
            ComputeOperator::Jit => {
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return LangValue::from(LowValue::Parameterized);
                }
                let Some(LowValue::Function(function)) = operand.as_enum() else {
                    // A non-function jit target is a *reported* type error (the
                    // checker's function-ness gate), not an invariant violation —
                    // stay lazy rather than panicking.
                    return LangValue::from(LowValue::Parameterized);
                };
                match compile_fragment(module, function) {
                    Ok(fragment) => {
                        let id = alloc_kernel_id();
                        kernels().lock().unwrap().insert(id, fragment);
                        LangValue::from(ComputeValue::Kernel(id))
                    }
                    Err(err) => {
                        // The body uses an operator outside the kernel-safe
                        // subset — record nothing and stay lazy; the definition
                        // pass's error channel reports the unbound result.
                        let _ = err;
                        LangValue::from(LowValue::Parameterized)
                    }
                }
            }
            ComputeOperator::Launch => {
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return LangValue::from(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("Launch expects an operand array of [kernel, arg]")
                };
                let operands = operands.items();
                let Some(ComputeValue::Kernel(id)) = module
                    .node_value(operands[0].node)
                    .and_then(|v| v.as_enum())
                else {
                    return LangValue::from(LowValue::Parameterized);
                };
                // The argument is a scalar `USize` for an arity-1 kernel, or
                // an `Array` (possibly nested for a tuple-of-tuples domain)
                // for a tuple-domain kernel.  Flatten it to the wasm argument
                // vector.  Anything else (a non-literal element, e.g. a
                // computed scalar) stays lazy — the definition pass reports
                // the unbound result.
                let mut args: Vec<i64> = Vec::new();
                match module.node_value(operands[1].node).and_then(|v| v.as_enum()) {
                    Some(LowValue::USize(n)) => args.push(n as i64),
                    Some(LowValue::Array(_)) => {
                        if !collect_args(module, operands[1].node, &mut args) {
                            return LangValue::from(LowValue::Parameterized);
                        }
                    }
                    _ => return LangValue::from(LowValue::Parameterized),
                };
                match run_kernel(id, &args) {
                    Ok(result) => LangValue::from(LowValue::USize(result)),
                    Err(..) => LangValue::from(LowValue::Parameterized),
                }
            }
        }
    }
}

// --- Codegen: lichen graph → a scalar `(i64) -> i64` wasm function body -----

/// Lower `[param_pair] → function.return` for the kernel-safe subset (scalar
/// arith over a scalar or a tuple of scalars) into a [`KernelFragment`] — the
/// function's **body**, not a module.  `jit` emits a fragment per function;
/// module assembly is a separate step ([`link_fragment`]), so a later JIT can
/// emit fragments lazily and a `launch` can link a reachable set of them into
/// one module.
///
/// The parameter's domain shape (scalar vs tuple of scalars) is derived from
/// its *type* and recorded on the parameter's value cell — the level-3 shape
/// marker a backend reads instead of re-deriving the type half.  The body
/// emitter then reads that shape to distinguish a scalar parameter read
/// (`local.get 0`) from a tuple-element read (`local.get k`).
fn compile_fragment(
    module: &mut Module<LangProgram>,
    function: AnyFunctionId,
) -> Result<KernelFragment, String> {
    use wasm_encoder::{Function, Instruction};

    let AnyFunctionId::Dynamic(fid) = function else {
        return Err("static (imported) functions are not kernel-compilable v1".into());
    };
    let param_pair = module.functions[fid].parameter;
    let ret = module.functions[fid].r#return;
    // The function's `return` is the `[value, type]` pair node; the kernel's
    // result is the pair's *value* (element 0).
    let ret_value = match module.array_items(ret) {
        Some(items) if !items.is_empty() => dyn_node(items[0].node)?,
        _ => return Err("function return is not a [value, type] pair".into()),
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

    let mut body = Function::new([]);
    emit_node(module, param_pair, param_value, ret_value, &mut body)?;
    body.instruction(&Instruction::End);

    Ok(KernelFragment {
        param_shape,
        body: body.into_raw_body(),
    })
}

/// Lower a [`KernelFragment`] into a standalone wasm module exporting a single
/// `main` function.  This is the degenerate link — one fragment = one module.
/// A later step replaces it with a linker that assembles a kernel's whole
/// reachable fragment set and emits imports for cross-module callees.
fn link_fragment(fragment: &KernelFragment) -> Result<Vec<u8>, String> {
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, FunctionSection, Module as WasmModule, TypeSection,
        ValType,
    };

    let arity = flat_arity(&fragment.param_shape);
    let mut wasm = WasmModule::new();
    let mut types = TypeSection::new();
    types.ty().function(vec![ValType::I64; arity], vec![ValType::I64]);
    wasm.section(&types);
    let mut funcs = FunctionSection::new();
    funcs.function(0);
    wasm.section(&funcs);
    let mut exports = ExportSection::new();
    exports.export("main", ExportKind::Func, 0);
    wasm.section(&exports);
    let mut code = CodeSection::new();
    code.raw(&fragment.body);
    wasm.section(&code);
    Ok(wasm.finish())
}

/// The [`LowShape`] of a function's parameter, from its type cell: a tuple
/// parameter `(T0, .., Tn)` yields `Tuple(..)` with arity `n + 1`; an
/// (annotated or unannotated) scalar `Int` yields `USize`.  This is the one
/// place the codegen reads the type half — once, to seed the parameter's
/// shape marker; afterwards the body emitter reads only [`LowShape`]s.
fn kernel_param_shape(
    module: &Module<LangProgram>,
    param_pair: NodeId,
) -> Result<LowShape, String> {
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
fn element_shape(module: &Module<LangProgram>, type_node: AnyNodeId) -> Result<LowShape, String> {
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
        .and_then(|v| v.as_enum())
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

/// Emit wasm instructions for one lichen graph node — the scalar kernel-safe
/// subset: integer constants, `Add`/`Sub`/`Leq`/`Eq`, and the parameter value
/// (`Index(param_pair, 0)` → `local.get 0`).
fn emit_node(
    module: &Module<LangProgram>,
    param_pair: NodeId,
    param_value: NodeId,
    node: NodeId,
    body: &mut wasm_encoder::Function,
) -> Result<(), String> {
    use wasm_encoder::Instruction;

    if let Some(value) = module.node_value(AnyNodeId::Dynamic(node)) {
        match value.as_enum() {
            Some(LowValue::USize(n)) => {
                body.instruction(&Instruction::I64Const(n as i64));
                return Ok(());
            }
            _ => {}
        }
    }
    let Some(operation) = module.nodes[node].operation else {
        return Err(format!("kernel body hits a node with neither value nor operation"));
    };
    match &operation.operator {
        LangOperator::TypeOperator(op) => match op {
            TypeOperator::Add | TypeOperator::Sub | TypeOperator::Leq | TypeOperator::Eq => {
                let (left, right) = operand_pair(module, operation.operand)?;
                emit_node(module, param_pair, param_value, left, body)?;
                emit_node(module, param_pair, param_value, right, body)?;
                match op {
                    TypeOperator::Add => {
                        body.instruction(&Instruction::I64Add);
                    }
                    TypeOperator::Sub => {
                        body.instruction(&Instruction::I64Sub);
                    }
                    TypeOperator::Leq => {
                        body.instruction(&Instruction::I64LeS);
                        body.instruction(&Instruction::I64ExtendI32U);
                    }
                    TypeOperator::Eq => {
                        body.instruction(&Instruction::I64Eq);
                        body.instruction(&Instruction::I64ExtendI32U);
                    }
                    _ => unreachable!(),
                }
            }
            _ => return Err(format!("unsupported highlevel operator in kernel body: {op:?}")),
        },
        LangOperator::LowOperator(LowOperator::Index) => {
            let (target, index) = operand_pair(module, operation.operand)?;
            // A parameter read at some index path → a wasm `local.get`.  (This
            // must run before the value_of defuse: `Index(param_pair, 0)` is
            // a node's value slot, not a general extraction.)
            if let Some(path) = param_path(module, param_pair, node) {
                let domain = module
                    .node_shape(param_value)
                    .ok_or_else(|| "kernel parameter has no domain shape".to_string())?;
                let offset = flatten_offset(domain, &path)?;
                body.instruction(&Instruction::LocalGet(offset as u32));
                return Ok(());
            }
            // A `value_of` extraction — `Index(pair, 0)` with a constant `0`
            // index and `pair` a `[value, type]` pair.  Emit the pair's value
            // slot instead of treating the extraction as a real index.
            if usize_value(module, index) == Some(0) {
                if let Some(value_node) = value_of_node(module, node) {
                    return emit_node(module, param_pair, param_value, value_node, body);
                }
            }
            // A conditional `if c then a else b` lowers to `[b, a][c]` — a
            // 2-element array value indexed by a *computed* (non-constant)
            // selector, a wasm `select`.  The array may be reached through a
            // value_of extraction; look through it.
            if usize_value(module, index).is_none() {
                if let Some(array_value) = value_of_node(module, target).or(Some(target)) {
                    if let Some(items) = module.array_items(array_value) {
                        if items.len() == 2 {
                            let then_node = dyn_node(items[1].node)?;
                            let else_node = dyn_node(items[0].node)?;
                            emit_node(module, param_pair, param_value, then_node, body)?;
                            emit_node(module, param_pair, param_value, else_node, body)?;
                            emit_node(module, param_pair, param_value, index, body)?;
                            body.instruction(&Instruction::I32WrapI64);
                            body.instruction(&Instruction::Select);
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
        other => {
            return Err(format!(
                "unsupported operation in kernel body: {other:?} (kernel-safe subset is scalar arith)"
            ))
        }
    }
    Ok(())
}

/// Is `node` the parameter's *value* node — `Index(param_pair, 0)`?  The
/// scalar `Int -> Int` kernel reads the value directly; the tuple-domain
/// kernel reads element `k` through `Index(Index(param_pair, 0), k)`, whose
/// target is this node.
fn is_param_value(module: &Module<LangProgram>, param_pair: NodeId, node: NodeId) -> bool {
    let Some(operation) = module.nodes[node].operation else {
        return false;
    };
    if !matches!(operation.operator, LangOperator::LowOperator(LowOperator::Index)) {
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
        .and_then(|v| v.as_enum())
        .is_some_and(|v| matches!(v, LowValue::USize(0)))
}

/// The constant `USize` value behind `node`, if it is one (an `Index`'s
/// selector must be a compile-time constant in a kernel body).
fn usize_value(module: &Module<LangProgram>, node: NodeId) -> Option<usize> {
    match module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|v| v.as_enum())
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
fn value_of_node(module: &Module<LangProgram>, node: NodeId) -> Option<NodeId> {
    let operation = module.nodes[node].operation?;
    if !matches!(operation.operator, LangOperator::LowOperator(LowOperator::Index)) {
        return None;
    }
    let (target, index) = operand_pair(module, operation.operand).ok()?;
    if usize_value(module, index)? != 0 {
        return None;
    }
    let items = module.array_items(target)?;
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
fn param_path(module: &Module<LangProgram>, param_pair: NodeId, node: NodeId) -> Option<Vec<usize>> {
    let operation = module.nodes[node].operation?;
    if !matches!(operation.operator, LangOperator::LowOperator(LowOperator::Index)) {
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
fn collect_args(module: &Module<LangProgram>, node: AnyNodeId, out: &mut Vec<i64>) -> bool {
    match module.node_value(node).and_then(|v| v.as_enum()) {
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
fn operand_pair(module: &Module<LangProgram>, operand: Option<NodeId>) -> Result<(NodeId, NodeId), String> {
    let Some(operand) = operand else {
        return Err("binary operator/Index operand is missing".into());
    };
    let items = operand_items(module, operand)?;
    if items.len() != 2 {
        return Err("binary/Index operand array must have two elements".into());
    }
    Ok((dyn_node(items[0].node)?, dyn_node(items[1].node)?))
}

fn operand_items(module: &Module<LangProgram>, node: NodeId) -> Result<&'static [ArrayItem], String> {
    module.array_items(node).ok_or_else(|| "operand is not an array value".into())
}

fn dyn_node(id: AnyNodeId) -> Result<NodeId, String> {
    match id {
        AnyNodeId::Dynamic(n) => Ok(n),
        AnyNodeId::Static(_) => Err("static refs are not kernel-compilable v1".into()),
    }
}

/// Execute a compiled kernel on an argument vector with wasmi, returning the
/// `usize` result.  The dynamic [`wasmi::Func::call`] API accepts any number
/// of `i64` inputs, so a tuple-domain kernel (arity N) launches with N
/// arguments and a scalar kernel (arity 1) with one.
fn run_kernel(id: KernelId, args: &[i64]) -> Result<usize, String> {
    let fragment = kernels()
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| "kernel id is not registered".to_string())?;
    let bytes = link_fragment(&fragment)?;
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

// --- Native operators: the private contract with the plugin's source -------

/// `$jit(f)` — compile a function to a kernel.  The function-ness gate unifies
/// the argument's type with an arrow shape (the *gate*); the kernel type
/// `[sig, [TypeKernel, Type]]` carries the arrow's `[in, out]` shape as its
/// signature, so `launch` reads the domain/codomain out of the type.
struct JitOp;
static JIT_OP: JitOp = JitOp;

/// `$launch(k, a)` — run kernel `k` on `a`.  The kernel-ness gate unifies `k`'s
/// type with a kernel type (binding the domain/codomain to fresh cells); the
/// argument is unified against the domain and the result typed as the codomain.
struct LaunchOp;
static LAUNCH_OP: LaunchOp = LaunchOp;

/// The `lichen-compute` plugin's private native registry: the ops its embedded
/// source calls with `$jit`/`$launch`.  Attached only to the compilation of
/// `compute.lichen`, so the names resolve privately — no global string
/// namespace, so a second plugin registering its own `$jit` never collides.
pub fn native_ops() -> NativeOps<LangProgram> {
    &NATIVE_OPS
}
static NATIVE_OPS: [(&str, &dyn NativeOp<LangProgram>); 2] = [
    ("jit", &JIT_OP),
    ("launch", &LAUNCH_OP),
];

impl NativeOp<LangProgram> for JitOp {
    fn build(
        &self,
        ctx: &mut dyn Ctx<LangProgram>,
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
        let fn_marker = ctx.value_node(LangValue::function_type_marker());
        let universe = ctx.universe();
        let kind = ctx.array_node(&[fn_marker, universe]);
        let fn_ty = ctx.array_node(&[shape, kind]);
        ctx.check_unify(f.ty, fn_ty, loc, DiagKind::Guard);

        // Kernel type: `[sig, [TypeKernel, Type]]` where `sig = [d, c]` is the
        // arrow's signature — this is what `launch` reads the domain/codomain
        // from.  It references the cells the gate just bound, so a concrete
        // function signature flows into the kernel's type.
        let sig = ctx.array_node(&[d, c]);
        let k_marker = ctx.value_node(LangValue::from(ComputeValue::TypeKernel));
        let universe = ctx.universe();
        let k_kind = ctx.array_node(&[k_marker, universe]);
        let kernel_ty = ctx.array_node(&[sig, k_kind]);

        let op = ctx.op_node(LangOperator::from(ComputeOperator::Jit), Some(f.value));
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

impl NativeOp<LangProgram> for LaunchOp {
    fn build(
        &self,
        ctx: &mut dyn Ctx<LangProgram>,
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
        let k_marker = ctx.value_node(LangValue::from(ComputeValue::TypeKernel));
        let universe = ctx.universe();
        let k_kind = ctx.array_node(&[k_marker, universe]);
        let kernel_ty = ctx.array_node(&[sig, k_kind]);
        ctx.check_unify(k.ty, kernel_ty, loc.clone(), DiagKind::Guard);
        // Unify the argument against the kernel's domain.
        ctx.check_unify(a.ty, d, loc.clone(), DiagKind::Guard);
        // Emit the `Launch` operator over `[k, a]`, typed as the codomain.
        let operands = ctx.array_node(&[k.value, a.value]);
        let op = ctx.op_node(LangOperator::from(ComputeOperator::Launch), Some(operands));
        let pair = ctx.array_node(&[op, c]);
        NativeApply {
            node: pair,
            val: None,
            ty: c,
        }
    }
}

/// The `lichen-compute` plugin's embedded lichen source — the real `compute`
/// plugin file.  It defines the user-facing `jit`/`launch` functions as
/// ordinary typed lichen (whose bodies call the native `$jit`/`$launch`), and
/// exports them as the positional namespace tuple `compute`.
pub const WRAPPER_SOURCE: &str = "\
jit = f => $jit(f)
launch = k => a => $launch(k, a)
(jit, launch)
";
