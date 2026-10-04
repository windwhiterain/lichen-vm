//! Structured control flow: [`Flow`] and [`Terminator`] onto `waffle` blocks.
//!
//! **This file owns the stack's shape, and the instruction map owns the stack's
//! contents.** What a body *transfers* is a count, never a slot index
//! (`lichen_kernel_ir::body`), so the two questions a transfer raises are "how
//! many values" and "which block" — and `waffle`'s blockparams are the second
//! half of the first: a carried value is a blockparam, and an edge hands it over
//! as `BlockTarget::args`. There is no `OpPhi` to place and no local to allocate
//! by hand (`docs/notes/wasm-control-flow.md` §5).
//!
//! # The three shapes
//!
//! - [`Terminator::If`] is a `CondBr` to two arm blocks, each of which runs on the
//!   stack the branch inherited and arrives at a **join block whose parameters are
//!   the `passes` values**. An absent arm branches straight to the join with the
//!   stack as it stood, which is what makes a one-armed branch expressible without
//!   inventing a value — and therefore a one-armed branch passes **zero** values.
//! - [`Terminator::Jump`] and [`Flow::Jump`] are a `Br` whose `args` are the top
//!   `passes` values.
//! - [`Terminator::While`] is the spike's shape exactly
//!   (`crates/lichen-compute/src/waffle_spike.rs`): the carried tuple's
//!   blockparams are reserved on a header block **first**, the header's
//!   instructions run with those blockparams as their starting stack, and the test
//!   is a `CondBr` whose true edge is the exit — handed the top `passed_out` values
//!   of the header's own tuple — and whose false edge is the body, whose last act
//!   is a `Br` back to the header with the next state.
//!
//! What the three share is that **a label is either a block that exists or a join
//! a branch creates**. A branch out of a loop names the header or the exit, which
//! are the loop's own blocks and exist before its body is lowered; a branch to any
//! other label is a selection's join, which the branch defines by arriving. A
//! loop's state therefore never lives in the entry block — `FunctionBody::new`
//! builds the entry block's blockparams from the function signature and they may
//! not be added to, so a loop needs a block of its own, entered through a
//! preheader (`docs/notes/wasm-backend-handoff.md` §3.2).
//!
//! # What is refused, and why refusing is the only honest answer
//!
//! `waffle` computes every block's type — a loop frame is `[] -> []` because a
//! backedge carries no operands, and a branch *out* is the enclosing block's
//! (`docs/notes/wasm-control-flow.md` §1) — so a shape this file cannot express
//! has to be named rather than half-emitted. A silently dropped transfer is a body
//! that computes a different program.
//!
//! **The header must leave the tuple and then the condition.** That is the IR's
//! own contract (`lichen_kernel_ir::body::Terminator::While`), and it is checked
//! here rather than trusted: both counts are read off the stack *after* the
//! condition is popped, so a header that leaves anything else has no defined
//! exit.

use std::collections::HashMap;

use lichen_kernel_ir::{BlockId, Flow, KernelFragment, KernelId, KernelInstr, Terminator};
use waffle::{
    Block, BlockTarget, FunctionBody, Module, Signature, Terminator as WasmTerminator, Type, Value,
};

use super::assemble::BufferImports;
use super::lower::{Lower, ModuleCtx, lower_instr};
use crate::compute::param_classes;

/// The type of one carried value.
///
/// **The IR states a count, not a type**, because what a transfer hands over is a
/// count (`lichen_kernel_ir::body`): the values are on the stack, and only a
/// body's own instructions say what they hold. A loop's state is the tuple that
/// flows round its backedge, so its header's blockparams have to be typed when the
/// header is created, before any instruction has run. **An `Int`** is what every
/// scalar kernel's state is and what the acceptance case — a reduction with a
/// counter — carries; a loop whose state is a float is refused by name rather than
/// typed wrong.
const CARRIED: Type = Type::I64;

/// One value a body has left on its operand stack: the `waffle` value, and the
/// wasm type it holds.
///
/// **The type is what makes a crossing decidable**, and it is not the same
/// question as the class: a comparison yields the language's `0`/`1` scalar, which
/// is an `i64` in either class, and a narrowing turns a `0`/`1` scalar into an
/// `i32`. Both are facts about the value in hand, and both are what the next
/// instruction has to be told.
#[derive(Debug, Clone, Copy)]
pub(super) struct Slot {
    pub(super) value: Value,
    pub(super) ty: Type,
}

/// A block a branch arrives at with `expecting` values, and what is known about
/// it.
struct Join {
    label: BlockId,
    /// How many values a branch to it carries — the count every arrival is checked
    /// against.
    expecting: usize,
    /// Whether the enclosing `If` may leave this join unreached. A two-armed
    /// branch has an arm per side and neither need name the join; a one-armed one
    /// has nothing else, so its absent side arrives at the join.
    allow_unreached: bool,
    /// How many arms have arrived here.
    visited: usize,
}

impl Join {
    fn new(label: BlockId, expecting: usize) -> Self {
        Join {
            label,
            expecting,
            allow_unreached: false,
            visited: 0,
        }
    }
}

/// The top `count` slots of `stack` as a target's arguments, in the order a branch
/// hands them over.
fn args_into(stack: &[Slot], count: usize) -> Vec<Value> {
    stack[stack.len() - count..]
        .iter()
        .map(|slot| slot.value)
        .collect()
}

/// Which label each transfer resolves against, and the blocks that exist.
///
/// **The stack's shape is carried here rather than in a height integer**
/// (`docs/notes/wasm-control-flow.md` §2): a join knows how many values it
/// receives, so a branch that carries the wrong number is refused where it is
/// lowered, not at an assertion that has already lost the value it was about.
pub(super) struct Resolver {
    /// Every label that has a block: the fragment's loop header, each join a
    /// branch has created, and each loop exit.
    blocks: HashMap<BlockId, Block>,
    /// The joins of each open selection level, outermost first.
    joins: Vec<Join>,
    /// The open levels, innermost last, as indices into `joins`.
    levels: Vec<usize>,
}

impl Resolver {
    fn new() -> Self {
        Resolver {
            blocks: HashMap::new(),
            joins: Vec::new(),
            levels: Vec::new(),
        }
    }

    /// Open a selection level, whose arms arrive at `join`.
    fn push_join(&mut self, join: Join) -> usize {
        let level = self.joins.len();
        self.joins.push(join);
        self.levels.push(level);
        level
    }

    /// Close the innermost level after both arms have been lowered.
    fn pop_join(&mut self) {
        self.levels.pop();
    }

    /// The number of values a branch to `label` carries, or [`None`] if nothing
    /// has had to place it.
    fn expecting(&self, label: BlockId) -> Option<usize> {
        self.joins
            .iter()
            .rfind(|join| join.label == label)
            .map(|join| join.expecting)
    }

    /// The block a label is, if it has one.
    fn block_of(&self, label: BlockId) -> Option<Block> {
        self.blocks.get(&label).copied()
    }
}

/// The stack a flow starts from, and everything known about it.
///
/// **`block` is always the block the flow's own instructions run in**, and
/// `entry_stack` is the stack they start from, so the terminator has both without
/// asking again.
#[derive(Clone, Copy)]
struct Base<'a, 'b> {
    block: Block,
    /// The stack the block's instructions start from. **A block's stack is its
    /// blockparams' values**, so an arm's inherited stack and a loop header's
    /// carried tuple are the same kind of thing.
    entry_stack: &'a [Slot],
    /// The loop header a branch back to may name, or [`None`] outside a loop.
    header: Option<BlockId>,
    /// How many values that header carries, whose types are [`CARRIED`].
    carried: usize,
    /// What this stack is, so a refusal names it rather than calling it anonymous.
    what: &'b str,
}

/// The lowering of one fragment: the module being built, the fragment's own
/// signature, the label table, and the stack-shape state a transfer resolves
/// against.
pub(super) struct FlowCtx<'a, 'b> {
    builder: &'a mut FunctionBody,
    context: &'a ModuleCtx<'b>,
    resolver: Resolver,
    /// The function's results, which a `Return` hands over.
    returns: usize,
}

/// The one function a fragment becomes.
///
/// **The preheader is the function's entry block**: a `Flow::Block` whose `entry`
/// names a label is the loop's own header, and a block a backedge re-enters can
/// only be reached by a branch, which is what a preheader is
/// (`docs/notes/wasm-backend-handoff.md` §3.2).
pub(super) fn lower_fragment(
    fragment: &KernelFragment,
    callees: &[KernelFragment],
    index: &HashMap<KernelId, u32>,
    module: &Module,
    signature: Signature,
    imports: &BufferImports,
) -> Result<FunctionBody, String> {
    fragment
        .body
        .validate()
        .map_err(|broken| format!("compute.wasm: the kernel body is malformed: {broken}"))?;
    let Flow::Block {
        entry,
        instrs,
        terminator,
    } = &fragment.body.entry
    else {
        return Err(
            "compute.wasm: a kernel body must begin with a block, and a `Seq` is a loop's own body \
             rather than a fragment's"
                .to_string(),
        );
    };

    let mut builder = FunctionBody::new(module, signature);
    let function_entry = builder.entry;
    // The entry block's blockparams **are** the function's parameters.
    let function_params: Vec<Value> = builder.blocks[function_entry]
        .params
        .iter()
        .map(|(_, value)| *value)
        .collect();
    let context = ModuleCtx {
        callees,
        index,
        imports,
        leaves: &param_classes(fragment),
        params: &function_params,
    };
    let mut resolver = Resolver::new();

    // **The header is a block of the loop's own**, and the entry block stays the
    // preheader. A labelled entry block *is* the header's label, so the block that
    // holds the loop's instructions is created here, with its carried tuple as
    // blockparams — the only way the state can live anywhere, since
    // `FunctionBody::new` fixes the entry block's blockparams to the function's
    // parameters and nothing may add to them.
    let (block, entry_stack, entry_what, loop_header) = match entry {
        None => (
            function_entry,
            Vec::new(),
            "the fragment's entry block".to_string(),
            None,
        ),
        Some(label) => {
            let Terminator::While {
                carried,
                passed_out,
                ..
            } = &**terminator
            else {
                return Err(format!(
                    "compute.wasm: the fragment's own block is labelled {}, so that block *is* a \
                     loop's header — and this body's transfer is not a `While`, so nothing names the \
                     loop it heads",
                    label.0
                ));
            };
            if *passed_out != fragment.result_classes.len() {
                return Err(format!(
                    "compute.wasm: the loop at block {} hands {passed_out} value(s) to its exit, but \
                     the fragment returns {} — the exit's values are the loop's result",
                    label.0,
                    fragment.result_classes.len()
                ));
            }
            let header = builder.add_block();
            resolver.blocks.insert(*label, header);
            // The header's blockparams **are** the carried tuple: a branch into the
            // header hands them over, and the header's instructions start from
            // them. The preheader hands over the fragment's own arguments — one
            // value per carried slot, which is what the loop's state starts as.
            let state: Vec<Slot> = (0..*carried)
                .map(|_| Slot {
                    value: builder.add_blockparam(header, CARRIED),
                    ty: CARRIED,
                })
                .collect();
            let initial: Vec<Value> = if *carried <= function_params.len() {
                function_params[..*carried].to_vec()
            } else {
                return Err(format!(
                    "compute.wasm: the loop at block {} carries {carried} value(s), and the fragment's \
                     signature has only {} parameter(s) to start them from",
                    label.0,
                    function_params.len()
                ));
            };
            builder.set_terminator(
                function_entry,
                WasmTerminator::Br {
                    target: BlockTarget {
                        block: header,
                        args: initial,
                    },
                },
            );
            let what = format!("the loop at block {}, entered at its header", label.0);
            (header, state, what, Some((*label, *carried)))
        }
    };

    let mut ctx = FlowCtx {
        builder: &mut builder,
        context: &context,
        resolver,
        returns: fragment.result_classes.len(),
    };
    let base = Base {
        block,
        entry_stack: &entry_stack,
        header: loop_header.map(|(label, _)| label),
        carried: loop_header.map_or(0, |(_, carried)| carried),
        what: &entry_what,
    };
    let params = [base];
    lower_block(&mut ctx, instrs, terminator, &params)?;
    // A block this lowering created and no transfer reaches is a block `waffle`
    // would emit an empty frame for; naming it here reports the label rather than
    // leaving it to the type checker as a block with no terminator.
    let unreached = ctx
        .builder
        .blocks
        .entries()
        .filter(|(_, definition)| definition.terminator == WasmTerminator::None)
        .map(|(block, _)| format!("{block}"))
        .collect::<Vec<_>>();
    if let Some(at) = unreached.first() {
        return Err(format!(
            "compute.wasm: block {at} was created and no transfer arrives at it, so it has no \
             terminator; a label a body names has to be one a transfer reaches"
        ));
    }
    Ok(builder)
}

/// Whether a flow inside a loop reached the loop's header.
///
/// **This is the one fact the enclosing loop cannot read off the stack.** The
/// header's state runs round the backedge and the body branches to the exit from
/// the same level, so which of the two a body's last act was is only known at the
/// transfer that takes it — and a body that never arrives at the header has no
/// backedge at all. [`Backedge::Taken`] is the greater of the two, so a branch
/// whose arms disagree is a backedge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Backedge {
    NotTaken,
    Taken,
}

/// Lower one flow and return the stack it leaves behind, and whether it reached
/// the loop's header.
///
/// **`params.last()`'s base is the whole of the flow's context**: which block its
/// instructions run in, what stack they start from, and which loop it is inside.
/// The recursion is what an arm, a block and a loop body each add to that.
fn lower_flow(
    ctx: &mut FlowCtx<'_, '_>,
    flow: &Flow,
    params: &[Base<'_, '_>],
) -> Result<(Vec<Slot>, Backedge), String> {
    match flow {
        // A plain transfer is its whole content: there is no block for it, and
        // the branch that takes it is the flow.
        Flow::Jump { target, passes } => {
            let mut stack = params.last().ok_or_else(no_stack)?.entry_stack.to_vec();
            branch(ctx, params, &mut stack, *target, *passes)?;
            let back = if params.last().and_then(|base| base.header) == Some(*target) {
                Backedge::Taken
            } else {
                Backedge::NotTaken
            };
            Ok((Vec::new(), back))
        }
        Flow::Seq {
            instrs, terminator, ..
        } => lower_block(ctx, instrs, terminator, params),
        Flow::Block {
            entry: label,
            instrs,
            terminator,
        } => {
            // **A block's own entry is its label**, and the label of a block that
            // runs here is the loop that label heads: the lowering is already
            // inside its header, and the block's stack is that header's tuple. Any
            // other label would be a second arrival point for a block that already
            // has one.
            if let Some(label) = label {
                let base = params.last().ok_or_else(no_stack)?;
                match ctx.resolver.block_of(*label) {
                    Some(block) if block == base.block => {}
                    Some(_) => {
                        return Err(format!(
                            "compute.wasm: a flow is entered at block {} but is being lowered into \
                             another block; a label names one arrival point, and a loop's header is \
                             the block that carries its state",
                            label.0
                        ));
                    }
                    None => {
                        return Err(format!(
                            "compute.wasm: a flow is entered at block {}, which is not a loop's \
                             header — a block that is neither a label something defines nor a loop's \
                             header has no carried state to start from",
                            label.0
                        ));
                    }
                }
            }
            lower_block(ctx, instrs, terminator, params)
        }
    }
}

/// Lower a run of instructions and the transfer that follows them.
fn lower_block(
    ctx: &mut FlowCtx<'_, '_>,
    instrs: &[KernelInstr],
    terminator: &Terminator,
    params: &[Base<'_, '_>],
) -> Result<(Vec<Slot>, Backedge), String> {
    let base = *params.last().ok_or_else(no_stack)?;
    // A block's stack starts as its parameters' values, which is why a loop
    // header's carried tuple and an arm's inherited stack are the same shape.
    let mut stack = base.entry_stack.to_vec();
    for instruction in instrs {
        let lower = Lower {
            builder: ctx.builder,
            context: ctx.context,
            block: base.block,
            stack: &mut stack,
        };
        lower_instr(lower, *instruction)?;
    }
    lower_terminator(ctx, terminator, params, base.block, &mut stack)
}

/// Lower the transfer a run of instructions ends in, returning the stack left
/// behind and whether it was a backedge to the loop's header.
fn lower_terminator(
    ctx: &mut FlowCtx<'_, '_>,
    terminator: &Terminator,
    params: &[Base<'_, '_>],
    block: Block,
    stack: &mut Vec<Slot>,
) -> Result<(Vec<Slot>, Backedge), String> {
    let base = *params.last().ok_or_else(no_stack)?;
    match terminator {
        Terminator::Return => {
            let values = return_values(stack, ctx.returns, base.what)?;
            if let Some(rest) = stack.last() {
                return Err(format!(
                    "compute.wasm: the body leaves a {rest:?} value below the values it returns, and \
                     nothing consumes it"
                ));
            }
            ctx.builder.set_terminator(
                block,
                WasmTerminator::Return {
                    values: values.iter().map(|slot| slot.value).collect(),
                },
            );
            Ok((Vec::new(), Backedge::NotTaken))
        }
        // A plain transfer: the top `passes` values are the label's parameters.
        Terminator::Jump { target, passes } => {
            branch(ctx, params, stack, *target, *passes)?;
            let back = if base.header == Some(*target) {
                Backedge::Taken
            } else {
                Backedge::NotTaken
            };
            Ok((Vec::new(), back))
        }
        Terminator::If {
            on_one,
            on_zero,
            join,
            passes,
        } => {
            let (stack, back) = lower_if(
                ctx,
                params,
                block,
                stack,
                on_one,
                on_zero.as_deref(),
                *join,
                *passes,
            )?;
            Ok((stack, back))
        }
        Terminator::While {
            header,
            body,
            exit,
            carried,
            passed_out,
        } => lower_while(
            ctx,
            params,
            block,
            stack,
            *header,
            body,
            *exit,
            *carried,
            *passed_out,
        ),
    }
}

/// Lower a two-way branch: a `CondBr` to two arm blocks, both arriving at the
/// join.
#[allow(clippy::too_many_arguments)]
fn lower_if(
    ctx: &mut FlowCtx<'_, '_>,
    params: &[Base<'_, '_>],
    block: Block,
    stack: &mut Vec<Slot>,
    on_one: &Flow,
    on_zero: Option<&Flow>,
    join: BlockId,
    passes: usize,
) -> Result<(Vec<Slot>, Backedge), String> {
    let base = *params.last().ok_or_else(no_stack)?;
    let selector = pop_selector(stack)?;
    if on_zero.is_none() && passes != 0 {
        // The absent arm arrives with the stack as it stood, so it carries
        // nothing: a join with parameters would have no source on that path.
        return Err(format!(
            "compute.wasm: the one-armed branch joining at block {} passes {passes} value(s), and its \
             absent side has none to pass — an absent arm arrives with the stack as it stood \
             (`lichen_kernel_ir::body::Terminator::If`)",
            join.0
        ));
    }
    // **The two arms are two blocks**, and the `CondBr` hands each of them the
    // stack the branch inherited: the selector is consumed by the branch, so what
    // an arm starts from is what is below it.
    let one = ctx.builder.add_block();
    let zero = ctx.builder.add_block();
    let inherited: Vec<Value> = stack.iter().map(|slot| slot.value).collect();
    ctx.builder.set_terminator(
        block,
        WasmTerminator::CondBr {
            cond: selector,
            if_true: BlockTarget {
                block: one,
                args: inherited.clone(),
            },
            if_false: BlockTarget {
                block: zero,
                args: inherited,
            },
        },
    );

    let mut joining = Join::new(join, stack.len() + passes);
    joining.allow_unreached = on_zero.is_none();
    let level = ctx.resolver.push_join(joining);
    let one_back = arm(ctx, base, one, stack, level, on_one)?;
    let mut back = one_back;
    match on_zero {
        Some(on_zero) => {
            back = back.max(arm(ctx, base, zero, stack, level, on_zero)?);
        }
        None => {
            // **The absent side is a branch straight to the join** with the stack
            // as it stood, which is the whole reason a one-armed branch is
            // expressible without inventing a value.
            let landmark = ctx.resolver.joins[level].label;
            let expect = ctx.resolver.joins[level].expecting;
            let args = args_into(&stack, expect);
            let block = ctx.resolver.block_of(landmark).ok_or_else(|| {
                format!(
                    "compute.wasm: the one-armed branch joining at block {} had no arm arrive there, \
                     so there is no block for its absent side to branch to",
                    landmark.0
                )
            })?;
            ctx.builder.set_terminator(
                zero,
                WasmTerminator::Br {
                    target: BlockTarget { block, args },
                },
            );
        }
    }
    // The join's parameters are the stack the arms left, so the enclosing flow
    // continues from the join with that stack. A join no arm arrived at is a block
    // nothing defines: both arms left the loop instead, which is a transfer this
    // lowering has no block for.
    let landing = ctx
        .resolver
        .block_of(join)
        .ok_or_else(|| join_missing(join))?;
    ctx.resolver.pop_join();
    let joined = ctx.builder.blocks[landing]
        .params
        .iter()
        .map(|(ty, value)| Slot {
            value: *value,
            ty: *ty,
        })
        .collect();
    Ok((joined, back))
}

fn join_missing(join: BlockId) -> String {
    format!(
        "compute.wasm: neither arm of the branch joining at block {} arrives there, so it is a block \
         nothing defines; an arm either arrives at its own join or leaves the loop",
        join.0
    )
}

/// Lower one arm of the branch at `level`.
///
/// **An arm is a block of its own**, started on the stack the branch inherited:
/// the `CondBr` handed it over as this block's `BlockTarget::args`, and the base
/// it is lowered from says so.
fn arm(
    ctx: &mut FlowCtx<'_, '_>,
    base: Base<'_, '_>,
    block: Block,
    stack: &[Slot],
    level: usize,
    flow: &Flow,
) -> Result<Backedge, String> {
    ctx.resolver.joins[level].visited += 1;
    let arm_params = [Base {
        block,
        entry_stack: stack,
        ..base
    }];
    let (_, back) = lower_flow(ctx, flow, &arm_params)?;
    Ok(back)
}

/// Lower a loop: the header's carried tuple as blockparams, the test as a
/// `CondBr`, and the body's last act as the backedge.
#[allow(clippy::too_many_arguments)]
fn lower_while(
    ctx: &mut FlowCtx<'_, '_>,
    params: &[Base<'_, '_>],
    block: Block,
    stack: &mut Vec<Slot>,
    header: BlockId,
    body: &Flow,
    exit: BlockId,
    carried: usize,
    passed_out: usize,
) -> Result<(Vec<Slot>, Backedge), String> {
    let base = *params.last().ok_or_else(no_stack)?;
    // **The header is reserved first**: its carried tuple's blockparams exist
    // before any of its instructions are lowered, so the test reads the state the
    // backedge will replace, and the body can branch back at a label that is
    // already a block.
    let header_block = ctx.resolver.block_of(header).ok_or_else(|| {
        format!(
            "compute.wasm: a loop's header is block {}, which nothing defines; a loop's state is that \
             block's parameters, so the block has to exist before the loop is entered",
            header.0
        )
    })?;
    // **The test runs in the header's own block**: the loop's condition is the
    // header's code, so the `CondBr` is that block's terminator and the header is
    // where every entry — the preheader's and the backedge's — arrives.
    if block != header_block {
        return Err(format!(
            "compute.wasm: a loop tests at block {}, where its header is block {}; the test is the \
             header's own code, and it runs on every entry (`lichen_kernel_ir::body::Flow::Block`)",
            header.0, base.block
        ));
    }
    if stack.len() < carried {
        return Err(format!(
            "compute.wasm: a loop carries {carried} value(s) but only {} are on the stack when it is \
             entered, so its state has no source",
            stack.len()
        ));
    }

    // The exit's values are **the header's own** top `passed_out`, which is what
    // gives a zero-trip loop a defined result
    // (`lichen_kernel_ir::body::Terminator::While`). The exit is the loop's
    // fall-through, so it is created before the body: a body that branches straight
    // out of the loop finds its block already there.
    let exiting = match ctx.resolver.block_of(exit) {
        Some(block) => block,
        None => {
            let block = ctx.builder.add_block();
            let values: Vec<Value> = (0..passed_out)
                .map(|_| ctx.builder.add_blockparam(block, CARRIED))
                .collect();
            ctx.builder.set_terminator(
                block,
                WasmTerminator::Return {
                    values: values.clone(),
                },
            );
            let mut joining = Join::new(exit, passed_out);
            joining.allow_unreached = true;
            ctx.resolver.blocks.insert(exit, block);
            ctx.resolver.push_join(joining);
            block
        }
    };

    // **The header leaves the tuple and then the condition.** Its instructions
    // start from the header's blockparams — the carried tuple — and the condition
    // ends on top of them, so the stack at the `CondBr` is `state(carried),
    // condition` and nothing else. That is the contract
    // `lichen_kernel_ir::body::Terminator::While` states, and both of the loop's
    // counts are read off this one stack: the exit takes the top `passed_out` of
    // it, and the body inherits all of it.
    let state_height = base.entry_stack.len();
    if stack.len() != state_height + 1 {
        return Err(format!(
            "compute.wasm: the header of the loop at block {} reaches its test with {} value(s) on the \
             stack, where its state is {state_height} and the condition ends above it; the header must \
             leave the tuple and then the condition \
             (`lichen_kernel_ir::body::Terminator::While`)",
            header.0,
            stack.len()
        ));
    }
    let selector = pop_selector(stack)?;
    if stack.len() < passed_out {
        return Err(format!(
            "compute.wasm: a loop hands {passed_out} value(s) to its exit but its state leaves only {} \
             on the stack after the condition is popped",
            stack.len()
        ));
    }
    // **The two edges read the same stack.** The exit takes the top `passed_out`
    // of the header's own values; the body takes the whole tuple, which is the
    // next iteration's state until the body replaces it. Both are read here and
    // neither consumes the other's values.
    let state: Vec<Slot> = stack.clone();
    let passed: Vec<Slot> = state[state.len() - passed_out..].to_vec();
    if passed.iter().any(|slot| slot.ty != CARRIED) {
        return Err(format!(
            "compute.wasm: the loop at block {} hands its exit a value that is not {CARRIED}; this \
             lowering types a loop's carried state as one {CARRIED} value per slot",
            header.0
        ));
    }
    ctx.builder.set_terminator(
        block,
        WasmTerminator::CondBr {
            cond: selector,
            if_true: BlockTarget {
                block: exiting,
                args: passed.iter().map(|slot| slot.value).collect(),
            },
            if_false: BlockTarget {
                block: header_block,
                args: state.iter().map(|slot| slot.value).collect(),
            },
        },
    );

    // The body runs with the header's tuple as its starting stack, so the stack it
    // leaves at the backedge carries that tuple again — the next state.
    let body_block = ctx.builder.add_block();
    let body_base = Base {
        block: body_block,
        entry_stack: &state,
        header: Some(header),
        carried,
        what: base.what,
    };
    let body_params = [body_base];
    let (_, back) = lower_flow(ctx, body, &body_params)?;
    if back == Backedge::NotTaken {
        // **The backedge is the loop.** A body that never arrives at the header
        // runs at most once and hands the header's own state back, so it is a
        // `While` whose loop has no way to continue — and `waffle` has no `br` to
        // the header to emit.
        return Err(format!(
            "compute.wasm: the body of the loop at block {} never arrives back at its header, so the \
             loop has no backedge; a loop body ends by handing the next state over \
             (`lichen_kernel_ir::body::Terminator::Jump`)",
            header.0
        ));
    }
    ctx.resolver.joins.pop();
    // The loop is the whole fragment's last act, so the enclosing flow continues
    // from the stack it started on rather than from the header's: the carried tuple
    // is the header's blockparams, and what is under it is the enclosing flow's own
    // values.
    let outer_height = base.entry_stack.len().saturating_sub(base.carried);
    stack.truncate(outer_height);
    Ok((Vec::new(), Backedge::NotTaken))
}

/// A branch: the top `passes` values of `stack` are the label's parameters.
fn branch(
    ctx: &mut FlowCtx<'_, '_>,
    params: &[Base<'_, '_>],
    stack: &mut Vec<Slot>,
    target: BlockId,
    passes: usize,
) -> Result<(), String> {
    let base = *params.last().ok_or_else(no_stack)?;
    if stack.len() < passes {
        return Err(format!(
            "compute.wasm: a transfer to block {} carries {passes} value(s), and {} are on the stack \
             when it is taken",
            target.0,
            stack.len()
        ));
    }
    if let Some(expecting) = ctx.resolver.expecting(target)
        && expecting != passes
    {
        return Err(format!(
            "compute.wasm: a branch to block {} carries {passes} value(s), but the block receives \
             {expecting}; a join's parameters are its arrivals' values, and one of them disagrees",
            target.0
        ));
    }
    // A block that exists is the loop's header, the loop's exit, or a join an
    // earlier arm created; a label nothing has defined is a join the branch makes.
    let args = args_into(stack, passes);
    let block = match ctx.resolver.block_of(target) {
        Some(block) => block,
        None => materialize_join(ctx, target, passes, stack)?,
    };
    ctx.builder.set_terminator(
        base.block,
        WasmTerminator::Br {
            target: BlockTarget { block, args },
        },
    );
    Ok(())
}

/// Create the block a join is, with the values the arriving branch hands over as
/// its blockparams.
///
/// **The join's parameters are typed by the values that arrive**, not by a count:
/// a join receives a loop's, a comparison's or a computed value, and the branch
/// that names it is the only place the type is in hand.
fn materialize_join(
    ctx: &mut FlowCtx<'_, '_>,
    label: BlockId,
    passes: usize,
    stack: &[Slot],
) -> Result<Block, String> {
    let block = ctx.builder.add_block();
    for slot in &stack[stack.len() - passes..] {
        ctx.builder.add_blockparam(block, slot.ty);
    }
    ctx.resolver.blocks.insert(label, block);
    Ok(block)
}

fn pop_selector(stack: &mut Vec<Slot>) -> Result<Value, String> {
    let selector = stack.pop().ok_or_else(|| {
        "compute.wasm: a branch reads its condition from the top of the stack, and the stack is empty \
         — the body's operands are not the ones the branch consumes"
            .to_string()
    })?;
    if selector.ty != Type::I32 {
        return Err(format!(
            "compute.wasm: a branch's condition is a {} value; the language's comparison yields an i64 \
             0/1 scalar, and the narrowing to wasm's i32 is `KernelInstr::I32WrapI64`, which this body \
             did not apply",
            selector.ty
        ));
    }
    Ok(selector.value)
}

/// The top `results` slots, in order, and the stack without them.
fn return_values(stack: &mut Vec<Slot>, results: usize, what: &str) -> Result<Vec<Slot>, String> {
    if stack.len() < results {
        return Err(format!(
            "compute.wasm: the body returns {results} value(s) but {what} leaves only {} on the stack",
            stack.len()
        ));
    }
    Ok(stack.split_off(stack.len() - results))
}

fn no_stack() -> String {
    "compute.wasm: a flow is lowered with no stack".to_string()
}
