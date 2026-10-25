//! Several independent fragments that read one shared input become one fragment.
//! See `docs/notes/graph-fusion.md` §5.
//!
//! # Invariant
//! **No branch reads another branch's output, and the shared input is one port**: the branches
//! are bodies that stand beside one another, so each is held once in the merged body and the
//! value they share is read at a single port however many of them read it.

use crate::compose::{
    Renumber, ScalarUnion, branch_targeting, buffer_reads, compact, duplicated_mark,
    first_slice_ordinal, landed_slot, mapped_terminator, position_constant, shapes_differ,
    shifted_past, shifted_write_positions, splice_runtime_scalars,
};
use crate::fan_in::returns_of;
use crate::{
    Barrier, BasicBlock, ComposeRefusal, KernelFragment, KernelMark, KernelRoles, KernelShape,
    ValueDef, ValueId,
};

/// One branch of a sibling merge: the fragment, and the input the shared value reaches.
///
/// # Invariant
/// **The shared ordinal names a buffer the branch reads**: the value is the producer's own
/// output, materialized outside the merge, so a branch that does not read it has no claim on the
/// shared port and the merge would carry a port no body uses.
#[derive(Debug, Clone, Copy)]
pub struct Sibling<'a> {
    /// The fragment to inline.
    pub fragment: &'a KernelFragment,
    /// The input ordinal the shared value reaches.
    pub shared: u32,
}

/// The fragment the branches become, or the decision the merge cannot make.
///
/// # Invariant
/// **The merged inputs are the branches' own ports less the shared one**: the shared value is
/// one port, read by every branch, and every other port is one branch's own.
///
/// **The merged parameter list is the union of the branches' scalar leaves**, each its own, and
/// the merged outputs are **the branches' sinks, in order**.
pub fn compose_siblings(branches: &[Sibling<'_>]) -> Result<KernelFragment, ComposeRefusal> {
    let [head, ..] = branches else {
        return Err(ComposeRefusal::NoBranchToInline);
    };
    if branches.len() < 2 {
        return Err(ComposeRefusal::OneBranchIsNotASiblingSet {
            branches: branches.len(),
        });
    }
    let first = head.fragment;
    if first.input_classes.len() != first.inputs {
        return Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!(
                "the first branch declares {} input(s) and states {} class(es) for them",
                first.inputs,
                first.input_classes.len()
            ),
        });
    }
    // **The union of the branches' ports**, the shared value counted once.
    let ports = branches
        .iter()
        .map(|branch| branch.fragment.inputs)
        .sum::<usize>()
        .saturating_sub(branches.len() - 1);
    // **The union of the branches' domains**, in the order the branches are composed.
    let shapes: Vec<&KernelShape> = branches
        .iter()
        .map(|branch| &branch.fragment.param_shape)
        .collect();
    let Some(scalars) = ScalarUnion::of(&shapes) else {
        return Err(shapes_differ(
            first,
            branches[branches.len() - 1].fragment,
            "one of the domains names no extent and index, so it has no runtime scalar to tell \
             from the index"
                .to_string(),
        ));
    };
    // **Every condition before a body is touched**, with every fragment still alone.
    let mut edges = Vec::with_capacity(branches.len());
    for branch in branches.iter() {
        edges.push(Edge::of(branch, first, ports)?);
    }
    // **Every branch binds its own domain**, or it has no leaf to bind.
    for (index, edge) in edges.iter().enumerate() {
        let fragment = edge.fragment;
        let parameters = fragment.body.parameters().len();
        let leaves = fragment.param_shape.flat_arity();
        if parameters == leaves {
            continue;
        }
        if index == 0 {
            return Err(shapes_differ(
                first,
                branches[branches.len() - 1].fragment,
                format!(
                    "the first branch's entry block receives {parameters} parameter(s) and its \
                     domain declares {leaves} leaf(ies), so it has no runtime scalar to bind"
                ),
            ));
        }
        return Err(ComposeRefusal::ParameterCountsDiffer {
            producer: first.body.parameters().len(),
            consumer: parameters,
            leaves,
        });
    }
    // **Where each branch's surviving inputs land**: the first branch's own stay where they are.
    let mut unions = vec![0; edges.len()];
    let mut union = first.inputs;
    for (index, edge) in edges.iter().enumerate().skip(1) {
        unions[index] = union;
        union = union.saturating_add(edge.fragment.inputs.saturating_sub(1));
    }
    debug_assert_eq!(
        union, ports,
        "the counted union is the ports the fragment declares"
    );
    // **The shared value is one port**: the first branch's own ordinal names it.
    let shared_port = edges[0].shared;

    let mut body = first.body.clone();
    // **A composed body is as exact as the bodies it composes**: it carries their definitions.
    body.exact_float_arithmetic = branches
        .iter()
        .any(|branch| branch.fragment.body.exact_float_arithmetic);
    // **The runtime scalars are spliced into the merged body's own domain.**
    let total = scalars.total();
    splice_runtime_scalars(&mut body, scalars.own(0), total - scalars.own(0));
    let first_parameters = body.blocks[body.entry].params.clone();
    // **Each branch lands on the block the one before it returns from.**
    let mut landing = returns_of(first)?;
    let mut marks: Vec<KernelMark> = first.marks.clone();
    let mut input_classes = first.input_classes.clone();
    let mut output_classes = first.output_classes.clone();
    let mut outputs = first.outputs;
    let mut slices = first.slices.clone();
    let mut blocks_base = body.blocks.len();
    for (index, edge) in edges.iter().enumerate().skip(1) {
        let branch = edge.fragment;
        // **At most one branch stages into slices**: two would claim one slice holds two values.
        if !slices.is_empty() && !branch.slices.is_empty() {
            return Err(ComposeRefusal::SlicesCollide {
                producer: first_slice_ordinal(&first.body),
                consumer: first_slice_ordinal(&branch.body),
            });
        }
        // The branch's values are renumbered past the first branch's.
        let mut renumber = Renumber::default();
        for (slot, &parameter) in branch.body.parameters().iter().enumerate() {
            renumber.bind(
                parameter,
                first_parameters
                    [landed_slot(slot, scalars.own(index), scalars.before(index), total)],
            );
        }
        let landed: Vec<Option<ValueId>> = (0..branch.body.values.len())
            .map(|value| renumber.reserve(ValueId(value as u32), &mut body))
            .collect();
        // **Every surviving read takes the union's position for it**; the shared one takes the
        // shared port.
        let mut shifted = shifted_past(&mut body, &edge.reads, unions[index], &[edge.shared]);
        let shared = position_constant(&mut body, i64::try_from(shared_port).unwrap_or(i64::MAX));
        for read in &edge.reads_shared {
            shifted.insert(*read, shared);
        }
        // **And every write of the branch takes the sum of the outputs before it.**
        let writes = shifted_write_positions(&mut body, &branch.body, outputs);
        let entry = branch.body.entry;
        let base = blocks_base;
        let mapped_block = |block: usize| -> usize {
            if block == entry {
                landing
            } else {
                base + block - usize::from(block > entry)
            }
        };
        let mut sequences: Vec<Vec<ValueId>> = Vec::with_capacity(branch.body.blocks.len());
        for block in branch.body.blocks.iter() {
            let mut sequence: Vec<ValueId> = Vec::with_capacity(block.instrs.len());
            for &value in &block.instrs {
                let Some(&constant) = shifted.get(&value).or_else(|| writes.get(&value)) else {
                    sequence.push(renumber.resolve(value));
                    continue;
                };
                sequence.push(constant);
                sequence.push(renumber.resolve(value));
            }
            sequences.push(sequence);
        }
        // **After the copies**: an operand that read another port now reads the merged one.
        for (value, definition) in branch.body.values.iter().enumerate() {
            let Some(target) = landed[value] else {
                continue;
            };
            body.values[target.0 as usize] = match definition {
                ValueDef::BlockParam { block, index } => ValueDef::BlockParam {
                    block: mapped_block(*block),
                    index: *index,
                },
                ValueDef::Instr { op, args, classes } => {
                    let mut args: Vec<ValueId> =
                        args.iter().map(|&arg| renumber.resolve(arg)).collect();
                    if let Some(&constant) = shifted
                        .get(&ValueId(value as u32))
                        .or_else(|| writes.get(&ValueId(value as u32)))
                        && let Some(first) = args.first_mut()
                    {
                        *first = constant;
                    }
                    ValueDef::Instr {
                        op: *op,
                        args,
                        classes: classes.clone(),
                    }
                }
            };
        }
        let resolve = |value: ValueId| renumber.resolve(value);
        for (block_index, block) in branch.body.blocks.iter().enumerate() {
            if block_index == entry {
                continue;
            }
            body.blocks.push(BasicBlock {
                params: block.params.iter().map(|&param| resolve(param)).collect(),
                instrs: std::mem::take(&mut sequences[block_index]),
                terminator: mapped_terminator(&block.terminator, &resolve, &mapped_block),
            });
        }
        // **Each branch lands where the branch before it returns from** and takes over the
        // terminator of that block.
        body.blocks[landing]
            .instrs
            .extend(std::mem::take(&mut sequences[entry]));
        body.blocks[landing].terminator = mapped_terminator(
            &branch.body.blocks[entry].terminator,
            &resolve,
            &mapped_block,
        );
        for expr in &branch.body.barriers {
            body.barriers.push(Barrier {
                write: resolve(expr.write),
                read: resolve(expr.read),
            });
        }
        input_classes.extend(
            branch
                .input_classes
                .iter()
                .copied()
                .enumerate()
                .filter(|(ordinal, _)| *ordinal != edge.shared)
                .map(|(_, class)| class),
        );
        output_classes.extend(branch.output_classes.iter().copied());
        outputs += branch.outputs;
        if slices.is_empty() {
            slices.clone_from(&branch.slices);
        }
        blocks_base += branch.body.blocks.len() - 1;
        landing = mapped_block(returns_of(branch)?);
    }

    compact(&mut body, &mut marks);
    let fragment = KernelFragment {
        param_shape: scalars.shape(),
        roles: KernelRoles::default(),
        body,
        inputs: ports,
        outputs,
        input_classes,
        output_classes,
        result_classes: edges[edges.len() - 1].fragment.result_classes.clone(),
        target: first.target,
        slices,
        marks,
    };
    if let Err(cause) = fragment.body.validate() {
        return Err(ComposeRefusal::MergedBodyIsRefused { cause });
    }
    if let Err(cause) = fragment.validate() {
        return Err(ComposeRefusal::MergedFragmentIsRefused { cause });
    }
    Ok(fragment)
}

/// One branch of a sibling merge: what it reads, what it writes, and what it drops.
///
/// # Invariant
/// **Every condition is checked before a body is touched**: the shared ordinal is resolved
/// against the branch's own inputs, the shape and the parameter list are asked of each branch,
/// and the shared port is proved read — so a branch this composition cannot serve is refused
/// while every fragment still stands alone.
struct Edge<'a> {
    fragment: &'a KernelFragment,
    /// The input ordinal the shared value reaches.
    shared: usize,
    /// Every buffer read of the body, with the position it addresses.
    reads: Vec<(ValueId, u32)>,
    /// The reads that fill the shared slot.
    reads_shared: Vec<ValueId>,
}

impl<'a> Edge<'a> {
    /// Everything one branch must answer before the merged body is built.
    fn of(
        branch: &'a Sibling<'a>,
        first: &KernelFragment,
        ports: usize,
    ) -> Result<Edge<'a>, ComposeRefusal> {
        let fragment = branch.fragment;
        if let Err(cause) = fragment.body.validate() {
            return Err(ComposeRefusal::ConsumerIsNotWellFormed { cause });
        }
        if first.target != fragment.target {
            return Err(ComposeRefusal::TargetsDiffer {
                producer: first.target,
                consumer: fragment.target,
            });
        }
        if fragment.input_classes.len() != fragment.inputs {
            return Err(ComposeRefusal::ConsumerIsNotWellFormed {
                cause: format!(
                    "the branch declares {} input(s) and states {} class(es) for them",
                    fragment.inputs,
                    fragment.input_classes.len()
                ),
            });
        }
        duplicated_mark(&fragment.marks)?;
        if let Some(block) = branch_targeting(&fragment.body, fragment.body.entry) {
            return Err(ComposeRefusal::ConsumerEntryIsABranchTarget { block });
        }
        // **The block the next branch lands on**, so a branch must return from exactly one.
        returns_of(fragment)?;
        let shared = branch.shared as usize;
        if shared >= fragment.inputs {
            return Err(ComposeRefusal::AbsorbedIsOutOfRange {
                absorbed: branch.shared,
                inputs: fragment.inputs,
            });
        }
        let reads = buffer_reads(&fragment.body, "consumer")?;
        // Only there do two bodies' ordinals disagree about which buffer a position names.
        if let Some(position) = reads
            .iter()
            .map(|(_, position)| *position)
            .find(|position| *position as usize >= fragment.inputs)
        {
            return Err(ComposeRefusal::ProducerReadsOutsideTheMerge {
                position,
                inputs: ports,
            });
        }
        let reads_shared: Vec<ValueId> = reads
            .iter()
            .filter(|(_, position)| *position as usize == shared)
            .map(|(value, _)| *value)
            .collect();
        if reads_shared.is_empty() {
            return Err(ComposeRefusal::AbsorbedIsNotRead {
                absorbed: branch.shared,
            });
        }
        Ok(Edge {
            fragment,
            shared,
            reads,
            reads_shared,
        })
    }
}
