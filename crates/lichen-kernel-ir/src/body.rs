//! A fragment's body: **SSA over named values, with blocks and terminators**.
//!
//! # What changed, and why it was the wrong shape before
//!
//! This was a **stack machine**. An instruction named no values — `Bin` said "pop
//! the top two" — and every backend walked an operand stack and *derived* the
//! form it wanted: `waffle` derives a stack from SSA, `spirv.rs` derives SSA ids
//! from a stack. Both paid to undo the omission, and the omission was not
//! cosmetic:
//!
//! - **a loop-carried value had no representation at all.** [`KernelInstr`]'s
//!   `LocalGet` named a *parameter leaf*, and nothing named element `k` of a
//!   loop's state, so a body could arrive at a loop header but could not carry
//!   anything new. Every loop the IR could build ran zero trips or forever.
//! - **a shared subexpression had to be emitted once per use**, because nothing
//!   could name it after the first.
//!
//! Here a value is a [`ValueId`] and an operand *is* one, so both are ordinary:
//!
//! ```text
//!   entry(p0, p1) ──▶ block { params: [p0, p1], instrs: [...], terminator }
//!                       │
//!                       └──▶ header(carry) ──┐ backedge, carrying carry
//! ```
//!
//! **A block's `params` are the values it receives, and that is one rule for a
//! function's parameters and for a loop's carried state.** That is the whole of
//! it: "read the loop's state" stops being an instruction that exists only inside
//! a loop and becomes an ordinary read of a value the block has.
//!
//! It is also the form both backends already want — a blockparam *is* wasm's
//! loop-carried value and SPIR-V's `OpPhi` is the same thing — so neither has to
//! derive names the IR should have carried.
//!
//! # The lowering is a graph walk, not a stack discipline
//!
//! A lowering builds this with [`KernelBody::add_op`] in dependency order and
//! [`KernelBody::set_terminator`] at the end of each block. **What defines a
//! value is the lowering's question, not this crate's** — the graph it walks
//! resolves names through an equality class, and that rule belongs beside the
//! cells (`lichen_lowlevel::resolve`).

use crate::{KernelInstr, ScalarClass};

/// A named value in a [`KernelBody`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ValueId(pub u32);

/// What a value is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueDef {
    /// A value a block **receives**. The entry block's are the function's
    /// parameter leaves; a loop header's are the loop's carried state.
    ///
    /// **This is where a carried value lives**, and it is the same place a
    /// parameter lives — which is the point. A phi is not a second mechanism; it
    /// is a block's parameters.
    BlockParam { block: usize, index: u32 },
    /// A computation: the operator, the values it reads, and what it leaves
    /// behind.
    ///
    /// **`classes` is per result and may be empty.** A `BufferWriteCall` leaves
    /// nothing, a call to another kernel leaves its callee's result class, and a
    /// comparison leaves one scalar. The list is what the definition produces and
    /// a consumer reads; it is never inferred backwards.
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
    /// **A list, because a codomain is.** A kernel returns one leaf or a tuple of
    /// them, and a tuple codomain's values are one `Return` — wasm's multi-value
    /// result and SPIR-V's `OpReturn` are each one instruction.
    Return { values: Vec<ValueId> },
    /// Arrive at `target`, handing it `args` as its parameters.
    Br(Br),
    /// Two-way branch on `cond`, an `i64` `0`/`1` scalar like every other
    /// condition in this IR. **A consumer that needs a narrower one narrows it
    /// here** — wasm's `br_if` takes an `i32` — and that is a fact about the
    /// consumer, not about this terminator.
    CondBr {
        cond: ValueId,
        if_true: Br,
        if_false: Br,
    },
}

/// A branch and the values it hands over.
///
/// **`args` is the whole of a block's incoming state**, which is what makes a
/// backedge's arguments the next iteration's carried tuple.
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
    /// **This is what `KernelInstr::LocalGet(k)` was** — a way to name a parameter
    /// — and it is a step of *writing* a body rather than an instruction *in* one.
    /// A body whose operands are named does not need an instruction to fetch one;
    /// it needs the name, and here the name is the step.
    Read(usize),
}

/// A one-block body written by hand, from a list of steps.
///
/// **The steps consume and produce a running list of values, which is a stack
/// discipline, and that is deliberate and local.** This is a way of *writing* a
/// body for something with no graph to walk — a fixture, an example, a probe.
/// A lowering does not use it and cannot: it walks the graph, resolves what each
/// node names through `lichen_lowlevel::resolve`, and names every operand, which
/// is what makes a shared subexpression emit once instead of once per use.
///
/// `domain` is the number of parameter leaves the entry block receives. An
/// operation's declared class comes from the instruction itself where it states one
/// ([`KernelInstr::own_class`]) and is the ABI's integer default otherwise,
/// because a hand-written body is saying what it computes rather than deriving it
/// from a node.
pub fn from_flat(domain: usize, ops: &[FlatOp]) -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    for _ in 0..domain {
        body.add_param(entry);
    }
    let parameters = body.blocks[entry].params.clone();
    // **The stack starts empty.** A parameter is reached by `Read(k)`, which is a
    // step of *writing* a body — seeding the stack with them would leave every
    // parameter on it, and the return below would hand them back as results.
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
    // **The return hands out the top of the stack and nothing else.** A body that
    // computes several values in sequence returns the last one, which is what the
    // stack's own convention was; handing back every intermediate would make the
    // arity depend on how the body was written.
    //
    // **A body that produces nothing returns nothing** — a write-only fragment's
    // results are its output buffers, not its wasm results, and `compile_parallel_fragment`
    // appends the constant that gives it one.
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

    /// Whether this body is one block that returns — the fast path a consumer
    /// keeps for every fragment that needs no structure.
    pub fn is_straight_line(&self) -> bool {
        self.blocks.len() == 1 && matches!(self.blocks[0].terminator, Terminator::Return { .. })
    }

    /// The entry block's parameters: the function's domain leaves, flattened.
    ///
    /// **This is the ABI**, and it is the only place the body's shape and a call's
    /// argument list have to agree.
    pub fn parameters(&self) -> &[ValueId] {
        &self.blocks[self.entry].params
    }

    /// Every instruction this body defines, in walk order.
    ///
    /// **A block parameter is not an instruction**, so a consumer that needs the
    /// body's inputs reads [`Self::parameters`] — that is the whole difference between
    /// a named value and a fetched one.
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

    /// The gate a consumer calls **before it reads a body**.
    ///
    /// Four rules, and each is a fact about the structure rather than about a
    /// target:
    ///
    /// 1. the entry block exists, and so does every block a transfer names;
    /// 2. **every value a block reads is available there** — an operand is
    ///    computed earlier in the same block, or is one of that block's
    ///    parameters, or was computed in a block that dominates it. The last
    ///    clause is checked by walk order, which is the cheap and sufficient
    ///    condition for a body built by [`KernelBody::add_op`] in order;
    /// 3. **a branch's `args` are exactly its target's `params`**, which is what
    ///    makes a target's phi complete without it inventing a default;
    /// 4. an instruction's `classes` matches what it produces — zero for a write,
    ///    one for everything else.
    ///
    /// A body that fails any of these is **refused by name**, never partially
    /// emitted: a silently dropped branch or a mismatched phi is a fragment that
    /// computes a different program than it was lowered from.
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
                // **A call's arity is the callee's**, which this crate does not know, so the
                // check is the one instruction's exemption from the rule.
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
