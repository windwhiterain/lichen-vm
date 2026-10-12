//! Two fragments in a chain become one.
//! See `docs/notes/graph-fusion.md` §4.

use std::collections::{HashMap, HashSet};

use crate::{
    Barrier, BasicBlock, Br, KernelBody, KernelFragment, KernelInstr, KernelMark, KernelRoles,
    KernelShape, ScalarClass, TargetFacts, Terminator, ValueDef, ValueId,
};

/// The fragment `producer` and `consumer` become, or the decision the merge cannot make.
///
/// # Invariant
/// Values are renumbered past the producer's, its store and marks go, and the consumer's entry block
/// is inlined into the producer's return block. `compose` never sees the graph, so a caller whose
/// producer reads and surviving consumer inputs are not the same buffers in the same order gets a
/// wrong answer, not a refusal; the merge pass's `PortsDoNotCorrespond` is that check.
pub fn compose(
    producer: &KernelFragment,
    consumer: &KernelFragment,
    absorbed: u32,
) -> Result<KernelFragment, ComposeRefusal> {
    if let Err(cause) = producer.body.validate() {
        return Err(ComposeRefusal::ProducerIsNotWellFormed { cause });
    }
    if let Err(cause) = consumer.body.validate() {
        return Err(ComposeRefusal::ConsumerIsNotWellFormed { cause });
    }
    if producer.param_shape != consumer.param_shape {
        return Err(ComposeRefusal::ShapesDiffer {
            producer: producer.param_shape.clone(),
            consumer: consumer.param_shape.clone(),
        });
    }
    if producer.target != consumer.target {
        return Err(ComposeRefusal::TargetsDiffer {
            producer: producer.target,
            consumer: consumer.target,
        });
    }
    let absorbed_at = absorbed as usize;
    if absorbed_at >= consumer.inputs {
        return Err(ComposeRefusal::AbsorbedIsOutOfRange {
            absorbed,
            inputs: consumer.inputs,
        });
    }
    if consumer.input_classes.len() != consumer.inputs {
        return Err(ComposeRefusal::ConsumerIsNotWellFormed {
            cause: format!(
                "the consumer declares {} input(s) and states {} class(es) for them",
                consumer.inputs,
                consumer.input_classes.len()
            ),
        });
    }
    if !producer.slices.is_empty() && !consumer.slices.is_empty() {
        return Err(ComposeRefusal::SlicesCollide {
            producer: first_slice_ordinal(&producer.body),
            consumer: first_slice_ordinal(&consumer.body),
        });
    }
    let producer_parameters = producer.body.parameters();
    let consumer_parameters = consumer.body.parameters();
    if producer_parameters.len() != consumer_parameters.len() {
        return Err(ComposeRefusal::ParameterCountsDiffer {
            producer: producer_parameters.len(),
            consumer: consumer_parameters.len(),
        });
    }
    duplicated_mark(&consumer.marks)?;

    // A `BufferWriteCall` produces no value, so the write forwards its own value operand.
    let writes = write_sites(&producer.body);
    let [write] = writes[..] else {
        return Err(if writes.is_empty() {
            ComposeRefusal::ProducerWritesNothing
        } else {
            ComposeRefusal::ProducerWritesSeveral {
                writes: writes.len(),
            }
        });
    };
    let (produced, forwarded) = write_edge(&producer.body, write)?;
    if produced != consumer.input_classes[absorbed_at] {
        return Err(ComposeRefusal::ClassesDiffer {
            produced,
            expected: consumer.input_classes[absorbed_at],
        });
    }
    let returns: Vec<usize> = producer
        .body
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| matches!(block.terminator, Terminator::Return { .. }))
        .map(|(block, _)| block)
        .collect();
    let [return_block] = returns[..] else {
        return Err(ComposeRefusal::ProducerDoesNotReturnInOneBlock {
            returns: returns.len(),
        });
    };
    if let Some(&lane) = producer_parameters.last() {
        let incoming = producer.body.incoming_arguments();
        if producer.body.depends_on(forwarded, lane, &incoming) {
            return Err(ComposeRefusal::ForwardedValueIsLaneDependent { value: forwarded });
        }
    }
    if let Some(block) = branch_targeting(&consumer.body, consumer.body.entry) {
        return Err(ComposeRefusal::ConsumerEntryIsABranchTarget { block });
    }
    buffer_reads(&producer.body, "producer")?;
    let consumer_reads = buffer_reads(&consumer.body, "consumer")?;
    let absorbed_reads: Vec<ValueId> = consumer_reads
        .iter()
        .filter(|(_, position)| *position == absorbed)
        .map(|(value, _)| *value)
        .collect();
    if absorbed_reads.is_empty() {
        return Err(ComposeRefusal::AbsorbedIsNotRead { absorbed });
    }

    // The consumer's values are renumbered past the producer's, less what is bound below.
    let mut bound: HashMap<ValueId, ValueId> = HashMap::new();
    for (slot, &parameter) in consumer_parameters.iter().enumerate() {
        bound.insert(parameter, producer_parameters[slot]);
    }
    for read in &absorbed_reads {
        bound.insert(*read, forwarded);
    }
    let mut body = producer.body.clone();
    // The producer's store goes: its buffer is not an output of the merged fragment.
    for block in &mut body.blocks {
        block.instrs.retain(|&value| value != write);
    }
    let producer_blocks = body.blocks.len();
    let entry = consumer.body.entry;
    let mapped_block = |block: usize| -> usize {
        if block == entry {
            return_block
        } else {
            producer_blocks + block - usize::from(block > entry)
        }
    };
    let mut value_map: Vec<Option<ValueId>> = vec![None; consumer.body.values.len()];
    for (index, _) in consumer.body.values.iter().enumerate() {
        if bound.contains_key(&ValueId(index as u32)) {
            continue;
        }
        value_map[index] = Some(ValueId(body.values.len() as u32));
        body.values
            .push(ValueDef::BlockParam { block: 0, index: 0 });
    }
    let resolve = |value: ValueId| -> ValueId {
        bound
            .get(&value)
            .copied()
            .or_else(|| value_map[value.0 as usize])
            .unwrap_or(value)
    };
    for (index, definition) in consumer.body.values.iter().enumerate() {
        let Some(target) = value_map[index] else {
            continue;
        };
        body.values[target.0 as usize] = match definition {
            ValueDef::BlockParam { block, index } => ValueDef::BlockParam {
                block: mapped_block(*block),
                index: *index,
            },
            ValueDef::Instr { op, args, classes } => ValueDef::Instr {
                op: *op,
                args: args.iter().map(|&arg| resolve(arg)).collect(),
                classes: classes.clone(),
            },
        };
    }
    for (index, block) in consumer.body.blocks.iter().enumerate() {
        if index == entry {
            continue;
        }
        body.blocks.push(BasicBlock {
            params: block.params.iter().map(|&param| resolve(param)).collect(),
            instrs: block
                .instrs
                .iter()
                .filter(|value| !bound.contains_key(value))
                .map(|&value| resolve(value))
                .collect(),
            terminator: mapped_terminator(&block.terminator, &resolve, &mapped_block),
        });
    }
    let inlined: Vec<ValueId> = consumer.body.blocks[entry]
        .instrs
        .iter()
        .filter(|value| !bound.contains_key(value))
        .map(|&value| resolve(value))
        .collect();
    body.blocks[return_block].instrs.extend(inlined);
    body.blocks[return_block].terminator = mapped_terminator(
        &consumer.body.blocks[entry].terminator,
        &resolve,
        &mapped_block,
    );
    for pair in &consumer.body.barriers {
        body.barriers.push(Barrier {
            write: resolve(pair.write),
            read: resolve(pair.read),
        });
    }
    // The producer's marks go with its store: they name a role of an interface the merge drops.
    let mut marks: Vec<KernelMark> = consumer
        .marks
        .iter()
        .map(|mark| KernelMark {
            value: resolve(mark.value),
            payload: mark.payload.clone(),
        })
        .collect();

    // The merged inputs are the consumer's less the absorbed one: a higher read moves down one.
    let inputs = consumer.inputs - 1;
    renumber_reads(&mut body, absorbed, inputs)?;

    compact(&mut body, &mut marks);
    let mut input_classes = consumer.input_classes.clone();
    input_classes.remove(absorbed_at);
    let fragment = KernelFragment {
        param_shape: producer.param_shape.clone(),
        roles: KernelRoles::default(),
        body,
        inputs,
        outputs: consumer.outputs,
        input_classes,
        output_classes: consumer.output_classes.clone(),
        result_classes: consumer.result_classes.clone(),
        target: producer.target,
        slices: if producer.slices.is_empty() {
            consumer.slices.clone()
        } else {
            producer.slices.clone()
        },
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

/// Why two fragments could not become one.
///
/// # Invariant
/// One variant per fact the merge cannot decide for itself, and each names what it met: a decision
/// guessed here would be a fragment that runs and answers another program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeRefusal {
    /// The producer's body is not one this crate reads.
    ProducerIsNotWellFormed {
        /// What `KernelBody::validate` refused.
        cause: String,
    },
    /// The consumer's body is not one this crate reads.
    ConsumerIsNotWellFormed {
        /// What `KernelBody::validate` refused.
        cause: String,
    },
    /// The two domains are not the same shape.
    ShapesDiffer {
        /// The producer's domain.
        producer: KernelShape,
        /// The consumer's domain.
        consumer: KernelShape,
    },
    /// The two fragments declare different targets.
    TargetsDiffer {
        /// The facts the producer declares.
        producer: TargetFacts,
        /// The facts the consumer declares.
        consumer: TargetFacts,
    },
    /// The producer's output class is not the class the absorbed input expects.
    ClassesDiffer {
        /// The class the producer's write stages.
        produced: ScalarClass,
        /// The class the consumer's absorbed input declares.
        expected: ScalarClass,
    },
    /// The two entry blocks do not receive the same number of parameters.
    ParameterCountsDiffer {
        /// How many the producer's entry block receives.
        producer: usize,
        /// How many the consumer's entry block receives.
        consumer: usize,
    },
    /// The position to absorb is not one of the consumer's declared inputs.
    AbsorbedIsOutOfRange {
        /// The position named.
        absorbed: u32,
        /// How many inputs the consumer declares.
        inputs: usize,
    },
    /// The consumer reads no buffer at the position to absorb.
    AbsorbedIsNotRead {
        /// The position named.
        absorbed: u32,
    },
    /// A buffer read addresses its buffer at a position that is not an integer constant.
    ReadPositionIsNotAConstant {
        /// Which body holds the read.
        body: &'static str,
        /// The read.
        read: ValueId,
    },
    /// The producer reads the very position the consumer's output feeds.
    ProducerReadsTheAbsorbedInput {
        /// The position the producer reads.
        position: u32,
    },
    /// The producer reads an input position the merged fragment does not declare.
    ProducerReadsOutsideTheMerge {
        /// The position the producer reads.
        position: u32,
        /// How many inputs the merged fragment declares.
        inputs: usize,
    },
    /// The producer's body writes no buffer.
    ProducerWritesNothing,
    /// The producer's body writes more than one buffer.
    ProducerWritesSeveral {
        /// How many writes the body holds.
        writes: usize,
    },
    /// The producer's body returns from more than one block.
    ProducerDoesNotReturnInOneBlock {
        /// How many return blocks the body holds.
        returns: usize,
    },
    /// A branch targets the consumer's entry block.
    ConsumerEntryIsABranchTarget {
        /// The block that branches there.
        block: usize,
    },
    /// The forwarded value is reachable from the producer's invocation index.
    ForwardedValueIsLaneDependent {
        /// The value being forwarded.
        value: ValueId,
    },
    /// Two of the marks a merged fragment keeps carry one text.
    DuplicatedMark {
        /// The text both marks carry.
        text: String,
    },
    /// Both fragments stage into slices.
    SlicesCollide {
        /// The lowest ordinal the producer's body addresses.
        producer: u32,
        /// The lowest ordinal the consumer's body addresses.
        consumer: u32,
    },
    /// The merged body failed `KernelBody::validate`.
    MergedBodyIsRefused {
        /// What the gate refused.
        cause: String,
    },
    /// The merged fragment failed `KernelFragment::validate`.
    MergedFragmentIsRefused {
        /// What the gate refused.
        cause: String,
    },
}

impl ComposeRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// The refusal's name, for a report that has to say which check refused a merge.
    ///
    /// # Invariant
    /// One variant, one name: a report that printed the sentence alone could not be matched
    /// against the variant a caller reads, so the two would drift.
    pub fn name(&self) -> &'static str {
        match self {
            ComposeRefusal::ProducerIsNotWellFormed { .. } => "ProducerIsNotWellFormed",
            ComposeRefusal::ConsumerIsNotWellFormed { .. } => "ConsumerIsNotWellFormed",
            ComposeRefusal::ShapesDiffer { .. } => "ShapesDiffer",
            ComposeRefusal::TargetsDiffer { .. } => "TargetsDiffer",
            ComposeRefusal::ClassesDiffer { .. } => "ClassesDiffer",
            ComposeRefusal::ParameterCountsDiffer { .. } => "ParameterCountsDiffer",
            ComposeRefusal::AbsorbedIsOutOfRange { .. } => "AbsorbedIsOutOfRange",
            ComposeRefusal::AbsorbedIsNotRead { .. } => "AbsorbedIsNotRead",
            ComposeRefusal::ReadPositionIsNotAConstant { .. } => "ReadPositionIsNotAConstant",
            ComposeRefusal::ProducerReadsTheAbsorbedInput { .. } => "ProducerReadsTheAbsorbedInput",
            ComposeRefusal::ProducerReadsOutsideTheMerge { .. } => "ProducerReadsOutsideTheMerge",
            ComposeRefusal::ProducerWritesNothing => "ProducerWritesNothing",
            ComposeRefusal::ProducerWritesSeveral { .. } => "ProducerWritesSeveral",
            ComposeRefusal::ProducerDoesNotReturnInOneBlock { .. } => {
                "ProducerDoesNotReturnInOneBlock"
            }
            ComposeRefusal::ConsumerEntryIsABranchTarget { .. } => "ConsumerEntryIsABranchTarget",
            ComposeRefusal::ForwardedValueIsLaneDependent { .. } => "ForwardedValueIsLaneDependent",
            ComposeRefusal::DuplicatedMark { .. } => "DuplicatedMark",
            ComposeRefusal::SlicesCollide { .. } => "SlicesCollide",
            ComposeRefusal::MergedBodyIsRefused { .. } => "MergedBodyIsRefused",
            ComposeRefusal::MergedFragmentIsRefused { .. } => "MergedFragmentIsRefused",
        }
    }
}

impl std::fmt::Display for ComposeRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposeRefusal::ProducerIsNotWellFormed { cause } => {
                write!(formatter, "the producer's body is not well formed: {cause}")
            }
            ComposeRefusal::ConsumerIsNotWellFormed { cause } => {
                write!(formatter, "the consumer's body is not well formed: {cause}")
            }
            ComposeRefusal::ShapesDiffer { producer, consumer } => write!(
                formatter,
                "the producer's domain {producer:?} and the consumer's {consumer:?} are not the \
                 same shape, and a merged fragment has one parameter list"
            ),
            ComposeRefusal::TargetsDiffer { producer, consumer } => write!(
                formatter,
                "the producer declares {producer:?} and the consumer {consumer:?}, and a merged \
                 fragment states one target"
            ),
            ComposeRefusal::ClassesDiffer { produced, expected } => write!(
                formatter,
                "the producer stages a {produced:?} element and the consumer's absorbed input \
                 declares {expected:?}, so the forwarded value would be read at the wrong class"
            ),
            ComposeRefusal::ParameterCountsDiffer { producer, consumer } => write!(
                formatter,
                "the producer's entry block receives {producer} parameter(s) and the consumer's \
                 {consumer}, so an absorbed read cannot be bound position for position"
            ),
            ComposeRefusal::AbsorbedIsOutOfRange { absorbed, inputs } => write!(
                formatter,
                "the consumer declares {inputs} input(s) and position {absorbed} is not one of \
                 them, so there is no input to absorb"
            ),
            ComposeRefusal::AbsorbedIsNotRead { absorbed } => write!(
                formatter,
                "the consumer reads no buffer at input position {absorbed}, so the edge the \
                 caller named is not in the body"
            ),
            ComposeRefusal::ReadPositionIsNotAConstant { body, read } => write!(
                formatter,
                "the {body}'s read {read:?} addresses a buffer at a position that is not an \
                 integer constant, so which buffer it reads cannot be decided"
            ),
            ComposeRefusal::ProducerReadsTheAbsorbedInput { position } => write!(
                formatter,
                "the producer reads input position {position}, which is the position the \
                 consumer's output feeds, so the merge would be a cycle"
            ),
            ComposeRefusal::ProducerReadsOutsideTheMerge { position, inputs } => write!(
                formatter,
                "the producer reads input position {position} and the merged fragment declares \
                 {inputs} input(s), so that read has no buffer"
            ),
            ComposeRefusal::ProducerWritesNothing => write!(
                formatter,
                "the producer's body writes no buffer, so it forwards no value"
            ),
            ComposeRefusal::ProducerWritesSeveral { writes } => write!(
                formatter,
                "the producer's body writes {writes} buffers, and which of them the consumer \
                 absorbs is an edge neither fragment states"
            ),
            ComposeRefusal::ProducerDoesNotReturnInOneBlock { returns } => write!(
                formatter,
                "the producer's body returns from {returns} block(s), and the consumer's entry \
                 block can be inlined into one"
            ),
            ComposeRefusal::ConsumerEntryIsABranchTarget { block } => write!(
                formatter,
                "block {block} branches to the consumer's entry block, whose parameters are bound \
                 once, so a second arrival would re-enter what is inlined away"
            ),
            ComposeRefusal::ForwardedValueIsLaneDependent { value } => write!(
                formatter,
                "the forwarded value {value:?} is reachable from the producer's invocation index, \
                 so binding it in a lane's own block reads another lane's value — a slice and a \
                 barrier rather than an inlining (`docs/notes/graph-fusion.md` §4)"
            ),
            ComposeRefusal::DuplicatedMark { text } => write!(
                formatter,
                "two of the marks a merged fragment keeps carry `{text}`, and a name two marks carry \
                 is the ambiguity the kernel IR's reader refuses while the language's reader takes \
                 the first (`docs/notes/graph-fusion.md` §4)"
            ),
            ComposeRefusal::SlicesCollide { producer, consumer } => write!(
                formatter,
                "the producer stages into slice {producer} and the consumer into slice \
                 {consumer}, and whether the two become one slice claims two values are one \
                 (`docs/notes/graph-fusion.md` §4)"
            ),
            ComposeRefusal::MergedBodyIsRefused { cause } => write!(
                formatter,
                "the merged body is refused by `KernelBody::validate`: {cause}"
            ),
            ComposeRefusal::MergedFragmentIsRefused { cause } => write!(
                formatter,
                "the merged fragment is refused by `KernelFragment::validate`: {cause}"
            ),
        }
    }
}

/// The class the producer's write declares and the value operand it forwards.
fn write_edge(body: &KernelBody, write: ValueId) -> Result<(ScalarClass, ValueId), ComposeRefusal> {
    let Some(ValueDef::Instr {
        op: KernelInstr::BufferWriteCall(class),
        args,
        ..
    }) = body.values.get(write.0 as usize)
    else {
        return Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!("{write:?} is not a buffer write"),
        });
    };
    match args.as_slice() {
        [_, _, value] => Ok((*class, *value)),
        _ => Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!(
                "the buffer write {write:?} reads {} operand(s) and stages a value with three",
                args.len()
            ),
        }),
    }
}

/// Every buffer write this body lists.
fn write_sites(body: &KernelBody) -> Vec<ValueId> {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter())
        .copied()
        .filter(|value| {
            matches!(
                body.values.get(value.0 as usize),
                Some(ValueDef::Instr {
                    op: KernelInstr::BufferWriteCall(_),
                    ..
                })
            )
        })
        .collect()
}

/// Every buffer read this body lists, with the position it addresses.
fn buffer_reads(
    body: &KernelBody,
    side: &'static str,
) -> Result<Vec<(ValueId, u32)>, ComposeRefusal> {
    let mut reads = Vec::new();
    for block in &body.blocks {
        for &value in &block.instrs {
            let Some(ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                args,
                ..
            }) = body.values.get(value.0 as usize)
            else {
                continue;
            };
            let Some(position) = args.first().and_then(|&at| body.constant(Some(at))) else {
                return Err(ComposeRefusal::ReadPositionIsNotAConstant {
                    body: side,
                    read: value,
                });
            };
            reads.push((value, position as u32));
        }
    }
    Ok(reads)
}

/// The block that branches to `target`, when one does.
fn branch_targeting(body: &KernelBody, target: usize) -> Option<usize> {
    body.blocks.iter().position(|block| {
        block
            .branches()
            .iter()
            .any(|branch| branch.target == target)
    })
}

/// The lowest slice ordinal this body's accesses address.
fn first_slice_ordinal(body: &KernelBody) -> u32 {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter())
        .filter_map(|&value| match body.values.get(value.0 as usize) {
            Some(ValueDef::Instr {
                op: KernelInstr::SliceWriteCall(_) | KernelInstr::SliceReadCall(_),
                ..
            }) => body.slice_ordinal(value),
            _ => None,
        })
        .min()
        .map(|ordinal| ordinal as u32)
        .unwrap_or(0)
}

/// The text the surviving marks carry twice, when one does.
fn duplicated_mark(marks: &[KernelMark]) -> Result<(), ComposeRefusal> {
    let mut seen: HashSet<&str> = HashSet::new();
    for mark in marks {
        let Some(text) = mark.text() else {
            continue;
        };
        if !seen.insert(text) {
            return Err(ComposeRefusal::DuplicatedMark {
                text: text.to_string(),
            });
        }
    }
    Ok(())
}

/// A terminator with every value and block named through the maps.
fn mapped_terminator(
    terminator: &Terminator,
    value: &impl Fn(ValueId) -> ValueId,
    block: &impl Fn(usize) -> usize,
) -> Terminator {
    let mapped = |branch: &Br| Br {
        target: block(branch.target),
        args: branch.args.iter().map(|&arg| value(arg)).collect(),
    };
    match terminator {
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|&held| value(held)).collect(),
        },
        Terminator::Br(branch) => Terminator::Br(mapped(branch)),
        Terminator::CondBr {
            cond,
            if_true,
            if_false,
        } => Terminator::CondBr {
            cond: value(*cond),
            if_true: mapped(if_true),
            if_false: mapped(if_false),
        },
    }
}

/// Point every buffer read at the position it holds once the absorbed input is gone.
///
/// # Invariant
/// A read's position is a constant **value**, not an offset, so a read above the absorbed position
/// is given a fresh constant before it in its own block; a read at the absorbed position itself is
/// a producer read, because the consumer's reads of it were dropped.
fn renumber_reads(
    body: &mut KernelBody,
    absorbed: u32,
    inputs: usize,
) -> Result<(), ComposeRefusal> {
    for block in 0..body.blocks.len() {
        let listed = body.blocks[block].instrs.clone();
        let mut rebuilt: Vec<ValueId> = Vec::with_capacity(listed.len());
        for value in listed {
            let position = match body.values.get(value.0 as usize) {
                Some(ValueDef::Instr {
                    op: KernelInstr::BufferReadCall(_),
                    args,
                    ..
                }) => args
                    .first()
                    .copied()
                    .and_then(|at| match body.values.get(at.0 as usize) {
                        Some(ValueDef::Instr {
                            op: KernelInstr::Const(ScalarClass::Int, bits),
                            ..
                        }) => Some(*bits as u32),
                        _ => None,
                    }),
                _ => None,
            };
            match position {
                Some(position) if position == absorbed => {
                    return Err(ComposeRefusal::ProducerReadsTheAbsorbedInput { position });
                }
                Some(position) if position > absorbed => {
                    if position - 1 >= inputs as u32 {
                        return Err(ComposeRefusal::ProducerReadsOutsideTheMerge {
                            position,
                            inputs,
                        });
                    }
                    let constant = ValueId(body.values.len() as u32);
                    body.values.push(ValueDef::Instr {
                        op: KernelInstr::Const(ScalarClass::Int, i64::from(position - 1)),
                        args: Vec::new(),
                        classes: vec![ScalarClass::Int],
                    });
                    if let Some(ValueDef::Instr { args, .. }) =
                        body.values.get_mut(value.0 as usize)
                        && let Some(first) = args.first_mut()
                    {
                        *first = constant;
                    }
                    rebuilt.push(constant);
                }
                _ => {}
            }
            rebuilt.push(value);
        }
        body.blocks[block].instrs = rebuilt;
    }
    Ok(())
}

/// Drop every value nothing reads, and give the survivors fresh ids.
///
/// # Invariant
/// A block parameter nothing reads is removed with the branch argument that feeds it — an orphan
/// parameter passes `KernelBody::validate` and is refused only when the artifact is read, which
/// re-derives a parameter's index from the block that receives it.
fn compact(body: &mut KernelBody, marks: &mut [KernelMark]) {
    let count = body.values.len();
    let mut live = vec![false; count];
    let mut pending: Vec<ValueId> = Vec::new();
    for block in &body.blocks {
        match &block.terminator {
            Terminator::Return { values } => pending.extend(values.iter().copied()),
            Terminator::CondBr { cond, .. } => pending.push(*cond),
            Terminator::Br(_) => {}
        }
    }
    for pair in &body.barriers {
        pending.push(pair.write);
        pending.push(pair.read);
    }
    for mark in marks.iter() {
        pending.push(mark.value);
    }
    // The entry's parameters are the ABI's leaves: a caller binds them in order, read or not.
    pending.extend(body.blocks[body.entry].params.iter().copied());
    for block in &body.blocks {
        for &instr in &block.instrs {
            // A write leaves nothing, so nothing reads it: it is here for its operands.
            if let Some(ValueDef::Instr { classes, .. }) = body.values.get(instr.0 as usize)
                && classes.is_empty()
            {
                pending.push(instr);
            }
        }
    }
    while let Some(value) = pending.pop() {
        let Some(flag) = live.get_mut(value.0 as usize) else {
            continue;
        };
        if std::mem::replace(flag, true) {
            continue;
        }
        match body.values.get(value.0 as usize) {
            Some(ValueDef::Instr { args, .. }) => pending.extend(args.iter().copied()),
            Some(ValueDef::BlockParam { block, index }) => {
                for other in &body.blocks {
                    for branch in other.branches() {
                        if branch.target == *block
                            && let Some(&argument) = branch.args.get(*index as usize)
                        {
                            pending.push(argument);
                        }
                    }
                }
            }
            None => {}
        }
    }
    let keeps: Vec<Vec<usize>> = body
        .blocks
        .iter()
        .map(|block| {
            (0..block.params.len())
                .filter(|&at| live[block.params[at].0 as usize])
                .collect()
        })
        .collect();
    let mut map: Vec<Option<ValueId>> = vec![None; count];
    let mut survivors = 0u32;
    for (index, flag) in live.iter().enumerate() {
        if *flag {
            map[index] = Some(ValueId(survivors));
            survivors += 1;
        }
    }
    for index in 0..body.blocks.len() {
        let params: Vec<ValueId> = keeps[index]
            .iter()
            .map(|&at| body.blocks[index].params[at])
            .collect();
        let instrs: Vec<ValueId> = body.blocks[index]
            .instrs
            .iter()
            .copied()
            .filter(|value| match body.values.get(value.0 as usize) {
                Some(ValueDef::Instr { classes, .. }) => {
                    live[value.0 as usize] || classes.is_empty()
                }
                _ => false,
            })
            .collect();
        let terminator = pruned(&body.blocks[index].terminator, &keeps, &map);
        body.blocks[index].params = params.iter().map(|&held| renumbered(&map, held)).collect();
        body.blocks[index].instrs = instrs.iter().map(|&held| renumbered(&map, held)).collect();
        body.blocks[index].terminator = terminator;
    }
    let mut values: Vec<ValueDef> = Vec::with_capacity(count);
    for (index, definition) in body.values.iter().enumerate() {
        if map[index].is_none() {
            continue;
        }
        values.push(match definition {
            ValueDef::Instr { op, args, classes } => ValueDef::Instr {
                op: *op,
                args: args.iter().map(|&arg| renumbered(&map, arg)).collect(),
                classes: classes.clone(),
            },
            ValueDef::BlockParam { .. } => ValueDef::BlockParam { block: 0, index: 0 },
        });
    }
    body.values = values;
    for block in 0..body.blocks.len() {
        for index in 0..body.blocks[block].params.len() {
            let param = body.blocks[block].params[index];
            body.values[param.0 as usize] = ValueDef::BlockParam {
                block,
                index: index as u32,
            };
        }
    }
    for pair in &mut body.barriers {
        pair.write = renumbered(&map, pair.write);
        pair.read = renumbered(&map, pair.read);
    }
    for mark in marks.iter_mut() {
        mark.value = renumbered(&map, mark.value);
    }
}

/// A terminator with the arguments of a dropped parameter removed and the survivors renumbered.
fn pruned(terminator: &Terminator, keeps: &[Vec<usize>], map: &[Option<ValueId>]) -> Terminator {
    let branch = |held: &Br| {
        let keep = keeps.get(held.target).map(Vec::as_slice).unwrap_or(&[]);
        Br {
            target: held.target,
            args: keep
                .iter()
                .filter_map(|&at| held.args.get(at).copied())
                .map(|arg| renumbered(map, arg))
                .collect(),
        }
    };
    match terminator {
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(|&held| renumbered(map, held)).collect(),
        },
        Terminator::Br(held) => Terminator::Br(branch(held)),
        Terminator::CondBr {
            cond,
            if_true,
            if_false,
        } => Terminator::CondBr {
            cond: renumbered(map, *cond),
            if_true: branch(if_true),
            if_false: branch(if_false),
        },
    }
}

/// The id a surviving value holds, or the id itself for one no map reaches.
fn renumbered(map: &[Option<ValueId>], value: ValueId) -> ValueId {
    map.get(value.0 as usize)
        .copied()
        .flatten()
        .unwrap_or(value)
}
