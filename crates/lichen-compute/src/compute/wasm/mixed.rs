//! Which class each value is, read before anything is emitted.
//!
//! **A read of the lowered IR, not an emission, and it runs before the module's
//! first entity exists** (see [`assemble_module`]). That placement is the whole
//! design: whether two classes meet is a property of the lowered body, and the
//! answer is available before a section — let alone an instruction — is written,
//! so the refusal costs nothing. A branch inside the lowering would be too late
//! to be free and would report a position inside an emission rather than the
//! instruction the mix is in.
//!
//! **What a mix is, and what it is not.** A body may hold values of both classes
//! — that is what per-value classes are for, and a float kernel's index beside
//! its data is the ordinary case. What it may not do is meet them in one
//! operation, because `Int` and `Float` do not convert in either direction
//! (`docs/notes/floating-point.md` §4.2): so the walk refuses where the two
//! actually meet, which is the `Bin` whose operands disagree, the `write` storing
//! one class into the other's buffer, or an index that is not an integer.
//!
//! The two backends can only agree because they agree on *what the IR means*:
//! every instruction carries the class it was lowered in, a parameter leaf takes
//! its class from `param_shape` by the offset `LocalGet` names, and a comparison's
//! `0`/`1` is `1`/`0` in either class. The fragment has no class to consult —
//! `fragment_class` is its first write's, and a body may compute in more than
//! one.

use lichen_kernel_ir::{Flow, KernelBin, KernelFragment, KernelInstr, ScalarClass, Terminator};

use crate::compute::param_classes;

/// The class a value on the fragment's stack has, as far as a walk of the
/// lowered body can see it.
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
    /// A value this walk cannot classify: a cross-kernel call's results (the
    /// arity is the callee's domain, not this fragment's) and a loop's carried
    /// values (which arrive from a label this walk does not resolve).
    ///
    /// **Unclassifiable is not the other class.** It is the absence of an answer,
    /// and a check that read it as one would refuse a fragment the IR says
    /// nothing about — so it is carried, never judged.
    Opaque,
}

/// Refuse a fragment whose body meets an `Int` and a `Float` in one operation.
pub(super) fn refuse_mixed_classes(fragment: &KernelFragment) -> Result<(), String> {
    let params = param_classes(fragment);
    // A function's parameters are locals rather than stack values, so the stack a
    // body starts from is empty and `LocalGet` is what puts a value on it.
    let mut stack = Vec::new();
    check_flow(&fragment.body.entry, &params, &mut stack)
}

/// Walk one flow, carrying the classes the enclosing block left on the stack.
fn check_flow(
    flow: &Flow,
    params: &[ScalarClass],
    stack: &mut Vec<OperandClass>,
) -> Result<(), String> {
    match flow {
        // A jump hands its values to a label this walk does not resolve; the
        // values were checked where they were computed.
        Flow::Jump { .. } => Ok(()),
        // A `Seq` is a loop body computing its carried values and then handing
        // them back — the same instruction-then-terminator shape as a `Block`,
        // so the walk is the same.
        Flow::Seq {
            instrs, terminator, ..
        } => {
            for (at, instruction) in instrs.iter().enumerate() {
                check_instr(instruction, params, stack, at)?;
            }
            check_terminator(terminator, params, stack)
        }
        Flow::Block {
            instrs, terminator, ..
        } => {
            // `at` is the index within this block's own instruction list, which is
            // what the SPIR-V refusal's `at` counts: the same numbering the other
            // emitter reports, over the same list.
            for (at, instruction) in instrs.iter().enumerate() {
                check_instr(instruction, params, stack, at)?;
            }
            check_terminator(terminator, params, stack)
        }
    }
}

/// Pop the top of the walk's stack, or [`OperandClass::Opaque`] if it is empty.
///
/// **An underflow is a question, not an answer**, and the honest one to give back
/// is that this walk does not know what was there: the emitter's own
/// `UnbalancedStack` is the refusal for a body this shape cannot happen in, and
/// inventing one here would be a second cause for a single defect.
fn pop_class(stack: &mut Vec<OperandClass>) -> OperandClass {
    stack.pop().unwrap_or(OperandClass::Opaque)
}

/// The class a binary operator runs over, from its two operands.
///
/// **One operand is enough to state it**, because an operator's operands are one
/// class by construction — the lowering reads the class off the instruction, and
/// the language refuses `(x : Float) => x * 2` before any of this. Two operands
/// that disagree are therefore not a case to resolve here but the case the caller
/// refuses ([`as_operand_class`]), so the second is only consulted when the first
/// says nothing.
fn operand_class(lhs: OperandClass, rhs: OperandClass) -> Option<ScalarClass> {
    match (lhs, rhs) {
        (OperandClass::Scalar(class), _) => Some(class),
        (_, OperandClass::Scalar(class)) => Some(class),
        _ => None,
    }
}

/// `operand` in a position that wants `want`, or the refusal when the two classes
/// meet and neither gives way.
///
/// **The refusal is the whole point of the walk.** `Int` and `Float` do not
/// convert in either direction (`docs/notes/floating-point.md` §4.2), so a value
/// of one class in a position of the other is a malformed fragment rather than a
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

/// Check one instruction's effect on the stack.
///
/// **Every instruction that carries a class is checked against its own**, which
/// is what makes this walk exact: a value's class is a fact of the value, so the
/// only question a mix raises is whether two values of *different* classes meet
/// in one operation. The fragment has no class to consult — `fragment_class` is
/// its first write's, and a body may compute in more than one.
fn check_instr(
    instruction: &KernelInstr,
    params: &[ScalarClass],
    stack: &mut Vec<OperandClass>,
    at: usize,
) -> Result<(), String> {
    match *instruction {
        // A literal is a value of its own class: `1` is an integer and `1.0` is
        // not, and the language refuses an integer in a float position before
        // this walk sees it (`(x : Float) => x * 2` does not check), so nothing
        // here converts one into the other.
        KernelInstr::Const(class, _) => {
            stack.push(OperandClass::Scalar(class));
        }
        // **A parameter leaf's class is its own**, read from `param_shape` by the
        // offset the instruction names.  A parallel fragment's count and index
        // are both `Int` (`compile_parallel_fragment` declares them so), and a
        // scalar kernel's leaves are whatever it declared.
        KernelInstr::LocalGet(k) => {
            stack.push(
                params
                    .get(k as usize)
                    .copied()
                    .map_or(OperandClass::Opaque, OperandClass::Scalar),
            );
        }
        // The narrowing a `0`/`1` scalar takes to become a `select` condition.
        KernelInstr::I32WrapI64 => {
            if let Some(top) = stack.last_mut() {
                *top = OperandClass::Condition;
            }
        }
        KernelInstr::Select => {
            pop_class(stack);
            let otherwise = pop_class(stack);
            let then = pop_class(stack);
            // A `select`'s arms are the value it yields, so they are one class;
            // the narrowing above is what makes the condition a `0`/`1` scalar.
            let class = operand_class(then, otherwise).unwrap_or(ScalarClass::Int);
            as_operand_class(then, class, at)?;
            as_operand_class(otherwise, class, at)?;
            stack.push(OperandClass::Scalar(class));
        }
        KernelInstr::BufferReadCall(class) => {
            let element = pop_class(stack);
            pop_class(stack);
            // An access chain's index is an integer.
            as_operand_class(element, ScalarClass::Int, at)?;
            // The element is the class the instruction names, which is the class
            // `Positions::input_classes` declares that position to be.
            stack.push(OperandClass::Scalar(class));
        }
        KernelInstr::BufferWriteCall(class) => {
            let value = pop_class(stack);
            let element = pop_class(stack);
            pop_class(stack);
            // **This is where a mix is caught**: a value of the other class
            // stored into this buffer, or an index that is not an integer.
            as_operand_class(value, class, at)?;
            as_operand_class(element, ScalarClass::Int, at)?;
        }
        KernelInstr::Bin(class, operator) => {
            let rhs = pop_class(stack);
            let lhs = pop_class(stack);
            as_operand_class(lhs, class, at)?;
            as_operand_class(rhs, class, at)?;
            // A comparison yields the language's `0`/`1` scalar rather than a
            // value of the operand class, and a target whose `select` takes a
            // narrower condition (this one) narrows it here.
            stack.push(match operator {
                KernelBin::Lt
                | KernelBin::Gt
                | KernelBin::Leq
                | KernelBin::Geq
                | KernelBin::Eq
                | KernelBin::Neq => OperandClass::Condition,
                _ => OperandClass::Scalar(class),
            });
        }
        // The language's two class crossings (`int2float`/`float2int`) as one
        // instruction. **The pair is what makes it explicit**: `Int → Float` and
        // `Float → Int` have the same shape on a stack machine, so a backend that
        // read the direction off the operand would be guessing — and the two
        // backends could guess differently (`docs/notes/floating-point.md` §5.1).
        //
        // The operand must be the class the instruction converts *from*, and
        // nothing gives way: `1.5 + 1` and `int2float 1.0` are refusals made
        // before any of this.
        //
        // `from == to` is a **reclassification, not a no-op**: it is what the
        // lowering writes for a value that already holds its result's
        // representation, so this walk reads it as a `to` rather than as the
        // class it was. A value the walk cannot name stays unnamed rather than
        // being asserted into `to`.
        KernelInstr::Conv { from, to } => {
            let seen = pop_class(stack);
            let seen = as_operand_class(seen, from, at)?;
            stack.push(match seen {
                OperandClass::Opaque => OperandClass::Opaque,
                _ => OperandClass::Scalar(to),
            });
        }
        // **The stack is unknown from here on**, so it is emptied rather than
        // guessed at: the callee's arity is its own domain's, this walk has only
        // the fragment, and a value it cannot place is not a value of the other
        // class.
        KernelInstr::CallKernel(_) => stack.clear(),
    }
    Ok(())
}

/// Check one terminator, and walk into the flows it opens.
fn check_terminator(
    terminator: &Terminator,
    params: &[ScalarClass],
    stack: &mut Vec<OperandClass>,
) -> Result<(), String> {
    match terminator {
        Terminator::Return => Ok(()),
        // A plain transfer: the walk does not resolve the label it arrives at, and
        // the values it hands over were checked where they were computed.
        Terminator::Jump { .. } => Ok(()),
        Terminator::If {
            on_one,
            on_zero,
            passes,
            ..
        } => {
            // The `0`/`1` selector is on top and each arm runs on what is below
            // it, so each arm is checked against its own copy of that stack.
            pop_class(stack);
            let mut arm = stack.clone();
            check_flow(on_one, params, &mut arm)?;
            if let Some(on_zero) = on_zero {
                let mut arm = stack.clone();
                check_flow(on_zero, params, &mut arm)?;
            }
            // The join receives `passes` values whose class depends on which arm
            // ran, so what is left on the stack is how many there are and not
            // what they are.
            stack.resize(stack.len() + passes, OperandClass::Opaque);
            Ok(())
        }
        Terminator::While { body, carried, .. } => {
            // **The loop's inherited values are unclassified.** The values the
            // backedge brings back arrive from a label rather than from an
            // instruction here, so nothing the body inherits from the header's
            // entry stack can be judged — only what the body computes from it.
            let mut body_stack = vec![OperandClass::Opaque; stack.len() + carried];
            let result = check_flow(body, params, &mut body_stack);
            // The loop leaves through its `exit` label, which this walk does not
            // resolve, so the enclosing block continues from nothing known.
            stack.clear();
            result
        }
    }
}

/// The reason a `jit` reports when one operation meets an `Int` and a `Float`.
///
/// **The sentence is the SPIR-V emitter's, byte for byte** — the one
/// `SpirvRefusal::MixedClasses` renders in
/// `crates/lichen-compute-gpu/src/spirv.rs`. The two emitters read the same
/// untyped IR and reach this answer independently, and a program that runs on
/// both backends must not be told two different things about the same fragment —
/// so the wording is a contract between the two crates rather than a constant
/// one of them owns, neither crate depending on the other.
///
/// `at` is **the index of the instruction in its own block**, which is what
/// SPIR-V's `at` is: the position the refusal was raised at, not a line of the
/// program. It is why a mix is reported where the two classes actually meet — a
/// `write` storing an integer into a float buffer is refused at the write, and
/// the arithmetic that computed the integer is not where the reader should look.
pub(super) fn mixed_classes(at: usize) -> String {
    format!(
        "instruction {at} mixed an integer and a float in one operation. `Int` and `Float` do not \
         convert in either direction, so nothing here can make the two operands meet: this is a \
         malformed fragment rather than an unsupported shape."
    )
}
