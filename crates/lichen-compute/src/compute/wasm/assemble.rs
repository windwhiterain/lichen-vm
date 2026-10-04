//! Assembling a launch set onto a `waffle` [`Module`].
//!
//! **This replaces the emitter's own section-by-section assembly rather than
//! sitting beside it.** `waffle`'s `to_wasm_bytes` writes every section from the
//! module's index spaces, so what this file has to get right is not *how* a
//! section is serialized but *what order the entities are pushed in* — the order
//! is the index space, and the host's linker resolves by name while the
//! cross-kernel call indices are computed from it.
//!
//! The three facts that must be reproduced exactly:
//!
//! - imports `env.read_i64`/`env.write_i64` (and the `f32` pair), **each class's
//!   `read` first and then its `write`**, so `base` is `2 ×` the classes;
//! - one function per fragment, in the `ordered` BFS order, with the root
//!   (position 0) exported as `main`;
//! - a [`KernelInstr::CallKernel`]'s index is `base` + the callee's position.

use std::collections::HashMap;

use lichen_kernel_ir::{KernelFragment, KernelId, ScalarClass};
use waffle::{
    Export, ExportKind, Func, FuncDecl, Import, ImportKind, Module, Signature, SignatureData, Type,
};

use super::flow::lower_fragment;
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
    /// The wasm function index the first defined function takes, which is every
    /// buffer import — so `base + i` is `ordered[i]`.
    pub(super) base: u32,
    pub(super) read: HashMap<ScalarClass, Func>,
    pub(super) write: HashMap<ScalarClass, Func>,
}

/// Assemble an ordered slice of kernel fragments into a single wasm module.
///
/// `ordered[i]` becomes wasm function index `base + i`; `index` maps each callee
/// [`KernelId`] to its position, so a cross-kernel [`KernelInstr::CallKernel`]
/// lowers to an in-module `call`. The root (position 0) is exported as `main`.
/// For a single-kernel set this is the degenerate link — one fragment = one
/// module; for a kernel that cross-calls others it is the launch-time assembly
/// that pulls the relative kernel set into one module.
pub(crate) fn assemble_module(
    ordered: &[KernelFragment],
    index: &HashMap<KernelId, u32>,
) -> Result<Vec<u8>, String> {
    let Some((root, rest)) = ordered.split_first() else {
        return Err("compute.wasm: a launch set must hold at least the root fragment".to_string());
    };

    // **Every fragment is read before the module's first entity is created.** A
    // class is a property of the lowered IR rather than of anything this file
    // emits, so the whole launch set is checked in one pass here — and a refusal
    // costs nothing: no entity exists yet, let alone an instruction.
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
        let body = lower_fragment(fragment, ordered, index, &module, signature, &imports)?;
        // **The function index is the push order**, so a fragment is pushed in
        // `ordered`'s order and a cross-kernel call resolves through `base` plus
        // the callee's position — the same two numbers the linker sees.
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

/// Declare one `read`/`write` import pair per class, the reads first.
///
/// **The order is the ABI's**, because it decides the import function indices:
/// the reads come first and the writes after them, one entry per class.
/// `run_parallel_range` resolves its closures against the same list, so the two
/// cannot disagree about which index a class took.
fn declare_buffer_imports(module: &mut Module, classes: &[ScalarClass]) -> BufferImports {
    let mut imports = BufferImports {
        base: 0,
        read: HashMap::new(),
        write: HashMap::new(),
    };
    for class in classes {
        // **The position and the index are `i64` in every class**, and only the
        // element's own type follows the class: a position is a compile-time
        // ordinal in the buffer space and an index is a lane number, so neither
        // is ever the data (`docs/notes/floating-point.md` §4.4). The host's
        // closures in `run_parallel_range` declare the same signatures.
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

/// The classes a launch set's buffer `read`/`write` calls name, in a fixed order
/// — `Int` before `Float` — and empty for a set that calls neither.
///
/// **One `read`/`write` pair per class the set uses**, which is what lets one
/// module hold fragments of different classes at all: a class nobody reads or
/// writes costs no import.
///
/// **Every instruction in the body, transfers included** — the question is "which
/// classes does this fragment call a buffer import in", and a read or write inside
/// a branch still needs its import declared. A walk over `straight_line_instrs`
/// would miss a branch and leave the module calling an import it never declared
/// ([`KernelBody::instrs`](lichen_kernel_ir::KernelBody::instrs)).
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
    // A fixed order, so the indices are a function of the *set* rather than of
    // the order the walk happened to meet the classes in.
    classes.sort_by_key(|class| match class {
        ScalarClass::Int => 0,
        ScalarClass::Float => 1,
    });
    classes
}
