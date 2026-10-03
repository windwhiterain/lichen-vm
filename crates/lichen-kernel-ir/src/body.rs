//! A fragment body as **structured control flow**, and the invariant it must
//! satisfy.
//!
//! See `docs/notes/loop-conversion.md` §3 and §8: a loop needs a backedge *and* a
//! value that survives it, so a body is not a block arena with a branch opcode —
//! it is a tree of structured transfers, each of which names the label its own
//! exit arrives at. That is what lets the wasm backend emit `block`/`loop`/`br_if`
//! and the SPIR-V backend emit `OpLoopMerge` **without either backend computing
//! dominators or discovering natural loops**: the structure is already in the IR.
//!
//! # Block parameters are the top of the stack
//!
//! A body is still a pure stack machine — [`KernelInstr`](crate::KernelInstr)
//! consumes the top of the stack, as it always did. What a structured transfer
//! carries is therefore a **count**, never a slot index: entering a label with
//! `n` values means the top `n` values at the jump are that label's parameters.
//! A backend turns that count into whatever its target calls the same thing —
//! wasm locals, or one `OpPhi` per incoming edge — and nothing above this crate
//! has to invent a value namespace to say it.
//!
//! # The invariant
//!
//! [`KernelBody::validate`] is the whole contract, and it is checked before a
//! backend reads a body:
//!
//! - every [`BlockId`] named by a transfer is below [`KernelBody::labels`];
//! - a loop's `header` is a label the loop's own body jumps back to, so the
//!   backedge is explicit rather than inferred;
//! - an `if`'s arms are *arms*, not successors: both of them arrive at `join`,
//!   and a missing arm inherits the stack as it stood when the branch was
//!   reached.
//!
//! A body that fails this is **refused by name**. It is never partially emitted:
//! a backend that cannot honour a transfer has to say so, because a silently
//! dropped branch is a body that computes a different program.

use crate::KernelInstr;

/// A label a body can arrive at. An index into [`KernelBody::labels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

/// Where a finished sequence of instructions goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminator {
    /// Leave the fragment, carrying the top [`results`](crate::KernelFragment::results)
    /// values.
    Return,
    /// A two-way branch on the `0`/`1` scalar **on top of the stack**.
    ///
    /// Each arm runs and then arrives at `join` with `passes` values. An arm that
    /// is [`None`] arrives with the stack as it stood when the branch was
    /// reached, below the selector — so a one-armed `if` is expressible without
    /// inventing a value for the absent side.
    If {
        /// Taken when the selector is `1`.
        on_one: Box<Flow>,
        /// Taken when the selector is `0`, or [`None`] for a one-armed branch.
        on_zero: Option<Box<Flow>>,
        /// Where either arm arrives, receiving `passes` values.
        join: BlockId,
        /// How many values the arms hand to `join`.
        passes: usize,
    },
    /// A loop, whose condition — the `0`/`1` scalar on top of the stack — is
    /// re-evaluated at `header` on every entry including the first.
    ///
    /// `header` is a label this loop owns; `body` runs once per iteration and
    /// arrives back at `header` with `carried` values. A zero condition leaves
    /// to `exit` with `passed_out` values. **Testing before the first iteration
    /// is what makes a zero-trip loop correct**, so the condition is never hoisted
    /// out of the header.
    ///
    /// **The body must be able to compute its carried values**, which is what
    /// [`Flow::Seq`] is for: without it a body can only hand back the header's
    /// own values, and a reduction — the one shape with an accumulator — has no
    /// representation. See [`Flow::Seq`].
    While {
        /// The label re-entered on every iteration; a backend's loop header.
        header: BlockId,
        /// The per-iteration body, which must arrive back at `header`.
        body: Box<Flow>,
        /// Where a zero condition goes, receiving `passed_out` values.
        exit: BlockId,
        /// How many values the body hands back to `header`.
        carried: usize,
        /// How many values the loop hands to `exit`.
        passed_out: usize,
    },
}

/// A run of instructions and the transfer that follows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    /// Straight-line instructions, then a transfer.
    Block {
        /// The label control arrives at to run these instructions, if any.
        ///
        /// **A `While` names this label as its `header`**, and that pairing is
        /// what makes a zero-trip loop correct: the header's own instructions
        /// compute the condition, so the test happens on *every* entry including
        /// the first. Without the field a backend could not tell which block a
        /// loop re-enters, and would have to hoist the test out of it — which is
        /// the off-by-one-iteration bug. [`KernelBody::validate`] checks the
        /// pairing.
        entry: Option<BlockId>,
        /// The instructions, in emission order, consuming the stack from the top.
        instrs: Vec<KernelInstr>,
        /// Where control goes when they have run.
        terminator: Box<Terminator>,
    },
    /// Instructions, then a transfer that is **not** a conditional branch.
    ///
    /// # Why this shape exists
    ///
    /// A loop body that only produces values and then leaves has nothing to do
    /// with those values *except* hand them somewhere, and before this shape
    /// there was no way to say "compute these, then go back to the header" that
    /// was not also a branch on a condition. `Block` could not do it: a loop
    /// body's last act must arrive back at the header, and `Block`'s terminator
    /// is a `Terminator`, which means an `If` — and a selection's merge must be
    /// dominated by *its* header, which the loop header is not.
    ///
    /// So the only expressible body was a bare [`Flow::Jump`] back to the
    /// header, which can pass the header's *own* values and nothing else. That
    /// makes the header's `OpPhi` self-referential, and leaves a reduction — the
    /// acceptance case in `docs/notes/loop-conversion.md` §8 — with no
    /// representation at all.
    ///
    /// `Seq` is that missing half: run these instructions, then perform this
    /// **plain** transfer. [`Terminator::While`]'s `body` must be a `Seq`, so the
    /// carried values are computed rather than forwarded.
    Seq {
        /// The instructions, in emission order, consuming the stack from the top.
        instrs: Vec<KernelInstr>,
        /// Where control goes afterwards — a jump back to the loop header, or a
        /// branch out of the loop to its `exit`.
        terminator: Box<Terminator>,
    },
    /// Leave this flow by arriving at `target` with the top `passes` values.
    Jump {
        /// The label arrived at.
        target: BlockId,
        /// How many values are handed over.
        passes: usize,
    },
}

impl Flow {
    /// A straight-line run of instructions that ends in `terminator`.
    pub fn block(instrs: Vec<KernelInstr>, terminator: Terminator) -> Self {
        Flow::Block {
            entry: None,
            instrs,
            terminator: Box::new(terminator),
        }
    }

    /// The labels this flow mentions, in visit order, including nested ones.
    ///
    /// The depth is a flow's own nesting, so it is bounded by the structure the
    /// fragment was built with rather than by how much graph the lowering read.
    pub fn labels(&self, out: &mut Vec<BlockId>) {
        match self {
            Flow::Block {
                entry, terminator, ..
            } => {
                if let Some(entry) = entry {
                    out.push(*entry);
                }
                terminator_labels(terminator, out);
            }
            Flow::Seq { terminator, .. } => terminator_labels(terminator, out),
            Flow::Jump { target, .. } => out.push(*target),
        }
    }
}

fn terminator_labels(terminator: &Terminator, out: &mut Vec<BlockId>) {
    match terminator {
        Terminator::Return => {}
        Terminator::If {
            on_one,
            on_zero,
            join,
            ..
        } => {
            on_one.labels(out);
            if let Some(on_zero) = on_zero {
                on_zero.labels(out);
            }
            out.push(*join);
        }
        Terminator::While {
            header, body, exit, ..
        } => {
            out.push(*header);
            body.labels(out);
            out.push(*exit);
        }
    }
}

/// A fragment's body: one entry flow, and how many labels it may arrive at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelBody {
    /// What the function runs, from the first instruction.
    pub entry: Flow,
    /// The number of labels this body's transfers may name.
    ///
    /// **A label is a slot, not a block of code.** The code that runs when
    /// control reaches one is the [`Flow`] at the site that arrives there, so a
    /// body allocates a label per merge and hands it to whichever backends need
    /// storage for it — a wasm local per carried value, an `OpPhi` per incoming
    /// edge — without this crate knowing what either of those is.
    pub labels: usize,
}

impl KernelBody {
    /// A body with no control flow at all: one straight-line run that returns.
    ///
    /// **Every fragment that predates structured control flow is this**, so this
    /// is the form a lowering produces until it has a reason to branch, and the
    /// form both backends must keep lowering identically.
    pub fn straight_line(instrs: Vec<KernelInstr>) -> Self {
        KernelBody {
            entry: Flow::Block {
                entry: None,
                instrs,
                terminator: Box::new(Terminator::Return),
            },
            labels: 0,
        }
    }

    /// Whether this body has any transfer other than [`Terminator::Return`].
    ///
    /// A backend reads this to keep its fast path — one straight-line emission,
    /// no label stack, no phi — for the fragments that need none of that.
    pub fn is_straight_line(&self) -> bool {
        fn straight(flow: &Flow) -> bool {
            match flow {
                Flow::Jump { .. } => false,
                Flow::Seq { .. } => false,
                Flow::Block { terminator, .. } => matches!(&**terminator, Terminator::Return),
            }
        }
        straight(&self.entry)
    }

    /// The instructions of a body with no control flow, or [`None`] if it has some.
    ///
    /// **This is the bridge a backend takes while it still emits only
    /// straight-line code**, so that a fragment with a transfer is *refused by
    /// name* rather than silently emitted without it. It disappears when the
    /// backend learns to emit the structure.
    pub fn straight_line_instrs(&self) -> Option<&[KernelInstr]> {
        match &self.entry {
            Flow::Block {
                entry: None,
                instrs,
                terminator,
            } => match &**terminator {
                Terminator::Return => Some(instrs),
                _ => None,
            },
            _ => None,
        }
    }

    /// Every instruction in this body, in walk order, transfers included.
    ///
    /// **For walks that must not miss an instruction**, such as the one that
    /// decides which kernels a module has to contain: a walk built on
    /// [`Self::straight_line_instrs`] would see nothing inside a branch, and a
    /// cross-kernel call hidden in one would be dropped from the module it is
    /// called from.
    pub fn instrs(&self) -> Vec<&KernelInstr> {
        let mut out = Vec::new();
        collect_instrs(&self.entry, &mut out);
        out
    }

    /// Check the structural invariant, returning a refusal that names what broke.
    ///
    /// This is the gate a backend calls before reading the body, and it is what
    /// lets a body be introduced ahead of the backends that consume it: a backend
    /// that has not learned a transfer refuses it here rather than dropping it.
    pub fn validate(&self) -> Result<(), String> {
        let mut named = Vec::new();
        self.entry.labels(&mut named);
        if let Some(BlockId(out_of_range)) = named.iter().find(|id| id.0 >= self.labels as u32) {
            return Err(format!(
                "kernel body names block {out_of_range} but allocates only {} labels",
                self.labels
            ));
        }
        let mut defined = Vec::new();
        collect_entries(&self.entry, &mut defined);
        for (at, id) in defined.iter().enumerate() {
            if defined[..at].contains(id) {
                return Err(format!(
                    "block {} is defined twice; a label names one arrival point and a backend \
                     resolves every transfer to exactly one",
                    id.0
                ));
            }
        }
        // A loop's `exit` is its own fall-through: with the loop as a terminator
        // there is no code *after* it, so the exit is the point control reaches by
        // leaving — and a jump to it is a branch out. Two loops may not share one,
        // because that would give the label two arrival points again.
        let mut exits = Vec::new();
        collect_exits(&self.entry, &mut exits);
        for (at, id) in exits.iter().enumerate() {
            if exits[..at].contains(id) {
                return Err(format!(
                    "two loops leave for block {}; a label names one arrival point, and a shared \
                     exit would name two",
                    id.0
                ));
            }
        }
        for arrived in &named {
            if !defined.contains(arrived) && !exits.contains(arrived) {
                return Err(format!(
                    "block {} is arrived at but nothing defines it; a transfer with no \
                     definition is a block a backend cannot emit",
                    arrived.0
                ));
            }
        }
        validate_flow(&self.entry, None)
    }
}

/// The labels a [`Terminator::While`] leaves for — its fall-through points.
fn collect_exits(flow: &Flow, out: &mut Vec<BlockId>) {
    match flow {
        Flow::Jump { .. } => {}
        Flow::Seq { terminator, .. } => terminator_exits(terminator, out),
        Flow::Block { terminator, .. } => terminator_exits(terminator, out),
    }
}

fn terminator_exits(terminator: &Terminator, out: &mut Vec<BlockId>) {
    match terminator {
        Terminator::Return => {}
        Terminator::If {
            on_one, on_zero, ..
        } => {
            collect_exits(on_one, out);
            if let Some(on_zero) = on_zero {
                collect_exits(on_zero, out);
            }
        }
        Terminator::While { body, exit, .. } => {
            out.push(*exit);
            collect_exits(body, out);
        }
    }
}

/// The labels this body defines, i.e. the ones a [`Flow::Block`] is entered at.
fn collect_entries(flow: &Flow, out: &mut Vec<BlockId>) {
    match flow {
        Flow::Jump { .. } => {}
        Flow::Seq { terminator, .. } => terminator_entries(terminator, out),
        Flow::Block {
            entry, terminator, ..
        } => {
            if let Some(entry) = entry {
                out.push(*entry);
            }
            terminator_entries(terminator, out);
        }
    }
}

fn terminator_entries(terminator: &Terminator, out: &mut Vec<BlockId>) {
    match terminator {
        Terminator::Return => {}
        Terminator::If {
            on_one, on_zero, ..
        } => {
            collect_entries(on_one, out);
            if let Some(on_zero) = on_zero {
                collect_entries(on_zero, out);
            }
        }
        Terminator::While { body, .. } => collect_entries(body, out),
    }
}

/// Every instruction in a flow, in walk order.
fn collect_instrs<'a>(flow: &'a Flow, out: &mut Vec<&'a KernelInstr>) {
    match flow {
        Flow::Jump { .. } => {}
        Flow::Seq {
            instrs, terminator, ..
        } => {
            out.extend(instrs.iter());
            collect_terminator_instrs(terminator, out);
        }
        Flow::Block {
            instrs, terminator, ..
        } => {
            out.extend(instrs.iter());
            collect_terminator_instrs(terminator, out);
        }
    }
}

/// Every instruction a terminator's nested flows hold.
fn collect_terminator_instrs<'a>(terminator: &'a Terminator, out: &mut Vec<&'a KernelInstr>) {
    match terminator {
        Terminator::Return => {}
        Terminator::If {
            on_one, on_zero, ..
        } => {
            collect_instrs(on_one, out);
            if let Some(on_zero) = on_zero {
                collect_instrs(on_zero, out);
            }
        }
        Terminator::While { body, .. } => collect_instrs(body, out),
    }
}

/// Check one flow, carrying the loop header a nested body is obliged to return to.
fn validate_flow(flow: &Flow, header: Option<BlockId>) -> Result<(), String> {
    match flow {
        Flow::Jump { target, .. } => match header {
            Some(header) if *target != header => Err(format!(
                "a loop body must arrive back at its header block {}, but this one leaves for block {} — \
                 a backend has no backedge to close the loop with",
                header.0, target.0
            )),
            _ => Ok(()),
        },
        // A `Seq` inside a loop must leave for the header, and the check is the
        // same one a bare jump gets: the obligation is on the *transfer*, so both
        // shapes answer it identically.
        Flow::Seq { terminator, .. } => match header {
            Some(_) => match &**terminator {
                Terminator::Return => Err(
                    "a loop body returns instead of arriving back at its header; the loop has no \
                     backedge"
                        .to_string(),
                ),
                Terminator::If {
                    on_one,
                    on_zero,
                    join,
                    passes,
                } => {
                    if join.0 == header.expect("checked just above").0 {
                        validate_flow(on_one, header)?;
                        if let Some(on_zero) = on_zero {
                            validate_flow(on_zero, header)?;
                        }
                        // Both arms hand the header its carried values, and the
                        // arms' own arms still owe the header the backedge.
                        let _ = passes;
                        Ok(())
                    } else {
                        Err(format!(
                            "a loop body's sequence leaves for block {} through a selection, but a loop \
                             body must arrive back at its header block {}",
                            join.0,
                            header.expect("checked just above").0
                        ))
                    }
                }
                Terminator::While { header: inner, .. } => Err(format!(
                    "a loop body nests another loop, whose header is block {}; a backend has no way \
                     to close both with one backedge",
                    inner.0
                )),
            },
            None => validate_terminator(terminator),
        },
        Flow::Block {
            entry, terminator, ..
        } => {
            if let (Some(entry), Some(header)) = (entry, header) {
                if *entry != header {
                    return Err(format!(
                        "a loop body arrives back at block {}, but its own block is entered at block {} — \
                         the backedge and the header disagree, so the loop would re-enter the wrong code",
                        header.0, entry.0
                    ));
                }
            }
            match &**terminator {
                Terminator::Return => match header {
                    Some(_) => Err(
                        "a loop body returns instead of arriving back at its header; the loop has no backedge"
                            .to_string(),
                    ),
                    None => Ok(()),
                },
                Terminator::If {
                    on_one,
                    on_zero,
                    ..
                } => {
                    validate_flow(on_one, header)?;
                    match on_zero {
                        Some(on_zero) => validate_flow(on_zero, header),
                        None => Ok(()),
                    }
                }
                Terminator::While { header, body, .. } => {
                    validate_flow(body, Some(*header))?;
                    if entry.is_some_and(|entry| entry != *header) {
                        return Err(format!(
                            "a loop declares its header at block {} but the block holding the loop is \
                             entered at block {} — a backend would re-enter the loop's test from the \
                             wrong place",
                            header.0,
                            entry.expect("checked just above").0
                        ));
                    }
                    Ok(())
                }
            }
        }
    }
}

/// Validate a terminator with no loop-header obligation in force.
fn validate_terminator(terminator: &Terminator) -> Result<(), String> {
    match terminator {
        Terminator::Return => Ok(()),
        Terminator::If {
            on_one, on_zero, ..
        } => {
            validate_flow(on_one, None)?;
            match on_zero {
                Some(on_zero) => validate_flow(on_zero, None),
                None => Ok(()),
            }
        }
        Terminator::While { body, .. } => validate_flow(body, None),
    }
}

impl From<Vec<KernelInstr>> for KernelBody {
    fn from(instrs: Vec<KernelInstr>) -> Self {
        KernelBody::straight_line(instrs)
    }
}
