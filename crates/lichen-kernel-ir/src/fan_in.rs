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
    first_slice_ordinal, landed_slot, lane_dependent, mapped_terminator, position_constant,
    read_address, shapes_differ, shifted_past, shifted_write_positions, splice_runtime_scalars,
    write_edge, write_sites,
};
use crate::{
    Barrier, BasicBlock, ComposeRefusal, Cone, KernelFragment, KernelInstr, KernelMark,
    KernelRoles, KernelShape, NotIndexLocal, ScalarClass, Terminator, ValueDef, ValueId,
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
    compose_fan_in_staged(producers, &[])
}

/// The fragment the region `producers` becomes, with the named producers' stores kept, or the
/// decision it cannot make.
///
/// # Invariant
/// **A staged producer keeps its store, and the body that reads it reads the store back**: the
/// producer's output buffers are written by the merged body, the read that would have answered
/// them becomes an `OutputReadCall` at the producer's output ordinal, and a barrier names the
/// write with the read that observes it. A producer the caller did not name is recomputed, as
/// [`compose_fan_in`] does.
///
/// **Only a value no address re-derives is staged**: a staged producer's value is a loop-carried
/// accumulation, and a producer whose value another lane's read or a callee's own index decides
/// is refused rather than staged, because a store would carry it where it does not belong.
pub fn compose_fan_in_staged(
    producers: &[Producer<'_>],
    staged: &[usize],
) -> Result<KernelFragment, ComposeRefusal> {
    let [head, ..] = producers else {
        return Err(ComposeRefusal::NoProducerToInline);
    };
    if producers.len() < 2 {
        return Err(ComposeRefusal::OneBodyIsNotAFanIn {
            bodies: producers.len(),
        });
    }
    // **The staged producers are the caller's own list**, and each names a producer once.
    let mut is_staged = vec![false; producers.len()];
    for &producer in staged {
        let Some(slot) = is_staged.get_mut(producer) else {
            return Err(ComposeRefusal::StagedProducerIsNotAProducer { producer });
        };
        // **The consumer stages nothing**: it is the body that reads.
        if *slot || producer + 1 == producers.len() {
            return Err(ComposeRefusal::StagedProducerIsNotAProducer { producer });
        }
        *slot = true;
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
    // **One way per producer, asked of its own body**: the cone a read copies, or the store the
    // body keeps. A producer the caller named is staged, and a producer whose value no address
    // re-derives is refused unless the caller named it.
    let mut cones: Vec<Option<Cone>> = (0..edges.len()).map(|_| None).collect();
    // **The output ordinal each staged producer's store fills**, in the merged output space.
    let mut staged_ordinals: Vec<Option<u64>> = vec![None; edges.len()];
    // **Where each staged producer's own outputs land** in the merged output space.
    let mut output_base: Vec<usize> = vec![0; edges.len()];
    let mut staged_outputs = 0usize;
    for (body, edge) in edges.iter().enumerate() {
        if body + 1 == edges.len() {
            break;
        }
        let forward = forwards[body].as_ref().expect("a producer forwards");
        let read = edges
            .iter()
            .flat_map(|later| later.reads_absorbed.iter())
            .find(|(_, from, _)| *from == body)
            .map(|(read, _, _)| *read)
            .expect("a body but the last is absorbed from by the body after it");
        match forward.forwarding(&producers[body].fragment) {
            Ok(cone) => {
                if !is_staged[body] {
                    cones[body] = Some(cone);
                    continue;
                }
            }
            Err(blocker) => {
                // **Only a carried accumulation is staged**: the others name another lane's
                // value or an index of the callee's own, which a store would carry wrongly.
                if !is_staged[body] || !matches!(blocker, NotIndexLocal::Carried { .. }) {
                    return Err(ComposeRefusal::ForwardedValueIsLaneDependent {
                        value: forward.forwarded,
                        read,
                        blocker,
                    });
                }
            }
        }
        staged_ordinals[body] = Some(staged_ordinal(&producers[body].fragment, edge)?);
        output_base[body] = staged_outputs;
        staged_outputs += producers[body].fragment.outputs;
    }
    // **A staged producer's output is read back, and a recomputed producer's cone is not**: a
    // copy of that cone would read the staged buffer at an input ordinal, and that ordinal names
    // another buffer in the merged fragment.
    for (body, edge) in edges.iter().enumerate() {
        if body + 1 == edges.len() || is_staged[body] {
            continue;
        }
        if let Some(&(from, _)) = edge.absorbed.iter().find(|(from, _)| is_staged[*from]) {
            return Err(ComposeRefusal::StagedOutputIsReadByARecomputedProducer {
                staged: from,
                producer: body,
            });
        }
    }

    let mut body = first.body.clone();
    // **The runtime scalars are spliced into the merged body's own domain.**
    let total = scalars.total();
    splice_runtime_scalars(&mut body, scalars.own(0), total - scalars.own(0));
    // **The first producer's store goes** — unless it is staged, when it answers the body that
    // reads it.
    if !is_staged[0] {
        for block in &mut body.blocks {
            block.instrs.retain(|&value| value != edges[0].stores[0]);
        }
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
        // **The staged reads**: a read of a staged producer's output reads that output back.
        let mut staged_reads: HashMap<ValueId, ValueId> = HashMap::new();
        for &(read, from, _) in &edge.reads_absorbed {
            let Some(ordinal) = staged_ordinals[from] else {
                // The read's own id until the copy below replaces it, so it reserves no value of its own.
                renumber.bind(read, read);
                continue;
            };
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
            let produced = forwards[from]
                .as_ref()
                .expect("a producer forwards")
                .produced;
            if stated != produced {
                return Err(ComposeRefusal::ClassesDiffer {
                    produced,
                    expected: stated,
                });
            }
            // The read's position is the producer's output ordinal, in the merged output space.
            let at = output_base[from] + ordinal as usize;
            staged_reads.insert(read, position_constant(&mut body, at as i64));
        }
        let landed: Vec<Option<ValueId>> = (0..consumer.body.values.len())
            .map(|value| renumber.reserve(ValueId(value as u32), &mut body))
            .collect();
        // **Every surviving read takes the union's position for it**, the absorbed ones dropped.
        let absorbed: Vec<usize> = edge.absorbed.iter().map(|(_, ordinal)| *ordinal).collect();
        let shifted = shifted_past(&mut body, &edge.reads, unions[index], &absorbed);
        // **The merged body's output space**: the staged producers' own outputs come first, and
        // the consumer's fill what they leave.
        let writes = if index + 1 == edges.len() {
            shifted_write_positions(&mut body, &consumer.body, staged_outputs)
        } else if is_staged[index] {
            shifted_write_positions(&mut body, &consumer.body, output_base[index])
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
            .filter(|(_, from, _)| staged_ordinals[*from].is_none())
            .map(|&(read, from, at)| (read, (from, at)))
            .collect();
        // **Every store but the consumer's and the staged producers' goes**: the survivor's
        // outputs are the merged ones.
        let keeps_store = index + 1 == edges.len() || is_staged[index];
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
                        && let Some(head) = args.first_mut()
                    {
                        *head = constant;
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
        // **The staged path's barrier**: the store and the read that observes it.
        for &(read, from, _) in &edge.reads_absorbed {
            if staged_ordinals[from].is_none() {
                continue;
            }
            let write = forwards[from]
                .as_ref()
                .expect("a producer forwards")
                .write;
            // **The first body's values are the merged body's own**, and every later body's are
            // read through the map that landed them.
            let merged_write = renumbers[from]
                .as_ref()
                .map_or(write, |renumber| renumber.resolve(write));
            body.barriers.push(Barrier {
                write: merged_write,
                read: resolve(read),
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
    // **The merged output space**: the staged producers' own outputs, in body order, then the
    // consumer's.
    let mut output_classes: Vec<ScalarClass> = Vec::new();
    for (body, edge) in edges.iter().enumerate() {
        if body + 1 == edges.len() {
            break;
        }
        if is_staged[body] {
            output_classes.extend(edge.fragment.output_classes.iter().copied());
        }
    }
    output_classes.extend(consumer.output_classes.iter().copied());
    let fragment = KernelFragment {
        param_shape: scalars.shape(),
        roles: KernelRoles::default(),
        body,
        inputs: ports,
        outputs: staged_outputs + consumer.outputs,
        input_classes,
        output_classes,
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
    /// The store this producer's body holds, when it holds one.
    write: ValueId,
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
            write,
        })
    }

    /// The cone this value's recomputation at an address is built from, or what stops it.
    ///
    /// # Invariant
    /// **The producer's own body is asked**, exactly as the expr asks the producer's: a value
    /// that never reaches the invocation index is the same in every lane and is reused, and one
    /// that does is refused where the expr refuses it.
    fn forwarding(&self, fragment: &KernelFragment) -> Result<Cone, NotIndexLocal> {
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
            Err(blocker) => Err(blocker),
        }
    }
}

/// The output ordinal the producer's store fills, read off its write.
///
/// # Invariant
/// **The write names its output position, and that position is a constant**: a position that is
/// not a number names no ordinal, and the merged fragment's output space has no slot for it.
fn staged_ordinal(fragment: &KernelFragment, edge: &Edge<'_>) -> Result<u64, ComposeRefusal> {
    let [write] = edge.stores[..] else {
        return Err(if edge.stores.is_empty() {
            ComposeRefusal::ProducerWritesNothing
        } else {
            ComposeRefusal::ProducerWritesSeveral {
                writes: edge.stores.len(),
            }
        });
    };
    let Some(ValueDef::Instr { args, .. }) = fragment.body.values.get(write.0 as usize) else {
        return Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!("the buffer write {write:?} is not an instruction"),
        });
    };
    let Some(position) = args.first().copied() else {
        return Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!("the buffer write {write:?} names no output position"),
        });
    };
    match fragment.body.constant(Some(position)) {
        Some(ordinal) => Ok(ordinal),
        None => Err(ComposeRefusal::ProducerIsNotWellFormed {
            cause: format!(
                "the buffer write {write:?} names its output position {position:?}, which is not \
                 an integer constant"
            ),
        }),
    }
}

/// The producers whose value no address re-derives, so their store must answer the consumer.
///
/// # Invariant
/// **The producer's own body is asked**, exactly as the composition asks it: a value that never
/// reaches the invocation index is the same in every lane and is recomputed, and one that
/// reaches it and cannot be re-derived at another address is a carried accumulation, which
/// only a store can carry.
pub fn carried_producers(producers: &[Producer<'_>]) -> Vec<usize> {
    let mut carried = Vec::new();
    for (index, producer) in producers.iter().enumerate() {
        // **The consumer forwards nothing**: it is the body that reads.
        if index + 1 == producers.len() {
            break;
        }
        let fragment = producer.fragment;
        let writes = write_sites(&fragment.body);
        let [write] = writes[..] else {
            continue;
        };
        let Ok((_, forwarded, written_at)) = write_edge(&fragment.body, write) else {
            continue;
        };
        let parameters = fragment.body.parameters();
        let lane = parameters.last().copied();
        if let Err(NotIndexLocal::Carried { .. }) =
            fragment.body.index_local_cone(forwarded, written_at, lane)
            && lane_dependent(&fragment.body, forwarded, &parameters)
        {
            carried.push(index);
        }
    }
    carried
}

/// The block a body returns from, refused unless it returns from exactly one.
pub(crate) fn returns_of(fragment: &KernelFragment) -> Result<usize, ComposeRefusal> {
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
