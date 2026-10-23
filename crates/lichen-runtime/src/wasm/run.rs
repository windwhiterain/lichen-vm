//! Running an assembled kernel on the engine.
//!
//! # Invariant
//!
//! The engine is the runtime's: a module that reaches a value is the module the
//! assembler wrote, and the values that cross the boundary carry their class, so
//! a kernel that answers the wrong class is refused rather than read as another.

use std::collections::HashMap;

use lichen_kernel_ir::{KernelFragment, KernelId, ScalarClass};

use crate::module_cache::cached_module;

/// The value the engine hands a kernel, in the class's own lane.
///
/// # Invariant
/// wasm has no 16-bit float, so a bf16 takes the `F32` lane the assembler
/// declares for it: the pattern is the half's own, and a module carrying one
/// cannot link the host's import expr.
pub fn to_wasmi(class: ScalarClass, bits: i64) -> wasmi::Val {
    match class {
        ScalarClass::Int => wasmi::Val::I64(bits),
        ScalarClass::Float => wasmi::Val::F32(wasmi::F32::from_bits(bits as u32)),
        ScalarClass::BFloat16 => wasmi::Val::F32(wasmi::F32::from_bits(bits as u32 & !0xFFFF)),
    }
}

/// The zero a run's result vector starts from — `wasmi` sizes its result
/// slots, it does not fill them.
pub fn wasmi_zero(class: ScalarClass) -> wasmi::Val {
    to_wasmi(class, 0)
}

/// The bits `value` holds, read as `class`.
///
/// # Invariant
/// The lane carries the half in the high bits, so the word is that pattern at
/// the width the class holds.
pub fn from_wasmi(class: ScalarClass, value: &wasmi::Val) -> Option<i64> {
    let bits = match class {
        ScalarClass::Int => value.i64()?,
        ScalarClass::Float => i64::from(value.f32()?.to_bits()),
        ScalarClass::BFloat16 => i64::from(value.f32()?.to_bits() & !0xFFFF),
    };
    Some(bits)
}

/// The word a `read`/`write` import's value argument carries.
///
/// # Invariant
/// A float arrives as `f32` whatever the class that named the import, so a
/// `BFloat16`'s word is the half's own pattern at the width its class holds.
pub fn value_word(class: ScalarClass, value: &wasmi::Val) -> i64 {
    match class {
        ScalarClass::Int => value.i64().unwrap_or(0),
        ScalarClass::Float => i64::from(value.f32().map_or(0.0, |bits| bits.to_float()).to_bits()),
        ScalarClass::BFloat16 => {
            i64::from(value.f32().map_or(0.0, |bits| bits.to_float()).to_bits() & !0xFFFF)
        }
    }
}

/// Run the kernel `id` over `args` and answer its results.
///
/// # Invariant
/// The module is the one `assemble` writes, cached by the kernel's id, and the
/// engine that runs it is the runtime's: a backend that runs no wasm never
/// resolves this. The output count is the caller's fact, so a fragment that
/// leaves one value at least still has a slot to answer in.
pub fn run_kernel(
    id: KernelId,
    assemble: impl FnOnce() -> Result<Vec<u8>, String>,
    class: ScalarClass,
    outputs: usize,
    args: &[(ScalarClass, i64)],
) -> Result<Vec<i64>, String> {
    let (engine, module) = cached_module(id, assemble)?;
    let mut store = wasmi::Store::new(&engine, ());
    let linker = wasmi::Linker::new(&engine);
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| e.to_string())?;
    let main = instance
        .get_func(&store, "main")
        .ok_or_else(|| "kernel has no export `main`".to_string())?;
    let inputs: Vec<wasmi::Val> = args
        .iter()
        .map(|(class, bits)| to_wasmi(*class, *bits))
        .collect();
    let mut results: Vec<wasmi::Val> = (0..outputs.max(1)).map(|_| wasmi_zero(class)).collect();
    main.call(&mut store, &inputs, &mut results)
        .map_err(|e| e.to_string())?;
    results
        .iter()
        .enumerate()
        .map(|(position, value)| {
            from_wasmi(class, value).ok_or_else(|| {
                format!(
                    "kernel `main` returned a non-{class:?} value at result {position}, against \
                     the {class:?} signature this module was emitted for"
                )
            })
        })
        .collect()
}

/// The fragments a launch set names, in the order the assembler wants them.
pub type OrderedLaunchSet = (Vec<KernelFragment>, HashMap<KernelId, u32>);
