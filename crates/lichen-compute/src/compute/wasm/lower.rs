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
//!
//! **What a body *transfers* is not here.** Which block a stack ends in, and how a
//! carried value survives a backedge, is [`flow`](super::flow)'s question; this
//! file answers only what one instruction computes.

use std::collections::HashMap;

use lichen_kernel_ir::{KernelBin, KernelFragment, KernelId, KernelInstr, ScalarClass};
use waffle::{Block, Func, FunctionBody, Operator, Type, Value};

use super::assemble::BufferImports;
use super::flow::Slot;

/// What one fragment's lowering needs from the rest of the assembly, and nothing
/// about the fragment it is lowering.
///
/// **Read-only, and shared with the flow walk**, because these are facts of the
/// launch set rather than of one body: a cross-kernel call is typed by its
/// callee's own `result_classes`, and a parameter leaf's class by the ABI's
/// flattened `param_shape`.
pub(super) struct ModuleCtx<'a> {
    /// The launch set, so a cross-kernel call can be typed by its callee's own
    /// result list rather than left unnamed.
    pub(super) callees: &'a [KernelFragment],
    /// Each callee [`KernelId`]'s position in `callees`.
    pub(super) index: &'a HashMap<KernelId, u32>,
    pub(super) imports: &'a BufferImports,
    /// The parameter leaves' classes, in flattening order — the ABI types each
    /// [`KernelInstr::LocalGet`] reads back.
    ///
    /// **A leaf's class is its own, not the fragment's**: a float fragment's
    /// count and index are `f32` parameters whatever the body computes.
    pub(super) leaves: &'a [ScalarClass],
    /// The entry block's blockparams, which **are** the function's parameters, in
    /// the ABI's flattened leaf order. A [`KernelInstr::LocalGet`] reads one of
    /// these directly rather than emitting a `local.get` against an index.
    ///
    /// **A blockparam and not a local**, because the entry block has no
    /// predecessors to hand a local to: `FunctionBody::new` builds these from the
    /// signature, and nothing may add to them
    /// (`docs/notes/wasm-backend-handoff.md` §3.2).
    pub(super) params: &'a [Value],
}

/// One instruction's worth of emission state: where its operators go and what
/// they leave on the stack.
///
/// **The stack is a real operand stack**, in the sense that the next instruction
/// reads the top of it — what makes it a fact rather than a guess is that every
/// slot carries its type, so a crossing is decided by what the value holds
/// (`docs/notes/wasm-backend-handoff.md` §3.1).
pub(super) struct Lower<'a, 'b> {
    pub(super) builder: &'a mut FunctionBody,
    pub(super) context: &'a ModuleCtx<'b>,
    pub(super) block: Block,
    pub(super) stack: &'a mut Vec<Slot>,
}

impl Lower<'_, '_> {
    /// Add one operator to the current block and leave its result on the stack.
    pub(super) fn emit(&mut self, operator: Operator, args: &[Value], results: &[Type]) -> Value {
        let value = self.builder.add_op(self.block, operator, args, results);
        // A one-result operator is the only shape that leaves a value behind: a
        // `BufferWriteCall` returns nothing, and the arms that call this push the
        // slot themselves when there is one.
        if let [ty] = results {
            self.stack.push(Slot { value, ty: *ty });
        }
        value
    }

    /// Pop the top slot, or refuse: an underflow is a malformed body, and this
    /// lowering says so rather than emitting an operator with a missing operand.
    pub(super) fn pop(&mut self) -> Result<Slot, String> {
        self.stack.pop().ok_or_else(|| {
            "compute.wasm: an instruction reads below the bottom of the stack — the body's operands \
             are not the ones the instruction consumes"
                .to_string()
        })
    }

    /// Pop `count` slots, deepest first.
    pub(super) fn pop_many(&mut self, count: usize) -> Result<Vec<Value>, String> {
        if self.stack.len() < count {
            return Err(format!(
                "compute.wasm: a call consumes {count} value(s) but only {} are on the stack",
                self.stack.len()
            ));
        }
        Ok(self
            .stack
            .split_off(self.stack.len() - count)
            .into_iter()
            .map(|slot| slot.value)
            .collect())
    }
}

/// Lower one instruction, consuming and producing slots.
pub(super) fn lower_instr(
    mut lower: Lower<'_, '_>,
    instruction: KernelInstr,
) -> Result<(), String> {
    // `KernelInstr` is `Copy`, so this matches it **by value**: an instruction's
    // class is a value, not a borrow, and every arm below reads it directly.
    match instruction {
        KernelInstr::Const(class, bits) => {
            let (operator, ty) = match class {
                ScalarClass::Int => (Operator::I64Const { value: bits as u64 }, Type::I64),
                ScalarClass::Float => (Operator::F32Const { value: bits as u32 }, Type::F32),
            };
            lower.emit(operator, &[], &[ty]);
        }
        KernelInstr::Bin(class, operator) => {
            let rhs = lower.pop()?;
            let lhs = lower.pop()?;
            let native = binary_operator(class, operator)?;
            if !is_comparison(operator) {
                let result = super::assemble::value_type(class);
                lower.emit(native, &[lhs.value, rhs.value], &[result]);
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
            let narrow =
                lower
                    .builder
                    .add_op(lower.block, native, &[lhs.value, rhs.value], &[Type::I32]);
            lower.emit(Operator::I64ExtendI32U, &[narrow], &[Type::I64]);
        }
        KernelInstr::LocalGet(offset) => {
            let leaf = usize::try_from(offset).unwrap_or(usize::MAX);
            let ty = lower
                .context
                .leaves
                .get(leaf)
                .copied()
                .map(super::assemble::value_type)
                .ok_or_else(|| {
                    format!(
                        "compute.wasm: a body reads parameter leaf {leaf}, which a domain of {} \
                         leaf/leaves does not have",
                        lower.context.leaves.len()
                    )
                })?;
            let value = *lower.context.params.get(leaf).ok_or_else(|| {
                format!(
                    "compute.wasm: a body reads parameter leaf {leaf}, which the function's \
                     signature does not have"
                )
            })?;
            lower.stack.push(Slot { value, ty });
        }
        // The narrowing a `0`/`1` scalar takes to become a `select` condition or a
        // branch's `CondBr`. **The slot's type is the `i32` it now is**, which is
        // the only thing either consumer can use it for.
        KernelInstr::I32WrapI64 => {
            let top = lower.pop()?;
            lower.emit(Operator::I32WrapI64, &[top.value], &[Type::I32]);
        }
        KernelInstr::Select => {
            // **The condition is on top**, so the three pops read it first, then
            // the two arms — and wasm's `select` takes them in the order
            // `[then, else, condition]`, so the pops are reversed to hand them
            // over. The hand-written emitter took the *top* as the result's class
            // here, which is the condition rather than either arm; nothing caught
            // it because a body that selects is returning the value immediately,
            // and the type it tracked was never read again.
            let selector = lower.pop()?;
            let otherwise = lower.pop()?;
            let then = lower.pop()?;
            // The arms are the value, so either one's type is the result's: they
            // are one class by construction (`refuse_mixed_classes` refuses
            // otherwise).
            lower.emit(
                Operator::Select,
                &[then.value, otherwise.value, selector.value],
                &[otherwise.ty],
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
            let operand = lower.pop()?;
            match (operand.ty, to) {
                // Already the target's representation: the conversion is a
                // reclassification and wasm is told nothing.
                (seen, to) if seen == super::assemble::value_type(to) => {
                    lower.stack.push(Slot {
                        value: operand.value,
                        ty: seen,
                    });
                }
                (Type::I64, ScalarClass::Float) => {
                    lower.emit(Operator::F32ConvertI64U, &[operand.value], &[Type::F32]);
                }
                (Type::F32, ScalarClass::Int) => {
                    lower.emit(Operator::I64TruncF32U, &[operand.value], &[Type::I64]);
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
            let at = *lower.context.index.get(&callee).ok_or_else(|| {
                format!("cross-kernel call to kernel {callee} is not in the assembled set")
            })?;
            let target = lower.context.callees.get(at as usize).ok_or_else(|| {
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
            let arity = callee_arity(lower.context, callee)?;
            let args = lower.pop_many(arity)?;
            let call = lower.builder.add_op(
                lower.block,
                Operator::Call {
                    function_index: Func::from(lower.context.imports.base + at),
                },
                &args,
                &results,
            );
            lower.stack.push(Slot {
                value: call,
                ty: result,
            });
        }
        // The `read` import for this element's class, over `[position, index]`.
        KernelInstr::BufferReadCall(class) => {
            let index = lower.pop()?;
            let position = lower.pop()?;
            let function_index = *lower.context.imports.read.get(&class).ok_or_else(|| {
                format!("compute.wasm: no `read` import was declared for {class:?} elements")
            })?;
            lower.emit(
                Operator::Call { function_index },
                &[position.value, index.value],
                &[super::assemble::value_type(class)],
            );
        }
        // The `write` import for this element's class, over
        // `[position, index, value]`, and nothing left behind.
        KernelInstr::BufferWriteCall(class) => {
            let value = lower.pop()?;
            let index = lower.pop()?;
            let position = lower.pop()?;
            let function_index = *lower.context.imports.write.get(&class).ok_or_else(|| {
                format!("compute.wasm: no `write` import was declared for {class:?} elements")
            })?;
            lower.builder.add_op(
                lower.block,
                Operator::Call { function_index },
                &[position.value, index.value, value.value],
                &[],
            );
        }
    }
    Ok(())
}

/// How many values a cross-kernel call consumes — the callee's own domain.
pub(super) fn callee_arity(context: &ModuleCtx<'_>, callee: KernelId) -> Result<usize, String> {
    let at = *context.index.get(&callee).ok_or_else(|| {
        format!("cross-kernel call to kernel {callee} is not in the assembled set")
    })?;
    let target = context.callees.get(at as usize).ok_or_else(|| {
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
