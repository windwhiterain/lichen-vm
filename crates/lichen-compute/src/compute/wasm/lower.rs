//! A [`KernelBody`] onto `waffle`'s SSA — **one walk, no operand stack**.
//!
//! # What this replaced
//!
//! Two files and about 1,300 lines: `lower.rs`, which mapped one [`KernelInstr`]
//! onto operators while consuming and producing an operand `Vec<Slot>`, and
//! `flow.rs`, which turned the old `Flow`/`Terminator` tree into waffle blocks.
//! Both worked on the same body in two passes, and the stack between them was the
//! thing being rebuilt at every step.
//!
//! The body is SSA, so the walk is a **map from `ValueId` to what waffle made of
//! it** — no stack, no heights, no pop, and a shared subexpression emitted once
//! rather than once per use.
//!
//! # Three passes, and why there are three
//!
//! 1. **Create the blocks and their blockparams.** waffle's entry block's
//!    blockparams are built from the signature and may not be added to
//!    (`docs/notes/wasm-backend-handoff.md` §3.2), so those come from
//!    [`ModuleCtx`] rather than being added here.
//! 2. **Type the non-entry blockparams.** A block's parameter type is the type of
//!    what its predecessors hand it, and a predecessor's arguments may themselves
//!    be computed — so this is a **fixed point**, seeded from the entry block and
//!    iterated until nothing changes. Two incoming edges that disagree on a
//!    parameter's type are refused by name, not merged.
//! 3. **Emit the instructions, then the terminators.** Every operand is already
//!    mapped, so an instruction is a direct translation with nothing to discover.
//!
//! # The representation is tracked, and it is not the class
//!
//! Every value carries a wasm [`Type`], because [`KernelInstr::Conv`] names the
//! classes the *language* asked for while the value was built by whatever produced
//! it — and at the ABI the two disagree: a float fragment's index and count arrive
//! in `f32` parameters while the language's number is an [`ScalarClass::Int`].
//! The body's declared class says which *operator* to emit; the tracked type says
//! what the value actually holds.

use std::collections::HashMap;

use lichen_kernel_ir::{
    Br, KernelBin, KernelBody, KernelFragment, KernelId, KernelInstr, ScalarClass, Terminator,
    ValueDef, ValueId,
};
use waffle::{Block, BlockTarget, Func, FunctionBody, Operator, Type, Value};

use super::assemble::BufferImports;

/// What one fragment's lowering needs from the rest of the assembly, and nothing
/// about the fragment it is lowering.
///
/// **Read-only, and shared**, because these are facts of the launch set rather
/// than of one body: a cross-kernel call is typed by its callee's own
/// `result_classes`, and a parameter leaf's class by the ABI's flattened
/// `param_shape`.
pub(super) struct ModuleCtx<'a> {
    /// The launch set, so a cross-kernel call can be typed by its callee's own
    /// result list rather than left unnamed.
    pub(super) callees: &'a [KernelFragment],
    /// Each callee [`KernelId`]'s position in `callees`.
    pub(super) index: &'a HashMap<KernelId, u32>,
    pub(super) imports: &'a BufferImports,
    /// The parameter leaves' classes, in flattening order — the ABI types of the
    /// entry block's blockparams.
    ///
    /// **A leaf's class is its own, not the fragment's**: a float fragment's count
    /// and index are `f32` parameters whatever the body computes.
    pub(super) leaves: &'a [ScalarClass],
    /// The entry block's blockparams, which **are** the function's parameters, in
    /// the ABI's flattened leaf order.
    ///
    /// **A blockparam and not a local**, because the entry block has no predecessors
    /// to hand a local to: `FunctionBody::new` builds these from the signature, and
    /// nothing may add to them.
    pub(super) params: &'a [Value],
}

/// A value and the representation it actually holds.
type Slot = (Value, Type);

/// The walk's state: what each kernel-IR value became, and which block is
/// current.
struct Walk<'a, 'b> {
    body: &'a KernelBody,
    builder: &'a mut FunctionBody,
    context: &'a ModuleCtx<'b>,
    /// **The map that replaces the operand stack.** Every value the body defines is
    /// here once it has been emitted, so an operand is a lookup rather than a pop.
    values: HashMap<ValueId, Slot>,
    /// Each kernel-IR block's waffle block, and its blockparams in order.
    blocks: Vec<(Block, Vec<Value>)>,
    /// Which block operators go into.
    current: usize,
}

impl Walk<'_, '_> {
    fn operand(&self, value: ValueId) -> Result<Slot, String> {
        self.values.get(&value).copied().ok_or_else(|| {
            format!(
                "compute.wasm: the body reads value {value:?} where it is not available — it is \
                 defined in a block that does not reach this one, or the body was not validated \
                 before lowering"
            )
        })
    }

    fn add(&mut self, operator: Operator, args: &[Value], results: &[Type]) -> Value {
        let block = self.blocks[self.current].0;
        self.builder.add_op(block, operator, args, results)
    }
}

/// Lower `body` into `builder`, whose entry block already carries the signature's
/// blockparams.
pub(super) fn lower_body(
    body: &KernelBody,
    builder: &mut FunctionBody,
    context: &ModuleCtx<'_>,
) -> Result<(), String> {
    // `validate` is the gate, and it runs **before** anything is read: a body that
    // fails it would otherwise be emitted with a branch silently dropped.
    body.validate()?;
    if body.blocks.is_empty() {
        return Err("compute.wasm: a body must have at least its entry block".into());
    }

    let mut walk = Walk {
        body,
        builder,
        context,
        values: HashMap::new(),
        blocks: Vec::new(),
        current: 0,
    };

    // Pass 1: bind the blocks. **waffle's entry block already exists** — it is the
    // one the signature built its blockparams on — so it is *used* rather than
    // added again; every other block is new. Leaving that entry block
    // unterminated is what puts an `unreachable` at the top of the function.
    let entry = walk.builder.entry;
    let mut bound = vec![(entry, Vec::new())];
    for _ in 1..body.blocks.len() {
        let block = walk.builder.add_block();
        bound.push((block, Vec::new()));
    }
    walk.blocks = bound;
    let entry_params: Vec<Value> = context.params.to_vec();
    for (offset, &value) in body.blocks[body.entry].params.iter().enumerate() {
        let slot = *entry_params.get(offset).ok_or_else(|| {
            format!(
                "compute.wasm: this function's signature has {} parameter(s) but its body declares {}",
                entry_params.len(),
                body.blocks[body.entry].params.len()
            )
        })?;
        walk.values.insert(
            value,
            (
                slot,
                context
                    .leaves
                    .get(offset)
                    .copied()
                    .map(super::assemble::value_type)
                    .unwrap_or(Type::I64),
            ),
        );
    }
    walk.blocks[body.entry].1 = entry_params;

    // Pass 2: type every value the body defines.
    type_values(&mut walk)?;

    // Pass 3: the instructions, then the terminators.
    for index in 0..body.blocks.len() {
        walk.current = index;
        let instrs = body.blocks[index].instrs.clone();
        for instr in instrs {
            lower_instr(&mut walk, instr)?;
        }
    }
    for index in 0..body.blocks.len() {
        walk.current = index;
        lower_terminator(&mut walk, index)?;
    }
    Ok(())
}

/// Decide the wasm type of **every value the body defines**, then bind each
/// non-entry block's `params` to waffle blockparams.
///
/// **The type of a value is a fact about the whole body, and it has to be
/// computed before anything is emitted.** waffle needs a block parameter's type
/// at the moment the parameter is added, and a parameter's type is the type of
/// what its predecessors hand it — which may be an *instruction the entry block
/// computes*, not an entry parameter.  Every loop's carried state is exactly
/// that (a buffer read, the argument's leaves), so deciding parameters from
/// block to block over the already-known values can never decide them: the
/// entry block's own instructions are not among what is known.
///
/// So the fixed point runs over **values**, in three rules that mirror
/// [`lower_instr`]'s own:
///
/// 1. an instruction's type follows from its operator and its operands'
///    types — a comparison widens to `i64`, `I32WrapI64` narrows, a conversion
///    is its target class, a call is its callee's result class;
/// 2. a block parameter's type is the type its incoming branches hand it, and
///    two incoming edges that disagree are a refusal rather than a merge;
/// 3. nothing is guessed: a rule whose operands are not typed yet waits for the
///    next sweep, and a body where no sweep changes anything with values still
///    untyped is refused by name.
fn type_values(walk: &mut Walk<'_, '_>) -> Result<(), String> {
    let body = walk.body;
    let count = body.values.len();
    // What each value's type is, once a rule has decided it.  The entry block's
    // parameters are decided by the ABI before anything runs.
    let mut types: Vec<Option<Type>> = vec![None; count];
    for (offset, &value) in body.blocks[body.entry].params.iter().enumerate() {
        types[value.0 as usize] = Some(
            walk.context
                .leaves
                .get(offset)
                .copied()
                .map(super::assemble::value_type)
                .unwrap_or(Type::I64),
        );
    }
    // The incoming edges, found by reading every terminator.
    let mut incoming: Vec<Vec<Vec<ValueId>>> = vec![Vec::new(); body.blocks.len()];
    for block in &body.blocks {
        for br in block.branches() {
            if br.target < body.blocks.len() {
                incoming[br.target].push(br.args.clone());
            }
        }
    }

    // **A value's definition is what decides it**, so the sweep reads the body's
    // own value table rather than the walk order: an instruction in a later
    // block may be what an earlier block's parameter receives.
    loop {
        let mut changed = false;
        for (index, definition) in body.values.iter().enumerate() {
            // A decided **instruction** is done.  A decided **block parameter**
            // is not: a later sweep may type an edge the earlier one could not
            // name, and that edge still has to agree.
            if types[index].is_some() && matches!(definition, ValueDef::Instr { .. }) {
                continue;
            }
            match definition {
                ValueDef::Instr { op, args, .. } => {
                    let operands: Option<Vec<Type>> =
                        args.iter().map(|arg| types[arg.0 as usize]).collect();
                    let Some(operands) = operands else {
                        continue;
                    };
                    let Some(ty) = instruction_type(walk.context, *op, &operands) else {
                        continue;
                    };
                    types[index] = Some(ty);
                    changed = true;
                }
                // **A block parameter is typed by the first edge that can name
                // its argument's type, and every later edge is checked against
                // it.** A loop header has two incoming edges and one of them is
                // its own body's backedge, so "wait until every edge is typed"
                // waits forever: the entry edge is the fact that seeds the
                // cycle, and a disagreement between the two is what the
                // refusal below is for.
                ValueDef::BlockParam {
                    block,
                    index: position,
                } => {
                    let mut decided: Option<Type> = types[index];
                    for args in &incoming[*block] {
                        let Some(&arg) = args.get(*position as usize) else {
                            continue;
                        };
                        let Some(ty) = types[arg.0 as usize] else {
                            continue;
                        };
                        match decided {
                            None => {
                                decided = Some(ty);
                                if types[index].is_none() {
                                    changed = true;
                                }
                            }
                            Some(seen) if seen == ty => {}
                            Some(seen) => {
                                return Err(format!(
                                    "compute.wasm: block {block}'s parameter {position} arrives as \
                                     {seen:?} from one branch and {ty:?} from another — a merge \
                                     cannot pick one"
                                ));
                            }
                        }
                    }
                    types[index] = decided;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Every value the body reads has to have a type before anything is emitted:
    // an untyped one is a body whose definitions do not reach it, and waffle
    // would be handed an operator over a value that has none.  A value that
    // **produces nothing** — a write — is not read by anything and has no type
    // to decide.
    for (index, ty) in types.iter().enumerate() {
        if ty.is_some() {
            continue;
        }
        let block = match &body.values[index] {
            ValueDef::BlockParam { block, .. } => format!("block {block}'s parameter"),
            ValueDef::Instr { op, .. } if op.produces() == 0 => continue,
            ValueDef::Instr { op, .. } => format!("{op:?}"),
        };
        return Err(format!(
            "compute.wasm: the body's {block} has no type: nothing in the body defines the value \
             it is, so it is read where it is not available"
        ));
    }

    // Bind each non-entry block's blockparams, in the order the body declares
    // them.
    for index in 0..body.blocks.len() {
        if index == body.entry {
            continue;
        }
        let block = walk.blocks[index].0;
        let params: Vec<Value> = body.blocks[index]
            .params
            .iter()
            .map(|value| {
                walk.builder
                    .add_blockparam(block, types[value.0 as usize].unwrap())
            })
            .collect();
        for (offset, &value) in body.blocks[index].params.iter().enumerate() {
            walk.values
                .insert(value, (params[offset], types[value.0 as usize].unwrap()));
        }
        walk.blocks[index].1 = params;
    }
    Ok(())
}

/// The wasm type `op` leaves when its operands are `operands`, or [`None`] for
/// an operation that produces nothing (a write) — and for an operation whose
/// result this walk cannot name.
///
/// **This is the typing half of [`lower_instr`], and the two must agree**:
/// every arm here states the type that arm there emits, from the same facts.
fn instruction_type(context: &ModuleCtx<'_>, op: KernelInstr, operands: &[Type]) -> Option<Type> {
    match op {
        KernelInstr::Const(class, _) => Some(super::assemble::value_type(class)),
        // A comparison is two operators: wasm's narrow one, then the widening to
        // the language's `i64` `0`/`1` scalar.
        KernelInstr::Bin(_, operator) if is_comparison(operator) => Some(Type::I64),
        KernelInstr::Bin(class, _) => Some(super::assemble::value_type(class)),
        KernelInstr::I32WrapI64 => Some(Type::I32),
        // The arms are the value, so either one's type is the result's.
        KernelInstr::Select => operands.first().copied(),
        KernelInstr::Conv { to, .. } => Some(super::assemble::value_type(to)),
        KernelInstr::CallKernel(callee) => {
            let at = *context.index.get(&callee)?;
            let target = context.callees.get(at as usize)?;
            target
                .result_classes
                .first()
                .copied()
                .map(super::assemble::value_type)
        }
        KernelInstr::BufferReadCall(class) => Some(super::assemble::value_type(class)),
        // A write leaves nothing, so it has no type to decide.
        KernelInstr::BufferWriteCall(_) => None,
    }
}

/// Lower one instruction. **No stack**: its operands are named by its definition,
/// and its result is a value this walk records.
fn lower_instr(walk: &mut Walk<'_, '_>, instr: ValueId) -> Result<(), String> {
    let Some(ValueDef::Instr { op, args, .. }) = walk.body.values.get(instr.0 as usize) else {
        return Err(format!(
            "compute.wasm: {instr:?} is a block parameter, not an instruction"
        ));
    };
    let (op, args) = (*op, args.clone());
    let mut operands = Vec::with_capacity(args.len());
    for &arg in &args {
        operands.push(walk.operand(arg)?);
    }

    // `KernelInstr` is `Copy`, so this matches by value.
    match op {
        KernelInstr::Const(class, bits) => {
            let (operator, ty) = match class {
                ScalarClass::Int => (Operator::I64Const { value: bits as u64 }, Type::I64),
                ScalarClass::Float => (Operator::F32Const { value: bits as u32 }, Type::F32),
            };
            let value = walk.add(operator, &[], &[ty]);
            walk.values.insert(instr, (value, ty));
        }
        KernelInstr::Bin(class, operator) => {
            let native = binary_operator(class, operator)?;
            let (lhs, _) = operands[0];
            let (rhs, _) = operands[1];
            if !is_comparison(operator) {
                let result = super::assemble::value_type(class);
                let value = walk.add(native, &[lhs, rhs], &[result]);
                walk.values.insert(instr, (value, result));
                return Ok(());
            }
            // **A comparison is two operators.** The language's comparison yields an
            // `i64` `0`/`1` scalar; wasm's yields an `i32`. The widening is a named
            // step rather than a comment beside each comparison
            // (`docs/notes/wasm-control-flow.md` §2), and the converse narrowing is
            // [`KernelInstr::I32WrapI64`].
            let narrow = walk.add(native, &[lhs, rhs], &[Type::I32]);
            let value = walk.add(Operator::I64ExtendI32U, &[narrow], &[Type::I64]);
            walk.values.insert(instr, (value, Type::I64));
        }
        KernelInstr::I32WrapI64 => {
            let (operand, _) = operands[0];
            let value = walk.add(Operator::I32WrapI64, &[operand], &[Type::I32]);
            walk.values.insert(instr, (value, Type::I32));
        }
        KernelInstr::Select => {
            // **The arms are the value, so either one's type is the result's**: they
            // are one class by construction, and `refuse_mixed_classes` refuses
            // otherwise.
            let (then, then_ty) = operands[0];
            let (otherwise, otherwise_ty) = operands[1];
            let (selector, _) = operands[2];
            if then_ty != otherwise_ty {
                return Err(format!(
                    "compute.wasm: a select's arms are {then_ty:?} and {otherwise_ty:?}; wasm's \
                     `select` is typed by both"
                ));
            }
            let value = walk.add(Operator::Select, &[then, otherwise, selector], &[then_ty]);
            walk.values.insert(instr, (value, then_ty));
        }
        // The crossing. `from` and `to` are what the **language** asked for; what
        // wasm holds is the value's actual representation, and the two are different
        // questions.
        KernelInstr::Conv { from, to } => {
            let (operand, seen) = operands[0];
            match (seen, to) {
                // Already the target's representation: a reclassification, and wasm
                // is told nothing.
                (seen, to) if seen == super::assemble::value_type(to) => {
                    walk.values.insert(instr, (operand, seen));
                }
                (Type::I64, ScalarClass::Float) => {
                    let value = walk.add(Operator::F32ConvertI64U, &[operand], &[Type::F32]);
                    walk.values.insert(instr, (value, Type::F32));
                }
                (Type::F32, ScalarClass::Int) => {
                    let value = walk.add(Operator::I64TruncF32U, &[operand], &[Type::I64]);
                    walk.values.insert(instr, (value, Type::I64));
                }
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
            let arity = callee_arity(walk.context, callee)?;
            let at = *walk.context.index.get(&callee).ok_or_else(|| {
                format!("cross-kernel call to kernel {callee} is not in the assembled set")
            })?;
            let target = walk.context.callees.get(at as usize).ok_or_else(|| {
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
            if results.len() > 1 {
                // The frontend refuses this before a fragment is built, so reaching
                // it means the fragment was assembled by something other than `jit`.
                return Err(format!(
                    "compute.wasm: kernel {callee} returns {} values, and a cross-kernel call here \
                     leaves one value — a multi-value call is refused by `jit` before it reaches a \
                     backend, so this fragment was not built by it",
                    results.len()
                ));
            }
            let Some(result) = results.first().copied() else {
                return Err(format!(
                    "compute.wasm: kernel {callee} returns no value, so a call to it cannot leave one \
                     for the caller's body to compute with"
                ));
            };
            if operands.len() != arity {
                return Err(format!(
                    "compute.wasm: a call to kernel {callee} takes {arity} argument(s) and this \
                     body gives it {}",
                    operands.len()
                ));
            }
            let call_args: Vec<Value> = operands.iter().map(|(value, _)| *value).collect();
            let value = walk.add(
                Operator::Call {
                    function_index: Func::from(walk.context.imports.base + at),
                },
                &call_args,
                &[result],
            );
            walk.values.insert(instr, (value, result));
        }
        KernelInstr::BufferReadCall(class) => {
            let (position, _) = operands[0];
            let (index, _) = operands[1];
            let function_index = *walk.context.imports.read.get(&class).ok_or_else(|| {
                format!("compute.wasm: no `read` import was declared for {class:?} elements")
            })?;
            let result = super::assemble::value_type(class);
            let value = walk.add(
                Operator::Call { function_index },
                &[position, index],
                &[result],
            );
            walk.values.insert(instr, (value, result));
        }
        KernelInstr::BufferWriteCall(class) => {
            let (position, _) = operands[0];
            let (index, _) = operands[1];
            let (value, _) = operands[2];
            let function_index = *walk.context.imports.write.get(&class).ok_or_else(|| {
                format!("compute.wasm: no `write` import was declared for {class:?} elements")
            })?;
            let block = walk.blocks[walk.current].0;
            walk.builder.add_op(
                block,
                Operator::Call { function_index },
                &[position, index, value],
                &[],
            );
        }
    }
    Ok(())
}

/// Lower one block's terminator.
fn lower_terminator(walk: &mut Walk<'_, '_>, index: usize) -> Result<(), String> {
    let block = walk.blocks[index].0;
    let terminator = match &walk.body.blocks[index].terminator {
        Terminator::Return { values } => {
            let mut held = Vec::with_capacity(values.len());
            for &value in values {
                held.push(walk.operand(value)?.0);
            }
            waffle::Terminator::Return { values: held }
        }
        Terminator::Br(br) => waffle::Terminator::Br {
            target: block_target(walk, br)?,
        },
        Terminator::CondBr {
            cond,
            if_true,
            if_false,
        } => {
            let (condition, ty) = walk.operand(*cond)?;
            // **A branch's condition is wasm's `i32`.** The language's is an `i64`
            // `0`/`1` scalar, so the narrowing is a named step here rather than a
            // re-derivation every consumer would otherwise make.
            let condition = match ty {
                Type::I32 => condition,
                Type::I64 => walk.add(Operator::I32WrapI64, &[condition], &[Type::I32]),
                ty => {
                    return Err(format!(
                        "compute.wasm: a branch condition is a {ty:?} here, and neither an `i32` \
                         nor an `i64` `0`/`1` scalar can be taken from it"
                    ));
                }
            };
            waffle::Terminator::CondBr {
                cond: condition,
                if_true: block_target(walk, if_true)?,
                if_false: block_target(walk, if_false)?,
            }
        }
    };
    walk.builder.set_terminator(block, terminator);
    Ok(())
}

/// A branch's wasm target: the block, and the values it hands over.
fn block_target(walk: &Walk<'_, '_>, br: &Br) -> Result<BlockTarget, String> {
    let Some((block, _)) = walk.blocks.get(br.target) else {
        return Err(format!(
            "compute.wasm: a branch names block {}, which this body does not have",
            br.target
        ));
    };
    let mut args = Vec::with_capacity(br.args.len());
    for &arg in &br.args {
        args.push(walk.operand(arg)?.0);
    }
    Ok(BlockTarget {
        block: *block,
        args,
    })
}

/// How many values a cross-kernel call consumes — the callee's own domain.
fn callee_arity(context: &ModuleCtx<'_>, callee: KernelId) -> Result<usize, String> {
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
/// **`Rem` and the bitwise trio have no float form**, and the refusal is here
/// rather than at the site: `refuse_mixed_classes` already rejects a float operand
/// for them before any module exists, so a float reaching this is a walk that
/// disagreed with the emitter.
fn binary_operator(class: ScalarClass, operator: KernelBin) -> Result<Operator, String> {
    use KernelBin as Bin;
    use ScalarClass::{Float, Int};
    Ok(match (class, operator) {
        (Int, Bin::Add) => Operator::I64Add,
        (Int, Bin::Sub) => Operator::I64Sub,
        (Int, Bin::Mul) => Operator::I64Mul,
        // An `Int` is unsigned, so these are the unsigned division and remainder
        // (`DivS` would agree below 2^63 and differ above). A **float division is
        // IEEE and unguarded** (`docs/notes/floating-point.md` §4.4).
        (Int, Bin::Div) => Operator::I64DivU,
        (Int, Bin::Rem) => Operator::I64RemU,
        (Int, Bin::BitAnd) => Operator::I64And,
        (Int, Bin::BitOr) => Operator::I64Or,
        (Int, Bin::BitXor) => Operator::I64Xor,
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
