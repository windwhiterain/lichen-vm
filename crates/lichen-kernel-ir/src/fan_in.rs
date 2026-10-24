//! Several producers and the one consumer that reads them become one fragment.
//! See `docs/notes/graph-fusion.md` §5.
//!
//! # Invariant
//! **One value per absorbed input, and every absorbed input named before a body is touched**: the
//! conditions are [`Edge`]'s, asked of each body in turn, so a fan-in this composition cannot
//! serve is refused while every fragment still stands alone. **Each body lands on the block the
//! one before it returns from**, which is where a producer's own loop is still there to land on.

use std::collections::HashMap;

use crate::compose::{
    Renumber, ScalarUnion, branch_targeting, buffer_reads, compact, copy_cone, duplicated_mark,
    first_slice_ordinal, landed_slot, lane_dependent, mapped_terminator, read_address,
    shapes_differ, shifted_past, shifted_write_positions, splice_runtime_scalars, write_edge,
    write_sites,
};
use crate::{
    Barrier, BasicBlock, ComposeRefusal, Cone, KernelFragment, KernelMark, KernelRoles,
    KernelShape, ScalarClass, Terminator, ValueDef, ValueId,
};

/// One body of a fan-in, and the producers whose values reach its inputs.
///
/// # Invariant
/// **Evaluation order, and every index in [`Self::absorbs`] names an earlier entry**: a body reads
/// only what a body before it forwarded, so the list is topologically ordered by construction and
/// the composition is not told an order it would have to take on trust. The last entry is the
/// consumer — the one body whose store survives, so nothing absorbs from it.
#[derive(Debug)]
pub struct Producer<'a> {
    /// The fragment to inline.
    pub fragment: &'a KernelFragment,
    /// `(the entry whose value arrives, the input ordinal it reaches)`, in input-ordinal order.
    ///
    /// # Invariant
    /// Each ordinal appears once and each producer at most once per body: one value, one slot. Two
    /// entries naming one producer would ask one value for two slots, which the symbolic language
    /// has no spelling for, and two entries naming one ordinal would drop an input from the union.
    pub absorbs: Vec<(usize, usize)>,
}

/// The fragment the region `producers` becomes, or the decision it cannot make.
///
/// # Invariant
/// **The bodies run in the list's order and the merged inputs are their union**, and **one cone
/// is built per producer**, asked of that producer's own body.
///
/// **The merged parameter list is the union of the bodies' scalar leaves**, each its own.
///
/// A third shape beside [`compose`](crate::compose): this lands each body on the block the one
/// before it returns from.
pub fn compose_fan_in(producers: &[Producer<'_>]) -> Result<KernelFragment, ComposeRefusal> {
    let [head, ..] = producers else {
        return Err(ComposeRefusal::NoProducerToInline);
    };
    if producers.len() < 2 {
        return Err(ComposeRefusal::OneBodyIsNotAFanIn {
            bodies: producers.len(),
        });
    }
    // **The first body is the merged body**, so its parameters and shape are the merged ones.
    let first = head.fragment;
    if first.input_classes.len() != first.inputs {
        return Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!(
                "the first producer declares {} input(s) and states {} class(es) for them",
                first.inputs,
                first.input_classes.len()
            ),
        });
    }
    let ports = ports(producers);
    // **The union of the bodies' domains**, in the order the bodies are composed.
    let shapes: Vec<&KernelShape> = producers
        .iter()
        .map(|producer| &producer.fragment.param_shape)
        .collect();
    let Some(scalars) = ScalarUnion::of(&shapes) else {
        return Err(shapes_differ(
            first,
            producers[producers.len() - 1].fragment,
            "one of the domains names no extent and index, so it has no runtime scalar to tell \
             from the index"
                .to_string(),
        ));
    };
    // **Every condition before a body is touched**, with every fragment still alone.
    let mut edges = Vec::with_capacity(producers.len());
    for (body, producer) in producers.iter().enumerate() {
        edges.push(Edge::of(producer, body, first, producers.len(), ports)?);
    }
    // **Every body binds its own domain**, or it has no leaf to bind.
    for (body, producer) in producers.iter().enumerate() {
        let fragment = producer.fragment;
        let parameters = fragment.body.parameters().len();
        let leaves = fragment.param_shape.flat_arity();
        if parameters == leaves {
            continue;
        }
        if body == 0 {
            return Err(shapes_differ(
                first,
                producers[producers.len() - 1].fragment,
                format!(
                    "the first body's entry block receives {parameters} parameter(s) and its \
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
    // **Where each body's surviving inputs land**: the first body's own stay where they are.
    let mut unions = vec![0; edges.len()];
    let mut union = first.inputs;
    for (body, edge) in edges.iter().enumerate().skip(1) {
        unions[body] = union;
        union = union.saturating_add(edge.fragment.inputs.saturating_sub(edge.absorbed.len()));
    }
    debug_assert_eq!(
        union, ports,
        "the counted union is the ports the fragment declares"
    );
    // **One forward per producer, asked of its own body**: the second is proved as the first.
    let mut forwards: Vec<Option<Forwarded>> = Vec::with_capacity(edges.len());
    for (body, edge) in edges.iter().enumerate() {
        if body + 1 == edges.len() {
            forwards.push(None);
            continue;
        }
        let forward = Forwarded::of(&producers[body].fragment, edge)?;
        for later in edges
            .iter()
            .filter(|later| later.absorbed_from(body).is_some())
        {
            let expected = later.fragment.input_classes[later.absorbed_from(body).unwrap()];
            if forward.produced != expected {
                return Err(ComposeRefusal::ClassesDiffer {
                    produced: forward.produced,
                    expected,
                });
            }
        }
        forwards.push(Some(forward));
    }
    // **One cone per producer**, refused here rather than at the first read that needs one.
    let mut cones: Vec<Option<Cone>> = Vec::with_capacity(edges.len());
    for (body, forward) in forwards.iter().enumerate() {
        cones.push(match forward {
            None => None,
            Some(forward) => {
                let read = edges
                    .iter()
                    .flat_map(|later| later.reads_absorbed.iter())
                    .find(|(_, from, _)| *from == body)
                    .map(|(read, _, _)| *read)
                    .expect("a body but the last is absorbed from by the body after it");
                Some(forward.cone(&producers[body].fragment, read)?)
            }
        });
    }

    let mut body = first.body.clone();
    // **The runtime scalars are spliced into the merged body's own domain.**
    let total = scalars.total();
    splice_runtime_scalars(&mut body, scalars.own(0), total - scalars.own(0));
    // **The first producer's store goes**: its buffer is not an output of the merged fragment.
    for block in &mut body.blocks {
        block.instrs.retain(|&value| value != edges[0].stores[0]);
    }
    let mut landing = returns_of(first)?;
    let mut marks: Vec<KernelMark> = Vec::new();
    let mut input_classes = first.input_classes.clone();
    let mut slices = first.slices.clone();
    let mut blocks_base = body.blocks.len();
    let first_parameters = body.blocks[body.entry].params.clone();
    // **Where each body's values landed**: a spliced producer's value is read through this.
    let mut renumbers: Vec<Option<Renumber>> = (0..edges.len()).map(|_| None).collect();
    for (index, edge) in edges.iter().enumerate().skip(1) {
        let consumer = edge.fragment;
        // **At most one body stages into slices**: two would claim one slice holds two values.
        if !slices.is_empty() && !consumer.slices.is_empty() {
            return Err(ComposeRefusal::SlicesCollide {
                producer: first_slice_ordinal(&first.body),
                consumer: first_slice_ordinal(&consumer.body),
            });
        }
        // The body's values are renumbered past the first body's, less what is bound below.
        let mut renumber = Renumber::default();
        for (slot, &parameter) in consumer.body.parameters().iter().enumerate() {
            renumber.bind(
                parameter,
                first_parameters
                    [landed_slot(slot, scalars.own(index), scalars.before(index), total)],
            );
        }
        for &(read, _, _) in &edge.reads_absorbed {
            // The read's own id until the copy below replaces it, so it reserves no value of its own.
            renumber.bind(read, read);
        }
        let landed: Vec<Option<ValueId>> = (0..consumer.body.values.len())
            .map(|value| renumber.reserve(ValueId(value as u32), &mut body))
            .collect();
        // **Every surviving read takes the union's position for it**, the absorbed ones dropped.
        let absorbed: Vec<usize> = edge.absorbed.iter().map(|(_, ordinal)| *ordinal).collect();
        let shifted = shifted_past(&mut body, &edge.reads, unions[index], &absorbed);
        // **The consumer's own output is the merged body's first**, so no write of it moves.
        let writes = if index + 1 == edges.len() {
            shifted_write_positions(&mut body, &consumer.body, 0)
        } else {
            HashMap::new()
        };
        let base = blocks_base;
        let mapped_block = |block: usize| -> usize {
            if block == consumer.body.entry {
                landing
            } else {
                base + block - usize::from(block > consumer.body.entry)
            }
        };
        let absorbed_at: HashMap<ValueId, (usize, ValueId)> = edge
            .reads_absorbed
            .iter()
            .map(|&(read, from, at)| (read, (from, at)))
            .collect();
        // **Every store but the consumer's goes**: the survivor's outputs are the merged ones.
        let keeps_store = index + 1 == edges.len();
        // One copy of the right producer's cone per read, at that read's own address.
        let mut shared: HashMap<(usize, ValueId, usize), (usize, ValueId)> = HashMap::new();
        let mut sequences: Vec<Vec<ValueId>> = Vec::with_capacity(consumer.body.blocks.len());
        for (block_index, block) in consumer.body.blocks.iter().enumerate() {
            let mut sequence: Vec<ValueId> = Vec::with_capacity(block.instrs.len());
            let merged_block = mapped_block(block_index);
            for &value in &block.instrs {
                if !keeps_store && edge.stores.contains(&value) {
                    continue;
                }
                let Some(&(from, at)) = absorbed_at.get(&value) else {
                    // The position is a constant **value**, so it stands before the read that uses it.
                    if let Some(&constant) = shifted.get(&value).or_else(|| writes.get(&value)) {
                        sequence.push(constant);
                    }
                    sequence.push(renumber.resolve(value));
                    continue;
                };
                let cone = cones[from]
                    .as_ref()
                    .expect("every producer but the consumer has a cone");
                // **One copy per address per block per producer**: the landing block dominates them.
                let address = renumber.resolve(at);
                let key = (merged_block, address, from);
                let root = match shared.get(&key) {
                    Some(&(placed, root)) if placed == landing || placed == merged_block => root,
                    _ => {
                        let mut fresh: HashMap<ValueId, ValueId> = HashMap::new();
                        // **A spliced-in producer renumbers its own values**, so the copy reads
                        // them through the map that landed it.
                        let live = |value: ValueId| {
                            renumbers[from]
                                .as_ref()
                                .map_or(value, |renumber| renumber.resolve(value))
                        };
                        let root = copy_cone(
                            cone,
                            forwards[from]
                                .as_ref()
                                .expect("a producer forwards")
                                .forwarded,
                            &producers[from].fragment.body,
                            &live,
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
        // **After the copies**: an operand that read an absorbed buffer now reads that producer.
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
                    if let Some(&constant) = shifted
                        .get(&ValueId(value as u32))
                        .or_else(|| writes.get(&ValueId(value as u32)))
                        && let Some(head) = args.first_mut()
                    {
                        *head = constant;
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
        for (block_index, block) in consumer.body.blocks.iter().enumerate() {
            if block_index == consumer.body.entry {
                continue;
            }
            body.blocks.push(BasicBlock {
                params: block.params.iter().map(|&param| resolve(param)).collect(),
                instrs: std::mem::take(&mut sequences[block_index]),
                terminator: mapped_terminator(&block.terminator, &resolve, &mapped_block),
            });
        }
        // **Each body lands where the body before it returns from** and takes over the
        // terminator of that block.
        body.blocks[landing]
            .instrs
            .extend(std::mem::take(&mut sequences[consumer.body.entry]));
        body.blocks[landing].terminator = mapped_terminator(
            &consumer.body.blocks[consumer.body.entry].terminator,
            &resolve,
            &mapped_block,
        );
        for expr in &consumer.body.barriers {
            body.barriers.push(Barrier {
                write: resolve(expr.write),
                read: resolve(expr.read),
            });
        }
        // Only the consumer's marks are kept: a merged node carries no claim to read them.
        if index + 1 == edges.len() {
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
                .filter(|(ordinal, _)| !absorbed.contains(ordinal))
                .map(|(_, class)| *class),
        );
        if slices.is_empty() {
            slices.clone_from(&consumer.slices);
        }
        blocks_base += consumer.body.blocks.len() - 1;
        // The block this body returns from is where the next one lands; the consumer keeps its own.
        if index + 1 < edges.len() {
            landing = mapped_block(returns_of(consumer)?);
        }
        // **Every value this body landed is read through the map that landed it**.
        renumbers[index] = Some(renumber);
    }

    compact(&mut body, &mut marks);
    let consumer = edges[edges.len() - 1].fragment;
    let fragment = KernelFragment {
        param_shape: scalars.shape(),
        roles: KernelRoles::default(),
        body,
        inputs: ports,
        outputs: consumer.outputs,
        input_classes,
        output_classes: consumer.output_classes.clone(),
        result_classes: consumer.result_classes.clone(),
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

/// One body of a fan-in: what it reads, what it fills, and what it drops.
///
/// # Invariant
/// **Every condition is checked before a body is touched**: the `absorbed` ordinals are resolved
/// against the body's own inputs, each named producer is an earlier entry of the same list, and
/// the shape, the parameter list and the mark facts are asked of each body — so an edge this
/// composition cannot serve is refused while every fragment still stands alone.
struct Edge<'a> {
    fragment: &'a KernelFragment,
    /// `(the producer's index in the list, this body's input ordinal)`, in input-ordinal order.
    absorbed: Vec<(usize, usize)>,
    /// Every buffer read of the body, with the position it addresses.
    reads: Vec<(ValueId, u32)>,
    /// The reads that fill an absorbed slot, with the producer each takes and its address.
    reads_absorbed: Vec<(ValueId, usize, ValueId)>,
    /// The body's own stores, which the merged body drops.
    stores: Vec<ValueId>,
}

impl<'a> Edge<'a> {
    /// Everything one body must answer before the merged body is built.
    fn of(
        producer: &Producer<'a>,
        body: usize,
        first: &KernelFragment,
        bodies: usize,
        ports: usize,
    ) -> Result<Edge<'a>, ComposeRefusal> {
        let fragment = producer.fragment;
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
                    "the body declares {} input(s) and states {} class(es) for them",
                    fragment.inputs,
                    fragment.input_classes.len()
                ),
            });
        }
        duplicated_mark(&fragment.marks)?;
        if let Some(block) = branch_targeting(&fragment.body, fragment.body.entry) {
            return Err(ComposeRefusal::ConsumerEntryIsABranchTarget { block });
        }
        // **The block the next body lands on**, so a body must return from exactly one.
        returns_of(fragment)?;
        let stores = write_sites(&fragment.body);
        // **Only the consumer keeps its store**, so only a producer is held to writing one buffer.
        if body + 1 < bodies && stores.len() != 1 {
            return Err(if stores.is_empty() {
                ComposeRefusal::ProducerWritesNothing
            } else {
                ComposeRefusal::ProducerWritesSeveral {
                    writes: stores.len(),
                }
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
        let mut absorbed: Vec<(usize, usize)> = Vec::with_capacity(producer.absorbs.len());
        let mut filled: Vec<usize> = Vec::with_capacity(producer.absorbs.len());
        for &(from, ordinal) in &producer.absorbs {
            if from >= body {
                return Err(ComposeRefusal::AbsorbedIsNotAnEarlierBody {
                    body,
                    producer: from,
                    input: ordinal,
                });
            }
            if ordinal >= fragment.inputs {
                return Err(ComposeRefusal::AbsorbedIsOutOfRange {
                    absorbed: ordinal as u32,
                    inputs: fragment.inputs,
                });
            }
            if filled.contains(&ordinal) {
                return Err(ComposeRefusal::OneInputIsFilledTwice {
                    body,
                    input: ordinal,
                });
            }
            if let Some((other, _)) = absorbed.iter().find(|(held, _)| *held == from) {
                return Err(ComposeRefusal::OneProducerFeedsTwoInputs {
                    body,
                    producer: *other,
                });
            }
            if !reads
                .iter()
                .any(|(_, position)| *position as usize == ordinal)
            {
                return Err(ComposeRefusal::AbsorbedIsNotRead {
                    absorbed: ordinal as u32,
                });
            }
            filled.push(ordinal);
            absorbed.push((from, ordinal));
        }
        // **Sorted by the body's own input ordinal**, so the union shift counts them in that order.
        absorbed.sort_unstable_by_key(|(_, ordinal)| *ordinal);
        let reads_absorbed = reads
            .iter()
            .filter(|(_, position)| absorbed.iter().any(|(_, held)| *held == *position as usize))
            .map(|&(read, position)| {
                let ordinal = position as usize;
                let &(from, _) = absorbed
                    .iter()
                    .find(|(_, held)| *held == ordinal)
                    .expect("a filled ordinal names one entry of the same list");
                (read, from, read_address(&fragment.body, read))
            })
            .collect();
        Ok(Edge {
            fragment,
            absorbed,
            reads,
            reads_absorbed,
            stores,
        })
    }

    /// The input ordinal at which `producer`'s value reaches this body, when it does.
    fn absorbed_from(&self, producer: usize) -> Option<usize> {
        self.absorbed
            .iter()
            .find(|(from, _)| *from == producer)
            .map(|(_, ordinal)| *ordinal)
    }
}

/// What one producer's body forwards: the class it stages, the value, and the address it fills.
struct Forwarded {
    produced: ScalarClass,
    forwarded: ValueId,
    written_at: ValueId,
}

impl Forwarded {
    /// The one store this producer's body holds, and what it stages.
    fn of(fragment: &KernelFragment, edge: &Edge<'_>) -> Result<Forwarded, ComposeRefusal> {
        let [write] = edge.stores[..] else {
            return Err(if edge.stores.is_empty() {
                ComposeRefusal::ProducerWritesNothing
            } else {
                ComposeRefusal::ProducerWritesSeveral {
                    writes: edge.stores.len(),
                }
            });
        };
        let (produced, forwarded, written_at) = write_edge(&fragment.body, write)?;
        Ok(Forwarded {
            produced,
            forwarded,
            written_at,
        })
    }

    /// The cone this value's recomputation at an address is built from.
    ///
    /// # Invariant
    /// **The producer's own body is asked**, exactly as the expr asks the producer's: a value
    /// that never reaches the invocation index is the same in every lane and is reused, and one
    /// that does is refused where the expr refuses it.
    fn cone(&self, fragment: &KernelFragment, read: ValueId) -> Result<Cone, ComposeRefusal> {
        let parameters = fragment.body.parameters();
        match fragment.body.index_local_cone(
            self.forwarded,
            self.written_at,
            parameters.last().copied(),
        ) {
            Ok(cone) => Ok(cone),
            Err(_) if !lane_dependent(&fragment.body, self.forwarded, &parameters) => {
                Ok(Cone::reused(self.forwarded))
            }
            Err(blocker) => Err(ComposeRefusal::ForwardedValueIsLaneDependent {
                value: self.forwarded,
                read,
                blocker,
            }),
        }
    }
}

/// The block a body returns from, refused unless it returns from exactly one.
fn returns_of(fragment: &KernelFragment) -> Result<usize, ComposeRefusal> {
    let returns: Vec<usize> = fragment
        .body
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| matches!(block.terminator, Terminator::Return { .. }))
        .map(|(block, _)| block)
        .collect();
    let [block] = returns[..] else {
        return Err(ComposeRefusal::BodyDoesNotReturnInOneBlock {
            returns: returns.len(),
        });
    };
    Ok(block)
}

/// How many input ports the merged fragment declares.
///
/// # Invariant
/// Every body's own inputs less the ones an earlier body fills, counted forward: the union is
/// `first.inputs` and each later body adds what survives of it, so the ports are that sum and no
/// other ordering is read off it.
fn ports(producers: &[Producer<'_>]) -> usize {
    producers
        .iter()
        .map(|producer| producer.fragment.inputs)
        .sum::<usize>()
        .saturating_sub(
            producers
                .iter()
                .map(|producer| producer.absorbs.len())
                .sum::<usize>(),
        )
}
