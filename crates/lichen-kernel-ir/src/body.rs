//! A fragment's body: SSA over named values, with blocks and terminators.
//!
//! # Invariant
//! A block's `params` are the values it receives — one rule for a function's
//! parameters and for a loop's carried state — and a `Br`'s `args` are exactly its
//! target's `params`. What this replaced is `docs/notes/lichen-compute.md` §4.

use crate::{KernelInstr, ScalarClass};

/// A named value in a [`KernelBody`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ValueId(pub u32);

/// What a value is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueDef {
    /// A value a block **receives**: a function's parameter or a loop's carried state.
    ///
    /// # Invariant
    /// A carried value lives in the same place a parameter does, which is the point:
    /// a phi is not a second mechanism, it is a block's parameters.
    BlockParam { block: usize, index: u32 },
    /// A computation: the operator, the values it reads, and what it leaves behind.
    ///
    /// # Invariant
    /// `classes` is per result and may be empty: a `BufferWriteCall` leaves nothing, a
    /// call to another kernel leaves its callee's result class, and a comparison
    /// leaves one scalar. The list is what the definition produces and a consumer
    /// reads; it is never inferred backwards.
    Instr {
        op: KernelInstr,
        args: Vec<ValueId>,
        classes: Vec<ScalarClass>,
    },
}

/// One basic block: the values it receives, the values it computes, where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    /// The values this block **receives**.
    pub params: Vec<ValueId>,
    /// The values this block computes, **in an order that respects operands**.
    pub instrs: Vec<ValueId>,
    pub terminator: Terminator,
}

/// Where a block goes when its instructions have run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminator {
    /// Leave the function, carrying these values.
    ///
    /// # Invariant
    /// A list, because a codomain is: a kernel returns one leaf or a tuple of them,
    /// and a tuple codomain's values are one `Return` — wasm's multi-value result and
    /// SPIR-V's `OpReturn` are each one instruction.
    Return { values: Vec<ValueId> },
    /// Arrive at `target`, handing it `args` as its parameters.
    Br(Br),
    /// Two-way branch on `cond`, an `i64` `0`/`1` scalar; a consumer needing narrower
    /// narrows it.
    CondBr {
        cond: ValueId,
        if_true: Br,
        if_false: Br,
    },
}

/// A branch and the values it hands over.
///
/// # Invariant
/// `args` is the whole of a block's incoming state, which is what makes a backedge's
/// arguments the next iteration's carried tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Br {
    pub target: usize,
    pub args: Vec<ValueId>,
}

/// A fragment's body: SSA values, blocks, and where control starts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KernelBody {
    pub values: Vec<ValueDef>,
    pub blocks: Vec<BasicBlock>,
    /// Which block control starts in.
    pub entry: usize,
}

impl KernelBody {
    pub fn new() -> Self {
        KernelBody::default()
    }

    /// A fresh block, empty and unterminated.
    pub fn add_block(&mut self) -> usize {
        self.blocks.push(BasicBlock {
            params: Vec::new(),
            instrs: Vec::new(),
            terminator: Terminator::Return { values: Vec::new() },
        });
        self.blocks.len() - 1
    }

    /// Add a value `block` receives, and bind it as that block's `index`-th
    /// parameter.
    pub fn add_param(&mut self, block: usize) -> ValueId {
        let value = ValueId(self.values.len() as u32);
        let index = self.blocks[block].params.len() as u32;
        self.values.push(ValueDef::BlockParam { block, index });
        self.blocks[block].params.push(value);
        value
    }

    /// Define `op`'s result in `block`, reading `args`.
    pub fn add_op(
        &mut self,
        block: usize,
        op: KernelInstr,
        args: Vec<ValueId>,
        classes: Vec<ScalarClass>,
    ) -> ValueId {
        let value = ValueId(self.values.len() as u32);
        self.values.push(ValueDef::Instr { op, args, classes });
        self.blocks[block].instrs.push(value);
        value
    }

    /// Define a constant in `block`, of the class the opcode reads its bits in.
    pub fn add_const(&mut self, block: usize, class: ScalarClass, bits: i64) -> ValueId {
        self.add_op(
            block,
            KernelInstr::Const(class, bits),
            Vec::new(),
            vec![class],
        )
    }

    pub fn set_terminator(&mut self, block: usize, terminator: Terminator) {
        self.blocks[block].terminator = terminator;
    }
}

/// One step of a **hand-written** body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatOp {
    /// An operation, reading the values above it — as many as its own
    /// [`KernelInstr::arity`] says, most recent first.
    Instr(KernelInstr),
    /// Read the entry block's `index`-th parameter, and push it.
    ///
    /// # Invariant
    /// What `KernelInstr::LocalGet(k)` was: a step of *writing* a body rather than an
    /// instruction *in* one. A body whose operands are named needs the name, not an
    /// instruction to fetch it, and here the name is the step.
    Read(usize),
}

/// A one-block body written by hand, from a list of steps.
///
/// # Invariant
/// The steps consume and produce a running list of values — a stack discipline that
/// is deliberate and *local*, for something with no graph to walk. `domain` is the
/// number of parameter leaves the entry block receives; an operation's declared
/// class comes from the instruction itself where it states one
/// ([`KernelInstr::own_class`]) and is the ABI's integer default otherwise.
pub fn from_flat(domain: usize, ops: &[FlatOp]) -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    for _ in 0..domain {
        body.add_param(entry);
    }
    let parameters = body.blocks[entry].params.clone();
    // **The stack starts empty**: seeding it with the parameters would return them as
    // results.
    let mut values: Vec<ValueId> = Vec::new();
    for op in ops {
        match *op {
            FlatOp::Read(index) => {
                let Some(value) = parameters.get(index).copied() else {
                    continue;
                };
                values.push(value);
            }
            FlatOp::Instr(instr) => {
                let take = instr.arity().unwrap_or(0).min(values.len());
                let args = values[values.len() - take..].to_vec();
                values.truncate(values.len() - take);
                let classes = if instr.produces() == 0 {
                    Vec::new()
                } else {
                    vec![instr.own_class().unwrap_or(ScalarClass::Int)]
                };
                let value = body.add_op(entry, instr, args, classes);
                if instr.produces() > 0 {
                    values.push(value);
                }
            }
        }
    }
    // **The return hands out the top of the stack and nothing else**, and a body that
    // produces nothing returns nothing.
    body.set_terminator(
        entry,
        Terminator::Return {
            values: values.last().copied().into_iter().collect(),
        },
    );
    body
}

impl KernelBody {
    /// A one-block body written by hand — see [`from_flat`], which this only
    /// forwards to.
    pub fn from_flat(domain: usize, ops: &[FlatOp]) -> KernelBody {
        from_flat(domain, ops)
    }

    /// Whether this body is one block that returns: the fast path for a fragment with
    /// no structure.
    pub fn is_straight_line(&self) -> bool {
        self.blocks.len() == 1 && matches!(self.blocks[0].terminator, Terminator::Return { .. })
    }

    /// The entry block's parameters: the function's domain leaves, flattened.
    ///
    /// # Invariant
    /// This is the ABI, and the only place the body's shape and a call's argument list
    /// have to agree.
    pub fn parameters(&self) -> &[ValueId] {
        &self.blocks[self.entry].params
    }

    /// Every instruction this body defines, in walk order.
    ///
    /// # Invariant
    /// A block parameter is not an instruction, so a consumer that needs the body's
    /// inputs reads [`Self::parameters`].
    pub fn instrs(&self) -> Vec<&KernelInstr> {
        self.blocks
            .iter()
            .flat_map(|block| block.instrs.iter())
            .filter_map(|&value| match &self.values[value.0 as usize] {
                ValueDef::Instr { op, .. } => Some(op),
                ValueDef::BlockParam { .. } => None,
            })
            .collect()
    }

    /// Every value this body **reads**: each instruction's operands, and every
    /// transfer's.
    ///
    /// # Invariant
    /// A body's inputs are among them, because an operand may be a block parameter — so
    /// this is the whole of what a caller must supply.
    pub fn operands(&self) -> Vec<ValueId> {
        let mut operands: Vec<ValueId> = self
            .blocks
            .iter()
            .flat_map(|block| block.instrs.iter())
            .filter_map(|&value| match &self.values[value.0 as usize] {
                ValueDef::Instr { args, .. } => Some(args.iter().copied()),
                ValueDef::BlockParam { .. } => None,
            })
            .flatten()
            .collect();
        for block in &self.blocks {
            match &block.terminator {
                Terminator::Return { values } => operands.extend(values.iter().copied()),
                Terminator::Br(br) => operands.extend(br.args.iter().copied()),
                Terminator::CondBr { cond, .. } => operands.push(*cond),
            }
            for br in block.branches() {
                operands.extend(br.args.iter().copied());
            }
        }
        operands
    }

    /// Whether `value` is one of the entry block's parameters — the domain leaves.
    pub fn is_a_parameter(&self, value: ValueId) -> bool {
        matches!(
            self.values.get(value.0 as usize),
            Some(ValueDef::BlockParam { block, .. }) if *block == self.entry
        )
    }

    /// The gate a consumer calls **before it reads a body**: a failure is refused by
    /// name, never partially emitted.
    ///
    /// # Invariant
    /// (1) The entry block exists, and so does every block a transfer names; (2) every
    /// value a block reads is available there, checked by walk order; (3) a branch's
    /// `args` are exactly its target's `params`; (4) an instruction's `classes` matches
    /// what it produces — zero for a write, one for everything else.
    pub fn validate(&self) -> Result<(), String> {
        if self.blocks.is_empty() {
            return Err("a body must have at least its entry block".into());
        }
        if self.entry >= self.blocks.len() {
            return Err(format!(
                "entry block {} is not one of this body's {} blocks",
                self.entry,
                self.blocks.len()
            ));
        }
        for (index, block) in self.blocks.iter().enumerate() {
            for br in block.branches() {
                if br.target >= self.blocks.len() {
                    return Err(format!(
                        "block {index} branches to block {}, which does not exist",
                        br.target
                    ));
                }
                let arity = self.blocks[br.target].params.len();
                if br.args.len() != arity {
                    return Err(format!(
                        "block {index} hands {} value(s) to block {}, which takes {arity}",
                        br.args.len(),
                        br.target
                    ));
                }
                for arg in &br.args {
                    if self.values.get(arg.0 as usize).is_none() {
                        return Err(format!(
                            "block {index} hands over a value it does not define"
                        ));
                    }
                }
            }
            for &instr in &block.instrs {
                let ValueDef::Instr { op, args, classes } = &self.values[instr.0 as usize] else {
                    return Err(format!("block {index} lists a parameter as an instruction"));
                };
                if op.produces() != classes.len() {
                    return Err(format!(
                        "{op:?} produces {} value(s) but is declared with {}",
                        op.produces(),
                        classes.len()
                    ));
                }
                // **A call's arity is the callee's**, which this crate does not know: the
                // one exemption.
                if op.arity().is_some_and(|arity| args.len() != arity) {
                    return Err(format!(
                        "{op:?} reads {} value(s) but is given {}",
                        op.arity().unwrap_or_default(),
                        args.len()
                    ));
                }
                for arg in args {
                    if self.values.get(arg.0 as usize).is_none() {
                        return Err(format!("{op:?} reads a value this body does not define"));
                    }
                }
            }
        }
        // Every value a terminator hands out must be one this body defines.
        for (index, block) in self.blocks.iter().enumerate() {
            let Terminator::Return { values } = &block.terminator else {
                continue;
            };
            for value in values {
                if self.values.get(value.0 as usize).is_none() {
                    return Err(format!(
                        "block {index} returns a value this body does not define"
                    ));
                }
            }
        }
        Ok(())
    }
}

impl BasicBlock {
    /// The branches this block takes, in the order it names them.
    pub fn branches(&self) -> Vec<&Br> {
        match &self.terminator {
            Terminator::Return { .. } => Vec::new(),
            Terminator::Br(br) => vec![br],
            Terminator::CondBr {
                if_true, if_false, ..
            } => vec![if_true, if_false],
        }
    }
}
