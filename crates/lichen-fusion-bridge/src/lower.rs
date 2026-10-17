//! The walk that reads a marked body value as one symbolic expression.
//!
//! # Invariant
//! **A transcendental is read as the operator whose definition the body holds**: a registered
//! fragment carries [`KernelBody::expand_transcendentals`]'s expansion, so a `sqrt` arrives as
//! a Newton iteration of `Select`s. Each is read through its definition's own reader, which
//! answers only a subtree that *is* that definition; any other is refused as it really stands.

use std::collections::HashMap;

use lichen_fusion::{Expr, UnaryOp, Variable};
use lichen_kernel_ir::{
    KernelBin, KernelBody, KernelFragment, KernelInstr, ScalarClass, ValueDef, ValueId,
    inlined_cos_operand, inlined_exp_operand, inlined_ln_operand, inlined_sin_operand,
    inlined_sqrt_operand,
};

use crate::operators::binary;
use crate::refusal::BridgeRefusal;
use crate::roles::ReadRoles;

/// How many expression nodes the walk will build before it refuses.
///
/// # Invariant
/// A lowered body is SSA, and a value the body defines once is read as often as it occurs —
/// so the *expression* is a tree where the body is a DAG, and a chain of shared values would
/// double per level. The budget keeps the walk total: past it the projection is refused by
/// name rather than expanded into a tree no report could hold.
const EXPRESSION_NODE_BUDGET: usize = 4096;

/// The largest integer a machine-integer immediate may state and still read as one real number.
///
/// # Invariant
/// The symbolic language's numbers are `f64`, whose significand is 53 bits: every integer up to
/// `2^53` is one it holds exactly and beyond that it rounds, so an immediate above this number
/// is refused as the machine integer it is rather than answered with a neighbouring number.
const EXACT_INTEGER_LIMIT: u64 = 1 << 53;

/// The symbolic expression a marked body value denotes.
///
/// # Invariant
/// The walk follows the marked value's own operands and stops at the three things a
/// projection is written over: a read (a datum, or the producer's value), a loop's carried
/// state, and a constant. Every other instruction is refused by name, and so is a value the
/// roles do not name.
pub fn projection(
    fragment: &KernelFragment,
    marked: ValueId,
    reads: ReadRoles,
) -> Result<Expr, BridgeRefusal> {
    fragment
        .body
        .validate()
        .map_err(|reason| BridgeRefusal::MalformedBody { reason })?;
    let mut walk = Walk {
        body: &fragment.body,
        entry: fragment.body.entry,
        invocation: invocation_of(fragment),
        reads,
        read: HashMap::new(),
        state_slots: Vec::new(),
        nodes: 0,
    };
    walk.value(marked)
}

/// The leaves of a parallel domain that spell `the invocation`, and the ones between them.
///
/// # Invariant
/// **The domain's own order: `extent`, the runtime scalars, `index`.** Leaf `0` is the extent the
/// ABI's first scalar states and the last the index [`KernelFragment::scalar_leaves`] names. **A
/// runtime scalar is one value for every lane of the launch**, so a projection is one function of
/// it in every lane and reads it at a slot of its own, named by its ordinal.
#[derive(Debug, Clone, Copy)]
struct Invocation {
    /// The leaf the launch extent arrives as.
    extent: u32,
    /// The leaf the invocation index arrives as.
    index: u32,
}

/// One walk of one body: the values already read, and the node count it is held to.
struct Walk<'a> {
    body: &'a KernelBody,
    entry: usize,
    /// The invocation's own two leaves, for a fragment that has one.
    invocation: Option<Invocation>,
    reads: ReadRoles,
    /// A value read twice is one expression value, cloned: the body shares it and the
    /// symbolic language has no sharing.
    read: HashMap<ValueId, Expr>,
    /// The carried-state slots the projection read, in the order it met them.
    state_slots: Vec<u32>,
    nodes: usize,
}

impl Walk<'_> {
    /// The expression the value `id` denotes.
    fn value(&mut self, id: ValueId) -> Result<Expr, BridgeRefusal> {
        if let Some(expr) = self.read.get(&id) {
            return Ok(expr.clone());
        }
        let expr = match self.body.values.get(id.0 as usize) {
            Some(ValueDef::BlockParam { block, index }) => self.parameter(*block, *index)?,
            Some(ValueDef::Instr { op, args, .. }) => self.instruction(id, *op, args)?,
            None => return Err(BridgeRefusal::NotAValue { marked: id }),
        };
        self.nodes += 1;
        if self.nodes > EXPRESSION_NODE_BUDGET {
            return Err(BridgeRefusal::ExpressionTooLarge {
                budget: EXPRESSION_NODE_BUDGET,
            });
        }
        self.read.insert(id, expr.clone());
        Ok(expr)
    }

    /// The symbolic slot a value a block receives stands for.
    ///
    /// # Invariant
    /// The entry block's parameters are the kernel's own domain: a **scalar** fragment's leaves are
    /// its slots, which the roles name, and a **parallel** fragment's are the launch's own — the
    /// extent, a slot per runtime scalar, then the invocation index, and a leaf past the table is
    /// refused. Every other block's parameters are a loop's carried state, one slot, so reading two
    /// is refused.
    fn parameter(&mut self, block: usize, index: u32) -> Result<Expr, BridgeRefusal> {
        if block == self.entry {
            let Some((datum, producer)) = self.reads.parameter_leaves else {
                return match self.invocation {
                    Some(invocation) if index == invocation.extent => {
                        Ok(Expr::Variable(Variable::EXTENT))
                    }
                    Some(invocation) if index > invocation.extent && index < invocation.index => {
                        runtime_scalar(index)
                    }
                    Some(invocation) if index == invocation.index => {
                        Ok(Expr::Variable(Variable::INVOCATION))
                    }
                    _ => Err(BridgeRefusal::ReadsARuntimeScalar { index }),
                };
            };
            let leaf = index as usize;
            if leaf == datum {
                return Ok(Expr::Variable(Variable::DATUM));
            }
            if leaf == producer {
                return Ok(Expr::Variable(Variable::PRODUCER));
            }
            return Err(BridgeRefusal::ReadsAnUnnamedLeaf { index });
        }
        if !self.state_slots.contains(&index) {
            self.state_slots.push(index);
        }
        if self.state_slots.len() > 1 {
            let mut slots = self.state_slots.clone();
            slots.sort_unstable();
            return Err(BridgeRefusal::AmbiguousState { slots });
        }
        Ok(Expr::Variable(Variable::CONSUMER_STATE))
    }

    /// The expression one instruction computes.
    ///
    /// # Invariant
    /// One arm per instruction, so an instruction a later compiler adds cannot be read as
    /// something else: the match is exhaustive, and every arm outside the arithmetic subset
    /// names its own refusal. A transcendental is read through the definition's own reader
    /// first, because the fragment a dispatcher registers holds the expansion rather than
    /// [`KernelInstr::Exp`] or [`KernelInstr::Sqrt`].
    fn instruction(
        &mut self,
        id: ValueId,
        op: KernelInstr,
        args: &[ValueId],
    ) -> Result<Expr, BridgeRefusal> {
        if let Some((operator, operand)) = inlined_transcendental(self.body, id) {
            return Ok(operator.apply(self.value(operand)?));
        }
        match op {
            KernelInstr::Const(ScalarClass::Float, bits) => Ok(Expr::Constant(float_of_bits(bits))),
            KernelInstr::Const(ScalarClass::Int, _) => {
                Err(BridgeRefusal::MachineInteger { value: id })
            }
            KernelInstr::Bin(ScalarClass::Float, operator) => {
                let Some(symbolic) = binary(operator) else {
                    return Err(BridgeRefusal::UnsupportedOperator { operator });
                };
                let left = self.argument(args, 0)?;
                let right = self.argument(args, 1)?;
                Ok(symbolic.apply(left, right))
            }
            KernelInstr::Bin(ScalarClass::Int, _) => {
                Err(BridgeRefusal::MachineInteger { value: id })
            }
            KernelInstr::I32WrapI64 => Err(BridgeRefusal::BooleanNarrowing),
            KernelInstr::Fma => Err(BridgeRefusal::FusedMultiplyAdd),
            KernelInstr::Negate => {
                // **A subtraction, not a refusal**: over the reals `-x` equals `0 - x` exactly.
                let Some(subtract) = binary(KernelBin::Sub) else {
                    return Err(BridgeRefusal::UnsupportedOperator {
                        operator: KernelBin::Sub,
                    });
                };
                let operand = self.argument(args, 0)?;
                Ok(subtract.apply(Expr::Constant(0.0), operand))
            }
            KernelInstr::Exp => {
                let operand = self.argument(args, 0)?;
                Ok(UnaryOp::Exp.apply(operand))
            }
            KernelInstr::Sqrt => {
                let operand = self.argument(args, 0)?;
                Ok(UnaryOp::Sqrt.apply(operand))
            }
            KernelInstr::Floor => Err(BridgeRefusal::Floor),
            KernelInstr::Ln => {
                let operand = self.argument(args, 0)?;
                Ok(UnaryOp::Ln.apply(operand))
            }
            KernelInstr::Sin => {
                let operand = self.argument(args, 0)?;
                Ok(UnaryOp::Sin.apply(operand))
            }
            KernelInstr::Cos => {
                let operand = self.argument(args, 0)?;
                Ok(UnaryOp::Cos.apply(operand))
            }
            KernelInstr::Conv { from, to } => self.conversion(args, from, to),
            KernelInstr::Select => Err(BridgeRefusal::Select),
            KernelInstr::CallKernel(kernel) => Err(BridgeRefusal::Call { kernel }),
            KernelInstr::BufferReadCall(ScalarClass::Float) => {
                let position = self.read_position(args)?;
                if self.reads.parameter_leaves.is_some() {
                    return Err(BridgeRefusal::ReadsABuffer);
                }
                // Each producer has its own slot, so two are two values and never
                // one value and a datum.
                Ok(Expr::Variable(
                    self.reads.producer_at(position).unwrap_or(Variable::DATUM),
                ))
            }
            KernelInstr::BufferReadCall(ScalarClass::Int) => {
                Err(BridgeRefusal::MachineInteger { value: id })
            }
            KernelInstr::BufferWriteCall(_) => Err(BridgeRefusal::BufferWrite {
                position: self.read_position(args)?,
            }),
            KernelInstr::SliceReadCall(_) | KernelInstr::SliceWriteCall(_) => {
                Err(BridgeRefusal::SliceAccess {
                    slice: self.read_position(args)?,
                })
            }
        }
    }

    /// The `position`-th operand of an instruction, read as an expression.
    fn argument(&mut self, args: &[ValueId], position: usize) -> Result<Expr, BridgeRefusal> {
        match args.get(position) {
            Some(arg) => self.value(*arg),
            None => Err(BridgeRefusal::MalformedBody {
                reason: format!("an instruction reads {} operand(s)", args.len()),
            }),
        }
    }

    /// The expression one class conversion computes.
    ///
    /// # Invariant
    /// **Only `Int → Float`, of a real the walk can name**: the integer *is* the real it
    /// denotes. An immediate is read as the number it names, every other source goes to the
    /// walk, and a **scalar fragment's** leaf is refused, its class being stated beside the
    /// parameter. `Float → Int` truncates, and a truncation inverts on nothing.
    fn conversion(
        &mut self,
        args: &[ValueId],
        from: ScalarClass,
        to: ScalarClass,
    ) -> Result<Expr, BridgeRefusal> {
        let [operand] = args[..] else {
            return Err(BridgeRefusal::Conversion { from, to });
        };
        if from != ScalarClass::Int || to != ScalarClass::Float {
            return Err(BridgeRefusal::Conversion { from, to });
        }
        if let Some(ValueDef::Instr {
            op: KernelInstr::Const(ScalarClass::Int, bits),
            ..
        }) = self.body.values.get(operand.0 as usize)
        {
            let number = *bits as u64;
            return if number > EXACT_INTEGER_LIMIT {
                Err(BridgeRefusal::MachineInteger { value: operand })
            } else {
                Ok(Expr::Constant(number as f64))
            };
        }
        if self.reads.parameter_leaves.is_some() && self.body.is_a_parameter(operand) {
            return Err(BridgeRefusal::Conversion { from, to });
        }
        self.value(operand)
    }

    /// The compile-time ordinal a memory instruction's first operand names: a buffer position or
    /// a slice ordinal.
    ///
    /// # Invariant
    /// The ordinal is the instruction's own, so a body whose first operand is not a
    /// constant cannot be read as one ordinal — it is refused rather than defaulted.
    fn read_position(&self, args: &[ValueId]) -> Result<usize, BridgeRefusal> {
        match args
            .first()
            .map(|first| self.body.values.get(first.0 as usize))
        {
            Some(Some(ValueDef::Instr {
                op: KernelInstr::Const(_, bits),
                ..
            })) => Ok(*bits as usize),
            _ => Err(BridgeRefusal::UnknownReadPosition),
        }
    }
}

/// The invocation a fragment's domain spells, when the fragment is dispatched over one.
///
/// # Invariant
/// **The extent, the runtime scalars between, and the index**: [`KernelFragment::scalar_leaves`]
/// is the domain's leaves but the index, so a fragment that has one has the extent at `0` and the
/// index at that count. A **scalar** fragment has none — its shape is its own domain and
/// `scalar_leaves` is `0` — so its leaves stay the roles' business.
fn invocation_of(fragment: &KernelFragment) -> Option<Invocation> {
    let leaves = fragment.scalar_leaves();
    (leaves > 0).then(|| Invocation {
        extent: 0,
        index: leaves as u32,
    })
}

/// The slot one runtime scalar leaf is read as, or the refusal past the table.
///
/// # Invariant
/// The leaf's own ordinal is its slot: leaf `1` is [`Variable::runtime_scalar`]'s `0`, beside the
/// extent, so the spelling moves with the ABI rather than with a count this crate kept.
fn runtime_scalar(index: u32) -> Result<Expr, BridgeRefusal> {
    Variable::runtime_scalar(index.saturating_sub(1) as usize)
        .map(Expr::Variable)
        .ok_or(BridgeRefusal::ReadsARuntimeScalar { index })
}

/// The value a float instruction's bits denote.
///
/// # Invariant
/// A float instruction carries its value as the `f32` bit pattern in the IR's one word, which
/// is exact in `f64`: every `f32` is a real number `f64` holds.
fn float_of_bits(bits: i64) -> f64 {
    f32::from_bits(bits as u32) as f64
}

/// The operator a body value holds as an inlined transcendental definition, and its operand.
///
/// # Invariant
/// **Each reader answers only a subtree that is its own definition**, so a value one of them
/// holds is read as the operator that definition computes and any other falls through to the
/// walk's own refusal. **The five are disjoint** — a match is the definition's whole
/// instruction set — and `floor`, which has no reader here, is refused by the `Select` its
/// expansion holds.
fn inlined_transcendental(body: &KernelBody, value: ValueId) -> Option<(UnaryOp, ValueId)> {
    if let Some(operand) = inlined_exp_operand(body, value) {
        return Some((UnaryOp::Exp, operand));
    }
    if let Some(operand) = inlined_sqrt_operand(body, value) {
        return Some((UnaryOp::Sqrt, operand));
    }
    if let Some(operand) = inlined_sin_operand(body, value) {
        return Some((UnaryOp::Sin, operand));
    }
    if let Some(operand) = inlined_ln_operand(body, value) {
        return Some((UnaryOp::Ln, operand));
    }
    let operand = inlined_cos_operand(body, value)?;
    Some((UnaryOp::Cos, operand))
}
