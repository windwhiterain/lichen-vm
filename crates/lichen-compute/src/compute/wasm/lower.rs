//! The wasm lowering: one fragment's SSA body onto `waffle` values.
//!
//! # Invariant
//! The body is SSA, so the walk is a map from value to what waffle made of it: no operand
//! stack, and a shared subexpression emitted once. Three passes — create blocks and blockparams,
//! type the non-entry blockparams to a fixed point, then emit instructions and terminators. Every
//! value also carries a wasm `Type`, because [`KernelInstr::Conv`] names the class the language
//! asked for while the value was built by whatever produced it.

use std::collections::HashMap;

use lichen_kernel_ir::{
    Br, KernelBin, KernelBody, KernelFragment, KernelId, KernelInstr, ScalarClass, Terminator,
    ValueDef, ValueId,
};
use waffle::{Block, BlockTarget, Func, FunctionBody, Operator, Type, Value};

use super::assemble::BufferImports;

/// What one fragment's lowering needs from the rest of the assembly.
///
/// # Invariant
/// Read-only and shared, because these are facts of the launch set rather than of one body: a
/// cross-kernel call is typed by its callee's own `result_classes`, a parameter leaf's class by
/// the ABI's flattened `param_shape`.
pub(super) struct ModuleCtx<'a> {
    /// The launch set, so a cross-kernel call can be typed by its callee's own
    /// result list rather than left unnamed.
    pub(super) callees: &'a [KernelFragment],
    /// Each callee [`KernelId`]'s position in `callees`.
    pub(super) index: &'a HashMap<KernelId, u32>,
    pub(super) imports: &'a BufferImports,
    /// The parameter leaves' classes, in flattening order.
    pub(super) leaves: &'a [ScalarClass],
    /// The entry block's blockparams, which are the function's parameters.
    ///
    /// # Invariant
    /// A blockparam and not a local: `FunctionBody::new` builds these from the signature, and
    /// nothing may add to them, because the entry block has no predecessors.
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
    /// The map that replaces the operand stack: every value the body defines.
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
    // `validate` is the gate, and it runs before anything is read.
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

    // Pass 1: bind the blocks. waffle's entry block already exists, so it is used rather
    // than added.
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

/// Decide the wasm type of every value the body defines, then bind each non-entry block's
/// params.
///
/// # Invariant
/// A value's type is a fact about the whole body and must be known before anything is emitted:
/// waffle needs a block parameter's type when it is added, and what its predecessors hand it
/// may be an instruction the entry block computes — every loop's carried state is that — so a
/// block-to-block decision can never decide them. The fixed point therefore runs over values,
/// not over blocks.
fn type_values(walk: &mut Walk<'_, '_>) -> Result<(), String> {
    let body = walk.body;
    let count = body.values.len();
    // What each value's type is, once a rule has decided it.
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

    // A value's definition decides it, so the sweep reads the body's value table rather
    // than the walk order.
    loop {
        let mut changed = false;
        for (index, definition) in body.values.iter().enumerate() {
            // A decided instruction is done; a decided parameter is not, since a later
            // sweep may type an edge.
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
                // A parameter is typed by the first edge that names its argument, and
                // later edges are checked against it.

                // Waiting for every edge to be typed waits forever on a loop header,
                // whose backedge is one of its two.
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

    // Every value the body reads must have a type before emission; a write produces
    // nothing and is read by nothing.
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

/// The wasm type `op` leaves when its operands are `operands`, or `None` for an operation
/// that produces nothing.
///
/// # Invariant
/// The typing half of [`lower_instr`], and the two must agree: every arm here states the type
/// that arm there emits, from the same facts.
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

/// Lower one instruction: its operands are named and its result is recorded.
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
            // A comparison is two operators: the language's yields an `i64` `0`/`1`, wasm's
            // an `i32`.
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
            // The arms are the value, so either one's type is the result's.
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
        // The crossing: `from` and `to` are what the language asked for, while what wasm
        // holds is the value's representation.
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
                // The frontend refuses this before a fragment is built.
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
            // A branch's condition is wasm's `i32`, so the narrowing is a named step.
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

/// The one wasm operator a binary operator lowers to, before a comparison's widening.
///
/// # Invariant
/// `Rem` and the bitwise trio have no float form, and the refusal is here rather than at the
/// site: `refuse_mixed_classes` rejects a float operand for them before any module exists.
fn binary_operator(class: ScalarClass, operator: KernelBin) -> Result<Operator, String> {
    use KernelBin as Bin;
    use ScalarClass::{Float, Int};
    Ok(match (class, operator) {
        (Int, Bin::Add) => Operator::I64Add,
        (Int, Bin::Sub) => Operator::I64Sub,
        (Int, Bin::Mul) => Operator::I64Mul,
        // An `Int` is unsigned, so these are the unsigned division and remainder; a float
        // division is IEEE and unguarded.
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
