//! Assembling a launch set onto a `waffle` module.
//!
//! # Invariant
//! What must be right is not how a section is serialized but the order entities are pushed in,
//! because the order is the index space. Three facts are reproduced exactly: the `read`/`write`
//! imports per class with each class's read first, one function per fragment in BFS order with
//! position 0 exported as `main`, and a call index of `base` plus the callee's position.

use std::collections::HashMap;

use lichen_kernel_ir::{KernelFragment, KernelId, ScalarClass};
use waffle::{
    Export, ExportKind, Func, FuncDecl, FunctionBody, Import, ImportKind, Module, Signature,
    SignatureData, Type, Value,
};

use super::lower::{ModuleCtx, lower_body};
use super::mixed::refuse_mixed_classes;
use crate::compute::{buffer_import_name, param_classes};

/// The wasm value type a class is lowered to.
pub(super) fn value_type(class: ScalarClass) -> Type {
    match class {
        ScalarClass::Int => Type::I64,
        ScalarClass::Float => Type::F32,
    }
}

/// The buffer imports the launch set declares, and where the defined functions
/// start.
pub(super) struct BufferImports {
    /// The wasm function index the first defined function takes: base + i is ordered[i].
    pub(super) base: u32,
    pub(super) read: HashMap<ScalarClass, Func>,
    pub(super) write: HashMap<ScalarClass, Func>,
}

/// Assemble an ordered slice of kernel fragments into a single wasm module.
///
/// # Invariant
/// `ordered[i]` becomes function index `base + i` and the root is exported as `main`; `index`
/// maps each callee to its position, so a cross-kernel call lowers to an in-module `call`.
pub(crate) fn assemble_module(
    ordered: &[KernelFragment],
    index: &HashMap<KernelId, u32>,
) -> Result<Vec<u8>, String> {
    let Some((root, rest)) = ordered.split_first() else {
        return Err("compute.wasm: a launch set must hold at least the root fragment".to_string());
    };

    // Every fragment is read before the first entity exists: a class is a property of the
    // IR, so a refusal costs nothing.
    for fragment in ordered {
        refuse_mixed_classes(fragment)?;
    }

    let mut module = Module::empty();
    let imports = declare_buffer_imports(&mut module, &buffered_classes(ordered));

    for (position, fragment) in std::iter::once(root).chain(rest).enumerate() {
        let signature = module.signatures.push(SignatureData {
            params: param_classes(fragment)
                .into_iter()
                .map(value_type)
                .collect(),
            returns: fragment
                .result_classes
                .iter()
                .copied()
                .map(value_type)
                .collect(),
        });
        let body = lower_into(fragment, ordered, index, &module, signature, &imports)?;
        // The function index is the push order: a cross-kernel call resolves through
        // `base` plus the callee's position.
        module
            .funcs
            .push(FuncDecl::Body(signature, format!("kernel{position}"), body));
    }

    module.exports.push(Export {
        name: "main".to_string(),
        // The root is the first defined function, which sits after every import.
        kind: ExportKind::Func(Func::from(imports.base)),
    });

    module
        .to_wasm_bytes()
        .map_err(|failure| format!("compute.wasm: waffle could not compile the module: {failure}"))
}

/// Build one fragment's waffle body, with the launch set as its context.
///
/// # Invariant
/// The signature builds the entry block's blockparams and nothing may add to them, so the
/// body's parameter values *are* those blockparams — one read for a function argument and a
/// loop's carried value.
fn lower_into<'a>(
    fragment: &'a KernelFragment,
    ordered: &'a [KernelFragment],
    index: &HashMap<KernelId, u32>,
    module: &Module,
    signature: waffle::Signature,
    imports: &BufferImports,
) -> Result<FunctionBody, String> {
    let leaves = param_classes(fragment);
    let mut builder = FunctionBody::new(module, signature);
    // The entry block's parameters are its blockparams, built from the signature and
    // not addable to.
    let params: Vec<Value> = (0..builder.n_params as u32).map(Value::from).collect();
    let context = ModuleCtx {
        callees: ordered,
        index,
        imports,
        leaves: &leaves,
        params: &params,
    };
    lower_body(&fragment.body, &mut builder, &context)?;
    Ok(builder)
}

/// Declare one `read`/`write` import pair per class, the reads first.
///
/// # Invariant
/// The order is the ABI's, because it decides the import indices, and
/// `run_parallel_range` resolves its closures against the same list.
fn declare_buffer_imports(module: &mut Module, classes: &[ScalarClass]) -> BufferImports {
    let mut imports = BufferImports {
        base: 0,
        read: HashMap::new(),
        write: HashMap::new(),
    };
    for class in classes {
        // The position and the index are `i64` in every class; only the element's own
        // type follows the class.
        let value = value_type(*class);
        let signature = module.signatures.push(SignatureData {
            params: vec![Type::I64, Type::I64],
            returns: vec![value],
        });
        imports.read.insert(
            *class,
            declare_import(module, signature, buffer_import_name(*class, "read")),
        );
    }
    for class in classes {
        let signature = module.signatures.push(SignatureData {
            params: vec![Type::I64, Type::I64, value_type(*class)],
            returns: Vec::new(),
        });
        imports.write.insert(
            *class,
            declare_import(module, signature, buffer_import_name(*class, "write")),
        );
    }
    // Every function entity pushed so far is an import, and no defined function
    // exists yet — so this count *is* the base.
    imports.base = u32::try_from(module.funcs.len()).unwrap_or(u32::MAX);
    imports
}

/// Push one function import and record it against the module.
fn declare_import(module: &mut Module, signature: Signature, name: String) -> Func {
    let function = module.funcs.push(FuncDecl::Import(signature, name.clone()));
    module.imports.push(Import {
        module: "env".to_string(),
        name,
        kind: ImportKind::Func(function),
    });
    function
}

/// The classes a launch set's buffer calls name, `Int` before `Float`, empty for a set
/// that calls neither.
///
/// # Invariant
/// One pair per class the set uses, so a class nobody reads or writes costs no import — which
/// is what lets one module hold fragments of different classes. Every instruction is walked,
/// transfers included: a read or write inside a branch still needs its import declared.
fn buffered_classes(ordered: &[KernelFragment]) -> Vec<ScalarClass> {
    let mut classes: Vec<ScalarClass> = Vec::new();
    for fragment in ordered {
        for instruction in fragment.body.instrs() {
            let class = match instruction {
                lichen_kernel_ir::KernelInstr::BufferReadCall(class)
                | lichen_kernel_ir::KernelInstr::BufferWriteCall(class) => *class,
                _ => continue,
            };
            if !classes.contains(&class) {
                classes.push(class);
            }
        }
    }
    // A fixed order, so the indices follow the set rather than the walk.
    classes.sort_by_key(|class| match class {
        ScalarClass::Int => 0,
        ScalarClass::Float => 1,
    });
    classes
}
