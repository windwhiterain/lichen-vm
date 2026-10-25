//! Two fragments in a chain become one.
//! See `docs/notes/graph-fusion.md` §4.

use std::collections::{HashMap, HashSet};

use crate::{
    Barrier, BasicBlock, Br, Cone, ConeOrigin, KernelBody, KernelFragment, KernelInstr, KernelMark,
    KernelRoles, KernelShape, NotIndexLocal, ScalarClass, TargetFacts, Terminator, ValueDef,
    ValueId,
};

/// The fragment `producer` and `consumer` become, or the decision the merge cannot make.
///
/// # Invariant
/// **The merged inputs are the union: the producer's inputs, then the consumer's surviving ones**,
/// the absorbed one left out — so a surviving consumer read moves up by the producer's input
/// count, and `input_classes` follows the same order. The producer's marks go.
///
/// One name is [`compose_fan_out`] with one consumer, so a chain is not a second mechanism's.
///
/// # Invariant
/// **The producer's store goes, unless it answers the consumer**: then the store is kept, the
/// consumer's read of that buffer stays a read, and a barrier separates the write from the read
/// — unless the two are one lane, which observes its own store and needs no barrier.
pub fn compose(
    producer: &KernelFragment,
    consumer: &KernelFragment,
    absorbed: u32,
) -> Result<KernelFragment, ComposeRefusal> {
    compose_fan_out(producer, &[(consumer, absorbed)])
}

/// The fragment `producer` and every one of `consumers` become, or the decision it cannot make.
///
/// # Invariant
/// **One producer, several consumers**: each entry names the input ordinal the producer's output
/// reaches. The store goes once and **one cone is copied per read**.
///
/// **The merged inputs are the producer's then each consumer's survivors**; every consumer but the
/// last leaves the return block.
///
/// **The merged parameter list is the union of the bodies' scalar leaves**, each its own.
pub fn compose_fan_out(
    producer: &KernelFragment,
    consumers: &[(&KernelFragment, u32)],
) -> Result<KernelFragment, ComposeRefusal> {
    if consumers.is_empty() {
        return Err(ComposeRefusal::NoConsumerToInline);
    }
    if let Err(cause) = producer.body.validate() {
        return Err(ComposeRefusal::ProducerIsNotWellFormed { cause });
    }
    if producer.input_classes.len() != producer.inputs {
        return Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!(
                "the producer declares {} input(s) and states {} class(es) for them",
                producer.inputs,
                producer.input_classes.len()
            ),
        });
    }
    let producer_parameters = producer.body.parameters();
    // **The union of the bodies' domains**, in the order the bodies are composed.
    let mut shapes: Vec<&KernelShape> = Vec::with_capacity(consumers.len() + 1);
    shapes.push(&producer.param_shape);
    shapes.extend(consumers.iter().map(|(consumer, _)| &consumer.param_shape));
    let Some(scalars) = ScalarUnion::of(&shapes) else {
        return Err(shapes_differ(
            producer,
            consumers[0].0,
            "one of the domains names no extent and index, so it has no runtime scalar to tell \
             from the index"
                .to_string(),
        ));
    };
    // **The producer's body is the merged body**, so it must bind its own domain.
    if !binds_its_domain(producer) {
        return Err(shapes_differ(
            producer,
            consumers[0].0,
            format!(
                "the producer's entry block receives {} parameter(s) and its domain declares {} \
                 leaf(ies), so it has no runtime scalar to bind",
                producer_parameters.len(),
                producer.param_shape.flat_arity()
            ),
        ));
    }
    // The union, folded from the consumers' own ports: the running `union` below is it counted
    // forward.
    let ports = consumers
        .iter()
        .fold(producer.inputs, |ports, (consumer, _)| {
            ports.saturating_add(consumer.inputs.saturating_sub(1))
        });
    let producer_reads = buffer_reads(&producer.body, "producer")?;
    // Only there do the two bodies' ordinals disagree about which buffer a position names.
    if let Some(position) = producer_reads
        .iter()
        .map(|(_, position)| *position)
        .find(|position| *position as usize >= producer.inputs)
    {
        return Err(ComposeRefusal::ProducerReadsOutsideTheMerge {
            position,
            inputs: ports,
        });
    }
    let mut edges = Vec::with_capacity(consumers.len());
    for (index, (consumer, absorbed)) in consumers.iter().enumerate() {
        edges.push(consumer_edge(
            producer,
            consumer,
            *absorbed,
            &producer_parameters,
            index,
            consumers.len(),
        )?);
    }
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
    let (produced, forwarded, written_at) = write_edge(&producer.body, write)?;
    for edge in &edges {
        let expected = edge.fragment.input_classes[edge.absorbed];
        if produced != expected {
            return Err(ComposeRefusal::ClassesDiffer { produced, expected });
        }
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
    // Every absorbed read is a copy of the cone at its own address — **or the producer's
    // store answers it**.
    let forwarding = match producer.body.index_local_cone(
        forwarded,
        written_at,
        producer_parameters.last().copied(),
    ) {
        Ok(cone) => Forwarding::Recomputed(cone),
        // A value that never reaches the invocation index is the same in every lane.
        Err(_) if !lane_dependent(&producer.body, forwarded, &producer_parameters) => {
            Forwarding::Recomputed(Cone::reused(forwarded))
        }
        Err(blocker) => {
            // **The staged path**: the producer's store answers the consumer.
            if !matches!(blocker, NotIndexLocal::Carried { .. }) {
                return Err(ComposeRefusal::ForwardedValueIsLaneDependent {
                    value: forwarded,
                    read: edges[0].absorbed_reads[0].0,
                    blocker,
                });
            }
            Forwarding::Staged
        }
    };
    // **The staged path copies nothing**, so the cone it would have copied is not read.
    let live = Cone::reused(forwarded);
    let cone = match &forwarding {
        Forwarding::Recomputed(cone) => cone,
        Forwarding::Staged => &live,
    };
    // **The staged path's own answer**: the output ordinal the producer's store fills, and
    // the class it stages.
    let staged_ordinal = match forwarding {
        Forwarding::Recomputed(_) => None,
        Forwarding::Staged => {
            let Some(ValueDef::Instr { args, .. }) = producer.body.values.get(write.0 as usize)
            else {
                return Err(ComposeRefusal::ProducerIsNotWellFormed {
                    cause: format!("the buffer write {write:?} is not an instruction"),
                });
            };
            let Some(position) = args.first().copied() else {
                return Err(ComposeRefusal::ProducerIsNotWellFormed {
                    cause: format!("the buffer write {write:?} names no output position"),
                });
            };
            match producer.body.constant(Some(position)) {
                Some(ordinal) => Some(ordinal),
                None => {
                    return Err(ComposeRefusal::ProducerIsNotWellFormed {
                        cause: format!(
                            "the buffer write {write:?} names its output position {position:?}, which \
                             is not an integer constant"
                        ),
                    });
                }
            }
        }
    };

    let mut body = producer.body.clone();
    // **A composed body is as exact as the bodies it composes**: it carries their definitions.
    body.exact_float_arithmetic = producer.body.exact_float_arithmetic
        || consumers
            .iter()
            .any(|(consumer, _)| consumer.body.exact_float_arithmetic);
    // **The runtime scalars are spliced into the merged body's own domain.**
    let total = scalars.total();
    splice_runtime_scalars(&mut body, scalars.own(0), total - scalars.own(0));
    // **The merged body's own parameter list**, which each consumer binds to position for position.
    let merged_parameters = body.blocks[body.entry].params.clone();
    // **The producer's store goes** — unless it answers the consumer, in which case it stays.
    if matches!(forwarding, Forwarding::Recomputed(_)) {
        for block in &mut body.blocks {
            block.instrs.retain(|&value| value != write);
        }
    }
    // The block and the port each consumer's own body lands at, both counted forward.
    let mut blocks_base = body.blocks.len();
    let mut union = producer.inputs;
    // **The merged inputs are the union**, and `input_classes` is that same union.
    let mut input_classes = producer.input_classes.clone();
    // **The staged path keeps the producer's store**, so its buffer is an output too.
    let mut output_classes = if staged_ordinal.is_some() {
        producer.output_classes.clone()
    } else {
        Vec::new()
    };
    // **The merged node's outputs keep their declared extents**, each the length its own declaration states.
    let mut output_extents = if staged_ordinal.is_some() {
        producer.roles.output_extents.clone()
    } else {
        Vec::new()
    };
    let mut outputs = if staged_ordinal.is_some() {
        producer.outputs
    } else {
        0
    };
    let mut result_classes = Vec::new();
    let mut slices = producer.slices.clone();
    // The producer's marks go with the interface the merge drops: a role of a struct the
    // merged fragment has not.
    let mut marks: Vec<KernelMark> = Vec::new();
    for (index, edge) in edges.iter().enumerate() {
        let consumer = edge.fragment;
        // The consumer's values are renumbered past the producer's, less what is bound below.
        let mut renumber = Renumber::default();
        for (slot, &parameter) in consumer.body.parameters().iter().enumerate() {
            renumber.bind(
                parameter,
                merged_parameters[landed_slot(
                    slot,
                    scalars.own(index + 1),
                    scalars.before(index + 1),
                    total,
                )],
            );
        }
        // **The staged read**: the consumer's read of the absorbed buffer is a read of
        // the producer's output buffer.
        let mut staged_reads: HashMap<ValueId, ValueId> = HashMap::new();
        if let Some(ordinal) = staged_ordinal {
            for &(read, _) in &edge.absorbed_reads {
                // **The read states the buffer's class**, as the write does.
                let stated = match consumer.body.values.get(read.0 as usize) {
                    Some(ValueDef::Instr {
                        op: KernelInstr::BufferReadCall(class),
                        ..
                    }) => *class,
                    _ => {
                        return Err(ComposeRefusal::ConsumerIsNotWellFormed {
                            cause: format!("the absorbed read {read:?} is not a buffer read"),
                        });
                    }
                };
                if stated != produced {
                    return Err(ComposeRefusal::ClassesDiffer {
                        produced,
                        expected: stated,
                    });
                }
                // The read's position is the producer's output ordinal, a **value**.
                staged_reads.insert(read, position_constant(&mut body, ordinal as i64));
            }
        } else {
            for &(read, _) in &edge.absorbed_reads {
                // The read's own id until the copy below replaces it, so it reserves no value of its own.
                renumber.bind(read, read);
            }
        }
        let landed: Vec<Option<ValueId>> = (0..consumer.body.values.len())
            .map(|value| renumber.reserve(ValueId(value as u32), &mut body))
            .collect();
        // **Every surviving read of the consumer's own buffer takes the union's position for it.**
        let shifted = shifted_positions(&mut body, &edge.reads, union, edge.absorbed);
        // **And every write of the consumer's own output takes the sum of the outputs before it.**
        let writes = shifted_write_positions(&mut body, &consumer.body, outputs);
        let entry = consumer.body.entry;
        let base = blocks_base;
        let mapped_block = |block: usize| -> usize {
            if block == entry {
                return_block
            } else {
                base + block - usize::from(block > entry)
            }
        };
        // One copy of the cone per read, at that read's own address, spliced in where it stood.
        let absorbed_at_address: HashMap<ValueId, ValueId> = if staged_ordinal.is_some() {
            HashMap::new()
        } else {
            edge.absorbed_reads.iter().copied().collect()
        };
        let mut shared: HashMap<(usize, ValueId), (usize, ValueId)> = HashMap::new();
        let mut sequences: Vec<Vec<ValueId>> = Vec::with_capacity(consumer.body.blocks.len());
        for (block_index, block) in consumer.body.blocks.iter().enumerate() {
            let mut sequence: Vec<ValueId> = Vec::with_capacity(block.instrs.len());
            let merged_block = mapped_block(block_index);
            for &value in &block.instrs {
                let Some(&address) = absorbed_at_address.get(&value) else {
                    // The position is a constant **value**, so it stands before the read that uses it.
                    if let Some(&constant) = staged_reads
                        .get(&value)
                        .or_else(|| shifted.get(&value))
                        .or_else(|| writes.get(&value))
                    {
                        sequence.push(constant);
                    }
                    sequence.push(renumber.resolve(value));
                    continue;
                };
                // **One copy per address per block**: the return block dominates them all.
                let address = renumber.resolve(address);
                let key = (merged_block, address);
                let root = match shared.get(&key) {
                    Some(&(placed, root)) if placed == return_block || placed == merged_block => {
                        root
                    }
                    _ => {
                        let mut fresh: HashMap<ValueId, ValueId> = HashMap::new();
                        let root = copy_cone(
                            &cone,
                            forwarded,
                            &producer.body,
                            &|value| value,
                            address,
                            &mut body,
                            &mut fresh,
                            &mut sequence,
                        );
                        shared.insert(key, (merged_block, root));
                        root
                    }
                };
                renumber.bind(value, root);
            }
            sequences.push(sequence);
        }
        // **After the copies**: an operand that read the absorbed buffer now reads the cone.
        for (value, definition) in consumer.body.values.iter().enumerate() {
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
                    // **The staged read names an output buffer**: the producer's own.
                    let op = match staged_reads.get(&ValueId(value as u32)) {
                        Some(_) => match op {
                            KernelInstr::BufferReadCall(class) => {
                                KernelInstr::OutputReadCall(*class)
                            }
                            other => *other,
                        },
                        None => *op,
                    };
                    if let Some(&constant) = staged_reads
                        .get(&ValueId(value as u32))
                        .or_else(|| shifted.get(&ValueId(value as u32)))
                        .or_else(|| writes.get(&ValueId(value as u32)))
                        && let Some(first) = args.first_mut()
                    {
                        *first = constant;
                    }
                    ValueDef::Instr {
                        op,
                        args,
                        classes: classes.clone(),
                    }
                }
            };
        }
        let resolve = |value: ValueId| renumber.resolve(value);
        for (index, block) in consumer.body.blocks.iter().enumerate() {
            if index == entry {
                continue;
            }
            body.blocks.push(BasicBlock {
                params: block.params.iter().map(|&param| resolve(param)).collect(),
                instrs: std::mem::take(&mut sequences[index]),
                terminator: mapped_terminator(&block.terminator, &resolve, &mapped_block),
            });
        }
        body.blocks[return_block]
            .instrs
            .extend(std::mem::take(&mut sequences[entry]));
        body.blocks[return_block].terminator = mapped_terminator(
            &consumer.body.blocks[entry].terminator,
            &resolve,
            &mapped_block,
        );
        for expr in &consumer.body.barriers {
            body.barriers.push(Barrier {
                write: resolve(expr.write),
                read: resolve(expr.read),
            });
        }
        // **The staged path's barrier**: the store and the read that observes it.
        for &(read, _) in &edge.absorbed_reads {
            if staged_ordinal.is_some() {
                let read = resolve(read);
                // **A same-lane pair records no barrier**: one lane sees its own store.
                if !same_lane(&body, write, read) {
                    body.barriers.push(Barrier { write, read });
                }
            }
        }
        // Only the first consumer's marks are kept: a merged node carries no claim to read them.
        if index == 0 {
            marks = consumer
                .marks
                .iter()
                .map(|mark| KernelMark {
                    value: resolve(mark.value),
                    payload: mark.payload.clone(),
                })
                .collect();
        }
        input_classes.extend(
            consumer
                .input_classes
                .iter()
                .enumerate()
                .filter(|(ordinal, _)| *ordinal != edge.absorbed)
                .map(|(_, class)| *class),
        );
        output_classes.extend(consumer.output_classes.iter().copied());
        output_extents.extend(consumer.roles.output_extents.iter().copied());
        outputs += consumer.outputs;
        // The last consumer's terminator is the merged body's, so its results are the merged ones.
        result_classes.clone_from(&consumer.result_classes);
        if slices.is_empty() {
            slices.clone_from(&consumer.slices);
        }
        blocks_base += consumer.body.blocks.len() - 1;
        union += consumer.inputs - 1;
    }

    compact(&mut body, &mut marks);
    let fragment = KernelFragment {
        param_shape: scalars.shape(),
        roles: KernelRoles {
            output_extents,
            ..KernelRoles::default()
        },
        body,
        inputs: ports,
        outputs,
        input_classes,
        output_classes,
        result_classes,
        target: producer.target,
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

/// The parameter list the composed bodies share: the extent, each body's own runtime scalars, the
/// index.
///
/// # Invariant
/// **Each body's own runtime scalars, in the order the bodies are composed**: a body's leaves
/// are bound to its own entries and to no other's, so a body never reads another body's number.
/// The extent is the count the launch dispatches over and the index is the lane's, so both stay
/// at the ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarUnion {
    /// Each body's own runtime scalar count, in the order the bodies are composed.
    counts: Vec<usize>,
    /// Each runtime scalar's class, in the merged list's own order.
    classes: Vec<ScalarClass>,
}

impl ScalarUnion {
    /// The union of the bodies' domains.
    ///
    /// # Invariant
    /// **A domain is the extent, its runtime scalars and the index**: a domain that names no index
    /// has no runtime scalar to tell from one, and a domain whose ends are not integers names a
    /// count or an index the launch does not bind. Either is refused here rather than read as a
    /// domain of leaves the merged list cannot hold.
    pub fn of(shapes: &[&KernelShape]) -> Option<Self> {
        let mut counts = Vec::with_capacity(shapes.len());
        let mut classes = Vec::new();
        for shape in shapes {
            let leaves = shape.leaf_classes();
            let [extent, .., index] = leaves.as_slice() else {
                return None;
            };
            if *extent != ScalarClass::Int || *index != ScalarClass::Int {
                return None;
            }
            counts.push(leaves.len() - 2);
            classes.extend(leaves[1..leaves.len() - 1].iter().copied());
        }
        Some(ScalarUnion { counts, classes })
    }

    /// How many runtime scalars the merged parameter list carries.
    pub fn total(&self) -> usize {
        self.counts.iter().sum()
    }

    /// How many runtime scalars the body at `ordinal` declares.
    pub fn own(&self, ordinal: usize) -> usize {
        self.counts[ordinal]
    }

    /// How many runtime scalars the bodies before `ordinal` declare.
    pub fn before(&self, ordinal: usize) -> usize {
        self.counts[..ordinal].iter().sum()
    }

    /// The merged parameter list's shape: the extent, every body's own runtime scalars, the index.
    ///
    /// # Invariant
    /// **The same leaves the bodies bind, in the order they bind them**: a body's own runtime
    /// scalars sit beside the extent, so a caller that reads the merged domain's leaves reads each
    /// body's own numbers and nobody else's.
    pub fn shape(&self) -> KernelShape {
        let mut leaves = Vec::with_capacity(self.classes.len() + 2);
        leaves.push(KernelShape::Scalar(ScalarClass::Int));
        leaves.extend(self.classes.iter().map(|&class| KernelShape::Scalar(class)));
        leaves.push(KernelShape::Scalar(ScalarClass::Int));
        KernelShape::Tuple(leaves)
    }
}

/// Whether a body's entry block receives its domain's leaves, position for position.
///
/// # Invariant
/// **A body binds the extent, its own runtime scalars and its index**: a body whose entry block
/// receives another number has no leaf to bind — its index would stand where a runtime scalar
/// stands — so the merged list would answer one number where the body asked for another.
pub fn binds_its_domain(fragment: &KernelFragment) -> bool {
    fragment.body.parameters().len() == fragment.param_shape.flat_arity()
}

/// Whether a staged pair's write and its read address the same lane.
///
/// # Invariant
/// **The same `ValueId` and nothing else**: the write's and the read's index operands are one
/// value, so one lane observes its own store in program order, and no barrier is needed. The
/// write precedes the read — the producer's body comes first. Two index expressions that
/// differ are two lanes' slots, and the barrier stays: below one workgroup it would be
/// non-uniform.
pub(crate) fn same_lane(body: &KernelBody, write: ValueId, read: ValueId) -> bool {
    let write_index = match body.values.get(write.0 as usize) {
        Some(ValueDef::Instr {
            op: KernelInstr::BufferWriteCall(_),
            args,
            ..
        }) => args.get(1).copied(),
        _ => None,
    };
    let read_index = match body.values.get(read.0 as usize) {
        Some(ValueDef::Instr {
            op: KernelInstr::OutputReadCall(_),
            args,
            ..
        }) => args.get(1).copied(),
        _ => None,
    };
    match (write_index, read_index) {
        (Some(write_index), Some(read_index)) => write_index == read_index,
        _ => false,
    }
}

/// The merged parameter a body's `slot`-th entry parameter lands on.
///
/// # Invariant
/// **The extent stays at `0`, a body's own runtime scalars land on its own entries and its index
/// lands last**: a body reads its own numbers and no other's, so its `j`-th runtime scalar lands
/// `before + j` and its index lands after every body's.
pub fn landed_slot(slot: usize, own: usize, before: usize, total: usize) -> usize {
    if slot == 0 {
        0
    } else if slot <= own {
        before + slot
    } else {
        total + 1
    }
}

/// `extra` fresh entry-block parameters on `body`, between its own runtime scalars and its index.
///
/// # Invariant
/// **Every entry-block parameter but the extent and the body's own runtime scalars moves past the
/// spliced leaves**, so the body's own index lands where the merged domain's index does and the
/// bodies bind position for position as they did when the shapes were equal. `extra` of `0`
/// changes nothing at all.
pub fn splice_runtime_scalars(body: &mut KernelBody, own: usize, extra: usize) {
    if extra == 0 {
        return;
    }
    let entry = body.entry;
    for definition in &mut body.values {
        if let ValueDef::BlockParam { block, index } = definition
            && *block == entry
            && *index > own as u32
        {
            *index += extra as u32;
        }
    }
    let base = body.blocks[entry].params.len() as u32;
    let at = ((own as u32) + 1).min(base) as usize;
    for ordinal in 0..extra as u32 {
        let value = ValueId(body.values.len() as u32);
        body.values.push(ValueDef::BlockParam {
            block: entry,
            index: at as u32 + ordinal,
        });
        body.blocks[entry]
            .params
            .insert(at + ordinal as usize, value);
    }
}

/// One consumer of the producer: the fragment, and the reads the composition reads off it.
///
/// # Invariant
/// The `absorbed` position is resolved against the consumer's own inputs before any value moves,
/// so an edge this composition cannot serve is refused while the bodies are still untouched.
struct Edge<'a> {
    fragment: &'a KernelFragment,
    absorbed: usize,
    reads: Vec<(ValueId, u32)>,
    absorbed_reads: Vec<(ValueId, ValueId)>,
}

/// Everything one consumer must answer before the merged body is built.
///
/// # Invariant
/// **Every condition is checked before a body is touched**, and the two that need the merged
/// parameter list are stated at the composition: a consumer that does not leave its entry block
/// has nowhere for the next consumer's body to go, while the last consumer keeps the merged
/// body's terminator.
fn consumer_edge<'a>(
    producer: &KernelFragment,
    consumer: &'a KernelFragment,
    absorbed: u32,
    producer_parameters: &[ValueId],
    index: usize,
    total: usize,
) -> Result<Edge<'a>, ComposeRefusal> {
    if let Err(cause) = consumer.body.validate() {
        return Err(ComposeRefusal::ConsumerIsNotWellFormed { cause });
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
    let consumer_parameters = consumer.body.parameters();
    // **The consumer's own parameter list is its domain's leaves.**
    if consumer_parameters.len() != consumer.param_shape.flat_arity() {
        return Err(ComposeRefusal::ParameterCountsDiffer {
            producer: producer_parameters.len(),
            consumer: consumer_parameters.len(),
            leaves: consumer.param_shape.flat_arity(),
        });
    }
    duplicated_mark(&consumer.marks)?;
    if let Some(block) = branch_targeting(&consumer.body, consumer.body.entry) {
        return Err(ComposeRefusal::ConsumerEntryIsABranchTarget { block });
    }
    // A branching entry leaves the merged body elsewhere: the next consumer would run off this path.
    if index + 1 < total
        && !matches!(
            consumer.body.blocks[consumer.body.entry].terminator,
            Terminator::Return { .. }
        )
    {
        return Err(ComposeRefusal::ConsumerEntryDoesNotReturn { consumer: index });
    }
    let reads = buffer_reads(&consumer.body, "consumer")?;
    let absorbed_reads: Vec<(ValueId, ValueId)> = reads
        .iter()
        .filter(|(_, position)| *position == absorbed)
        .map(|(value, _)| (*value, read_address(&consumer.body, *value)))
        .collect();
    if absorbed_reads.is_empty() {
        return Err(ComposeRefusal::AbsorbedIsNotRead { absorbed });
    }
    Ok(Edge {
        fragment: consumer,
        absorbed: absorbed_at,
        reads,
        absorbed_reads,
    })
}

/// The refusal two bodies earn when they cannot share one parameter list.
///
/// # Invariant
/// **The two domains and what met them**: a merged fragment carries one parameter list, and it is
/// the union of the bodies' scalar leaves, so a body that does not bind its own domain has no
/// leaf to bind.
pub(crate) fn shapes_differ(
    producer: &KernelFragment,
    consumer: &KernelFragment,
    blocker: String,
) -> ComposeRefusal {
    ComposeRefusal::ShapesDiffer {
        producer: producer.param_shape.clone(),
        consumer: consumer.param_shape.clone(),
        blocker,
    }
}

/// Why two fragments could not become one.
///
/// # Invariant
/// One variant per fact the merge cannot decide for itself, and each names what it met: a decision
/// guessed here would be a fragment that runs and answers another program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeRefusal {
    /// The composition names no consumer, so there is nothing to inline into the producer.
    NoConsumerToInline,
    /// The fan-in names no body, so there is nothing to inline into a merged fragment.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::NoConsumerToInline`] because it is a distinct question and it
    /// carries a distinct fact: that one is the composition's consumer list, this one the fan-in's
    /// body list — a caller that built a chain asked the other question and gets the other name.
    NoProducerToInline,
    /// The sibling set names no branch, so there is nothing to inline into a merged fragment.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::NoProducerToInline`] because it is a distinct question: that
    /// one is the fan-in's body list and this one is the sibling set's branch list.
    NoBranchToInline,
    /// A sibling set of one branch is not a sibling set: nothing stands beside it.
    ///
    /// # Invariant
    /// Refused rather than answered with the branch itself, because the name a report prints is
    /// the check that fired and "the composition is a sibling set of one" is not what a caller
    /// meant.
    OneBranchIsNotASiblingSet {
        /// How many branches the list holds.
        branches: usize,
    },
    /// A fan-in of one body is not a fan-in: nothing absorbs from the only body.
    ///
    /// # Invariant
    /// Refused rather than answered with the body itself, because the name a report prints is the
    /// check that fired and "the composition is a fan-in of one" is not what a caller meant.
    OneBodyIsNotAFanIn {
        /// How many bodies the list holds.
        bodies: usize,
    },
    /// An absorbed input names a body that does not come before the one reading it.
    AbsorbedIsNotAnEarlierBody {
        /// The body's index in the composition's own list.
        body: usize,
        /// The index it named.
        producer: usize,
        /// The input ordinal the value was to reach.
        input: usize,
    },
    /// One input of one body is filled twice, so the union drops it once and reads it twice.
    OneInputIsFilledTwice {
        /// The body's index in the composition's own list.
        body: usize,
        /// The input ordinal both entries named.
        input: usize,
    },
    /// One producer's value reaches one body at more than one input position.
    ///
    /// # Invariant
    /// The expr's [`Self::AbsorbedEdgeIsNotOne`] is about **one** producer and **one** consumer;
    /// this is the same fact inside a fan-in, where the reader has several producers to choose
    /// between, so it carries the two indices that disagree.
    OneProducerFeedsTwoInputs {
        /// The body's index in the composition's own list.
        body: usize,
        /// The producer both inputs named.
        producer: usize,
    },
    /// A body of a fan-in returns from more than one block, or from none.
    ///
    /// # Invariant
    /// Each body lands on the block the body before it returns from, so the block a body returns
    /// from is where the next one's entry goes: a body that returns from two has no single such
    /// block, and one that returns from none is not a body the merged body can continue past.
    BodyDoesNotReturnInOneBlock {
        /// How many return blocks the body holds.
        returns: usize,
    },
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
    /// The two bodies cannot share one parameter list.
    ///
    /// # Invariant
    /// A merged fragment carries **one** parameter list, and it is the union of the bodies' own
    /// scalar leaves: a body that does not bind its own domain has no leaf to bind, so the two
    /// domains and the bodies that read them are refused together.
    ShapesDiffer {
        /// The producer's domain.
        producer: KernelShape,
        /// The consumer's domain.
        consumer: KernelShape,
        /// What the two bodies met that stopped them.
        blocker: String,
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
    /// The consumer's entry block does not receive the leaves its domain declares.
    ///
    /// # Invariant
    /// **A body's parameter list is its domain's leaves**: the consumer is bound position for
    /// position into the merged list, so a consumer whose entry block receives another number has
    /// no leaf to bind — its index would stand where a runtime scalar stands.
    ParameterCountsDiffer {
        /// How many the producer's entry block receives.
        producer: usize,
        /// How many the consumer's entry block receives.
        consumer: usize,
        /// How many leaves the consumer's domain declares.
        leaves: usize,
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
    /// The producer reads a position the union hands to the consumer.
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
    /// A consumer that is not the last leaves its entry block by branching.
    ConsumerEntryDoesNotReturn {
        /// Which consumer, in the order the composition named them.
        consumer: usize,
    },
    /// The forwarded value is lane-dependent, and its cone is not a function of an address.
    ForwardedValueIsLaneDependent {
        /// The value being forwarded.
        value: ValueId,
        /// The first read of the absorbed buffer, as a report names it.
        read: ValueId,
        /// What stops the cone being recomputed at that address.
        blocker: NotIndexLocal,
    },
    /// The caller named a producer this fan-in does not have, or named one twice.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::OneBodyIsNotAFanIn`] because it is a distinct fact: that one
    /// says the fan-in holds no producer at all, this one says the caller's list names one the
    /// fan-in has not — the consumer stages nothing, and a producer is staged once.
    StagedProducerIsNotAProducer {
        /// The index the caller named.
        producer: usize,
    },
    /// A staged producer's output is read by a producer that is recomputed.
    ///
    /// # Invariant
    /// A distinct arm because it is a distinct fact: a staged producer's value reaches the body
    /// that reads it through the store, and a copy of a recomputed producer's cone would read
    /// that store at an input ordinal — which names another buffer in the merged fragment.
    StagedOutputIsReadByARecomputedProducer {
        /// The staged producer whose output is read.
        staged: usize,
        /// The producer that reads it and is recomputed.
        producer: usize,
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
            ComposeRefusal::NoConsumerToInline => "NoConsumerToInline",
            ComposeRefusal::NoProducerToInline => "NoProducerToInline",
            ComposeRefusal::NoBranchToInline => "NoBranchToInline",
            ComposeRefusal::OneBranchIsNotASiblingSet { .. } => "OneBranchIsNotASiblingSet",
            ComposeRefusal::OneBodyIsNotAFanIn { .. } => "OneBodyIsNotAFanIn",
            ComposeRefusal::AbsorbedIsNotAnEarlierBody { .. } => "AbsorbedIsNotAnEarlierBody",
            ComposeRefusal::OneInputIsFilledTwice { .. } => "OneInputIsFilledTwice",
            ComposeRefusal::OneProducerFeedsTwoInputs { .. } => "OneProducerFeedsTwoInputs",
            ComposeRefusal::BodyDoesNotReturnInOneBlock { .. } => "BodyDoesNotReturnInOneBlock",
            ComposeRefusal::ProducerIsNotWellFormed { .. } => "ProducerIsNotWellFormed",
            ComposeRefusal::ConsumerIsNotWellFormed { .. } => "ConsumerIsNotWellFormed",
            ComposeRefusal::ShapesDiffer { .. } => "ShapesDiffer",
            ComposeRefusal::TargetsDiffer { .. } => "TargetsDiffer",
            ComposeRefusal::ClassesDiffer { .. } => "ClassesDiffer",
            ComposeRefusal::ParameterCountsDiffer { .. } => "ParameterCountsDiffer",
            ComposeRefusal::AbsorbedIsOutOfRange { .. } => "AbsorbedIsOutOfRange",
            ComposeRefusal::AbsorbedIsNotRead { .. } => "AbsorbedIsNotRead",
            ComposeRefusal::ReadPositionIsNotAConstant { .. } => "ReadPositionIsNotAConstant",
            ComposeRefusal::ProducerReadsOutsideTheMerge { .. } => "ProducerReadsOutsideTheMerge",
            ComposeRefusal::ProducerWritesNothing => "ProducerWritesNothing",
            ComposeRefusal::ProducerWritesSeveral { .. } => "ProducerWritesSeveral",
            ComposeRefusal::ProducerDoesNotReturnInOneBlock { .. } => {
                "ProducerDoesNotReturnInOneBlock"
            }
            ComposeRefusal::ConsumerEntryIsABranchTarget { .. } => "ConsumerEntryIsABranchTarget",
            ComposeRefusal::ConsumerEntryDoesNotReturn { .. } => "ConsumerEntryDoesNotReturn",
            ComposeRefusal::ForwardedValueIsLaneDependent { .. } => "ForwardedValueIsLaneDependent",
            ComposeRefusal::StagedProducerIsNotAProducer { .. } => "StagedProducerIsNotAProducer",
            ComposeRefusal::StagedOutputIsReadByARecomputedProducer { .. } => {
                "StagedOutputIsReadByARecomputedProducer"
            }
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
            ComposeRefusal::NoConsumerToInline => {
                write!(
                    formatter,
                    "the composition names no consumer, so there is nothing to inline"
                )
            }
            ComposeRefusal::NoProducerToInline => write!(
                formatter,
                "the fan-in names no body, so there is nothing to inline into one fragment"
            ),
            ComposeRefusal::NoBranchToInline => write!(
                formatter,
                "the sibling set names no branch, so there is nothing to inline into one fragment"
            ),
            ComposeRefusal::OneBranchIsNotASiblingSet { branches } => write!(
                formatter,
                "the sibling set names {branches} branch(es), and a sibling set is two or more \
                 independent bodies that read one shared input"
            ),
            ComposeRefusal::OneBodyIsNotAFanIn { bodies } => write!(
                formatter,
                "the fan-in names {bodies} body(ies), and a fan-in is one consumer of several \
                 producers"
            ),
            ComposeRefusal::AbsorbedIsNotAnEarlierBody {
                body,
                producer,
                input,
            } => write!(
                formatter,
                "body {body} reads at input position {input} the value body {producer} forwards, \
                 and a body reads only what a body **before** it forwarded"
            ),
            ComposeRefusal::OneInputIsFilledTwice { body, input } => write!(
                formatter,
                "body {body} fills its input position {input} twice, so the union drops it once \
                 and two producers' values answer one read"
            ),
            ComposeRefusal::OneProducerFeedsTwoInputs { body, producer } => write!(
                formatter,
                "body {producer}'s value reaches body {body} at more than one input position, and \
                 one value has one slot in the merged fragment"
            ),
            ComposeRefusal::BodyDoesNotReturnInOneBlock { returns } => write!(
                formatter,
                "a body of the fan-in returns from {returns} block(s), and the next body lands in \
                 the block the one before it returns from"
            ),
            ComposeRefusal::ProducerIsNotWellFormed { cause } => {
                write!(formatter, "the producer's body is not well formed: {cause}")
            }
            ComposeRefusal::ConsumerIsNotWellFormed { cause } => {
                write!(formatter, "the consumer's body is not well formed: {cause}")
            }
            ComposeRefusal::ShapesDiffer {
                producer,
                consumer,
                blocker,
            } => write!(
                formatter,
                "the producer's domain {producer:?} and the consumer's {consumer:?} are not the \
                 same shape and not one with runtime scalars spliced in — a merged fragment has one \
                 parameter list, and it is the union of the two bodies' scalar leaves with each \
                 body bound to its own; {blocker}"
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
            ComposeRefusal::ParameterCountsDiffer {
                producer,
                consumer,
                leaves,
            } => write!(
                formatter,
                "the producer's entry block receives {producer} parameter(s) and the consumer's \
                 {consumer}, and the consumer's domain declares {leaves} leaf(ies), so the \
                 consumer has no leaf to bind position for position"
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
            ComposeRefusal::ProducerReadsOutsideTheMerge { position, inputs } => write!(
                formatter,
                "the producer reads input position {position}, and the merged fragment's {inputs} \
                 input(s) are the producer's own first and the consumer's surviving ones after \
                 them, so that position names a consumer's buffer and the producer's read would \
                 answer another one"
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
            ComposeRefusal::ConsumerEntryDoesNotReturn { consumer } => write!(
                formatter,
                "consumer {consumer} does not return from its entry block, and the next \
                 consumer's body is appended where that block leaves the merged body, so it would \
                 run on a path this consumer never takes"
            ),
            ComposeRefusal::ForwardedValueIsLaneDependent {
                value,
                read,
                blocker,
            } => write!(
                formatter,
                "the forwarded value {value:?} is reachable from the producer's invocation index, \
                 so a lane binding it in its own block reads another lane's value, and the read \
                 {read:?} asks the absorbed buffer at an address the producer's own cone cannot \
                 answer for: {blocker} — which is what a copy of the cone per read needs and \
                 cannot take (`docs/notes/graph-fusion.md` §5)"
            ),
            ComposeRefusal::StagedProducerIsNotAProducer { producer } => write!(
                formatter,
                "the caller names producer {producer} as staged, and it is not a producer of this \
                 fan-in that is staged once: the consumer stages nothing, and a producer is \
                 staged once"
            ),
            ComposeRefusal::StagedOutputIsReadByARecomputedProducer { staged, producer } => {
                write!(
                    formatter,
                    "producer {producer} is recomputed and reads producer {staged}'s output, and a \
                     copy of a recomputed producer's cone reads that output at an input ordinal — \
                     which names another buffer in the merged fragment, so the copy would answer \
                     another program's value"
                )
            }
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

/// How the producer's value reaches the consumer.
///
/// # Invariant
/// **One way or the other, never both**: the producer's cone is copied at the read's own
/// address, or the producer's store is kept and the consumer reads it back — and the second
/// is a *staged* body, which a barrier orders rather than a copy. The staged way writes the
/// producer's output too, and the consumer's read names that output.
enum Forwarding {
    /// The producer's cone, copied at each read's own address.
    Recomputed(Cone),
    /// The producer's store, kept and read back.
    Staged,
}

/// The consumer's values as the merged body holds them.
///
/// # Invariant
/// A bound value is one the merged body already has — the producer's parameters, and each
/// absorbed read once its copy has replaced it — and a landed value is one reserved for the
/// consumer's own; `resolve` reads both, and an id neither map holds is the producer's own.
pub(crate) struct Renumber {
    bound: HashMap<ValueId, ValueId>,
    landed: HashMap<ValueId, ValueId>,
}

impl Default for Renumber {
    fn default() -> Self {
        Renumber {
            bound: HashMap::new(),
            landed: HashMap::new(),
        }
    }
}

impl Renumber {
    /// Bind one of the consumer's values to an id the merged body already holds.
    pub(crate) fn bind(&mut self, value: ValueId, target: ValueId) {
        self.bound.insert(value, target);
    }

    /// The id `value` holds in the merged body.
    pub(crate) fn resolve(&self, value: ValueId) -> ValueId {
        self.bound
            .get(&value)
            .copied()
            .or_else(|| self.landed.get(&value).copied())
            .unwrap_or(value)
    }

    /// Reserve a value of `body` for `value`, unless it is bound to one that is already there.
    pub(crate) fn reserve(&mut self, value: ValueId, body: &mut KernelBody) -> Option<ValueId> {
        if self.bound.contains_key(&value) {
            return None;
        }
        let id = ValueId(body.values.len() as u32);
        body.values
            .push(ValueDef::BlockParam { block: 0, index: 0 });
        self.landed.insert(value, id);
        Some(id)
    }
}

/// One copy of `cone` at `address`, appended to `sequence`, and the value that answers it.
///
/// # Invariant
/// `sequence` already holds every value the copy reuses, so the copy's own values are appended in
/// the order `KernelBody::validate` reads. **A `Live` value is read through `live`**, the identity
/// when the producer's numbering is the merged body's own and a renumbering when it was spliced in.
pub(crate) fn copy_cone(
    cone: &Cone,
    root: ValueId,
    producer: &KernelBody,
    live: &impl Fn(ValueId) -> ValueId,
    address: ValueId,
    body: &mut KernelBody,
    fresh: &mut HashMap<ValueId, ValueId>,
    sequence: &mut Vec<ValueId>,
) -> ValueId {
    for &value in cone.order() {
        if cone.origin(value) != ConeOrigin::Rebuilt {
            continue;
        }
        let Some(ValueDef::Instr { op, args, classes }) = producer.values.get(value.0 as usize)
        else {
            continue;
        };
        let named: Vec<ValueId> = args
            .iter()
            .map(|&arg| match cone.origin(arg) {
                ConeOrigin::Address => address,
                ConeOrigin::Rebuilt => fresh.get(&arg).copied().unwrap_or(arg),
                ConeOrigin::Live(value) => live(value),
            })
            .collect();
        let id = ValueId(body.values.len() as u32);
        body.values.push(ValueDef::Instr {
            op: *op,
            args: named,
            classes: classes.clone(),
        });
        sequence.push(id);
        fresh.insert(value, id);
    }
    match cone.origin(root) {
        ConeOrigin::Rebuilt => fresh.get(&root).copied().unwrap_or(address),
        ConeOrigin::Live(value) => live(value),
        ConeOrigin::Address => address,
    }
}

/// Whether `forwarded` is reachable from the producer's invocation index.
pub(crate) fn lane_dependent(
    body: &KernelBody,
    forwarded: ValueId,
    parameters: &[ValueId],
) -> bool {
    parameters
        .last()
        .is_some_and(|&lane| body.depends_on(forwarded, lane, &body.incoming_arguments()))
}

/// The address the read `value` asks for.
pub(crate) fn read_address(body: &KernelBody, value: ValueId) -> ValueId {
    match body.values.get(value.0 as usize) {
        Some(ValueDef::Instr { args, .. }) if args.len() > 1 => args[1],
        _ => value,
    }
}

/// The class the producer's write declares, the value it forwards, and the address it fills.
pub(crate) fn write_edge(
    body: &KernelBody,
    write: ValueId,
) -> Result<(ScalarClass, ValueId, ValueId), ComposeRefusal> {
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
        [_, address, value] => Ok((*class, *value, *address)),
        _ => Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!(
                "the buffer write {write:?} reads {} operand(s) and stages a value with three",
                args.len()
            ),
        }),
    }
}

/// Every buffer write this body lists.
pub(crate) fn write_sites(body: &KernelBody) -> Vec<ValueId> {
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
pub(crate) fn buffer_reads(
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
pub(crate) fn branch_targeting(body: &KernelBody, target: usize) -> Option<usize> {
    body.blocks.iter().position(|block| {
        block
            .branches()
            .iter()
            .any(|branch| branch.target == target)
    })
}

/// The lowest slice ordinal this body's accesses address.
pub(crate) fn first_slice_ordinal(body: &KernelBody) -> u32 {
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
pub(crate) fn duplicated_mark(marks: &[KernelMark]) -> Result<(), ComposeRefusal> {
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
pub(crate) fn mapped_terminator(
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

/// The position constant each surviving read of the consumer takes in the merged body.
///
/// # Invariant
/// The union puts the producer's inputs first, then each consumer's survivors after the ones
/// already there, so a consumer's ordinal `c` lands at `base + c - (the absorbed ordinals below
/// it)`; an absorbed read is dropped and takes none, and each surviving read gets a fresh
/// constant **value**, because a read's position is a value and not an offset.
fn shifted_positions(
    merged: &mut KernelBody,
    consumer_reads: &[(ValueId, u32)],
    base: usize,
    absorbed: usize,
) -> HashMap<ValueId, ValueId> {
    shifted_past(merged, consumer_reads, base, &[absorbed])
}

/// [`shifted_positions`] over several absorbed ordinals, as a fan-in's consumer declares.
///
/// # Invariant
/// The `absorbed` ordinals are the body's **own** and in no order, so the count below an ordinal
/// is what the formula asks and a caller that lists one of them twice shifts a surviving read once
/// — which is why [`compose_fan_in`](crate::compose_fan_in) refuses a duplicated ordinal first.
pub(crate) fn shifted_past(
    merged: &mut KernelBody,
    consumer_reads: &[(ValueId, u32)],
    base: usize,
    absorbed: &[usize],
) -> HashMap<ValueId, ValueId> {
    let mut shifted = HashMap::new();
    for &(read, position) in consumer_reads {
        let position = position as usize;
        if absorbed.contains(&position) {
            continue;
        }
        let at = i64::try_from(
            base + position
                - absorbed
                    .iter()
                    .filter(|ordinal| **ordinal < position)
                    .count(),
        )
        .unwrap_or(i64::MAX);
        shifted.insert(read, position_constant(merged, at));
    }
    shifted
}

/// A fresh constant naming a buffer position, as a read's position is one.
///
/// # Invariant
/// **A position is a value, not an offset**: a read names its buffer through a constant, so a
/// read that moves to another port needs a constant of its own — the old one would still name the
/// port it read before.
pub(crate) fn position_constant(merged: &mut KernelBody, at: i64) -> ValueId {
    let id = ValueId(merged.values.len() as u32);
    merged.values.push(ValueDef::Instr {
        op: KernelInstr::Const(ScalarClass::Int, at),
        args: Vec::new(),
        classes: vec![ScalarClass::Int],
    });
    id
}

/// The position constant each write of a consumer takes in the merged body.
///
/// # Invariant
/// The merged fragment declares the consumers' outputs **summed**, so a consumer starting at `base`
/// names its own output `p` at `base + p`. A position that does not move takes **no** fresh value.
pub(crate) fn shifted_write_positions(
    merged: &mut KernelBody,
    consumer: &KernelBody,
    base: usize,
) -> HashMap<ValueId, ValueId> {
    let mut shifted = HashMap::new();
    for write in write_sites(consumer) {
        let Some(ValueDef::Instr { args, .. }) = consumer.values.get(write.0 as usize) else {
            continue;
        };
        let Some(&position) = args.first() else {
            continue;
        };
        let Some(value) = consumer.constant(Some(position)) else {
            continue;
        };
        let at = base.saturating_add(value as usize);
        if at == value as usize {
            continue;
        }
        shifted.insert(
            write,
            position_constant(merged, i64::try_from(at).unwrap_or(i64::MAX)),
        );
    }
    shifted
}

/// Drop every value nothing reads, and give the survivors fresh ids.
///
/// # Invariant
/// A block parameter nothing reads is removed with the branch argument that feeds it — an orphan
/// parameter passes `KernelBody::validate` and is refused only when the artifact is read, which
/// re-derives a parameter's index from the block that receives it.
pub(crate) fn compact(body: &mut KernelBody, marks: &mut [KernelMark]) {
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
    for expr in &body.barriers {
        pending.push(expr.write);
        pending.push(expr.read);
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
    for expr in &mut body.barriers {
        expr.write = renumbered(&map, expr.write);
        expr.read = renumbered(&map, expr.read);
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
