//! A kernel body whose value **is** a `@loop` fold: the store is inside the step.
//!
//! # Invariant
//!
//! The body's value is the fold, so the codomain's position is the loop and the walk
//! emits the fold's step — the write in it. The state carries the output buffer, so a
//! step's write names the buffer it was entered with: the version spine is what makes a
//! buffer carryable (`docs/notes/loop-conversion.md` §6).

use lichen_kernel_ir::{KernelInstr, Terminator, ValueDef};

use super::common;

/// The program, with the kernel body substituted.
///
/// # Invariant
/// The body reads no buffer, so the program holds no input group and the host builds no
/// data: a buffer a host cannot fill is not one the program needs
/// (`docs/notes/kernel-buffer-declarations.md`).
const TEMPLATE: &str = r#"
--- compute = import "compute.lichen" ---
Out := _x => {.y (compute.Buf (compute.BufParam{.type Float, .length 2}))}
Par := _x => {.n @any : Int, .taps @any : Int}
Arg := _x => {.n @any : Int, .taps @any : Int}
raw := (c <: Par) => {
  i := compute.range c.n
@BODY@
}
k := compute.parallel raw "cpu" (Out _)
out := (compute.plrun k ((Arg){.n 2, .taps 3}) : Struct)
compute.collect out.y : Array
"#;

/// One plain write: the control the fold is compared against.
const CONTROL: &str = r#"  y := compute.buf (compute.BufParam{.type Float, .length c.n})
  ((Out){.y (compute.write (compute.Write{.to y, .at i, .value 2.0}))})"#;

/// A fold whose step writes the countdown into the buffer it carries.
const LOOP: &str = r#"  y := compute.buf (compute.BufParam{.type Float, .length c.n})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (compute.write (compute.Write{.to s(1), .at i, .value (int2float s(0))})))
  ((Out){.y (fold (c.taps, y))})"#;

/// The elements the program answers.
fn answer(body: &str) -> Vec<f32> {
    let source = TEMPLATE.replace("@BODY@", body);
    let (module, value, _root) = common::run(&source);
    common::float_array(&module, &value)
}

/// The block the lowered body's store is in, and the block the entry branches to.
///
/// # Invariant
/// The store's own block is the loop's **step**: it branches back to the header the
/// entry block enters, so a store outside the nest would sit in a block of its own.
fn store_and_header(body: &str) -> (usize, usize) {
    let source = TEMPLATE.replace("@BODY@", body);
    let report = common::check(&source);
    assert!(report.ok(), "the program checks: {:?}", report.diagnostics);
    let build = report.build.expect("a checked program builds");
    let body = common::sole_fragment(&build.module).body;
    let store = body
        .blocks
        .iter()
        .enumerate()
        .find(|(_, block)| {
            block.instrs.iter().any(|&instr| {
                matches!(
                    body.values.get(instr.0 as usize),
                    Some(ValueDef::Instr {
                        op: KernelInstr::BufferWriteCall(_),
                        ..
                    })
                )
            })
        })
        .map(|(index, _)| index)
        .expect("the lowered body carries a store");
    let header = match &body.blocks[0].terminator {
        Terminator::Br(branch) => branch.target,
        other => panic!("the entry block branches to the loop's header: {other:?}"),
    };
    (store, header)
}

/// A plain write still fills the output the control says it does.
#[test]
fn one_plain_write_fills_the_output() {
    assert_eq!(answer(CONTROL), vec![2.0, 2.0]);
}

/// A body that **is** a fold writes inside the fold's step.
///
/// # Invariant
/// Three iterations store `int2float s(0)` — 3.0, 2.0, 1.0 — at each lane's own
/// index, and the last store wins, so each lane answers 1.0.
#[test]
fn a_body_that_is_a_fold_writes_inside_the_folds_step() {
    assert_eq!(answer(LOOP), vec![1.0, 1.0]);
    let (store, header) = store_and_header(LOOP);
    assert_ne!(
        store, 0,
        "the store is in the loop's step, not in the entry block"
    );
    assert_ne!(
        store, header,
        "the store is in the step, not in the header the entry block enters"
    );
}
