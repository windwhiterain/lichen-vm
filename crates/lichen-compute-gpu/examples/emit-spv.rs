//! Dump a fragment's SPIR-V so it can be validated and read offline.
//!
//! # Invariant
//! The emitter is hand-written, and its output can be checked without a device.
//! See docs/notes/lichen-compute-gpu.md.

use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    LaunchSet, ScalarClass,
};

fn main() {
    // `out[i] = in[i] + 1` over `count` indices: the body's last value is the dummy
    // a compute shader does not need.

    // The read is three instructions, not two, because a `BufferReadCall` takes
    // `[cfg_pos, idx]`.
    let count_prologue = vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // buffer position, in the *output* space
        FlatOp::Read(1),                                        // the index
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos, in the *input* space
        FlatOp::Read(1),                                        // the index
        FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
        FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)), // in[i] + 1
        FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
    ];

    let fragment = KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(2, &count_prologue),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        int_width: IntWidth::I64,
    };

    let binding = lichen_compute_gpu::Binding {
        inputs: 1,
        outputs: 1,
    };
    match lichen_compute_gpu::spirv::compile(&LaunchSet::single(&fragment), binding) {
        Ok(words) => {
            let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
            let path = std::path::Path::new("target/spirv-dump.spv");
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create target/");
            }
            std::fs::write(path, &bytes).expect("write the module");
            println!("wrote {} words to {}", words.len(), path.display());
        }
        Err(refusal) => {
            eprintln!("refused: {refusal}");
            std::process::exit(1);
        }
    }
}
