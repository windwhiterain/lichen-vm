//! The emitter's module contract, checked without a device.
//!
//! # Invariant
//! The risk in a hand-written emitter is a module's shape rather than its opcodes, and running the
//! words through `spirv-val` is what removes it. `spirv-val` is not a dependency, so the check runs
//! only where the tool is on `PATH`, and where it is not the test says the module was not covered
//! rather than passing as if it had been.

use std::io::Write;
use std::process::{Command, Stdio};

use lichen_compute_gpu::spirv::{self, Binding};
use lichen_kernel_ir::{
    Br, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaunchSet, ScalarClass, Terminator,
};

/// The validator, spelled the way it is installed on `PATH`.
const VALIDATOR: &str = "spirv-val";

/// What locomotion a float body computes with, as the bits of `2.5f32`.
///
/// # Invariant
/// A float body's `Const` payload is the float's bit pattern in the low 32 bits: the IR's constant is
/// an `i64` carrying no class, so the class comes from the fragment and the bits from the payload.
const TWO_POINT_FIVE: i64 = 0x4020_0000;

/// `out[i] = in[i] + 1` — the fragment `examples/emit-spv.rs` dumps.
///
/// # Invariant
/// The read is three instructions, not two: a `BufferReadCall` takes `[cfg_pos, idx]` off the stack.
fn adds_one() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos, in the *output* space
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos, in the *input* space
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)), // in[i] + 1
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// `out[i] = if in[0] then in[i] * 2.5 + (in[i] == 0.0) + in[i] / 2.5 else 0.0`
///
/// # Invariant
/// The float module's shape in one body: `OpTypeFloat 32` as scalar and element, a 32-bit index so no
/// `Int64` capability is declared, the `f32` constant pool, `OpFMul`/`OpFDiv`/`OpFAdd`, and the two
/// the emitter is careful about — bit-pattern equality and a bit-pattern condition, because a
/// `NaN`'s bits are non-zero. The read takes a constant element index.
fn scales_a_float() -> KernelFragment {
    let read = |body: &mut Vec<FlatOp>| {
        body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
        body.push(FlatOp::Read(1));
        body.push(FlatOp::Instr(KernelInstr::BufferReadCall(
            ScalarClass::Float,
        )));
    };
    // out_pos, then the index: the [position, index] a write takes.
    let mut body = vec![
        FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
        FlatOp::Read(1),
    ];
    // then: in[i] * 2.5
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(
        ScalarClass::Float,
        TWO_POINT_FIVE,
    )));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Float,
        KernelBin::Mul,
    )));
    // … + (in[i] == 0.0), a comparison materialised into the float `1.0`/`0.0`
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Float, 0))); // `0.0f32` is the bit pattern zero
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Eq,
    )));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Add,
    )));
    // … + in[i] / 2.5
    read(&mut body);
    body.push(FlatOp::Instr(KernelInstr::Const(
        ScalarClass::Float,
        TWO_POINT_FIVE,
    )));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Float,
        KernelBin::Div,
    )));
    body.push(FlatOp::Instr(KernelInstr::Bin(
        ScalarClass::Int,
        KernelBin::Add,
    )));
    // else: 0.0
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    // the selector: in[0], a float value rather than a comparison's bool, read at
    // a constant index
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    body.push(FlatOp::Instr(KernelInstr::BufferReadCall(
        ScalarClass::Float,
    )));
    body.push(FlatOp::Instr(KernelInstr::Select));
    body.push(FlatOp::Instr(KernelInstr::BufferWriteCall(
        ScalarClass::Float,
    )));
    body.push(FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)));
    KernelFragment {
        roles: KernelRoles::default(),
        // `(config, index)`, integers however the buffers are classed.
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(2, &body),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// A fragment over `(config, index)` writing one integer output buffer.
fn one_integer_output(body: KernelBody) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 0,
        outputs: 1,
        input_classes: Vec::new(),
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// A one-armed selection: an arm, and the join both edges arrive at.
fn branches_to_a_merge() -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _config = body.add_param(entry);
    let index = body.add_param(entry);
    let arm = body.add_block();
    let join = body.add_block();
    let carried = body.add_param(join);
    let cond = body.add_const(entry, ScalarClass::Int, 1);
    body.set_terminator(
        entry,
        Terminator::CondBr {
            cond,
            if_true: Br {
                target: arm,
                args: Vec::new(),
            },
            if_false: Br {
                target: join,
                args: vec![index],
            },
        },
    );
    body.set_terminator(
        arm,
        Terminator::Br(Br {
            target: join,
            args: vec![index],
        }),
    );
    let position = body.add_const(join, ScalarClass::Int, 0);
    let written = body.add_const(join, ScalarClass::Int, 7);
    body.add_op(
        join,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![position, index, written],
        Vec::new(),
    );
    body.set_terminator(
        join,
        Terminator::Return {
            values: vec![carried],
        },
    );
    one_integer_output(body)
}

/// A loop with a carried value: `n = 0; while (i < 10) { n = n + 1 }`; its exit arm is
/// `if_true` when `exit_first`.
fn counts_to_ten(exit_first: bool) -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _config = body.add_param(entry);
    let index = body.add_param(entry);
    let header = body.add_block();
    let carried = body.add_param(header);
    let latch = body.add_block();
    let next = body.add_param(latch);
    let exit = body.add_block();
    let result = body.add_param(exit);

    let seed = body.add_const(entry, ScalarClass::Int, 0);
    let enter = body.add_const(entry, ScalarClass::Int, 1);
    body.set_terminator(
        entry,
        Terminator::CondBr {
            cond: enter,
            if_true: Br {
                target: header,
                args: vec![seed],
            },
            if_false: Br {
                target: exit,
                args: vec![seed],
            },
        },
    );

    let ten = body.add_const(header, ScalarClass::Int, 10);
    // **The exit test is the condition in both spellings**: `i < 10` continues.
    let test_op = if exit_first {
        KernelBin::Geq
    } else {
        KernelBin::Lt
    };
    let below = body.add_op(
        header,
        KernelInstr::Bin(ScalarClass::Int, test_op),
        vec![index, ten],
        vec![ScalarClass::Int],
    );
    let test = body.add_op(
        header,
        KernelInstr::I32WrapI64,
        vec![below],
        vec![ScalarClass::Int],
    );
    let to_latch = Br {
        target: latch,
        args: vec![carried],
    };
    let to_exit = Br {
        target: exit,
        args: vec![carried],
    };
    let (if_true, if_false) = if exit_first {
        (to_exit, to_latch)
    } else {
        (to_latch, to_exit)
    };
    body.set_terminator(
        header,
        Terminator::CondBr {
            cond: test,
            if_true,
            if_false,
        },
    );

    let one = body.add_const(latch, ScalarClass::Int, 1);
    let step = body.add_op(
        latch,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![next, one],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        latch,
        Terminator::Br(Br {
            target: header,
            args: vec![step],
        }),
    );

    let position = body.add_const(exit, ScalarClass::Int, 0);
    let written = body.add_const(exit, ScalarClass::Int, 7);
    body.add_op(
        exit,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![position, index, written],
        Vec::new(),
    );
    body.set_terminator(
        exit,
        Terminator::Return {
            values: vec![result],
        },
    );
    one_integer_output(body)
}

/// A loop whose body leaves on a conditional backedge.
fn conditional_backedge() -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _config = body.add_param(entry);
    let index = body.add_param(entry);
    let header = body.add_block();
    let carried = body.add_param(header);
    let latch = body.add_block();
    let next = body.add_param(latch);
    let late = body.add_block();
    let late_value = body.add_param(late);
    let exit = body.add_block();
    let result = body.add_param(exit);

    let seed = body.add_const(entry, ScalarClass::Int, 0);
    let enter = body.add_const(entry, ScalarClass::Int, 1);
    body.set_terminator(
        entry,
        Terminator::CondBr {
            cond: enter,
            if_true: Br {
                target: header,
                args: vec![seed],
            },
            if_false: Br {
                target: exit,
                args: vec![seed],
            },
        },
    );

    let ten = body.add_const(header, ScalarClass::Int, 10);
    let below = body.add_op(
        header,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Lt),
        vec![index, ten],
        vec![ScalarClass::Int],
    );
    let test = body.add_op(
        header,
        KernelInstr::I32WrapI64,
        vec![below],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        header,
        Terminator::CondBr {
            cond: test,
            if_true: Br {
                target: latch,
                args: vec![carried],
            },
            if_false: Br {
                target: exit,
                args: vec![carried],
            },
        },
    );

    let one = body.add_const(latch, ScalarClass::Int, 1);
    let step = body.add_op(
        latch,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![next, one],
        vec![ScalarClass::Int],
    );
    let again = body.add_const(latch, ScalarClass::Int, 1);
    body.set_terminator(
        latch,
        Terminator::CondBr {
            cond: again,
            if_true: Br {
                target: header,
                args: vec![step],
            },
            if_false: Br {
                target: late,
                args: vec![step],
            },
        },
    );
    body.set_terminator(
        late,
        Terminator::Br(Br {
            target: header,
            args: vec![late_value],
        }),
    );

    let position = body.add_const(exit, ScalarClass::Int, 0);
    let written = body.add_const(exit, ScalarClass::Int, 7);
    body.add_op(
        exit,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![position, index, written],
        Vec::new(),
    );
    body.set_terminator(
        exit,
        Terminator::Return {
            values: vec![result],
        },
    );
    one_integer_output(body)
}

/// Hand one emitted module to `spirv-val`, or say it was not covered.
///
/// # Invariant
/// The return says whether the module was actually checked: a machine without the validator must not
/// look like one that validated.
fn validate(what: &str, words: &[u32]) -> bool {
    let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();

    // `vulkan1.1` is the target environment the manual step names.
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

    // The module goes in over standard input: the validator takes a filename.
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
    // Both classes, because the module's shape is where they differ.
    let integer = spirv::compile(
        &LaunchSet::single(&adds_one()),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("the emitter handles an integer fragment");
    let float = spirv::compile(
        &LaunchSet::single(&scales_a_float()),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("the emitter handles a float fragment");

    // The declaration and the device gate are one derivation: an integer module
    // needs `shaderInt64`.
    assert!(
        spirv::needs_int64(&LaunchSet::single(&adds_one()))
            .expect("an integer fragment has a class"),
        "an integer module declares a 64-bit integer"
    );
    assert!(
        !spirv::needs_int64(&LaunchSet::single(&scales_a_float()))
            .expect("a float fragment has a class"),
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
    // A skip is not a pass: both SKIPPED lines above are the record.
    if covered < 2 {
        eprintln!("only {covered} of 2 emitted module(s) were validated");
    }
}

/// `out[i] = int2float i + in[i]` — the §5.1 crossing, hand-built.
///
/// # Invariant
/// The index is a 32-bit unsigned integer in this target even in a float module, so `int2float` of it
/// is `OpConvertUToF` and not the no-op wasm answers with.
fn index_to_float() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(1),                                        // the index
                FlatOp::Read(1), // the same index, what `int2float` reads
                FlatOp::Instr(KernelInstr::Conv {
                    from: ScalarClass::Int,
                    to: ScalarClass::Float,
                }),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// `out[i] = int2float (float2int in[i])` — both directions in one float module.
///
/// # Invariant
/// The pair is what the IR's `Conv { from, to }` is for: read off the module the two directions look
/// the same, and a backend that guessed them from the fragment's class would swap the programs. Here
/// the float truncates toward zero (`OpConvertFToU`) and the integer widens back (`OpConvertUToF`).
fn crosses_both_ways() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Instr(KernelInstr::Conv {
                    from: ScalarClass::Float,
                    to: ScalarClass::Int,
                }),
                FlatOp::Instr(KernelInstr::Conv {
                    from: ScalarClass::Int,
                    to: ScalarClass::Float,
                }),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

#[test]
fn a_body_with_control_flow_validates() {
    // The two shapes a structured body has, and the families only a body with
    // transfers needs.
    let one_in_zero_out = Binding {
        inputs: 0,
        outputs: 1,
    };
    let mut covered = 0;
    for (what, fragment) in [
        ("a module with an `if`", branches_to_a_merge()),
        ("a module with a `while`", counts_to_ten(false)),
        (
            "a module with a `while` whose base arm is first",
            counts_to_ten(true),
        ),
        (
            "a module with a conditional backedge",
            conditional_backedge(),
        ),
    ] {
        fragment
            .body
            .validate()
            .unwrap_or_else(|broken| panic!("{what} is well formed: {broken}"));
        let words = spirv::compile(&LaunchSet::single(&fragment), one_in_zero_out)
            .unwrap_or_else(|refusal| panic!("{what} is emitted: {refusal}"));
        if validate(what, &words) {
            covered += 1;
        }
    }
    if covered < 4 {
        eprintln!("only {covered} of 4 control-flow module(s) were validated");
    }
}

/// `out[i] = int2float in[i] * 2.5` — **one module holding two buffer classes**.
///
/// # Invariant
/// A module declares one type chain per class its buffers hold, and the chains are
/// what a hand-written id range collided with: the constants sat at fixed ids
/// `16..21`, so a second chain reaching that far made `spirv-val` answer `Id 16 is
/// defined more than once`. The ids are computed now, and this is the module that
/// says so.
fn two_buffer_classes() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i], an Int buffer
                FlatOp::Instr(KernelInstr::Conv {
                    from: ScalarClass::Int,
                    to: ScalarClass::Float,
                }),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Float, TWO_POINT_FIVE)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul)),
                // …written to a Float one.
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

/// A callee whose domain is empty, so a flat-built caller can name it.
fn nullary_callee() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(Vec::new()),
        body: KernelBody::from_flat(0, &[FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1))]),
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// `out[i] = k0()` — the smallest cross-kernel module: two fragments, one call.
fn calls_a_nullary_kernel() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(1),                                        // the index
                FlatOp::Instr(KernelInstr::CallKernel(7)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 0,
        outputs: 1,
        input_classes: Vec::new(),
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// A module that declares two buffer classes is a legal module.
#[test]
fn a_module_holding_two_buffer_classes_validates() {
    let words = spirv::compile(
        &LaunchSet::single(&two_buffer_classes()),
        Binding {
            inputs: 1,
            outputs: 1,
        },
    )
    .expect("the emitter handles a fragment whose buffers are of two classes");
    if !validate("the two-class module", &words) {
        eprintln!("the two-class module was not validated");
    }
}

/// A launch set of two fragments is one module with two `OpFunction`s.
#[test]
fn a_module_holding_a_cross_kernel_call_validates() {
    let ordered = [calls_a_nullary_kernel(), nullary_callee()];
    let index: std::collections::HashMap<usize, u32> = [(7, 1)].into();
    let words = spirv::compile(
        &LaunchSet::new(&ordered, &index),
        Binding {
            inputs: 0,
            outputs: 1,
        },
    )
    .expect("a call whose callee is in the set is emitted");

    let seen = opcodes(&words);
    // 54 is `OpFunction` and 57 `OpFunctionCall`, the specification's own numbers.
    assert_eq!(
        seen.iter().filter(|opcode| **opcode == 54).count(),
        ordered.len(),
        "one OpFunction per fragment in the set"
    );
    assert_eq!(
        seen.iter().filter(|opcode| **opcode == 57).count(),
        1,
        "the call is one OpFunctionCall"
    );
    if !validate("the cross-kernel module", &words) {
        eprintln!("the cross-kernel module was not validated");
    }
}

/// The opcode of every instruction of a module, walked by word count.
///
/// # Invariant
/// A search for the word `112` would find an operand that happens to hold it, so this reads the
/// header's five words and then steps instruction by instruction.
fn opcodes(words: &[u32]) -> Vec<u16> {
    let mut out = Vec::new();
    let mut at = 5;
    while at < words.len() {
        let word = words[at];
        let (count, opcode) = ((word >> 16) as u16, (word & 0xffff) as u16);
        assert!(count > 0, "an instruction occupies at least one word");
        out.push(opcode);
        at += count as usize;
    }
    out
}

#[test]
fn the_two_conversions_validate_in_a_float_module() {
    // The two opcode numbers this test names are the specification's own.
    for (what, fragment, expected) in [
        ("int2float of the index", index_to_float(), 112),
        ("both directions at once", crosses_both_ways(), 109),
    ] {
        let words = spirv::compile(
            &LaunchSet::single(&fragment),
            Binding {
                inputs: 1,
                outputs: 1,
            },
        )
        .unwrap_or_else(|reason| panic!("{what} is refused by the emitter: {reason}"));
        let seen = opcodes(&words);
        assert!(
            seen.contains(&expected),
            "{what} emits opcode {expected}: {seen:?}"
        );
        validate(what, &words);
    }
}

/// `out[i] = in[i] + a`, over `(n, a, index)` — the launch's runtime scalar.
///
/// # Invariant
/// The block member is **eight bytes** here because the module is an integer one: a leaf's
/// width is the type the entry point loads it as, not the class's buffer width.
fn adds_a_runtime_scalar() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            3,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(2),                                        // the index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos
                FlatOp::Read(2),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Read(1), // the pushed runtime scalar
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        int_width: IntWidth::I64,
    }
}

/// `out[i] = in[i] * alpha`, over `(n, alpha, index)` — a `Float` leaf beside an `Int` one.
///
/// # Invariant
/// **Both leaves are four bytes here**, because the module is a float one: an integer leaf of
/// a float module is a 32-bit index, the same rule the invocation id follows. A block laid out
/// at the buffer widths would put `alpha` eight bytes in and the shader would read padding.
fn scales_by_a_runtime_float() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            3,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(2),                                        // the index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos
                FlatOp::Read(2),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Float)), // in[i]
                FlatOp::Read(1), // the pushed runtime scalar
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Float)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Float; 1],
        int_width: IntWidth::I64,
    }
}

/// A module that reads a launch scalar is a valid module.
///
/// # Invariant
/// Both classes, because the block's **member widths** are where they differ: an integer leaf
/// is eight bytes in one module and four in the other, and a member at an offset its type does
/// not divide is what `spirv-val` answers.
#[test]
fn a_module_pushing_a_runtime_scalar_validates() {
    let one_in_one_out = Binding {
        inputs: 1,
        outputs: 1,
    };
    let mut covered = 0;
    for (what, fragment) in [
        (
            "an integer module pushing a scalar",
            adds_a_runtime_scalar(),
        ),
        (
            "a float module pushing a float scalar",
            scales_by_a_runtime_float(),
        ),
    ] {
        fragment
            .body
            .validate()
            .unwrap_or_else(|broken| panic!("{what} is well formed: {broken}"));
        let words = spirv::compile(&LaunchSet::single(&fragment), one_in_one_out)
            .unwrap_or_else(|refusal| panic!("{what} is emitted: {refusal}"));
        if validate(what, &words) {
            covered += 1;
        }
    }
    if covered < 2 {
        eprintln!("only {covered} of 2 push-constant module(s) were validated");
    }
}

/// The merge block, continue target and true label of the module's loop header.
fn loop_header_facts(words: &[u32]) -> (u32, u32, u32) {
    let mut at = 5;
    while at < words.len() {
        let word = words[at];
        let (count, opcode) = ((word >> 16) as usize, (word & 0xffff) as u16);
        let operands = &words[at + 1..at + count];
        if opcode == LOOP_MERGE {
            return (operands[0], operands[1], words[at + count + 2]);
        }
        at += count;
    }
    panic!("this module emits no loop header");
}

/// `OpLoopMerge` (246) and `OpBranchConditional` (250), the specification's own numbers.
const LOOP_MERGE: u16 = 246;
const BRANCH_CONDITIONAL: u16 = 250;

/// A loop header leaves on its own exit arm, wherever the spelling put it.
///
/// # Invariant
/// The condition's polarity and the label its true arm names are one fact, so a
/// header whose `if_true` arm is the base leaves when the condition holds. A module
/// that breaks this is **legal** — the device answered `0` where the cpu answered the
/// reduction — so the arm order needs a reader
/// (`docs/notes/loop-conversion.md` §8.6 item 6), not a validator.
#[test]
fn a_loop_header_leaves_on_its_own_exit_arm() {
    for (what, exit_first) in [("continue arm first", false), ("base arm first", true)] {
        let module = counts_to_ten(exit_first);
        module
            .body
            .validate()
            .unwrap_or_else(|broken| panic!("{what} is well formed: {broken}"));
        let words = spirv::compile(
            &LaunchSet::single(&module),
            Binding {
                inputs: 0,
                outputs: 1,
            },
        )
        .unwrap_or_else(|refusal| panic!("{what} is emitted: {refusal}"));
        let seen = opcodes(&words);
        assert!(
            seen.contains(&BRANCH_CONDITIONAL),
            "{what} emits a conditional branch: {seen:?}"
        );
        let (merge, continue_target, on_true) = loop_header_facts(&words);
        let leaves = if exit_first { merge } else { continue_target };
        assert_eq!(
            on_true, leaves,
            "{what}: the condition's true arm is where the loop leaves"
        );
    }
}
