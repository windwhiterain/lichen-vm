//! A fragment's body: SSA over named values, with blocks and terminators.
//!
//! # Invariant
//! A block's `params` are the values it receives — one rule for a function's
//! parameters and for a loop's carried state — and a `Br`'s `args` are exactly its
//! target's `params`. What this replaced is `docs/notes/lichen-compute.md` §4.

use std::collections::{HashMap, HashSet};

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

/// A separated `(write, read)` expr: the data a barrier is, before it is a position.
///
/// # Invariant
/// `write` names the write that must happen and `read` the read that observes it: a slice
/// write with the slice read that names it, or a buffer write with the output read of it.
/// Both are [`ValueId`]s and never positions, because the one restructuring in the tree moves
/// an instruction between blocks and a value edge survives it
/// (`docs/notes/shared-memory-and-barriers.md` §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Barrier {
    /// The slice write that must have happened.
    pub write: ValueId,
    /// The read that must observe it.
    pub read: ValueId,
}

/// The kind of write a recorded barrier separates a read from.
///
/// # Invariant
/// **One arm per kind of pair the barrier vocabulary has**: a slice write with the slice read
/// that names it, and a buffer write with the output read that reads the same buffer back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteKind {
    /// A slice write, and the slice read whose version operand names it.
    Slice,
    /// A buffer write, and the output read of the same output buffer.
    Buffer,
}

/// A fragment's body: SSA values, blocks, and where control starts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KernelBody {
    pub values: Vec<ValueDef>,
    pub blocks: Vec<BasicBlock>,
    /// Which block control starts in.
    pub entry: usize,
    /// Whether this body inlined a transcendental definition, so its float arithmetic is exact.
    // `docs/notes/contraction-control.md`; on the body, so `fragment_digest` carries it.
    pub exact_float_arithmetic: bool,
    /// The exprs a barrier is owed for, in no order that matters.
    ///
    /// # Invariant
    /// An expr's read must be reached by every lane: a barrier in divergent control flow is
    /// invalid rather than imprecise, and [`KernelBody::validate`] refuses one.
    pub barriers: Vec<Barrier>,
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

/// One value of a recomputed cone: where a copy of it takes the value from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConeOrigin {
    /// The merged body already holds this value and the copy takes it as it is.
    Live(ValueId),
    /// The leaf a copy rebinds: the read's own address takes its place.
    Address,
    /// The copy computes this value again, from the origins its own operands carry.
    Rebuilt,
}

/// Why the producer's written value is not a function of the address it wrote.
///
/// # Invariant
/// One variant per fact the merge cannot decide for itself, and each names what it met: a copy
/// that guessed here is a merged fragment that answers another program
/// (`docs/notes/graph-fusion.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotIndexLocal {
    /// The write fills an address that is not the invocation index, so a copy answers an address
    /// the producer skipped.
    WriteIsNotIndexed {
        /// The address the write does fill.
        written: ValueId,
    },
    /// A slice read in the cone: another lane's value, and observing it is staging, not a copy.
    SliceRead {
        /// The slice read.
        value: ValueId,
    },
    /// A cross-kernel call in the cone: the callee dispatches over an index of its own.
    KernelCall {
        /// The call.
        value: ValueId,
    },
    /// A parameter of a block other than the entry: state a loop carried, which no address
    /// re-derives.
    Carried {
        /// The carried parameter.
        value: ValueId,
    },
}

impl NotIndexLocal {
    /// The refusal's name, as a report reads it.
    pub fn name(&self) -> &'static str {
        match self {
            NotIndexLocal::WriteIsNotIndexed { .. } => "WriteIsNotIndexed",
            NotIndexLocal::SliceRead { .. } => "SliceReadCall",
            NotIndexLocal::KernelCall { .. } => "CallKernel",
            NotIndexLocal::Carried { .. } => "NonEntryBlockParameter",
        }
    }
}

impl std::fmt::Display for NotIndexLocal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotIndexLocal::WriteIsNotIndexed { written } => write!(
                formatter,
                "the producer fills address {written:?}, which is not the invocation index, so an \
                 address it skipped holds nothing for a copy to answer with"
            ),
            NotIndexLocal::SliceRead { value } => write!(
                formatter,
                "the cone holds the slice read {value:?}, whose value is another lane's, and \
                 observing it at the read site is a slice and a barrier rather than a recomputation"
            ),
            NotIndexLocal::KernelCall { value } => write!(
                formatter,
                "the cone holds the call {value:?}, whose callee dispatches over an index of its \
                 own that no address of the merged fragment names"
            ),
            NotIndexLocal::Carried { value } => write!(
                formatter,
                "the cone holds the carried parameter {value:?}, which is state a loop handed \
                 forward and no address re-derives"
            ),
        }
    }
}

/// One definition cone, planned for a copy at another address.
///
/// # Invariant
/// A copy answers, at any address, what the producer stored there: `written` is the invocation
/// index, so each lane's own address is the one its cone computes. It may be emitted **anywhere
/// that dominates the read site**, which is `KernelBody::validate`'s scope rule to decide, and
/// `order` holds each operand before the value reading it so a copy walks it once.
pub(crate) struct Cone {
    order: Vec<ValueId>,
    origin: HashMap<ValueId, ConeOrigin>,
}

impl Cone {
    /// A cone with one value in it, already computed where it stands.
    pub(crate) fn reused(value: ValueId) -> Cone {
        let mut origin = HashMap::new();
        origin.insert(value, ConeOrigin::Live(value));
        Cone {
            order: vec![value],
            origin,
        }
    }

    /// Where a copy takes this value from; a value outside the cone is the one it already is.
    pub(crate) fn origin(&self, value: ValueId) -> ConeOrigin {
        self.origin
            .get(&value)
            .copied()
            .unwrap_or(ConeOrigin::Live(value))
    }

    /// The cone's values, each after the values it reads.
    pub(crate) fn order(&self) -> &[ValueId] {
        &self.order
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

    /// Replace every transcendental instruction this body computes with the primitive
    /// sequence that defines it.
    ///
    /// # Invariant
    /// **The gate between the language's instructions and a target's.** A transcendental
    /// the two targets agree on natively is emitted as itself; one that is in
    /// [`crate::TRANSCENDENTALS`] is defined by the IR, and this rewrite inlines that
    /// definition. Another operand count is refused by name.
    pub fn expand_transcendentals(&mut self) -> Result<(), String> {
        for (instruction, definition_of) in crate::TRANSCENDENTALS {
            let definition = definition_of();
            let mut work: Vec<(usize, usize, ValueId)> = Vec::new();
            for (block, basic) in self.blocks.iter().enumerate() {
                for (position, &value) in basic.instrs.iter().enumerate() {
                    if matches!(
                        self.values.get(value.0 as usize),
                        Some(ValueDef::Instr { op, .. }) if *op == instruction
                    ) {
                        work.push((block, position, value));
                    }
                }
            }
            // **Latest first**: an inlining adds values to the block it rewrites, so an
            // earlier position stays where it was read.
            for (block, position, value) in work.into_iter().rev() {
                let Some(ValueDef::Instr { args, .. }) = self.values.get(value.0 as usize) else {
                    continue;
                };
                let arguments = args.clone();
                let [operand] = arguments[..] else {
                    return Err(format!(
                        "`{instruction:?}` reads {} operand(s) and is defined over exactly one",
                        arguments.len()
                    ));
                };
                self.inline_definition(&definition, block, position, value, operand)?;
            }
        }
        Ok(())
    }

    /// [`Self::expand_transcendentals`]'s inlining: a one-parameter, one-result
    /// straight-line `definition` at `target`.
    ///
    /// # Invariant
    /// `target` **becomes** the definition's result, so every reader of the original value
    /// reads the definition's answer.
    ///
    /// **Public because the language's own lowering inlines through it too.** `sin` and
    /// `cos` carry no `KernelInstr` of their own, so they reach a kernel by splicing
    /// their definitions here rather than by naming an instruction the expansion rewrites.
    pub fn inline_definition(
        &mut self,
        definition: &KernelBody,
        block: usize,
        position: usize,
        target: ValueId,
        operand: ValueId,
    ) -> Result<(), String> {
        let source = &definition.blocks[definition.entry];
        let [parameter] = source.params[..] else {
            return Err(format!(
                "a transcendental's definition takes {} parameter(s) and is defined over one",
                source.params.len()
            ));
        };
        let Some(&result) = source.instrs.last() else {
            return Err("a transcendental's definition computes nothing".into());
        };
        let Terminator::Return { values } = &source.terminator else {
            return Err("a transcendental's definition must return".into());
        };
        if values.as_slice() != [result] {
            return Err(
                "a transcendental's definition must return the value it computes last".into(),
            );
        }
        let mut renamed: HashMap<ValueId, ValueId> = HashMap::new();
        renamed.insert(parameter, operand);
        let mut sequence: Vec<ValueId> = Vec::with_capacity(source.instrs.len());
        for &step in &source.instrs {
            let Some(ValueDef::Instr { op, args, classes }) =
                definition.values.get(step.0 as usize)
            else {
                return Err("a transcendental's definition lists a parameter as a step".into());
            };
            let mut named = Vec::with_capacity(args.len());
            for arg in args {
                let Some(value) = renamed.get(arg).copied() else {
                    return Err(
                        "a transcendental's definition reads a value its own sequence has not \
                         defined yet"
                            .into(),
                    );
                };
                named.push(value);
            }
            let id = if step == result {
                target
            } else {
                ValueId(self.values.len() as u32)
            };
            let definition_of_step = ValueDef::Instr {
                op: *op,
                args: named,
                classes: classes.clone(),
            };
            if step == result {
                self.values[target.0 as usize] = definition_of_step;
            } else {
                self.values.push(definition_of_step);
            }
            renamed.insert(step, id);
            sequence.push(id);
        }
        self.blocks[block]
            .instrs
            .splice(position..=position, sequence);
        // The spliced sequence's rounding points are the definition's, not a target's.
        self.exact_float_arithmetic = true;
        Ok(())
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

    /// The slice `value` was staged into, when its producer is the write that stages it.
    ///
    /// # Invariant
    /// The answer is the slice ordinal the write's first operand is, and only a slice write
    /// stages: a value with any other producer stages nothing, which a reader must not read as
    /// "the slice at ordinal zero" (`docs/notes/graph-fusion.md` §5).
    pub fn staged_slice(&self, value: ValueId) -> Option<usize> {
        match self.values.get(value.0 as usize) {
            Some(ValueDef::Instr {
                op: KernelInstr::SliceWriteCall(_),
                ..
            }) => self.slice_ordinal(value).map(|ordinal| ordinal as usize),
            _ => None,
        }
    }

    /// The slice ordinal a slice access addresses, when its operand is the constant it must be.
    ///
    /// # Invariant
    /// A slice is a compile-time ordinal, so an operand that is not an integer constant states
    /// no slice — and a caller that cannot resolve one must not read the `None` as "another
    /// slice", because that is the answer that drops a barrier.
    pub(crate) fn slice_ordinal(&self, value: ValueId) -> Option<u64> {
        match self.values.get(value.0 as usize) {
            Some(ValueDef::Instr { args, .. }) => self.constant(args.first().copied()),
            _ => None,
        }
    }

    /// The integer constant `value` is, when it is one.
    pub(crate) fn constant(&self, value: Option<ValueId>) -> Option<u64> {
        match self.values.get(value?.0 as usize) {
            Some(ValueDef::Instr {
                op: KernelInstr::Const(ScalarClass::Int, bits),
                ..
            }) => Some(*bits as u64),
            _ => None,
        }
    }

    /// The gate a consumer calls **before it reads a body**: a failure is refused by
    /// name, never partially emitted.
    ///
    /// # Invariant
    /// (1) The entry block exists, and every block a transfer names does; (2) every value
    /// a block reads exists, defined later in its own block or in one that dominates it;
    /// (3) a branch's `args` are exactly its target's `params`; (4) `classes` matches what
    /// it produces, none for a write; (5) a barrier exprs a slice write with the read
    /// observing it, in a block every lane reaches.
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
        // A use must sit where its value is defined, and after it in the same block.
        let dominance = self.dominance();
        let sites = self.definition_sites();
        for (index, block) in self.blocks.iter().enumerate() {
            for (position, &instr) in block.instrs.iter().enumerate() {
                let ValueDef::Instr { op, args, .. } = &self.values[instr.0 as usize] else {
                    continue;
                };
                let site = format!("{op:?}");
                for arg in args {
                    if let Some(cause) =
                        self.out_of_scope(&dominance, &sites, index, position, *arg, &site)
                    {
                        return Err(cause);
                    }
                }
            }
            let position = block.instrs.len();
            if let Terminator::CondBr { cond, .. } = &block.terminator {
                let site = format!("block {index}'s branch condition");
                if let Some(cause) =
                    self.out_of_scope(&dominance, &sites, index, position, *cond, &site)
                {
                    return Err(cause);
                }
            }
            for br in block.branches() {
                let site = format!("block {index}'s transfer to block {}", br.target);
                for arg in &br.args {
                    if let Some(cause) =
                        self.out_of_scope(&dominance, &sites, index, position, *arg, &site)
                    {
                        return Err(cause);
                    }
                }
            }
            let Terminator::Return { values } = &block.terminator else {
                continue;
            };
            let site = format!("block {index}'s return");
            for value in values {
                if let Some(cause) =
                    self.out_of_scope(&dominance, &sites, index, position, *value, &site)
                {
                    return Err(cause);
                }
            }
        }
        // A recorded barrier is an expr this body really holds, and one no lane can skip.
        let unavoidable = self.unavoidable_blocks(&dominance);
        for expr in &self.barriers {
            // **A barrier names a pair**: the write, and the read that observes it.
            let write_kind = match self.values.get(expr.write.0 as usize) {
                Some(ValueDef::Instr {
                    op: KernelInstr::SliceWriteCall(_),
                    ..
                }) => WriteKind::Slice,
                Some(ValueDef::Instr {
                    op: KernelInstr::BufferWriteCall(_),
                    ..
                }) => WriteKind::Buffer,
                _ => {
                    return Err(format!(
                        "the barrier ({:?}, {:?}) does not name a slice write or a buffer write",
                        expr.write, expr.read
                    ));
                }
            };
            let Some(ValueDef::Instr { op, args, .. }) = self.values.get(expr.read.0 as usize)
            else {
                return Err(format!(
                    "the barrier ({:?}, {:?}) does not name a read of this body",
                    expr.write, expr.read
                ));
            };
            match write_kind {
                WriteKind::Slice => {
                    if !matches!(op, KernelInstr::SliceReadCall(_)) {
                        return Err(format!(
                            "the barrier ({:?}, {:?}) does not name a slice read",
                            expr.write, expr.read
                        ));
                    }
                    if args.get(2) != Some(&expr.write) {
                        return Err(format!(
                            "the barrier's read {:?} does not observe the write {:?} it is paired \
                             with",
                            expr.read, expr.write
                        ));
                    }
                }
                WriteKind::Buffer => {
                    if !matches!(op, KernelInstr::OutputReadCall(_)) {
                        return Err(format!(
                            "the barrier ({:?}, {:?}) does not name an output read",
                            expr.write, expr.read
                        ));
                    }
                    // **The pair names one buffer**: the two ordinals are one number.
                    let written = self.output_ordinal(expr.write);
                    let read_at = self.output_ordinal(expr.read);
                    if written.is_none() || written != read_at {
                        return Err(format!(
                            "the barrier's read {:?} reads the output buffer {read_at:?} and the \
                             write {:?} fills the output buffer {written:?}, so the barrier does \
                             not separate one write from one read",
                            expr.read, expr.write
                        ));
                    }
                    // **The write precedes the read**: a buffer pair has no data edge.
                    let Some((write_block, write_at)) =
                        sites.get(expr.write.0 as usize).copied().flatten()
                    else {
                        return Err(format!(
                            "the barrier's write {:?} is not an instruction of this body",
                            expr.write
                        ));
                    };
                    let Some((read_block, read_at)) =
                        sites.get(expr.read.0 as usize).copied().flatten()
                    else {
                        return Err(format!(
                            "the barrier's read {:?} is not an instruction of this body",
                            expr.read
                        ));
                    };
                    let precedes = if write_block == read_block {
                        write_at < read_at
                    } else {
                        dominance.dominates(read_block, write_block)
                    };
                    if !precedes {
                        return Err(format!(
                            "the barrier's write {:?} does not precede its read {:?}, so the \
                             barrier separates nothing",
                            expr.write, expr.read
                        ));
                    }
                }
            }
            let Some(block) = self.block_of(expr.read) else {
                return Err(format!(
                    "the barrier's read {:?} is not an instruction of this body",
                    expr.read
                ));
            };
            if !unavoidable[block] {
                return Err(format!(
                    "the barrier's read {:?} sits in block {block}, which a lane can skip, so the \
                     barrier would be in divergent control flow — invalid rather than imprecise \
                     (`docs/notes/shared-memory-and-barriers.md` §9)",
                    expr.read
                ));
            }
        }
        Ok(())
    }

    /// The output ordinal an access names, when its position is an integer constant.
    ///
    /// # Invariant
    /// **An output ordinal, not an input one**: a buffer write and an output read both name
    /// their buffer by an output ordinal, and a position that is not a constant names no buffer
    /// at all — which a caller must not read as "the buffer at ordinal zero".
    fn output_ordinal(&self, value: ValueId) -> Option<u64> {
        match self.values.get(value.0 as usize) {
            Some(ValueDef::Instr { args, .. }) => self.constant(args.first().copied()),
            _ => None,
        }
    }

    /// The block whose instruction list defines `value`, or `None` for one it does not.
    fn block_of(&self, value: ValueId) -> Option<usize> {
        self.blocks
            .iter()
            .position(|block| block.instrs.contains(&value))
    }

    /// The dominator sets over the reachable CFG, computed once for both rules that read them.
    ///
    /// # Invariant
    /// `dominators[block][candidate]` states that every path from the entry to `block` passes
    /// through `candidate`; `reachable` states that a path reaches a block at all.
    fn dominance(&self) -> Dominance {
        let count = self.blocks.len();
        let mut successors: Vec<Vec<usize>> = vec![Vec::new(); count];
        for (index, block) in self.blocks.iter().enumerate() {
            for br in block.branches() {
                if br.target < count {
                    successors[index].push(br.target);
                }
            }
        }
        let mut reachable = vec![false; count];
        let mut stack = vec![self.entry];
        while let Some(block) = stack.pop() {
            if std::mem::replace(&mut reachable[block], true) {
                continue;
            }
            stack.extend(successors[block].iter().copied());
        }
        let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); count];
        for (index, targets) in successors.iter().enumerate() {
            if !reachable[index] {
                continue;
            }
            for &target in targets {
                predecessors[target].push(index);
            }
        }
        // Every block starts as all reachable blocks and shrinks to its predecessors'
        // intersection, except the entry.
        let mut dominators: Vec<Vec<bool>> = vec![vec![false; count]; count];
        for block in 0..count {
            if block != self.entry && reachable[block] {
                dominators[block] = reachable.clone();
                dominators[block][block] = true;
            }
        }
        dominators[self.entry][self.entry] = true;
        loop {
            let mut changed = false;
            for block in 0..count {
                if block == self.entry || !reachable[block] {
                    continue;
                }
                let mut next = vec![true; count];
                for &predecessor in &predecessors[block] {
                    for candidate in 0..count {
                        next[candidate] &= dominators[predecessor][candidate];
                    }
                }
                next[block] = true;
                if next != dominators[block] {
                    dominators[block] = next;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        Dominance {
            reachable,
            dominators,
        }
    }

    /// Every value each block receives, one list per incoming branch.
    ///
    /// # Invariant
    /// A block parameter is defined by all its incoming branches and not by one, so a cone walk
    /// following a single edge would call a carried value invariant.
    pub(crate) fn incoming_arguments(&self) -> Vec<Vec<Vec<ValueId>>> {
        let mut incoming: Vec<Vec<Vec<ValueId>>> = vec![Vec::new(); self.blocks.len()];
        for block in &self.blocks {
            for branch in block.branches() {
                if let Some(arguments) = incoming.get_mut(branch.target) {
                    arguments.push(branch.args.clone());
                }
            }
        }
        incoming
    }

    /// Whether `value`'s definition cone reaches `lane`.
    ///
    /// # Invariant
    /// A block parameter is defined by the values **every** incoming branch hands it, so a carried
    /// lane is seen through a loop's parameter rather than read as invariant; an operand a branch
    /// does not hand over counts as a dependence.
    pub(crate) fn depends_on(
        &self,
        value: ValueId,
        lane: ValueId,
        incoming: &[Vec<Vec<ValueId>>],
    ) -> bool {
        let mut stack = vec![value];
        let mut seen: HashSet<ValueId> = HashSet::new();
        while let Some(current) = stack.pop() {
            if current == lane {
                return true;
            }
            if !seen.insert(current) {
                continue;
            }
            match self.values.get(current.0 as usize) {
                Some(ValueDef::Instr { args, .. }) => stack.extend(args.iter().copied()),
                Some(ValueDef::BlockParam { block, index }) => {
                    for arguments in incoming.get(*block).into_iter().flatten() {
                        match arguments.get(*index as usize) {
                            Some(argument) => stack.push(*argument),
                            None => return true,
                        }
                    }
                }
                None => {}
            }
        }
        false
    }

    /// `value`'s definition cone, planned for a copy at some other address — or what stops it.
    ///
    /// # Invariant
    /// **Index-local**: the cone holds `written` and the entry's other parameters, and reads whose
    /// addresses are functions of that same address. It holds no `SliceReadCall`, whose value is
    /// another lane's and whose exchange at a read site is staging, not recomputation, no
    /// `CallKernel`, which has an index of its own, and no parameter of a block but the entry,
    /// which no address re-derives.
    pub(crate) fn index_local_cone(
        &self,
        root: ValueId,
        written: ValueId,
        lane: Option<ValueId>,
    ) -> Result<Cone, NotIndexLocal> {
        if lane != Some(written) {
            return Err(NotIndexLocal::WriteIsNotIndexed { written });
        }
        // **Operands before their users**: a value is popped only after every value it reads.
        let mut order: Vec<ValueId> = Vec::new();
        let mut origin: HashMap<ValueId, ConeOrigin> = HashMap::new();
        let mut entered: HashSet<ValueId> = HashSet::new();
        let mut work: Vec<ValueId> = vec![root];
        while let Some(value) = work.pop() {
            if !entered.insert(value) {
                order.push(value);
                continue;
            }
            work.push(value);
            if let Some(ValueDef::Instr { args, .. }) = self.values.get(value.0 as usize) {
                work.extend(args.iter().copied());
            }
        }
        for &value in &order {
            let resolved = self.cone_origin(value, written, &origin)?;
            origin.insert(value, resolved);
        }
        Ok(Cone { order, origin })
    }

    /// One value's own answer to "what does a copy of this cone take for it".
    fn cone_origin(
        &self,
        value: ValueId,
        written: ValueId,
        origin: &HashMap<ValueId, ConeOrigin>,
    ) -> Result<ConeOrigin, NotIndexLocal> {
        match self.values.get(value.0 as usize) {
            Some(ValueDef::BlockParam { block, .. }) => {
                if value == written {
                    Ok(ConeOrigin::Address)
                } else if *block == self.entry {
                    Ok(ConeOrigin::Live(value))
                } else {
                    Err(NotIndexLocal::Carried { value })
                }
            }
            Some(ValueDef::Instr { op, args, .. }) => match op {
                KernelInstr::SliceReadCall(_) => Err(NotIndexLocal::SliceRead { value }),
                KernelInstr::CallKernel(_) => Err(NotIndexLocal::KernelCall { value }),
                _ => {
                    // **A value is rebuilt when a value it reads is**: an operand the copy
                    // recomputes, or the address leaf the copy rebinds.
                    let rebuilt = args.iter().any(|&arg| {
                        matches!(
                            origin.get(&arg),
                            Some(ConeOrigin::Rebuilt | ConeOrigin::Address)
                        )
                    });
                    Ok(if rebuilt {
                        ConeOrigin::Rebuilt
                    } else {
                        ConeOrigin::Live(value)
                    })
                }
            },
            None => Ok(ConeOrigin::Live(value)),
        }
    }

    /// Which blocks every lane reaches, as a flag per block.
    ///
    /// # Invariant
    /// A block qualifies when it dominates every reachable `Return`, so every path that ends
    /// passes through it. A read in dead code is not here and is refused; a path that never
    /// returns is not an exit, so a non-terminating branch past the read is not caught.
    fn unavoidable_blocks(&self, dominance: &Dominance) -> Vec<bool> {
        let count = self.blocks.len();
        (0..count)
            .map(|block| {
                dominance.reachable[block]
                    && self.blocks.iter().enumerate().all(|(exit, basic)| {
                        !(dominance.reachable[exit]
                            && matches!(&basic.terminator, Terminator::Return { .. }))
                            || dominance.dominators[exit][block]
                    })
            })
            .collect()
    }

    /// Where each instruction of this body is defined: its block, and its position there.
    ///
    /// # Invariant
    /// A value that no block lists has no site, and a use of one is refused rather than read:
    /// an instruction no block lists is one the emitter never binds.
    fn definition_sites(&self) -> Vec<Option<(usize, usize)>> {
        let mut sites = vec![None; self.values.len()];
        for (block, basic) in self.blocks.iter().enumerate() {
            for (position, &value) in basic.instrs.iter().enumerate() {
                if let Some(site) = sites.get_mut(value.0 as usize) {
                    *site = Some((block, position));
                }
            }
        }
        sites
    }

    /// The refusal a use earns when it does not sit where its value is defined.
    ///
    /// # Invariant
    /// In scope means the value is a parameter of `block`, or an instruction earlier in the same
    /// block, or an instruction of a block that dominates it; `site` names the use in the refusal
    /// (`docs/notes/shared-memory-and-barriers.md` §10 item 7).
    fn out_of_scope(
        &self,
        dominance: &Dominance,
        sites: &[Option<(usize, usize)>],
        block: usize,
        position: usize,
        value: ValueId,
        site: &str,
    ) -> Option<String> {
        match self.values.get(value.0 as usize) {
            Some(ValueDef::BlockParam {
                block: defined_in, ..
            }) => {
                if *defined_in == block || dominance.dominates(block, *defined_in) {
                    return None;
                }
                Some(format!(
                    "{site} reads {value:?}, which block {defined_in} receives as a parameter, and \
                     block {defined_in} does not dominate block {block}"
                ))
            }
            Some(ValueDef::Instr { .. }) => {
                let Some((defined_in, defined_at)) = sites.get(value.0 as usize).copied().flatten()
                else {
                    return Some(format!(
                        "{site} reads {value:?}, which no block of this body defines"
                    ));
                };
                if defined_in == block {
                    if defined_at < position {
                        return None;
                    }
                    return Some(format!(
                        "{site} reads {value:?}, which block {block} defines later in the same block"
                    ));
                }
                if dominance.dominates(block, defined_in) {
                    return None;
                }
                Some(format!(
                    "{site} reads {value:?}, which block {defined_in} defines, and block \
                     {defined_in} does not dominate block {block}"
                ))
            }
            None => None,
        }
    }
}

/// What [`KernelBody::dominance`] computes, and the relation the scope rule reads.
///
/// # Invariant
/// An unreachable block has no dominators, so only a same-block use is in scope inside one.
struct Dominance {
    reachable: Vec<bool>,
    dominators: Vec<Vec<bool>>,
}

impl Dominance {
    /// Whether every path from the entry to `block` passes through `candidate`.
    fn dominates(&self, block: usize, candidate: usize) -> bool {
        self.dominators
            .get(block)
            .and_then(|set| set.get(candidate))
            .copied()
            .unwrap_or(false)
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
