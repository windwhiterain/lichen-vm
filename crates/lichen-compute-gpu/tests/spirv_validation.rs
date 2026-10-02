//! The emitter's module contract, checked without a device.
//!
//! `src/spirv.rs` says the risk in a hand-written emitter is a module's *shape*
//! rather than its opcodes, and that what removes that risk is running the words
//! through `spirv-val` before they reach a driver. Until this test nothing in the
//! repository did: the command was written down in `examples/emit-spv.rs` and
//! left for a developer to remember, so a module that stopped validating could
//! only be found by someone choosing to look for it.
//!
//! `spirv-val` is not a dependency of this crate, so the check runs only where
//! the tool is on `PATH`, and where it is not, the test **says the module was not
//! covered** rather than passing as if it had been — a machine without the Vulkan
//! SDK must not fail here, and a machine with no validator must not look like one
//! that validated.

use std::io::Write;
use std::process::{Command, Stdio};

use lichen_compute_gpu::spirv::{self, Binding};
use lichen_kernel_ir::{IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape};

/// The validator, spelled the way it is installed on `PATH`.
const VALIDATOR: &str = "spirv-val";

/// `out[i] = in[i] + 1` — the fragment `examples/emit-spv.rs` dumps, written out
/// here so the automated check and the documented manual step validate the same
/// module rather than two that happen to look alike.
///
/// The read is three instructions, not two: a `BufferReadCall` takes
/// `[cfg_pos, idx]` off the stack, so the position and the index both have to be
/// pushed before it.
fn adds_one() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body: vec![
            KernelInstr::Const(0),       // out_pos, in the *output* space
            KernelInstr::LocalGet(1),    // the index
            KernelInstr::Const(0),       // cfg_pos, in the *input* space
            KernelInstr::LocalGet(1),    // the index
            KernelInstr::BufferReadCall, // in[i]
            KernelInstr::Const(1),
            KernelInstr::Bin(KernelBin::Add), // in[i] + 1
            KernelInstr::BufferWriteCall,
            KernelInstr::Const(0),
        ],
        inputs: 1,
        outputs: 1,
        results: 1,
        int_width: IntWidth::I64,
    }
}

#[test]
fn the_emitted_module_validates() {
    let words = spirv::compile(
        &adds_one(),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("the emitter handles this fragment");
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();

    // `vulkan1.1` is the target environment the manual step names in
    // `examples/emit-spv.rs`: the same module, checked against the same rules.
    let mut child = match Command::new(VALIDATOR)
        .args(["--target-env", "vulkan1.1", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "SKIPPED the_emitted_module_validates: `{VALIDATOR}` is not on PATH, so the \
                 emitted module was not validated; put `spirv-val` on PATH to cover it"
            );
            return;
        }
        Err(error) => panic!("`{VALIDATOR}` is on PATH but could not be started: {error}"),
    };

    // The module goes in over standard input: the validator takes a filename, and
    // a file here would be a temporary the test has to place, name and remove.
    child
        .stdin
        .take()
        .expect("standard input was piped")
        .write_all(&bytes)
        .expect("the module is handed to the validator");
    let output = child.wait_with_output().expect("the validator finishes");

    assert!(
        output.status.success(),
        "`{VALIDATOR}` rejected the module the emitter produced:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
