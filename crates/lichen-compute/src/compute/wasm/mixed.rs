//! Refuse a fragment whose body meets an `Int` and a `Float` in one operation.
//! See docs/notes/floating-point.md §4.2.
//!
//! # Invariant
//! The two classes do not convert in either direction, so a value of one in the other's
//! position is a malformed fragment, and the refusal's wording is a contract between two
//! crates that do not depend on each other — SPIR-V renders the same sentence. It runs before
//! any module exists, so a mix costs no emitted instruction. In SSA every operand is named.

use std::collections::HashMap;

use lichen_kernel_ir::{KernelBin, KernelFragment, KernelInstr, ScalarClass, ValueDef, ValueId};

use crate::compute::param_classes;

/// The class a value has, as far as a walk of the lowered body can see it.
///
/// # Invariant
/// The same cases the SPIR-V emitter's `Kind` names: the IR is untyped and each emitter
/// supplies the types, so a refusal meaning one thing in one and another in the other would be
/// the disagreement this walk exists to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperandClass {
    /// A value of the class it was computed in — a literal's own, a buffer
    /// element's, a parameter leaf's, or an operator's.
    Scalar(ScalarClass),
    /// A comparison's `0`/`1` scalar, which is `1`/`0` in either class.
    Condition,
    /// A value this walk cannot classify: a cross-kernel call's results.
    ///
    /// # Invariant
    /// Unclassifiable is not the other class: a check that read it as one would refuse a
    /// fragment the IR says nothing about, so it is carried, never judged.
    Opaque,
}

/// Refuse a fragment whose body meets an `Int` and a `Float` in one operation.
pub(super) fn refuse_mixed_classes(fragment: &KernelFragment) -> Result<(), String> {
    let params = param_classes(fragment);
    let body = &fragment.body;

    // A leaf's class is its own, from `param_shape`, not the fragment's.
    let mut defined: HashMap<ValueId, OperandClass> = HashMap::new();
    for (offset, &value) in body.parameters().iter().enumerate() {
        defined.insert(
            value,
            params
                .get(offset)
                .copied()
                .map_or(OperandClass::Opaque, OperandClass::Scalar),
        );
    }

    // A block parameter's class comes from the branches that reach it, so this is a fixed
    // point rather than a sweep.
    for _ in 0..body.blocks.len() {
        let mut changed = false;
        for block in &body.blocks {
            for br in block.branches() {
                if let Some(target) = body.blocks.get(br.target) {
                    for (offset, &value) in target.params.iter().enumerate() {
                        if defined.contains_key(&value) {
                            continue;
                        }
                        let Some(argument) = br.args.get(offset) else {
                            continue;
                        };
                        let Some(seen) = defined.get(argument).copied() else {
                            continue;
                        };
                        defined.insert(value, seen);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    // A definition's class is its own declaration, checked against its operands.
    for block in &body.blocks {
        for (at, &instr) in block.instrs.iter().enumerate() {
            let Some(ValueDef::Instr { op, args, .. }) = body.values.get(instr.0 as usize) else {
                continue;
            };
            let (op, args) = (*op, args.clone());
            // Each definition records what it leaves behind, so an operand's class is
            // the defining instruction's.
            let produced = check(op, &args, &defined, at)?;
            if op.produces() > 0 {
                defined.insert(instr, produced);
            }
        }
    }
    // A terminator adds nothing to check: every value it hands over was checked where it
    // was computed.
    Ok(())
}

/// Check one instruction's operands against the class it declares, and answer what
/// it leaves behind.
fn check(
    op: KernelInstr,
    args: &[ValueId],
    defined: &HashMap<ValueId, OperandClass>,
    at: usize,
) -> Result<OperandClass, String> {
    let class_of = |value: &ValueId| defined.get(value).copied().unwrap_or(OperandClass::Opaque);
    match op {
        // A literal is a value of its own class, and nothing here converts one.
        KernelInstr::Const(class, _) => Ok(OperandClass::Scalar(class)),
        KernelInstr::Bin(class, operator) => {
            as_operand_class(class_of(&args[0]), class, at)?;
            as_operand_class(class_of(&args[1]), class, at)?;
            // A comparison yields the language's `0`/`1` scalar, in either class.
            Ok(match operator {
                KernelBin::Lt
                | KernelBin::Gt
                | KernelBin::Leq
                | KernelBin::Geq
                | KernelBin::Eq
                | KernelBin::Neq => OperandClass::Condition,
                _ => OperandClass::Scalar(class),
            })
        }
        // The narrowing a `0`/`1` scalar takes to become a condition.
        KernelInstr::I32WrapI64 => Ok(OperandClass::Condition),
        // A `select`'s arms are the value it yields, so they are one class.
        KernelInstr::Select => {
            let arms = arms_class(class_of(&args[0]), class_of(&args[1]));
            if let Some(class) = arms {
                as_operand_class(class_of(&args[0]), class, at)?;
                as_operand_class(class_of(&args[1]), class, at)?;
                return Ok(OperandClass::Scalar(class));
            }
            Ok(OperandClass::Opaque)
        }
        KernelInstr::BufferReadCall(class) => {
            // An access chain's index is an integer.
            as_operand_class(class_of(&args[1]), ScalarClass::Int, at)?;
            Ok(OperandClass::Scalar(class))
        }
        KernelInstr::BufferWriteCall(class) => {
            // This is where a mix is caught: the other class stored into a buffer.
            as_operand_class(class_of(&args[2]), class, at)?;
            as_operand_class(class_of(&args[1]), ScalarClass::Int, at)?;
            // A write leaves nothing behind, and the class it answers is the absent one.
            Ok(OperandClass::Opaque)
        }
        // The two class crossings as one instruction: the pair makes the direction
        // explicit rather than guessed.

        // The operand must be the class it converts from; `from == to` is a
        // reclassification, not a no-op.
        KernelInstr::Conv { from, to } => match as_operand_class(class_of(&args[0]), from, at)? {
            OperandClass::Opaque => Ok(OperandClass::Opaque),
            _ => Ok(OperandClass::Scalar(to)),
        },
        // The call's results are unclassified — the callee's arity is its own — so an
        // operand reading one back is `Opaque`.
        KernelInstr::CallKernel(_) => Ok(OperandClass::Opaque),
    }
}

/// The class two `select` arms agree on, if they do.
///
/// # Invariant
/// One operand is enough: a select's arms are one class by construction, and two that disagree
/// are refused by the caller.
fn arms_class(then: OperandClass, otherwise: OperandClass) -> Option<ScalarClass> {
    match (then, otherwise) {
        (OperandClass::Scalar(class), _) => Some(class),
        (_, OperandClass::Scalar(class)) => Some(class),
        _ => None,
    }
}

/// `operand` in a position that wants `want`, or the refusal when classes meet.
///
/// # Invariant
/// The two classes do not convert in either direction, so a value of one in the other's
/// position is a malformed fragment rather than one a conversion could serve.
fn as_operand_class(
    operand: OperandClass,
    want: ScalarClass,
    at: usize,
) -> Result<OperandClass, String> {
    match operand {
        // A comparison's `0`/`1` is `1`/`0` in either class: the one case the IR leaves
        // open on purpose.
        OperandClass::Condition => Ok(OperandClass::Scalar(want)),
        OperandClass::Scalar(seen) if seen == want => Ok(OperandClass::Scalar(want)),
        OperandClass::Scalar(_) => Err(mixed_classes(at)),
        OperandClass::Opaque => Ok(OperandClass::Opaque),
    }
}

/// The reason a `jit` reports when one operation meets an `Int` and a `Float`.
///
/// # Invariant
/// The sentence is the SPIR-V emitter's, byte for byte: the two emitters read the same untyped
/// IR and reach this answer independently, so a program that runs on both must not be told two
/// different things. `at` is the instruction's index in its own block, so a mix is reported
/// where the two classes meet rather than where the value was computed.
pub(super) fn mixed_classes(at: usize) -> String {
    format!(
        "instruction {at} mixed an integer and a float in one operation. `Int` and `Float` do not \
         convert in either direction, so nothing here can make the two operands meet: this is a \
         malformed fragment rather than an unsupported shape."
    )
}

/// Kept so the compiler names the type the walk actually uses.
const _: Option<ScalarClass> = None;
const _: Option<KernelBin> = None;
