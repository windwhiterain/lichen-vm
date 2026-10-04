//! Refuse a fragment whose body meets an `Int` and a `Float` in one operation.
//!
//! # What this walk is for, and why it is here rather than in a consumer
//!
//! `Int` and `Float` **do not convert in either direction**
//! (`docs/notes/floating-point.md` §4.2), so a value of one class in a position
//! of the other is a malformed fragment rather than a shape a conversion could
//! serve. Each backend used to answer that question from its own walk, and the
//! refusal wording is a **contract between two crates that do not depend on each
//! other**: SPIR-V's `SpirvRefusal::MixedClasses` renders the same sentence
//! [`mixed_classes`] renders here, and a program that runs on both must not be
//! told two different things about the same fragment.
//!
//! **It runs before any module exists**, so a fragment that mixes classes costs no
//! emitted instruction.
//!
//! # The stack is gone, and this walk got simpler because of it
//!
//! This walked an operand `Vec<OperandClass>`, popping and pushing as it went,
//! and an underflow was a real case it had to answer — the answer being
//! [`OperandClass::Opaque`], because a callee's arity is its own domain's and a
//! loop's carried values arrived from a label it did not resolve.
//!
//! **In SSA every operand is named**, so the walk is a map from
//! [`ValueId`](lichen_kernel_ir::ValueId) to a class, and the class of an operand
//! is a fact of the definition that produced it rather than something this walk
//! tracks by position. The `Opaque` case survives only where it really is
//! unanswerable: a block parameter whose incoming branches have not been read yet,
//! and a cross-kernel call's results.

use std::collections::HashMap;

use lichen_kernel_ir::{KernelBin, KernelFragment, KernelInstr, ScalarClass, ValueDef, ValueId};

use crate::compute::param_classes;

/// The class a value has, as far as a walk of the lowered body can see it.
///
/// **The same cases the SPIR-V emitter's `Kind` names**
/// (`crates/lichen-compute-gpu/src/spirv.rs`).** The IR is untyped and each
/// emitter supplies the types, so a refusal that meant one thing in one of them
/// and another in the other would be the very disagreement this walk exists to
/// remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperandClass {
    /// A value of the class it was computed in — a literal's own, a buffer
    /// element's, a parameter leaf's, or an operator's.
    Scalar(ScalarClass),
    /// A comparison's `0`/`1` scalar, which is `1`/`0` in either class.
    Condition,
    /// A value this walk cannot classify: a cross-kernel call's results, whose arity
    /// is the callee's domain rather than this fragment's.
    ///
    /// **Unclassifiable is not the other class.** It is the absence of an answer,
    /// and a check that read it as one would refuse a fragment the IR says nothing
    /// about — so it is carried, never judged.
    Opaque,
}

/// Refuse a fragment whose body meets an `Int` and a `Float` in one operation.
pub(super) fn refuse_mixed_classes(fragment: &KernelFragment) -> Result<(), String> {
    let params = param_classes(fragment);
    let body = &fragment.body;

    // The entry block's parameters are the ABI's leaves, in flattening order — so
    // **a leaf's class is its own**, read from `param_shape`, not the fragment's: a
    // parallel fragment's count and index are `Int` whatever the body computes.
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

    // **A block parameter's class comes from the branches that reach it**, and a
    // branch hands over values that some block computes — so this is a fixed point,
    // not a sweep. Two incoming branches that disagree are the same mix this walk
    // refuses, raised at the merge rather than at an operation.
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

    // Then the instructions, in walk order. **A definition's class is its own
    // declaration**, checked against what its operands say — which is what makes
    // this exact: a value's class is a fact of the value, so the only question a
    // mix raises is whether two values of *different* classes meet in one
    // operation. The fragment has no class to consult — `fragment_class` is its
    // first write's, and a body may compute in more than one.
    for block in &body.blocks {
        for (at, &instr) in block.instrs.iter().enumerate() {
            let Some(ValueDef::Instr { op, args, .. }) = body.values.get(instr.0 as usize) else {
                continue;
            };
            let (op, args) = (*op, args.clone());
            // **Each definition records what it leaves behind**, which is what makes
            // the walk a map rather than a stack: an operand's class is the class the
            // instruction that defined it produced, not something tracked by position.
            let produced = check(op, &args, &defined, at)?;
            if op.produces() > 0 {
                defined.insert(instr, produced);
            }
        }
    }
    // **A terminator adds nothing to check**: every value it hands over was
    // checked where it was computed, and a block's parameter classes were settled
    // by the fixed point above.
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
        // A literal is a value of its own class: `1` is an integer and `1.0` is
        // not, and the language refuses an integer in a float position before this
        // walk sees it (`(x : Float) => x * 2` does not check), so nothing here
        // converts one into the other. There is no operand to meet anything with.
        KernelInstr::Const(class, _) => Ok(OperandClass::Scalar(class)),
        KernelInstr::Bin(class, operator) => {
            as_operand_class(class_of(&args[0]), class, at)?;
            as_operand_class(class_of(&args[1]), class, at)?;
            // **A comparison yields the language's `0`/`1` scalar**, which is `1`/`0`
            // in either class — so it is not an integer and must not be refused as
            // one where a condition is wanted.
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
        // **The narrowing a `0`/`1` scalar takes** to become a `select` condition or
        // a branch's: whatever came in is now a condition, which is the one place
        // the two classes stop being a question.
        KernelInstr::I32WrapI64 => Ok(OperandClass::Condition),
        // **A `select`'s arms are the value it yields, so they are one class**,
        // and the narrowing is what makes the condition a `0`/`1` scalar.
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
            // **This is where a mix is caught**: a value of the other class stored
            // into this buffer, or an index that is not an integer.
            as_operand_class(class_of(&args[2]), class, at)?;
            as_operand_class(class_of(&args[1]), ScalarClass::Int, at)?;
            // A write leaves nothing behind, and the class this answers is the
            // absent one rather than a guess at what a caller would do with it.
            Ok(OperandClass::Opaque)
        }
        // The language's two class crossings (`int2float`/`float2int`) as one
        // instruction. **The pair is what makes it explicit**: the two directions
        // have the same shape, so a backend that read the direction off the operand
        // would be guessing — and the two backends could guess differently
        // (`docs/notes/floating-point.md` §5.1).
        //
        // The operand must be the class the instruction converts *from*, and
        // nothing gives way: `1.5 + 1` and `int2float 1.0` are refusals made before
        // any of this.
        //
        // `from == to` is a **reclassification, not a no-op**: it is what the
        // lowering writes for a value that already holds its result's
        // representation. A value this walk cannot name stays unnamed rather than
        // being asserted into `to`.
        KernelInstr::Conv { from, to } => match as_operand_class(class_of(&args[0]), from, at)? {
            OperandClass::Opaque => Ok(OperandClass::Opaque),
            _ => Ok(OperandClass::Scalar(to)),
        },
        // **The call's results are unclassified**: the callee's arity is its own
        // domain's and this walk holds only the fragment. A value it cannot place
        // is not a value of the other class, so it is absent rather than guessed —
        // and an operand that reads it back is `Opaque`, which no position judges.
        KernelInstr::CallKernel(_) => Ok(OperandClass::Opaque),
    }
}

/// The class two `select` arms agree on, if they do.
///
/// **One operand is enough to state it**, because a select's arms are one class by
/// construction; two that disagree are refused by the caller.
fn arms_class(then: OperandClass, otherwise: OperandClass) -> Option<ScalarClass> {
    match (then, otherwise) {
        (OperandClass::Scalar(class), _) => Some(class),
        (_, OperandClass::Scalar(class)) => Some(class),
        _ => None,
    }
}

/// `operand` in a position that wants `want`, or the refusal when the two classes
/// meet and neither gives way.
///
/// **The refusal is the whole point of the walk.** `Int` and `Float` do not
/// convert in either direction (`docs/notes/floating-point.md` §4.2), so a value of
/// one class in a position of the other is a malformed fragment rather than a
/// shape a conversion could serve.
fn as_operand_class(
    operand: OperandClass,
    want: ScalarClass,
    at: usize,
) -> Result<OperandClass, String> {
    match operand {
        // A comparison's `0`/`1` is `1`/`0` in either class, so a position that
        // wants one takes it: the one case the IR leaves open on purpose, and the
        // one the other emitter converts rather than refuses.
        OperandClass::Condition => Ok(OperandClass::Scalar(want)),
        OperandClass::Scalar(seen) if seen == want => Ok(OperandClass::Scalar(want)),
        OperandClass::Scalar(_) => Err(mixed_classes(at)),
        OperandClass::Opaque => Ok(OperandClass::Opaque),
    }
}

/// The reason a `jit` reports when one operation meets an `Int` and a `Float`.
///
/// **The sentence is the SPIR-V emitter's, byte for byte** — the one
/// `SpirvRefusal::MixedClasses` renders in
/// `crates/lichen-compute-gpu/src/spirv.rs`. The two emitters read the same
/// untyped IR and reach this answer independently, and a program that runs on both
/// backends must not be told two different things about the same fragment — so
/// the wording is a contract between the two crates rather than a constant one of
/// them owns, neither crate depending on the other.
///
/// `at` is **the index of the instruction in its own block**, which is what
/// SPIR-V's `at` is: the position the refusal was raised at, not a line of the
/// program. It is why a mix is reported where the two classes actually meet — a
/// `write` storing an integer into a float buffer is refused at the write, and the
/// arithmetic that computed the integer is not where the reader should look.
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
