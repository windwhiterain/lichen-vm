//! `KernelBody::validate`'s scope rule: a use sits where its value is defined.

use lichen_kernel_ir::{Br, KernelBin, KernelBody, KernelInstr, ScalarClass, Terminator, ValueId};

/// A body whose reading arm is not dominated by the arm that defines the value it reads.
///
/// # Invariant
/// The entry branches to both arms, so the definition reaches the use along one path only —
/// the shape `validate` refuses even though the value exists in the body.
fn sibling_arms() -> (KernelBody, ValueId) {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let defining = body.add_block();
    let reading = body.add_block();
    let condition = body.add_const(entry, ScalarClass::Int, 1);
    let value = body.add_const(defining, ScalarClass::Int, 7);
    let sum = body.add_op(
        reading,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![value, value],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        entry,
        Terminator::CondBr {
            cond: condition,
            if_true: Br {
                target: defining,
                args: Vec::new(),
            },
            if_false: Br {
                target: reading,
                args: Vec::new(),
            },
        },
    );
    body.set_terminator(reading, Terminator::Return { values: vec![sum] });
    (body, value)
}

#[test]
fn a_use_in_an_arm_the_definition_does_not_dominate_is_refused_by_name() {
    let (body, value) = sibling_arms();
    let refusal = body
        .validate()
        .expect_err("a use one path does not define is refused");
    assert!(
        refusal.contains("does not dominate") && refusal.contains(&format!("{value:?}")),
        "the refusal names the value and the cause: {refusal}"
    );
}

#[test]
fn a_use_before_its_definition_in_one_block_is_refused_by_name() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let first = body.add_const(entry, ScalarClass::Int, 1);
    let second = body.add_const(entry, ScalarClass::Int, 2);
    let sum = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![first, second],
        vec![ScalarClass::Int],
    );
    body.set_terminator(entry, Terminator::Return { values: vec![sum] });
    // **The instruction list is the order the emitter binds**: the operand now follows its use.
    body.blocks[entry].instrs = vec![sum, first, second];
    let refusal = body
        .validate()
        .expect_err("a use before its definition is refused");
    assert!(
        refusal.contains("defines later in the same block"),
        "the refusal names the order: {refusal}"
    );
}

#[test]
fn a_use_the_defining_block_dominates_is_accepted() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let after = body.add_block();
    let value = body.add_const(entry, ScalarClass::Int, 7);
    let parameter = body.add_param(after);
    let sum = body.add_op(
        after,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![value, parameter],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        entry,
        Terminator::Br(Br {
            target: after,
            args: vec![value],
        }),
    );
    body.set_terminator(after, Terminator::Return { values: vec![sum] });
    body.validate()
        .expect("a definition the entry dominates is in scope, and so is the block's parameter");
}
