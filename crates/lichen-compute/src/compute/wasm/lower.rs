//! The instruction map: [`KernelInstr`] onto `waffle`'s SSA values.
//!
//! **Every instruction carries its own class**, so nothing here is decided by the
//! fragment: a constant is already in the representation its opcode reads
//! ([`ScalarClass`] names the two), and an arithmetic operator is its own class's.
//! What *is* tracked here is **the representation each value actually holds**,
//! because one instruction cannot state it: a [`KernelInstr::Conv`] names the
//! classes the *language* asked for, while the value on the stack was built by
//! whatever produced it — and at the ABI the two disagree, since a float
//! fragment's index and count arrive in `f32` parameters while the language's
//! number is an [`ScalarClass::Int`].
//!
//! # Why every slot is typed, and what changed from the hand-written emitter
//!
//! The withdrawn emitter carried an `Option<ScalarClass>` per stack slot, `None`
//! meaning *this lowering cannot name it*, and a cross-kernel call's result was
//! the one slot that could not be. **Here every slot is a plain
//! [`Type`]**, because `waffle`'s IR has no untyped value — and the one slot that
//! was unnamed is nameable: the launch set is in hand, so a callee's result is
//! typed by the callee's own `result_classes`, which are exactly the types its
//! wasm signature declares. That is the `Option` disappearing into a fact rather
//! than into a guess.

use std::collections::HashMap;

use lichen_kernel_ir::{Flow, KernelBin, KernelFragment, KernelId, KernelInstr, ScalarClass};
use waffle::{
    Block, Func, FunctionBody, Module, Operator, Signature, Terminator as WasmTerminator, Type,
    Value,
};

use super::assemble::BufferImports;
use crate::compute::param_classes;

/// One value the body has left on the operand stack: the `waffle` value, and the
/// wasm type it holds.
///
/// **The type is what makes a crossing decidable**, and it is not the same
/// question as the class: a comparison yields the language's `0`/`1` scalar,
/// which is an `i64` in either class, and a narrowing turns a `0`/`1` scalar
/// into an `i32`. Both are facts about the value in hand, and both are what the
/// next instruction has to be told.
struct Slot {
    value: Value,
    ty: Type,
}

/// What one fragment's lowering needs from the rest of the assembly.
struct Lowering<'a> {
    /// The launch set, so a cross-kernel call can be typed by its callee's own
    /// result list rather than left unnamed.
    callees: &'a [KernelFragment],
    /// Each callee [`KernelId`]'s position in `callees`.
    index: &'a HashMap<KernelId, u32>,
    imports: &'a BufferImports,
    /// The parameter leaves' classes, in flattening order — the ABI types each
    /// [`KernelInstr::LocalGet`] reads back.
    ///
    /// **A leaf's class is its own, not the fragment's**: a float fragment's
    /// count and index are `f32` parameters whatever the body computes.
    leaves: &'a [ScalarClass],
    /// The entry block's blockparams, which **are** the function's parameters, in
    /// the ABI's flattened leaf order. A [`KernelInstr::LocalGet`] reads one of
    /// these directly rather than emitting a `local.get` against an index.
    params: &'a [Value],
}

/// Lower one fragment's body into a `waffle` [`FunctionBody`].
///
/// **One block, no control flow.** The entry block's blockparams *are* the
/// function's parameters (`FunctionBody::new` builds them), so the ABI's
/// flattened leaves are already named and a [`KernelInstr::LocalGet`] reads one
/// of them directly rather than a `local.get`.
///
/// A body's transfer other than [`lichen_kernel_ir::Terminator::Return`] is
/// refused by name — including the entry block naming a label, which is how a
/// `While` marks its own header. Serving those is `docs/notes/wasm-backend-handoff.md`
/// §3.2.
pub(super) fn lower_fragment(
    fragment: &KernelFragment,
    callees: &[KernelFragment],
    index: &HashMap<KernelId, u32>,
    module: &Module,
    signature: Signature,
    imports: &BufferImports,
) -> Result<FunctionBody, String> {
    fragment
        .body
        .validate()
        .map_err(|broken| format!("compute.wasm: the kernel body is malformed: {broken}"))?;
    let Flow::Block { entry, instrs, .. } = &fragment.body.entry else {
        return Err(
            "compute.wasm: a kernel body must begin with a block, and a `Seq` is a loop's own body \
             rather than a fragment's"
                .to_string(),
        );
    };
    if let Some(label) = entry {
        return Err(format!(
            "compute.wasm: the fragment's own block is labelled {} and so is a loop's header — a loop \
             needs a block of its own for the carried tuple, which this one-block lowering does not \
             yet give it",
            label.0
        ));
    }

    let mut body = FunctionBody::new(module, signature);
    let block = body.entry;
    let params: Vec<Value> = body.blocks[block]
        .params
        .iter()
        .map(|(_, value)| *value)
        .collect();
    let lowering = Lowering {
        callees,
        index,
        imports,
        leaves: &param_classes(fragment),
        params: &params,
    };

    let mut stack: Vec<Slot> = Vec::new();
    for instruction in instrs {
        lower_instr(*instruction, &mut body, block, &mut stack, &lowering)?;
    }

    let values = return_values(&mut stack, fragment.result_classes.len())?;
    body.set_terminator(
        block,
        WasmTerminator::Return {
            values: values.iter().map(|slot| slot.value).collect(),
        },
    );
    Ok(body)
}

/// The top `results` slots, in order, and the stack without them.
fn return_values(stack: &mut Vec<Slot>, results: usize) -> Result<Vec<Slot>, String> {
    if stack.len() < results {
        return Err(format!(
            "compute.wasm: the body returns {results} value(s) but leaves only {} on the stack",
            stack.len()
        ));
    }
    Ok(stack.split_off(stack.len() - results))
}

/// Lower one instruction onto `block`, consuming and producing slots.
fn lower_instr(
    instruction: KernelInstr,
    body: &mut FunctionBody,
    block: Block,
    stack: &mut Vec<Slot>,
    lowering: &Lowering<'_>,
) -> Result<(), String> {
    // `KernelInstr` is `Copy`, so this matches it **by value**: an instruction's
    // class is a value, not a borrow, and every arm below reads it directly.
    match instruction {
        KernelInstr::Const(class, bits) => {
            let (operator, ty) = match class {
                ScalarClass::Int => (Operator::I64Const { value: bits as u64 }, Type::I64),
                ScalarClass::Float => (Operator::F32Const { value: bits as u32 }, Type::F32),
            };
            emit(body, block, operator, &[], &[ty], stack);
        }
        KernelInstr::Bin(class, operator) => {
            let rhs = pop(stack)?;
            let lhs = pop(stack)?;
            let native = binary_operator(class, operator)?;
            if !is_comparison(operator) {
                let result = super::assemble::value_type(class);
                emit(
                    body,
                    block,
                    native,
                    &[lhs.value, rhs.value],
                    &[result],
                    stack,
                );
                return Ok(());
            }
            // **A comparison is two operators, and this is the fact the withdrawn
            // hand-written emitter got wrong four times.** The language's
            // comparison yields an `i64` `0`/`1` scalar; wasm's yields an `i32`.
            // The widening is a named step here rather than a comment beside each
            // comparison (`docs/notes/wasm-control-flow.md` §2), and the converse
            // narrowing — `I32WrapI64` — is what a `select` and a `CondBr` need.
            //
            // **The narrowing leaves no slot of its own**: two operators make one
            // value, and a stack slot per operator would put the `i32` underneath
            // the `i64` and make the next instruction read the wrong one.
            let narrow = body.add_op(block, native, &[lhs.value, rhs.value], &[Type::I32]);
            emit(
                body,
                block,
                Operator::I64ExtendI32U,
                &[narrow],
                &[Type::I64],
                stack,
            );
        }
        KernelInstr::LocalGet(offset) => {
            let leaf = usize::try_from(offset).unwrap_or(usize::MAX);
            let ty = lowering
                .leaves
                .get(leaf)
                .copied()
                .map(super::assemble::value_type)
                .ok_or_else(|| {
                    format!(
                        "compute.wasm: a body reads parameter leaf {leaf}, which a domain of {} \
                         leaf/leaves does not have",
                        lowering.leaves.len()
                    )
                })?;
            let value = *lowering.params.get(leaf).ok_or_else(|| {
                format!(
                    "compute.wasm: a body reads parameter leaf {leaf}, which the function's \
                         signature does not have"
                )
            })?;
            stack.push(Slot { value, ty });
        }
        // The narrowing a `0`/`1` scalar takes to become a `select` condition or a
        // branch's `CondBr`. **The slot's type is the `i32` it now is**, which is
        // the only thing either consumer can use it for.
        KernelInstr::I32WrapI64 => {
            let top = pop(stack)?;
            emit(
                body,
                block,
                Operator::I32WrapI64,
                &[top.value],
                &[Type::I32],
                stack,
            );
        }
        KernelInstr::Select => {
            // **The condition is on top**, so the three pops read it first, then
            // the two arms — and wasm's `select` takes them in the order
            // `[then, else, condition]`, so the pops are reversed to hand them
            // over. The hand-written emitter took the *top* as the result's class
            // here, which is the condition rather than either arm; nothing caught
            // it because a body that selects is returning the value immediately,
            // and the type it tracked was never read again.
            let selector = pop(stack)?;
            let otherwise = pop(stack)?;
            let then = pop(stack)?;
            // The arms are the value, so either one's type is the result's: they
            // are one class by construction (`refuse_mixed_classes` refuses
            // otherwise).
            emit(
                body,
                block,
                Operator::Select,
                &[then.value, otherwise.value, selector.value],
                &[otherwise.ty],
                stack,
            );
        }
        // The crossing. `from` and `to` are what the **language** asked for; what
        // wasm holds is the value the stack carries, and the two are different
        // questions — which is why the lowering reads one to answer the other.
        //
        // The classes are the language's, so this is the one place a
        // representation can differ from the class a later instruction names: a
        // float fragment's index and count are `f32` parameters already, while the
        // number the language means is an `Int`.
        KernelInstr::Conv { from, to } => {
            let operand = pop(stack)?;
            match (operand.ty, to) {
                // Already the target's representation: the conversion is a
                // reclassification and wasm is told nothing.
                (seen, to) if seen == super::assemble::value_type(to) => stack.push(Slot {
                    value: operand.value,
                    ty: seen,
                }),
                (Type::I64, ScalarClass::Float) => {
                    emit(
                        body,
                        block,
                        Operator::F32ConvertI64U,
                        &[operand.value],
                        &[Type::F32],
                        stack,
                    );
                }
                (Type::F32, ScalarClass::Int) => {
                    emit(
                        body,
                        block,
                        Operator::I64TruncF32U,
                        &[operand.value],
                        &[Type::I64],
                        stack,
                    );
                }
                // The instruction names a crossing this value is not on either
                // side of: it was never a representation the two classes could
                // meet at, and a narrowing to an `i32` is not one of them either.
                (seen, to) => {
                    return Err(format!(
                        "compute.wasm: a {seen} value cannot be lowered as a {to:?} conversion from \
                         {from:?} — the instruction names a crossing this body's value is not on \
                         either side of"
                    ));
                }
            }
        }
        KernelInstr::CallKernel(callee) => {
            let at = *lowering.index.get(&callee).ok_or_else(|| {
                format!("cross-kernel call to kernel {callee} is not in the assembled set")
            })?;
            let target = lowering.callees.get(at as usize).ok_or_else(|| {
                format!(
                    "cross-kernel call to kernel {callee} names position {at}, which the launch \
                         set does not hold"
                )
            })?;
            let results: Vec<Type> = target
                .result_classes
                .iter()
                .copied()
                .map(super::assemble::value_type)
                .collect();
            let Some(&result) = results.first() else {
                return Err(format!(
                    "compute.wasm: kernel {callee} returns no value, so a call to it cannot leave one \
                     for the caller's body to compute with"
                ));
            };
            if results.len() > 1 {
                // The frontend refuses this call before a fragment is built, so
                // reaching it means the fragment was assembled by something other
                // than `jit`. **A multi-result callee is a tuple**, and reading one
                // element of it is `PickOutput` in this IR — which the caller's
                // body would have to name, and `KernelInstr` does not.
                return Err(format!(
                    "compute.wasm: kernel {callee} returns {} values, and a cross-kernel call here \
                     leaves one stack slot — a multi-value call is refused by `jit` before it \
                     reaches a backend, so this fragment was not built by it",
                    results.len()
                ));
            }
            let arity = callee_arity(lowering, callee)?;
            let args = pop_many(stack, arity)?;
            let call = body.add_op(
                block,
                Operator::Call {
                    function_index: Func::from(lowering.imports.base + at),
                },
                &args,
                &results,
            );
            stack.push(Slot {
                value: call,
                ty: result,
            });
        }
        // The `read` import for this element's class, over `[position, index]`.
        KernelInstr::BufferReadCall(class) => {
            let index = pop(stack)?;
            let position = pop(stack)?;
            let function_index = *lowering.imports.read.get(&class).ok_or_else(|| {
                format!("compute.wasm: no `read` import was declared for {class:?} elements")
            })?;
            emit(
                body,
                block,
                Operator::Call { function_index },
                &[position.value, index.value],
                &[super::assemble::value_type(class)],
                stack,
            );
        }
        // The `write` import for this element's class, over
        // `[position, index, value]`, and nothing left behind.
        KernelInstr::BufferWriteCall(class) => {
            let value = pop(stack)?;
            let index = pop(stack)?;
            let position = pop(stack)?;
            let function_index = *lowering.imports.write.get(&class).ok_or_else(|| {
                format!("compute.wasm: no `write` import was declared for {class:?} elements")
            })?;
            body.add_op(
                block,
                Operator::Call { function_index },
                &[position.value, index.value, value.value],
                &[],
            );
        }
    }
    Ok(())
}

/// Add one operator to `block` and leave its result on the stack.
fn emit(
    body: &mut FunctionBody,
    block: Block,
    operator: Operator,
    args: &[Value],
    results: &[Type],
    stack: &mut Vec<Slot>,
) -> Value {
    let value = body.add_op(block, operator, args, results);
    // A one-result operator is the only shape that leaves a value behind: a
    // `BufferWriteCall` returns nothing, and `add_op`'s caller pushes the slot
    // itself when there is one.
    if let [ty] = results {
        stack.push(Slot { value, ty: *ty });
    }
    value
}

/// Pop the top slot, or refuse: an underflow is a malformed body, and this
/// lowering says so rather than emitting an operator with a missing operand.
fn pop(stack: &mut Vec<Slot>) -> Result<Slot, String> {
    stack.pop().ok_or_else(|| {
        "compute.wasm: an instruction reads below the bottom of the stack — the body's operands \
         are not the ones the instruction consumes"
            .to_string()
    })
}

/// Pop `count` slots, deepest first.
fn pop_many(stack: &mut Vec<Slot>, count: usize) -> Result<Vec<Value>, String> {
    if stack.len() < count {
        return Err(format!(
            "compute.wasm: a call consumes {count} value(s) but only {} are on the stack",
            stack.len()
        ));
    }
    Ok(stack
        .split_off(stack.len() - count)
        .into_iter()
        .map(|slot| slot.value)
        .collect())
}

/// How many values a cross-kernel call consumes — the callee's own domain.
fn callee_arity(lowering: &Lowering<'_>, callee: KernelId) -> Result<usize, String> {
    let at = *lowering.index.get(&callee).ok_or_else(|| {
        format!("cross-kernel call to kernel {callee} is not in the assembled set")
    })?;
    let target = lowering.callees.get(at as usize).ok_or_else(|| {
        format!(
            "cross-kernel call to kernel {callee} names position {at}, which the launch set does \
                 not hold"
        )
    })?;
    Ok(target.param_shape.flat_arity())
}

/// The one wasm operator a binary operator lowers to, **before** a comparison's
/// widening.
///
/// Split from [`is_comparison`] only to say why the two exist: a comparison's
/// *result* is always the language's `0`/`1` scalar, which the caller widens
/// with [`Operator::I64ExtendI32U`], so only the comparison's own operand type
/// follows the class.
///
/// **`Rem` and the bitwise trio have no float form**, and the refusal is here
/// rather than at the site: `refuse_mixed_classes` already rejects a float
/// operand for them before any module exists, so a float reaching this is a walk
/// that disagreed with the emitter — and picking the integer form anyway would
/// compile a module that computes something else.
fn binary_operator(class: ScalarClass, operator: KernelBin) -> Result<Operator, String> {
    use KernelBin as Bin;
    use ScalarClass::{Float, Int};
    Ok(match (class, operator) {
        (Int, Bin::Add) => Operator::I64Add,
        (Int, Bin::Sub) => Operator::I64Sub,
        (Int, Bin::Mul) => Operator::I64Mul,
        // An `Int` is unsigned, so these are the unsigned division and remainder
        // (`DivS` would agree below 2^63 and differ above). A **float division is
        // IEEE and unguarded** — the language does not specify a kernel's float
        // division by zero and promises nothing about it, so no guard is added
        // here (`docs/notes/floating-point.md` §4.4).
        (Int, Bin::Div) => Operator::I64DivU,
        (Int, Bin::Rem) => Operator::I64RemU,
        (Int, Bin::BitAnd) => Operator::I64And,
        (Int, Bin::BitOr) => Operator::I64Or,
        (Int, Bin::BitXor) => Operator::I64Xor,
        // wasm's float comparisons are `i32` too, which is the same widening an
        // integer comparison takes.
        (Int, Bin::Lt) => Operator::I64LtU,
        (Int, Bin::Gt) => Operator::I64GtU,
        (Int, Bin::Leq) => Operator::I64LeU,
        (Int, Bin::Geq) => Operator::I64GeU,
        (Int, Bin::Eq) => Operator::I64Eq,
        (Int, Bin::Neq) => Operator::I64Ne,
        (Float, Bin::Add) => Operator::F32Add,
        (Float, Bin::Sub) => Operator::F32Sub,
        (Float, Bin::Mul) => Operator::F32Mul,
        (Float, Bin::Div) => Operator::F32Div,
        (Float, Bin::Lt) => Operator::F32Lt,
        (Float, Bin::Gt) => Operator::F32Gt,
        (Float, Bin::Leq) => Operator::F32Le,
        (Float, Bin::Geq) => Operator::F32Ge,
        (Float, Bin::Eq) => Operator::F32Eq,
        (Float, Bin::Neq) => Operator::F32Ne,
        (Float, operator) => {
            return Err(format!(
                "compute.wasm: {operator:?} has no floating-point form — the bitwise operators and \
                 `Rem` are the language's integer-scalar operators, so a float operand here is a \
                 fragment the class walk should already have refused"
            ));
        }
    })
}

/// Whether a binary operator yields the language's `0`/`1` scalar rather than a
/// value of its operand class.
fn is_comparison(operator: KernelBin) -> bool {
    matches!(
        operator,
        KernelBin::Lt
            | KernelBin::Gt
            | KernelBin::Leq
            | KernelBin::Geq
            | KernelBin::Eq
            | KernelBin::Neq
    )
}
