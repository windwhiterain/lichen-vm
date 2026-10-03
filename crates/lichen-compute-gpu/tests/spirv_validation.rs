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
use lichen_kernel_ir::{
    IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape, ScalarClass,
};

/// The validator, spelled the way it is installed on `PATH`.
const VALIDATOR: &str = "spirv-val";

/// What locomotion a float body computes with, as the bits of `2.5f32`.
///
/// A float body's `Const` payload **is the float's bit pattern** in the low 32
/// bits: the IR's constant is an `i64` and carries no class, so the class comes
/// from the fragment and the bits from the payload (`spirv::Literals`).
const TWO_POINT_FIVE: i64 = 0x4020_0000;

/// `out[i] = in[i] + 1` — the fragment `examples/emit-spv.rs` dumps, written out
/// here so the automated check and the documented manual step validate the same
/// module rather than two that happen to look alike.
///
/// The read is three instructions, not two: a `BufferReadCall` takes
/// `[cfg_pos, idx]` off the stack, so the position and the index both have to be
/// pushed before it.
fn adds_one() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: vec![
            KernelInstr::Const(ScalarClass::Int, 0), // out_pos, in the *output* space
            KernelInstr::LocalGet(1),                // the index
            KernelInstr::Const(ScalarClass::Int, 0), // cfg_pos, in the *input* space
            KernelInstr::LocalGet(1),                // the index
            KernelInstr::BufferReadCall,             // in[i]
            KernelInstr::Const(ScalarClass::Int, 1),
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add), // in[i] + 1
            KernelInstr::BufferWriteCall,
            KernelInstr::Const(ScalarClass::Int, 0),
        ]
        .into(),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        results: 1,
        int_width: IntWidth::I64,
    }
}

/// `out[i] = if in[0] then in[i] * 2.5 + (in[i] == 0.0) + in[i] / 2.5 else 0.0`
///
/// The float module's shape, in one body, because that shape is the whole point
/// of this test: `OpTypeFloat 32` as the scalar and as the storage buffer's
/// element, a 32-bit **index** (so no `Int64` capability is declared and none is
/// needed), the `f32` constant pool, `OpFMul`/`OpFDiv`/`OpFAdd`, and — the two
/// the emitter is careful about — the bit-pattern equality (`OpBitcast`,
/// `OpIEqual`, because the language's `==` is `to_bits` and not IEEE) and the
/// bit-pattern condition (`OpBitcast`, `OpINotEqual`, because a `NaN`'s bits are
/// non-zero even though the value is equal to nothing).
///
/// The selector is written as the float `in[i]` rather than as the comparison,
/// which is what reaches the condition path; the comparison is an operand of the
/// `+` instead, which is what reaches the `1.0`/`0.0` materialisation. Its read
/// takes a **constant** element index, which is the one position where a body's
/// literal has to be read as an integer in a module whose values are floats — and
/// a 32-bit one, since a float module's integers are indices.
fn scales_a_float() -> KernelFragment {
    let read = |body: &mut Vec<KernelInstr>| {
        body.push(KernelInstr::Const(ScalarClass::Int, 0));
        body.push(KernelInstr::LocalGet(1));
        body.push(KernelInstr::BufferReadCall);
    };
    // out_pos, then the index: the [position, index] a write takes.
    let mut body = vec![
        KernelInstr::Const(ScalarClass::Int, 0),
        KernelInstr::LocalGet(1),
    ];
    // then: in[i] * 2.5
    read(&mut body);
    body.push(KernelInstr::Const(ScalarClass::Float, TWO_POINT_FIVE));
    body.push(KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul));
    // … + (in[i] == 0.0), a comparison materialised into the float `1.0`/`0.0`
    read(&mut body);
    body.push(KernelInstr::Const(ScalarClass::Float, 0)); // `0.0f32` is the bit pattern zero
    body.push(KernelInstr::Bin(ScalarClass::Float, KernelBin::Eq));
    body.push(KernelInstr::Bin(ScalarClass::Float, KernelBin::Add));
    // … + in[i] / 2.5
    read(&mut body);
    body.push(KernelInstr::Const(ScalarClass::Float, TWO_POINT_FIVE));
    body.push(KernelInstr::Bin(ScalarClass::Float, KernelBin::Div));
    body.push(KernelInstr::Bin(ScalarClass::Float, KernelBin::Add));
    // else: 0.0
    body.push(KernelInstr::Const(ScalarClass::Float, 0));
    // the selector: in[0], a float value rather than a comparison's bool, read at
    // a constant index
    body.push(KernelInstr::Const(ScalarClass::Int, 0));
    body.push(KernelInstr::Const(ScalarClass::Int, 0));
    body.push(KernelInstr::BufferReadCall);
    body.push(KernelInstr::Select);
    body.push(KernelInstr::BufferWriteCall);
    body.push(KernelInstr::Const(ScalarClass::Int, 0));
    KernelFragment {
        // `(config, index)`, integers, however the buffers are classed: this
        // target's index is the invocation id, not a value of that domain.
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: body.into(),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        results: 1,
        int_width: IntWidth::I64,
    }
}

/// Hand one emitted module to `spirv-val`, or say it was not covered.
///
/// Returns whether the module was actually checked: a machine without the
/// validator must not look like one that validated, and the name of the module
/// is in every message so a reader knows which one was covered.
fn validate(what: &str, words: &[u32]) -> bool {
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
                "SKIPPED the_emitted_module_validates for {what}: `{VALIDATOR}` is not on PATH, so \
                 that module was not validated; put `spirv-val` on PATH to cover it"
            );
            return false;
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
        "`{VALIDATOR}` rejected {what}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[test]
fn the_emitted_module_validates() {
    // Both classes, because the module's shape is where they differ: the
    // element type, the pointer into it, the array stride and the capability
    // list are all a function of it, and only the validator rules on whether the
    // result is a legal module.
    let integer = spirv::compile(
        &adds_one(),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("the emitter handles an integer fragment");
    let float = spirv::compile(
        &scales_a_float(),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("the emitter handles a float fragment");

    // What the module declares and what the device gate asks about are one
    // derivation: an integer module needs `shaderInt64` for its 64-bit type, and
    // a float module has no 64-bit integer to need it for. `dispatch` refuses an
    // integer fragment on a device without the feature and lets a float one
    // through on exactly this answer.
    assert!(
        spirv::needs_int64(&adds_one()).expect("an integer fragment has a class"),
        "an integer module declares a 64-bit integer"
    );
    assert!(
        !spirv::needs_int64(&scales_a_float()).expect("a float fragment has a class"),
        "a float module declares no 64-bit integer"
    );

    let mut covered = 0;
    for (what, words) in [
        ("the integer module", integer.as_slice()),
        ("the float module", float.as_slice()),
    ] {
        if validate(what, words) {
            covered += 1;
        }
    }
    // A skip is not a pass: if the validator answered for neither module, the
    // two SKIPPED lines above are the record, and this says nothing more.
    if covered < 2 {
        eprintln!("only {covered} of 2 emitted module(s) were validated");
    }
}
