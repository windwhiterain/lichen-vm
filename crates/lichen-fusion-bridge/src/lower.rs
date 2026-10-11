//! The walk that reads a marked body value as one symbolic expression.

use std::collections::HashMap;

use lichen_fusion::{Expr, Variable};
use lichen_kernel_ir::{KernelBody, KernelFragment, KernelInstr, ScalarClass, ValueDef, ValueId};

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
        reads,
        read: HashMap::new(),
        state_slots: Vec::new(),
        nodes: 0,
    };
    walk.value(marked)
}

/// One walk of one body: the values already read, and the node count it is held to.
struct Walk<'a> {
    body: &'a KernelBody,
    entry: usize,
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
    /// The entry block's parameters are the kernel's own domain: a **scalar** fragment's
    /// leaves are its slots, which the roles name, and a **parallel** fragment's extent and
    /// lane index are no projection's business. Every other block's parameters are a loop's
    /// carried state, one slot, so reading two of them is refused.
    fn parameter(&mut self, block: usize, index: u32) -> Result<Expr, BridgeRefusal> {
        if block == self.entry {
            let Some((datum, producer)) = self.reads.parameter_leaves else {
                return Err(BridgeRefusal::ReadsTheDomain { index });
            };
            let leaf = index as usize;
            if leaf == datum {
                return Ok(Expr::Variable(Variable::DATUM));
            }
            if leaf == producer {
                return Ok(Expr::Variable(Variable::PRODUCER));
            }
            return Err(BridgeRefusal::ReadsTheDomain { index });
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
    /// names its own refusal. No arm builds `Expr::Unary`, which is why a transcendental is
    /// [`BridgeRefusal::Transcendental`] rather than an expression.
    fn instruction(
        &mut self,
        id: ValueId,
        op: KernelInstr,
        args: &[ValueId],
    ) -> Result<Expr, BridgeRefusal> {
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
            KernelInstr::Exp | KernelInstr::Sqrt | KernelInstr::Floor => {
                Err(BridgeRefusal::Transcendental)
            }
            KernelInstr::Conv { from, to } => Err(BridgeRefusal::Conversion { from, to }),
            KernelInstr::Select => Err(BridgeRefusal::Select),
            KernelInstr::CallKernel(kernel) => Err(BridgeRefusal::Call { kernel }),
            KernelInstr::BufferReadCall(ScalarClass::Float) => {
                let position = self.read_position(args)?;
                if self.reads.parameter_leaves.is_some() {
                    return Err(BridgeRefusal::ReadsABuffer);
                }
                Ok(Expr::Variable(match self.reads.producer_input {
                    Some(producer) if producer == position => Variable::PRODUCER,
                    _ => Variable::DATUM,
                }))
            }
            KernelInstr::BufferReadCall(ScalarClass::Int) => {
                Err(BridgeRefusal::MachineInteger { value: id })
            }
            KernelInstr::BufferWriteCall(_) => Err(BridgeRefusal::BufferWrite {
                position: self.read_position(args)?,
            }),
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

    /// A buffer instruction's element position: its first operand is the compile-time
    /// ordinal the lowering emitted.
    ///
    /// # Invariant
    /// The position is the read's own slot, so a body whose position is not a constant
    /// cannot be read as one slot — it is refused rather than defaulted.
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

/// The value a float instruction's bits denote.
///
/// # Invariant
/// A float instruction carries its value as the `f32` bit pattern in the IR's one word, which
/// is exact in `f64`: every `f32` is a real number `f64` holds.
fn float_of_bits(bits: i64) -> f64 {
    f32::from_bits(bits as u32) as f64
}
