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
    Br, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaunchSet, ScalarClass, Terminator,
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
        // `(config, index)`, integers, however the buffers are classed: this
        // target's index is the invocation id, not a value of that domain.
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
///
/// The shapes are `docs/notes/loop-conversion.md` §8.4's table.
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

/// A loop with a carried value: `n = 0; while (i < 10) { n = n + 1 }`; the exit arm is
/// `if_true` when `exit_first` (`docs/notes/loop-conversion.md` §8.6 item 6).
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
    // **The exit test is the condition in both spellings**: `i < 10` continues and
    // `i >= 10` leaves, so which arm is `if_true` is the spelling's fact alone.
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

/// A loop whose body leaves on a **conditional** backedge: the header is both
/// the join of that selection and the block the selection is inside.
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

    // What the module declares and what the device gate asks about are one
    // derivation: an integer module needs `shaderInt64` for its 64-bit type, and
    // a float module has no 64-bit integer to need it for. `dispatch` refuses an
    // integer fragment on a device without the feature and lets a float one
    // through on exactly this answer.
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
    // A skip is not a pass: if the validator answered for neither module, the
    // two SKIPPED lines above are the record, and this says nothing more.
    if covered < 2 {
        eprintln!("only {covered} of 2 emitted module(s) were validated");
    }
}

/// `out[i] = int2float i + in[i]` — the §5.1 crossing, hand-built.
///
/// The index is the invocation id, a 32-bit unsigned integer in this target even
/// in a float module, so `int2float` of it is `OpConvertUToF` and not the no-op
/// the wasm backend answers with (where the same number already rides in an
/// `f32`).  One body, one crossing, one float buffer in and out.
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
/// The pair is what the IR's `Conv { from, to }` is for: read off the module the
/// two directions look the same on a stack, and a backend that guessed them from
/// the fragment's class would swap the two programs.  Here the float element
/// truncates toward zero (`OpConvertFToU`) and the integer that leaves widens
/// back (`OpConvertUToF`), so a body whose answer is the number it started with
/// says so with both opcodes.
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
    // The two shapes a structured body has, and the instruction families only a
    // body with transfers needs: an `OpSelectionMerge` with a merge block, and an
    // `OpLoopMerge` with a backedge and an `OpPhi` per incoming edge.
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

/// A callee whose domain is **empty**, so a caller built by [`KernelBody::from_flat`]
/// can name it: the flat builder gives a `CallKernel` no arguments, because the
/// arity is the callee's own and it has no callee to read it from.
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

/// A module that declares two buffer classes is a legal module, which is what a
/// fixed id range could not promise: see [`two_buffer_classes`].
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

/// A launch set of two fragments is **one module with two `OpFunction`s**, and the
/// call is the `OpFunctionCall` that names the callee's — the shape `spirv-val`
/// rules on and the reason a cross-kernel call is emittable at all.
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
    // 54 is `OpFunction` and 57 `OpFunctionCall`; `spirv::op` is private to the
    // crate, so the numbers are the specification's own.
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
/// A search for the word `112` would find an operand that happens to hold it, so
/// this reads the header's five words and then steps instruction by instruction.
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
    // `spirv::op` is private to the crate, so the two opcode numbers this test
    // names are the SPIR-V specification's own: 112 `OpConvertUToF`, 109
    // `OpConvertFToU`.
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

/// The emitted loop header's merge block, continue target, and the label its
/// condition's **true** arm names, walked by word count.
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

/// `OpLoopMerge` (246) and `OpBranchConditional` (250); `spirv::op` is private to
/// the crate, so the numbers are the specification's own.
const LOOP_MERGE: u16 = 246;
const BRANCH_CONDITIONAL: u16 = 250;

/// A loop header leaves when its **own** condition holds, whichever arm the
/// spelling wrote first: a condition's polarity and the label its true arm names
/// are one fact (`docs/notes/loop-conversion.md` §8.6 item 6).
///
/// `spirv-val` cannot see this. A header that branches to its own body on its exit
/// condition is a legal module — the device answered `0` where the CPU answered the
/// reduction — so the emitter's arm order needs a reader, not a validator.
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
