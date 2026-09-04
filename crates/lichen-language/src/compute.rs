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
    AnyFunctionId, AnyNodeId, ArrayItem, BlockId, LowOperator, LowValue, Module, NodeId,
    OperatorExt,
};
use lichen_utils::extend::AsEnum;

use crate::program::{LangOperator, LangProgram, LangValue};

/// A compiled kernel artifact's identity — a compact index into the process
/// kernel registry (the compiled wasm bytes).  A kernel value is host-owned
/// (a small `Copy` scalar), so it is never an arena payload and never needs GC
/// re-homing or static freeze.
pub type KernelId = usize;

/// The process kernel registry: compiled wasm bytes, keyed by [`KernelId`].
/// Kernels are immutable artifacts shared across modules in the process.
static KERNELS: OnceLock<Mutex<HashMap<KernelId, Vec<u8>>>> = OnceLock::new();
fn kernels() -> &'static Mutex<HashMap<KernelId, Vec<u8>>> {
    KERNELS.get_or_init(Default::default)
}
/// The next kernel id — process-global, so ids never collide across modules.
static NEXT_KERNEL_ID: AtomicUsize = AtomicUsize::new(0);
fn alloc_kernel_id() -> KernelId {
    NEXT_KERNEL_ID.fetch_add(1, Ordering::Relaxed)
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
                match codegen_function(module, function) {
                    Ok(bytes) => {
                        let id = alloc_kernel_id();
                        kernels().lock().unwrap().insert(id, bytes);
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
                let Some(LowValue::USize(arg)) = module
                    .node_value(operands[1].node)
                    .and_then(|v| v.as_enum())
                else {
                    return LangValue::from(LowValue::Parameterized);
                };
                match run_kernel(id, arg) {
                    Ok(result) => LangValue::from(LowValue::USize(result)),
                    Err(..) => LangValue::from(LowValue::Parameterized),
                }
            }
        }
    }
}

// --- Codegen: lichen graph → a scalar `(i64) -> i64` wasm module ----------

/// Lower `[param_pair] → function.return` for the scalar `Int -> Int` kernel
/// subset and emit a wasm module exporting `(func "main" (param i64)
/// (result i64))`.
fn codegen_function(
    module: &Module<LangProgram>,
    function: AnyFunctionId,
) -> Result<Vec<u8>, String> {
    use wasm_encoder::{CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, Module as WasmModule, TypeSection, ValType};

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

    let mut wasm = WasmModule::new();
    let mut types = TypeSection::new();
    types.ty().function([ValType::I64], [ValType::I64]);
    wasm.section(&types);
    let mut funcs = FunctionSection::new();
    funcs.function(0);
    wasm.section(&funcs);
    let mut exports = ExportSection::new();
    exports.export("main", ExportKind::Func, 0);
    wasm.section(&exports);

    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    emit_node(module, param_pair, ret_value, &mut body)?;
    body.instruction(&Instruction::End);
    code.function(&body);
    wasm.section(&code);
    Ok(wasm.finish())
}

/// Emit wasm instructions for one lichen graph node — the scalar kernel-safe
/// subset: integer constants, `Add`/`Sub`/`Leq`/`Eq`, and the parameter value
/// (`Index(param_pair, 0)` → `local.get 0`).
fn emit_node(
    module: &Module<LangProgram>,
    param_pair: NodeId,
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
                emit_node(module, param_pair, left, body)?;
                emit_node(module, param_pair, right, body)?;
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
            // The parameter value: `Index(parameter_pair, 0)` → `local.get 0`.
            let (target, index) = operand_pair(module, operation.operand)?;
            if target == param_pair
                && module
                    .node_value(AnyNodeId::Dynamic(index))
                    .and_then(|v| v.as_enum())
                    .is_some_and(|v| matches!(v, LowValue::USize(0)))
            {
                body.instruction(&Instruction::LocalGet(0));
                return Ok(());
            }
            return Err("unsupported index in kernel body (only `Index(param, 0)`)".into());
        }
        other => {
            return Err(format!(
                "unsupported operation in kernel body: {other:?} (kernel-safe subset is scalar arith)"
            ))
        }
    }
    Ok(())
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

/// Execute a compiled kernel on a scalar argument with wasmi, returning the
/// `usize` result.
fn run_kernel(id: KernelId, arg: usize) -> Result<usize, String> {
    let bytes = kernels()
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| "kernel id is not registered".to_string())?;
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes).map_err(|e| e.to_string())?;
    let mut store = wasmi::Store::new(&engine, ());
    let linker = wasmi::Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_typed_func::<i64, i64>(&store, "main")
        .map_err(|e| e.to_string())?;
    let result = main
        .call(&mut store, arg as i64)
        .map_err(|e| e.to_string())?;
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
