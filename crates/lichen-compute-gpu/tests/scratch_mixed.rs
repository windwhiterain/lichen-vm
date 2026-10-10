//! Throwaway probe: does a two-class module's id space collide?

use std::io::Write;
use std::process::{Command, Stdio};

use lichen_compute_gpu::spirv::{self, Binding};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    LaunchSet, ScalarClass,
};

fn mixed() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Conv {
                    from: ScalarClass::Int,
                    to: ScalarClass::Float,
                }),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Float, 0x4020_0000i64)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Float)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

#[test]
fn probe() {
    let words = spirv::compile(
        &LaunchSet::single(&mixed()),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("a mixed-class fragment emits");
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let mut child = Command::new("spirv-val")
        .arg("--target-env")
        .arg("vulkan1.1")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spirv-val");
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(&bytes)
        .expect("written");
    let out = child.wait_with_output().expect("finished");
    println!(
        "status {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "spirv-val rejected the module");
}
