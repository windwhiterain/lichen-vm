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

use lichen_highlevel::checker::Checker;
use lichen_highlevel::diagnostic::DiagKind;
use lichen_highlevel::ir::{ExprId, Span};
use lichen_highlevel::native::{NativeApply, NativeExt};
use lichen_highlevel::program::{TypeOperator, ValueType};
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

/// A native operator bound to source as a value (Option B).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NativeOp {
    Jit,
    Launch,
}

/// The intermediate of a curried `launch k` (stage 1): captures the kernel
/// value node and the kernel's signature cells, so the outer `(launch k) a`
/// (stage 2) can emit the `Launch` operator and type-check the argument.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaunchTarget {
    /// The kernel value node — the `k` of `launch k`.
    pub kernel: NodeId,
    /// The kernel's domain type cell.
    pub domain: NodeId,
    /// The kernel's codomain type cell.
    pub codomain: NodeId,
}

/// The compute value vocabulary — injected as a sibling leaf into the
/// language's value union (see [`crate::program`]).  A plain enum of exactly
/// this extension's variants, composed with [`lichen_utils::enum_ext!`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComputeValue {
    /// A compiled, runnable kernel artifact.
    Kernel(KernelId),
    /// A first-class native operator value (`jit`/`launch`).
    Native(NativeOp),
    /// The kind marker of kernel types — a kernel's type is
    /// `[signature, [TypeKernel, Type]]`.
    TypeKernel,
    /// The type of a `Native(Jit)` value — the callee type that routes an
    /// `Apply` to the `Jit` native operator.
    TypeNativeJit,
    /// The type of a `Native(Launch)` value — routes an `Apply` to the first
    /// stage of the curried `launch`.
    TypeNativeLaunch,
    /// The type of a [`LaunchTarget`] value — routes an `Apply` to the second
    /// stage of the curried `launch`.
    TypeLaunchTarget,
    /// The value produced by `launch k`: a function-shaped intermediate that
    /// the second apply `(launch k) a` completes.
    LaunchTarget(LaunchTarget),
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

/// The type of a [`Native(Jit)`](ComputeValue::Native) value.
pub fn jit_native_type() -> ComputeValue {
    ComputeValue::TypeNativeJit
}
pub fn launch_native_type() -> ComputeValue {
    ComputeValue::TypeNativeLaunch
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
                    Err(..) => {
                        // The body uses an operator outside the kernel-safe
                        // subset — record nothing and stay lazy; the definition
                        // pass's error channel reports the unbound result.
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

    if let Some(value) = module.nodes[node].value {
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
                && module.nodes[index]
                    .value
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

// --- The virtual compute package (the import bridge) ----------------------

/// The static export nodes of the compute package.  The package exposes a
/// tuple `[jit, launch, Kernel]` (for positional namespace access) **and** the
/// three items as direct `[value, type]` pairs (so an import can bind them as
/// names, which is what the checker needs to detect a native callee
/// statically — a tuple element read would stay a lazy `Index`).
pub(crate) struct ComputePackageExports {
    /// The exported tuple `[jit, launch, Kernel]` pair.
    pub export_pair: NodeId,
    /// The `[jit]` direct pair.
    pub jit: NodeId,
    /// The `[launch]` direct pair.
    pub launch: NodeId,
    /// The `[Kernel]` direct pair.
    pub kernel: NodeId,
}

/// Build the frozen `compute` package's module and its export nodes.
pub(crate) fn build_compute_module() -> (Module<LangProgram>, ComputePackageExports) {
    use lichen_highlevel::program::TypeValue;

    let mut module = Module::<LangProgram>::new();
    let block = module.add_block(None);
    // The canonical universe `[Type, ↺]` (`Type : Type`).
    let type_marker = module.add_node(block, None, Some(LangValue::type_marker()));
    let universe = module.add_node(block, None, None);
    let univ = [
        ArrayItem::new(AnyNodeId::Dynamic(type_marker)),
        ArrayItem::new(AnyNodeId::Dynamic(universe)),
    ];
    let univ_payload = module.alloc_array(&univ, block);
    module.nodes[universe].value = Some(LangValue::from(LowValue::Array(univ_payload)));

    let fn_node = |module: &mut Module<LangProgram>, v: LangValue| {
        module.add_node(block, None, Some(v))
    };

    // The three native exports and their element-type markers.
    let jit_v = fn_node(&mut module, LangValue::from(ComputeValue::Native(NativeOp::Jit)));
    let jit_ty = fn_node(&mut module, LangValue::from(ComputeValue::TypeNativeJit));
    let launch_v = fn_node(&mut module, LangValue::from(ComputeValue::Native(NativeOp::Launch)));
    let launch_ty = fn_node(&mut module, LangValue::from(ComputeValue::TypeNativeLaunch));
    let kernel_v = fn_node(&mut module, LangValue::from(ComputeValue::TypeKernel));
    let kernel_ty = fn_node(&mut module, LangValue::type_marker());

    // Direct `[value, type]` pairs for name binding.
    let jit = array_value(&mut module, block, &[jit_v, jit_ty]);
    let launch = array_value(&mut module, block, &[launch_v, launch_ty]);
    let kernel = array_value(&mut module, block, &[kernel_v, kernel_ty]);

    // Tuple value `[jit, launch, Kernel]`.
    let tuple_value = array_value(&mut module, block, &[jit_v, launch_v, kernel_v]);
    // Tuple type `[ [jit_ty, launch_ty, kernel_ty], [TypeTuple, Type] ]`.
    let shape = array_value(&mut module, block, &[jit_ty, launch_ty, kernel_ty]);
    let tuple_type_marker = fn_node(&mut module, LangValue::TypeValue(TypeValue::TypeTuple));
    let tuple_kind = array_value(&mut module, block, &[tuple_type_marker, universe]);
    let tuple_ty = array_value(&mut module, block, &[shape, tuple_kind]);

    // The exported `[value, type]` pair.
    let export_pair = array_value(&mut module, block, &[tuple_value, tuple_ty]);
    (
        module,
        ComputePackageExports {
            export_pair,
            jit,
            launch,
            kernel,
        },
    )
}

fn array_value(module: &mut Module<LangProgram>, block: BlockId, ids: &[NodeId]) -> NodeId {
    let items: Vec<ArrayItem> = ids
        .iter()
        .map(|&n| ArrayItem::new(AnyNodeId::Dynamic(n)))
        .collect();
    let payload = module.alloc_array(&items, block);
    module.add_node(
        block,
        None,
        Some(LangValue::from(LowValue::Array(payload))),
    )
}

// --- Executing and building kernels ---------------------------------------

struct JitNativeExt;
struct LaunchNativeExt;
struct LaunchTargetNativeExt;

/// The native-operator registry for [`LangProgram`]: maps a callee's *type* to
/// the [`NativeExt`] that lowers it.  This is what makes `jit f` / `launch k a`
/// a normal `Apply` — the callee's type carries the operator identity.
pub fn native_registry(
) -> Box<dyn Fn(&LangValue) -> Option<&'static dyn NativeExt<LangProgram>>> {
    Box::new(|ty_value: &LangValue| match ty_value.as_enum() {
        Some(ComputeValue::TypeNativeJit) => Some(&JIT),
        Some(ComputeValue::TypeNativeLaunch) => Some(&LAUNCH),
        Some(ComputeValue::TypeLaunchTarget) => Some(&LAUNCH_TARGET),
        _ => None,
    })
}

static JIT: JitNativeExt = JitNativeExt;
static LAUNCH: LaunchNativeExt = LaunchNativeExt;
static LAUNCH_TARGET: LaunchTargetNativeExt = LaunchTargetNativeExt;

impl NativeExt<LangProgram> for JitNativeExt {
    fn check_apply(
        &self,
        checker: &mut Checker<LangProgram>,
        _e: ExprId,
        _callee_value: NodeId,
        _callee_ty: NodeId,
        argument_value: NodeId,
        argument_ty: NodeId,
        _argument: ExprId,
        span: Option<Span>,
    ) -> NativeApply {
        let block = checker.current_block;
        // Function-ness gate: `jit` on a *concretely non-function* value is an
        // error (mirroring the ordinary apply guard).
        let d = checker.fresh_cell();
        let c = checker.fresh_cell();
        let shape = checker.array_node(block, &[d, c]);
        let fn_marker = checker.value_node(LangValue::function_type_marker());
        let universe = checker.type_expr_node();
        let kind = checker.array_node(block, &[fn_marker, universe]);
        let fn_ty = checker.array_node(block, &[shape, kind]);
        checker.check_unify(argument_ty, fn_ty, span, DiagKind::Guard);

        // Kernel type: `Kernel<Int -> Int>` = `[[int, int], [TypeKernel, Type]]`.
        let int_ty = checker.int_type_node();
        let sig = checker.array_node(block, &[int_ty, int_ty]);
        let k_marker = checker.value_node(LangValue::from(ComputeValue::TypeKernel));
        let universe = checker.type_expr_node();
        let k_kind = checker.array_node(block, &[k_marker, universe]);
        let kernel_ty = checker.array_node(block, &[sig, k_kind]);

        let op = checker.op_node(
            block,
            LangOperator::from(ComputeOperator::Jit),
            Some(argument_value),
        );
        // The expression's `term` must evaluate to a `[value, type]` pair, so
        // `value_of` can `Index` it; the op's own result is the bare `Kernel`.
        let pair = checker.array_node(block, &[op, kernel_ty]);
        NativeApply {
            node: pair,
            val: None,
            ty: kernel_ty,
        }
    }
}

impl NativeExt<LangProgram> for LaunchNativeExt {
    fn check_apply(
        &self,
        checker: &mut Checker<LangProgram>,
        _e: ExprId,
        _callee_value: NodeId,
        _callee_ty: NodeId,
        argument_value: NodeId,
        argument_ty: NodeId,
        _argument: ExprId,
        span: Option<Span>,
    ) -> NativeApply {
        let block = checker.current_block;
        // Kernel-ness gate: `launch` on a *concretely non-kernel* value is an
        // error.  The fresh kernel type's signature cells bind to the kernel's
        // actual domain/codomain.
        let d = checker.fresh_cell();
        let c = checker.fresh_cell();
        let shape = checker.array_node(block, &[d, c]);
        let k_marker = checker.value_node(LangValue::from(ComputeValue::TypeKernel));
        let universe = checker.type_expr_node();
        let k_kind = checker.array_node(block, &[k_marker, universe]);
        let kernel_ty = checker.array_node(block, &[shape, k_kind]);
        checker.check_unify(argument_ty, kernel_ty, span, DiagKind::Guard);

        // The curried intermediate: `launch k` is a function-shaped value
        // `domain -> codomain` that the outer apply completes.
        let lt = ComputeValue::LaunchTarget(LaunchTarget {
            kernel: argument_value,
            domain: d,
            codomain: c,
        });
        let lt_node = checker.value_node(LangValue::from(lt));
        let lt_ty = checker.value_node(LangValue::from(ComputeValue::TypeLaunchTarget));
        NativeApply {
            node: lt_node,
            val: Some(lt_node),
            ty: lt_ty,
        }
    }
}

impl NativeExt<LangProgram> for LaunchTargetNativeExt {
    fn check_apply(
        &self,
        checker: &mut Checker<LangProgram>,
        _e: ExprId,
        callee_value: NodeId,
        _callee_ty: NodeId,
        argument_value: NodeId,
        argument_ty: NodeId,
        _argument: ExprId,
        span: Option<Span>,
    ) -> NativeApply {
        let Some(ComputeValue::LaunchTarget(lt)) =
            checker.class_value(callee_value).and_then(|v| v.as_enum())
        else {
            unreachable!("a LaunchTarget-typed callee must carry a LaunchTarget value")
        };
        let block = checker.current_block;
        // Unify the argument against the kernel's domain.
        checker.check_unify(argument_ty, lt.domain, span, DiagKind::Guard);
        // Emit the `Launch` operator with operand `[kernel, arg]`.
        let operands = checker.array_node(block, &[lt.kernel, argument_value]);
        let op = checker.op_node(
            block,
            LangOperator::from(ComputeOperator::Launch),
            Some(operands),
        );
        // The expression's `term` must evaluate to a `[value, type]` pair, so
        // `value_of` can `Index` it; the op's own result is the bare result.
        let pair = checker.array_node(block, &[op, lt.codomain]);
        NativeApply {
            node: pair,
            val: None,
            ty: lt.codomain,
        }
    }
}
