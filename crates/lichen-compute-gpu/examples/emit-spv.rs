//! Dump a fragment's SPIR-V so it can be validated and read offline.
//!
//! The emitter is hand-written, and the reason that is safe is that its output
//! can be checked without a device: this writes the module to a file for
//! `spirv-val` (does it satisfy the spec?) and `spirv-dis` (what did it actually
//! emit?). Neither tool is a dependency of the crate — they are how the emitter
//! is developed, and how a broken module is diagnosed instead of guessed at.
//!
//! ```text
//! cargo run -p lichen-compute-gpu --example emit-spv
//! spirv-val --target-env vulkan1.1 target/spirv-dump.spv
//! spirv-dis target/spirv-dump.spv
//! ```

use lichen_kernel_ir::{
    IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape, ScalarClass,
};

fn main() {
    // `out[i] = in[i] + 1` over `count` indices, the shape a single-input
    // parallel kernel has: the body's last value is the dummy a compute shader
    // does not need, so the write is the whole effect.
    //
    // The read is three instructions, not two: a `BufferReadCall` takes
    // `[cfg_pos, idx]` off the stack, so the position and the index both have to
    // be pushed before it. A body that pushed only the index compiled and ran,
    // and computed `1 + i` instead of `in[i] + 1` — a wrong answer with nothing
    // refused anywhere.
    let count_prologue = vec![
        KernelInstr::Const(0),       // buffer position, in the *output* space
        KernelInstr::LocalGet(1),    // the index
        KernelInstr::Const(0),       // cfg_pos, in the *input* space
        KernelInstr::LocalGet(1),    // the index
        KernelInstr::BufferReadCall, // in[i]
        KernelInstr::Const(1),
        KernelInstr::Bin(KernelBin::Add), // in[i] + 1
        KernelInstr::BufferWriteCall,
        KernelInstr::Const(0),
    ];

    let fragment = KernelFragment {
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: count_prologue,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        results: 1,
        int_width: IntWidth::I64,
    };

    let binding = lichen_compute_gpu::Binding {
        inputs: 1,
        outputs: 1,
    };
    match lichen_compute_gpu::spirv::compile(&fragment, binding) {
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
