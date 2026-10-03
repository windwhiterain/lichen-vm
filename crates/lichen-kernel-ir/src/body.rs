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
    /// Leave this flow by arriving at `target` with the top `passes` values.
    ///
    /// # Why this is a `Terminator` and not only a [`Flow::Jump`]
    ///
    /// A plain transfer is a transfer, and the two enums here split transfers by
    /// *where they can appear* rather than by what they mean: an [`If`](Self::If)
    /// arm is a `Flow` (it may be a block of its own, or a bare jump), and a
    /// [`Flow::Seq`]'s own transfer is a `Terminator` — so without this variant a
    /// sequence could end in a branch or in a return but **not in a jump**, which is
    /// the one transfer its own documentation names ("run these instructions, then
    /// perform this **plain** transfer"). A loop body could therefore not be
    /// written at all: it has instructions to run and a backedge to take, and
    /// nothing could say both.
    ///
    /// The two are deliberately not merged. An `If` arm has no instructions of its
    /// own, so its transfer *is* its whole content and a [`Flow::Jump`] is the
    /// honest shape for it; a `Seq` has instructions, so what it owes is a
    /// `Terminator`. See `docs/notes/loop-body-expressiveness.md`.
    Jump {
        /// The label arrived at.
        target: BlockId,
        /// How many values are handed over.
        passes: usize,
    },
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
    /// # The one state tuple, and the two counts that read it
    ///
    /// `header` is a label this loop owns, and **the loop's whole state is the
    /// tuple of `carried` values the header's instructions start from**: when the
    /// header's own instructions run, the top `carried` values of the stack are
    /// that tuple, and whatever the header pushes above them ends in the `0`/`1`
    /// condition on top. So the stack at the terminator is
    /// `…, state(carried), condition`, and both counts are read off *that* stack
    /// after the condition is popped:
    ///
    /// - the **body** is handed the same `carried` values as its starting state —
    ///   which happens by construction, because the body *is* the code that runs
    ///   after the header's instructions, and the test is what decides whether it
    ///   runs at all;
    /// - when the condition is `0`, the **exit** is handed the top `passed_out` of
    ///   those same values. `passed_out ≤ carried` therefore holds, and a
    ///   reduction is the ordinary case rather than an accounting trick: it
    ///   carries `[counter, accumulator]` and hands its exit the accumulator.
    ///
    /// **The exit's values are the header's, not the body's, and that is what
    /// makes a zero-trip loop correct.** A trip count of zero never runs the body,
    /// so a `passed_out` the body had to compute would have no source on that
    /// path; reading it off the header's own tuple defines the result for free —
    /// the initial state is handed straight out. **Testing before the first
    /// iteration is the same fact**, so the condition is never hoisted out of the
    /// header.
    ///
    /// **The body must be able to compute its carried values and hand them back**,
    /// which is what [`Flow::Seq`] is for: without it a body can only hand back the
    /// header's own values, and a reduction — the one shape with an accumulator —
    /// has no representation. A body that has to *decide* between continuing and
    /// leaving does it with a [`Terminator::If`] whose arms name this loop's header
    /// and [`exit`](Self::While::exit).
    While {
        /// The label re-entered on every iteration; a backend's loop header.
        header: BlockId,
        /// The per-iteration body, which must arrive back at `header`.
        body: Box<Flow>,
        /// Where a zero condition goes, receiving the top `passed_out` values of
        /// the header's state.
        exit: BlockId,
        /// How many values the body hands back to `header`.
        carried: usize,
        /// How many of the header's own values the exit receives. Never more than
        /// `carried`.
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
    /// **plain** transfer — a [`Terminator::Jump`] back to the loop header, or a
    /// [`Terminator::If`] whose arms name the header and the loop's exit, so a body
    /// can decide between continuing and leaving.
    Seq {
        /// The instructions, in emission order, consuming the stack from the top.
        instrs: Vec<KernelInstr>,
        /// Where control goes afterwards.
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
        Terminator::Jump { target, .. } => out.push(*target),
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
        validate_flow(&self.entry, FlagEnd::Return)
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
        Terminator::Return | Terminator::Jump { .. } => {}
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
        Terminator::Return | Terminator::Jump { .. } => {}
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
        Terminator::Return | Terminator::Jump { .. } => {}
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

/// How the flow being validated may end.
///
/// The validation is a walk of the body's tree, and what a nested flow owes
/// depends on where it sits: a flow outside a loop ends by returning, and a flow
/// inside one ends by **arriving somewhere the loop can close from** — its header,
/// where the next iteration's state is handed over, or its exit, where the loop is
/// left. Carrying that obligation explicitly is what lets one walk accept a body
/// that computes its state on one path and leaves on another, which the old
/// "everything must reach the header" rule could not express
/// (`docs/notes/loop-body-expressiveness.md`).
#[derive(Debug, Clone, Copy)]
enum FlagEnd {
    /// The fragment's own end: a flow under this ends by returning.
    Return,
    /// A loop's level: a flow under this ends by arriving at `header` (the
    /// backedge, handing over the next state) or at `exit` (leaving the loop).
    Loop { header: BlockId, exit: BlockId },
}

/// Check one flow against the end it owes.
fn validate_flow(flow: &Flow, end: FlagEnd) -> Result<(), String> {
    match flow {
        // The obligation is on the *transfer*, so a bare jump and a sequence that
        // ends in one answer it identically.
        Flow::Jump { target, .. } => arrive(target, end),
        Flow::Seq { terminator, .. } => validate_terminator(terminator, end),
        Flow::Block {
            entry, terminator, ..
        } => {
            // A block a loop re-enters has to be entered at *its* header, or the
            // backedge and the header disagree and the loop would re-enter the
            // wrong code.
            if let FlagEnd::Loop { header, .. } = end
                && entry.is_some_and(|entry| entry != header)
            {
                return Err(format!(
                    "a loop body arrives back at block {}, but its own block is entered at block {} — \
                     the backedge and the header disagree, so the loop would re-enter the wrong code",
                    header.0,
                    entry.expect("checked just above").0
                ));
            }
            validate_terminator(terminator, end)
        }
    }
}

/// Check where one transfer arrives, against the end the flow owes.
fn arrive(target: &BlockId, end: FlagEnd) -> Result<(), String> {
    match end {
        FlagEnd::Return => Err(format!(
            "the body leaves for block {} instead of returning; a fragment's last act hands its \
             results over",
            target.0
        )),
        FlagEnd::Loop { header, exit } if *target == header || *target == exit => Ok(()),
        FlagEnd::Loop { header, exit } => Err(format!(
            "a loop body must arrive back at its header block {} or leave for its exit block {}, \
             but this one leaves for block {} — a backend has no backedge to close the loop with",
            header.0, exit.0, target.0
        )),
    }
}

/// Check one terminator, and walk into the flows it opens.
fn validate_terminator(terminator: &Terminator, end: FlagEnd) -> Result<(), String> {
    match terminator {
        Terminator::Return => match end {
            FlagEnd::Return => Ok(()),
            FlagEnd::Loop { header, .. } => Err(format!(
                "a loop body returns instead of arriving back at its header block {}; the loop has \
                 no backedge",
                header.0
            )),
        },
        Terminator::Jump { target, .. } => arrive(target, end),
        Terminator::If {
            on_one,
            on_zero,
            join,
            ..
        } => {
            // **A selection inside a loop decides between its own landmarks.** A
            // loop has two of them — the header, where the next state is handed over,
            // and the exit, where the loop is left — and both arms of the body's
            // selection may name either. They usually *branch* to them rather than
            // merge, so the join is where the arms would meet; naming one of the two
            // is what says the selection is part of the loop's own control flow
            // rather than a merge the loop would have to close.
            if let FlagEnd::Loop { header, exit } = end
                && *join != header
                && *join != exit
            {
                return Err(format!(
                    "a loop body's selection joins at block {}, but a loop body must arrive back at \
                     its header block {} or leave for its exit block {}",
                    join.0, header.0, exit.0
                ));
            }
            validate_flow(on_one, end)?;
            match on_zero {
                Some(on_zero) => validate_flow(on_zero, end),
                None => Ok(()),
            }
        }
        Terminator::While {
            header,
            body,
            exit,
            carried,
            passed_out,
        } => {
            if passed_out > carried {
                return Err(format!(
                    "a loop hands {passed_out} value(s) to its exit from a state of only {carried}; \
                     the exit's values are the header's own, and a zero trip count has no other \
                     place to get them"
                ));
            }
            // A loop body may not nest a second loop: a backend has one backedge to
            // close and no way to close two.
            if let FlagEnd::Loop { header: outer, .. } = end {
                return Err(format!(
                    "a loop whose header is block {} sits inside the level of the loop at block {}; \
                     a backend has no way to close both with one backedge",
                    header.0, outer.0
                ));
            }
            validate_flow(
                body,
                FlagEnd::Loop {
                    header: *header,
                    exit: *exit,
                },
            )
        }
    }
}

impl From<Vec<KernelInstr>> for KernelBody {
    fn from(instrs: Vec<KernelInstr>) -> Self {
        KernelBody::straight_line(instrs)
    }
}
