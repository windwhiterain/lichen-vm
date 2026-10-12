//! The region merge pass: a producer→consumer region becomes one node.
//! See docs/notes/graph-fusion.md §5 and §8.
//!
//! # Invariant
//! The pass merges and never reorders: the surviving node stands at the region's last position,
//! every other value keeps its order, and a region the pass declines changes nothing.
//!
//! A pack decides **which** regions are merged, never **what** a merge must satisfy: every
//! legality condition refuses by name under a pack exactly as it does under the floor.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::compute::{GraphPass, LaunchExtent, claimed_kernels, set_graph_pass};
use lichen_graph_ir::{Count, Graph, KernelNode, Node, ValueId as GraphValueId};
use lichen_kernel_ir::{ComposeRefusal, KernelFragment, ScalarClass, compose_fan_out};
use lichen_lowlevel::Module;

use crate::fusion_claim::{
    Instance, PortKind, REDUCTION, RegionSchema, SchemaHash, SchemaRefusal, claim_of,
    derive_regions, fragment_of, kernel_id, offsets, region_positions,
};
use crate::language::LangProgram;
use crate::placement_pack::{ConstraintRefusal, Fusion, PlacementPack};

/// Install the region merge pass for `module`, with no pack installed.
pub fn install_region_merge(module: &Module<LangProgram>) {
    install_region_merge_with_packs(module, &[]);
}

/// Install the region merge pass for `module`, with the packs a runner installed there.
///
/// # Invariant
/// The packs are consulted in order and the first whose `fuse` names a region's schema and whose
/// constraints hold licenses that region; a region no pack names is left to the floor, which
/// merges only what a declaration lets it prove (`docs/notes/graph-fusion.md` §5).
pub fn install_region_merge_with_packs(module: &Module<LangProgram>, packs: &[PlacementPack]) {
    set_graph_pass(region_merge_pass_with_packs(module, packs));
}

/// The pass `install_region_merge` installs, for a caller that wants to wrap it.
pub fn region_merge_pass(module: &Module<LangProgram>) -> Box<dyn GraphPass> {
    region_merge_pass_with_packs(module, &[])
}

/// The pass `install_region_merge_with_packs` installs, for a caller that wants to wrap it.
pub fn region_merge_pass_with_packs(
    module: &Module<LangProgram>,
    packs: &[PlacementPack],
) -> Box<dyn GraphPass> {
    Box::new(RegionMerge {
        instances: instances(module),
        packs: packs.to_vec(),
    })
}

/// The pass: a module's claimed kernels, joined to a graph node by its fragment digest.
struct RegionMerge {
    /// One instance per claimed kernel the module's own values name.
    instances: Vec<Instance>,
    /// The installed packs, in the order they are consulted.
    packs: Vec<PlacementPack>,
}

/// What licensed one region's merge, as the report names it.
enum Licence<'a> {
    /// The engine's own proof: every node claims the reduction chain and nothing else.
    Floor,
    /// A pack whose `fuse` names this region's schema and whose constraints hold.
    Pack(&'a PlacementPack),
}

/// One region's verdict: the line it reports, and its merge when it has one.
struct Decision {
    /// The line the region reports, merged or refused.
    line: String,
    /// The merge the region decided, when it may merge.
    merge: Option<Merge>,
}

/// One region's merge, as the rebuild reads it.
#[derive(Clone)]
struct Merge {
    /// The graph positions of the deleted nodes: the producer, and in a fan-out every consumer
    /// but the last.
    absorbed: Vec<usize>,
    /// The graph position of the node whose body survives: the region's last consumer.
    survivor: usize,
    /// One absorbed input position per consumer, in region order.
    absorbed_input: Vec<usize>,
    /// The graph values the merged fragment's outputs fill, in the order the consumers were named.
    outputs: Vec<GraphValueId>,
    /// The fragment the surviving node holds.
    fragment: KernelFragment,
    /// The surviving node's inputs: the producer's, then each consumer's but the absorbed one.
    inputs: Vec<GraphValueId>,
}

impl GraphPass for RegionMerge {
    /// Merge every two-node region whose legality conditions hold.
    ///
    /// # Invariant
    /// Every node is looked at and named: a region reports its nodes once, merged or refused, and
    /// a node no region holds — no claim names its fragment — is refused by name before the
    /// regions' lines. A merged region's line names the licence that answered for it. The graph
    /// handed over is the recording every time, because a rewrite is stored under its own digest.
    fn apply(
        &self,
        graph: &Graph,
        _extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let deferred;
        let instances = if self.instances.is_empty() {
            deferred = registry_instances();
            &deferred
        } else {
            &self.instances
        };
        let starts = offsets(graph);
        let positions = region_positions(graph, instances);
        // A node no region holds is named too: "no fusible region" and "no claim" are two answers.
        for position in skipped(graph, &positions) {
            lines.push(unclaimed(position));
        }
        let derived = derive_regions(graph, instances);
        let mut decisions: Vec<Decision> = Vec::new();
        for (index, (region, positions)) in derived.iter().zip(positions.iter()).enumerate() {
            let (line, merge) = match region {
                Err(refusal) => (
                    refused_line(index, positions, refusal.name(), &refusal.message()),
                    None,
                ),
                Ok(schema) => {
                    // The licence says the merge may be *attempted*; every condition below
                    // stands whatever answered it.
                    match licence(&schema, positions, &self.packs).and_then(|licensed| {
                        merge_region(graph, &starts, positions).map(|m| (licensed, m))
                    }) {
                        Ok((licensed, merge)) => (
                            merged_line(index, &merge, &licensed, &schema.hash),
                            Some(merge),
                        ),
                        Err(refusal) => (
                            refused_line(index, positions, refusal.name(), &refusal.message()),
                            None,
                        ),
                    }
                }
            };
            decisions.push(Decision { line, merge });
        }
        let merges: HashMap<usize, Merge> = decisions
            .iter()
            .filter_map(|decision| {
                decision
                    .merge
                    .as_ref()
                    .map(|merge| (merge.survivor, merge.clone()))
            })
            .collect();
        if merges.is_empty() {
            lines.extend(decisions.into_iter().map(|decision| decision.line));
            return None;
        }
        match rebuild(graph, &starts, &merges) {
            Some(rebuilt) => {
                lines.extend(decisions.into_iter().map(|decision| decision.line));
                Some(rebuilt)
            }
            None => {
                lines.extend(
                    decisions
                        .into_iter()
                        .filter(|decision| decision.merge.is_none())
                        .map(|decision| decision.line),
                );
                lines.push("a region merged and the graph could not be rebuilt".to_string());
                None
            }
        }
    }
}

/// Why one region's two nodes did not become one.
///
/// # Invariant
/// One variant per fact the merge cannot decide for itself, and each names what it met: a decision
/// guessed here would be a node that runs and answers another program.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeRefusal {
    /// The region is not one producer and one consumer.
    RegionIsNotAPair {
        /// How many instances the region holds.
        nodes: usize,
    },
    /// The consumer does not read the producer's output at exactly one input position.
    AbsorbedEdgeIsNotOne {
        /// How many of the consumer's input positions hold a producer output.
        inputs: usize,
    },
    /// The graph records no return, so a caller takes every value the merge would delete.
    GraphReturnsEveryValue {
        /// The value the merge deletes.
        value: GraphValueId,
    },
    /// A value the merge deletes is one the graph returns.
    ProducedValueIsReturned {
        /// The value the graph returns.
        value: GraphValueId,
    },
    /// A value the merge deletes is read outside the edge the merge absorbs.
    ProducedValueIsReadElsewhere {
        /// The value the merge deletes.
        value: GraphValueId,
        /// The node that reads it.
        node: usize,
    },
    /// The two nodes do not dispatch over the same extent.
    ExtentsDiffer {
        /// The producer's extent.
        producer: Count,
        /// The consumer's extent.
        consumer: Count,
    },
    /// The producer's absorbed output is not the class the consumer's input expects.
    ClassesDisagree {
        /// The class the producer's output states.
        produced: ScalarClass,
        /// The class the consumer's absorbed input declares.
        expected: ScalarClass,
    },
    /// One of the two fragments states no class for the absorbed buffer.
    ClassIsNotStated {
        /// The class the producer's output states, if it states one.
        produced: Option<ScalarClass>,
        /// The class the consumer's input declares, if it declares one.
        expected: Option<ScalarClass>,
    },
    /// A buffer the union names at a position the merged fragment has no port for.
    PortsDoNotCorrespond {
        /// The merged node's input position.
        position: usize,
        /// The value the union puts there.
        producer: GraphValueId,
        /// The value the merged fragment puts there, if it has that many inputs.
        merged: Option<GraphValueId>,
    },
    /// `compose` refused the two fragments.
    CompositionRefused {
        /// What `compose` refused.
        refusal: ComposeRefusal,
    },
    /// The region's edges are not one producer-to-consumer chain: a node two of them read from.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::InterfacesAreNotProved`] because it is a distinct fact: that
    /// one is what a node **claims**, this one how the region's values **flow**. A fan-out of
    /// reductions is refused here and not there, because the floor's own proof is the chain.
    NotAReductionChain {
        /// The graph node whose edges are not a chain's.
        node: usize,
        /// A pack that named this region and whose constraints did not hold, when one was
        /// consulted.
        inert: Option<(String, ConstraintRefusal)>,
    },
    /// The region holds an interface whose merge this engine has no algebra for.
    InterfacesAreNotProved {
        /// The interfaces the region's nodes claim, other than the reduction's.
        interfaces: Vec<String>,
        /// The region's schema, which is what a pack's `fuse` must name.
        region: SchemaHash,
        /// A pack that named it and whose constraints did not hold, when one was consulted.
        inert: Option<(String, ConstraintRefusal)>,
    },
}

impl MergeRefusal {
    /// The refusal's name, for a report that has to say which check refused a merge.
    ///
    /// # Invariant
    /// One variant, one name, and a composed refusal is named by `compose`'s own, so the name a
    /// report prints is the check a caller would match against.
    pub fn name(&self) -> &'static str {
        match self {
            MergeRefusal::RegionIsNotAPair { .. } => "RegionIsNotAPair",
            MergeRefusal::AbsorbedEdgeIsNotOne { .. } => "AbsorbedEdgeIsNotOne",
            MergeRefusal::GraphReturnsEveryValue { .. } => "GraphReturnsEveryValue",
            MergeRefusal::ProducedValueIsReturned { .. } => "ProducedValueIsReturned",
            MergeRefusal::ProducedValueIsReadElsewhere { .. } => "ProducedValueIsReadElsewhere",
            MergeRefusal::ExtentsDiffer { .. } => "ExtentsDiffer",
            MergeRefusal::ClassesDisagree { .. } => "ClassesDisagree",
            MergeRefusal::ClassIsNotStated { .. } => "ClassIsNotStated",
            MergeRefusal::PortsDoNotCorrespond { .. } => "PortsDoNotCorrespond",
            MergeRefusal::CompositionRefused { refusal } => refusal.name(),
            MergeRefusal::NotAReductionChain { .. } => "NotAReductionChain",
            MergeRefusal::InterfacesAreNotProved { .. } => "InterfacesAreNotProved",
        }
    }

    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for MergeRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MergeRefusal::RegionIsNotAPair { nodes } => write!(
                formatter,
                "the region holds {nodes} instance(s), and a merge is one producer and one consumer"
            ),
            MergeRefusal::AbsorbedEdgeIsNotOne { inputs } => write!(
                formatter,
                "the consumer reads the producer's output at {inputs} of its input position(s), and \
                 a merge absorbs exactly one"
            ),
            MergeRefusal::GraphReturnsEveryValue { value } => write!(
                formatter,
                "the graph records no return, so a caller takes every value — including {value}, \
                 which the merge deletes"
            ),
            MergeRefusal::ProducedValueIsReturned { value } => write!(
                formatter,
                "graph value {value} is one the graph returns, and the merge deletes the node that \
                 produces it"
            ),
            MergeRefusal::ProducedValueIsReadElsewhere { value, node } => write!(
                formatter,
                "graph value {value} is read at node {node} outside the edge the merge absorbs, so \
                 deleting its producer would drop a read"
            ),
            MergeRefusal::ExtentsDiffer { producer, consumer } => write!(
                formatter,
                "the producer dispatches over {producer:?} and the consumer over {consumer:?}, and \
                 a merged node has one extent"
            ),
            MergeRefusal::ClassesDisagree { produced, expected } => write!(
                formatter,
                "the producer's absorbed output is a {produced:?} element and the consumer's input \
                 declares {expected:?}, so the forwarded value would be read at the wrong class"
            ),
            MergeRefusal::ClassIsNotStated { produced, expected } => write!(
                formatter,
                "one of the two fragments states no class for the absorbed buffer: the producer \
                 says {produced:?} and the consumer {expected:?}"
            ),
            MergeRefusal::PortsDoNotCorrespond {
                position,
                producer,
                merged,
            } => write!(
                formatter,
                "the merged node reads graph value {producer} at input position {position}, and \
                 the merged fragment puts {merged:?} there"
            ),
            MergeRefusal::CompositionRefused { refusal } => write!(formatter, "{refusal}"),
            MergeRefusal::NotAReductionChain { node, inert } => {
                write!(
                    formatter,
                    "graph node {node}'s edges inside the region do not make one \
                     producer-to-consumer chain, so the floor's own proof cannot hold it, and a \
                     merge is licensed only by a pack whose `fuse` names this region's schema"
                )?;
                write_inert(formatter, inert)
            }
            MergeRefusal::InterfacesAreNotProved {
                interfaces,
                region,
                inert,
            } => {
                write!(
                    formatter,
                    "the region's node(s) claim {}, and a merge is licensed by the reduction chain \
                     this pass can prove or by a pack whose `fuse` names this region's schema \
                     {region}: no installed pack asks for this region",
                    interfaces.join(", ")
                )?;
                write_inert(formatter, inert)
            }
        }
    }
}

/// The pack that asked for this region and could not have it, when one did.
fn write_inert(
    formatter: &mut fmt::Formatter<'_>,
    inert: &Option<(String, ConstraintRefusal)>,
) -> fmt::Result {
    let Some((pack, refusal)) = inert else {
        return Ok(());
    };
    write!(
        formatter,
        " (the pack \"{pack}\" named it and its constraints did not hold: {})",
        refusal.message()
    )
}

/// The graph positions no region holds, in evaluation order.
///
/// # Invariant
/// The complement of [`region_positions`], read off that grouping rather than off a second join:
/// the grouping is what decides a node carries no claim, so a node it skipped is exactly a node
/// no instance's fragment is, and the two answers cannot disagree.
fn skipped(graph: &Graph, positions: &[Vec<usize>]) -> Vec<usize> {
    let mut grouped = vec![false; graph.len()];
    for region in positions {
        for &position in region {
            grouped[position] = true;
        }
    }
    (0..graph.len())
        .filter(|position| !grouped[*position])
        .collect()
}

/// The algebra this pass must be able to prove before it merges a region, or the pack that
/// decided it does.
///
/// # Invariant
/// The floor's licence is the reduction **chain**: a claim states what a kernel computes, not
/// that inlining pays. A region two edges leave, or two arrive at, is not that chain whatever
/// its nodes claim, so it is attempted **only** where a pack's `fuse` names its hash.
///
/// A pack changes which regions are attempted, never what must hold.
fn licence<'a>(
    schema: &RegionSchema,
    positions: &[usize],
    packs: &'a [PlacementPack],
) -> Result<Licence<'a>, MergeRefusal> {
    let mut inert = None;
    for pack in packs {
        let asks = pack.fuse().is_some_and(|directive| {
            directive.region() == Some(&schema.hash) && directive.form() == Fusion::OneKernel
        });
        if !asks {
            continue;
        }
        match pack.holds_for_class(first_input_class(schema)) {
            Ok(()) => return Ok(Licence::Pack(pack)),
            Err(refusal) => {
                if inert.is_none() {
                    inert = Some((pack.name().to_string(), refusal));
                }
            }
        }
    }
    // The chain first: a fan-out is refused by the shape's own name, not by its interfaces.
    if let Some(local) = schema.off_chain_node() {
        return Err(MergeRefusal::NotAReductionChain {
            node: positions[local],
            inert,
        });
    }
    let interfaces: BTreeSet<&str> = schema
        .nodes
        .iter()
        .map(|node| node.interface.as_str())
        .filter(|interface| *interface != REDUCTION)
        .collect();
    if interfaces.is_empty() {
        return Ok(Licence::Floor);
    }
    Err(MergeRefusal::InterfacesAreNotProved {
        interfaces: interfaces.into_iter().map(str::to_string).collect(),
        region: schema.hash.clone(),
        inert,
    })
}

/// The element class of the region's first input port, which is the one a pack's `requires`
/// is checked against.
///
/// # Invariant
/// One rule with the tiling pass's, read off two sources: a node's first input buffer is the
/// buffer an intra-kernel staging folds, and a region's first input port is the buffer its first
/// node reads that no node in the region produced. A region with no input port states no class,
/// so a pack needing one is inert rather than checked against something else.
fn first_input_class(schema: &RegionSchema) -> Option<ScalarClass> {
    schema
        .ports
        .iter()
        .find(|port| port.kind == PortKind::Input)
        .and_then(|port| port.class)
}

/// The merge one region's nodes decide, or why they do not.
///
/// # Invariant
/// **A producer and the consumers that read it**, in evaluation order: every position but the
/// first must read the first, because `compose` inlines consumers *of one producer* and no other
/// edge is a shape it serves. A merge that slipped past a condition would be a node reading a
/// buffer the program never wired to it, and the union cannot make one — `PortsDoNotCorrespond`.
fn merge_region(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
) -> Result<Merge, MergeRefusal> {
    let producer = *positions
        .first()
        .ok_or(MergeRefusal::RegionIsNotAPair { nodes: 0 })?;
    let consumers = &positions[1..];
    if consumers.is_empty()
        || !consumers
            .iter()
            .all(|position| reads(graph, starts, producer, *position))
    {
        return Err(MergeRefusal::RegionIsNotAPair {
            nodes: positions.len(),
        });
    }
    let Node::Kernel(producer_node) = &graph.nodes()[producer];
    let produced = starts[producer]..starts[producer] + producer_node.fragment.outputs;
    // One absorbed position per consumer, and the value each of them reads there.
    let mut absorbed_input: Vec<usize> = Vec::with_capacity(consumers.len());
    let mut absorbed_value: Vec<GraphValueId> = Vec::with_capacity(consumers.len());
    for &position in consumers {
        let Node::Kernel(consumer_node) = &graph.nodes()[position];
        let absorbed: Vec<usize> = consumer_node
            .inputs
            .iter()
            .enumerate()
            .filter(|(_, value)| produced.contains(value))
            .map(|(ordinal, _)| ordinal)
            .collect();
        let [ordinal] = absorbed[..] else {
            return Err(MergeRefusal::AbsorbedEdgeIsNotOne {
                inputs: absorbed.len(),
            });
        };
        absorbed_input.push(ordinal);
        absorbed_value.push(consumer_node.inputs[ordinal]);
    }
    let Some(returns) = graph.returns() else {
        return Err(MergeRefusal::GraphReturnsEveryValue {
            value: absorbed_value[0],
        });
    };
    if let Some(value) = produced.clone().find(|value| returns.contains(value)) {
        return Err(MergeRefusal::ProducedValueIsReturned { value });
    }
    // A deleted value is read at exactly the consumers that absorb it, and at no other node.
    for value in produced.clone() {
        let read = readers(graph, value);
        let expected: Vec<usize> = consumers
            .iter()
            .zip(&absorbed_value)
            .filter(|(_, absorbed)| **absorbed == value)
            .map(|(&position, _)| position)
            .collect();
        if read != expected {
            return Err(MergeRefusal::ProducedValueIsReadElsewhere {
                value,
                node: read.first().copied().unwrap_or(consumers[0]),
            });
        }
    }
    // The first consumer whose extent is not the producer's is the one named: a merged node
    // dispatches over one extent.
    if let Some(disagreeing) = consumers
        .iter()
        .find(|position| !extents_correspond(producer_node.count, count(graph, **position)))
    {
        return Err(MergeRefusal::ExtentsDiffer {
            producer: producer_node.count,
            consumer: count(graph, *disagreeing),
        });
    }
    // The merged node's inputs are the union: the producer's, then each consumer's survivors.
    let mut inputs: Vec<GraphValueId> = producer_node.inputs.clone();
    let mut bodies: Vec<(&KernelFragment, u32)> = Vec::with_capacity(consumers.len());
    let mut outputs: Vec<GraphValueId> = Vec::new();
    let mut ports = producer_node.fragment.inputs;
    for (&position, &absorbed) in consumers.iter().zip(&absorbed_input) {
        let Node::Kernel(consumer_node) = &graph.nodes()[position];
        let output = absorbed_value[bodies.len()] - starts[producer];
        match (
            producer_node.fragment.output_classes.get(output).copied(),
            consumer_node.fragment.input_classes.get(absorbed).copied(),
        ) {
            (Some(produced), Some(expected)) if produced == expected => {}
            (Some(produced), Some(expected)) => {
                return Err(MergeRefusal::ClassesDisagree { produced, expected });
            }
            (produced, expected) => {
                return Err(MergeRefusal::ClassIsNotStated { produced, expected });
            }
        }
        inputs.extend(
            consumer_node
                .inputs
                .iter()
                .copied()
                .enumerate()
                .filter(|(ordinal, _)| *ordinal != absorbed)
                .map(|(_, value)| value),
        );
        outputs.extend(starts[position]..starts[position] + consumer_node.fragment.outputs);
        bodies.push((&consumer_node.fragment, absorbed as u32));
        ports = ports
            .saturating_add(consumer_node.fragment.inputs)
            .saturating_sub(1);
    }
    // Past the merged ports, a position names another body's buffer: the union cannot fix it.
    if inputs.len() > ports {
        return Err(MergeRefusal::PortsDoNotCorrespond {
            position: ports,
            producer: inputs[ports],
            merged: None,
        });
    }
    let fragment = compose_fan_out(&producer_node.fragment, &bodies)
        .map_err(|refusal| MergeRefusal::CompositionRefused { refusal })?;
    Ok(Merge {
        absorbed: std::iter::once(producer)
            .chain(consumers[..consumers.len() - 1].iter().copied())
            .collect(),
        survivor: consumers[consumers.len() - 1],
        absorbed_input,
        outputs,
        fragment,
        inputs,
    })
}

/// The extent the node at `position` dispatches over.
fn count(graph: &Graph, position: usize) -> Count {
    let Node::Kernel(node) = &graph.nodes()[position];
    node.count
}

/// Whether the node at `consumer` reads a value the node at `producer` produced.
fn reads(graph: &Graph, starts: &[usize], producer: usize, consumer: usize) -> bool {
    let Node::Kernel(producer_node) = &graph.nodes()[producer];
    let produced = starts[producer]..starts[producer] + producer_node.fragment.outputs;
    let Node::Kernel(consumer_node) = &graph.nodes()[consumer];
    consumer_node
        .inputs
        .iter()
        .copied()
        .any(|value| produced.contains(&value))
}

/// Whether the two nodes dispatch over one extent, as `Count` states it.
fn extents_correspond(producer: Count, consumer: Count) -> bool {
    match (producer, consumer) {
        (Count::Constant(producer), Count::Constant(consumer)) => producer == consumer,
        (Count::Value(producer), Count::Value(consumer)) => producer == consumer,
        (
            Count::Device {
                value: producer,
                index: producer_index,
            },
            Count::Device {
                value: consumer,
                index: consumer_index,
            },
        ) => producer == consumer && producer_index == consumer_index,
        _ => false,
    }
}

/// Every read of `value` in the graph, one entry per read position.
fn readers(graph: &Graph, value: GraphValueId) -> Vec<usize> {
    let mut read = Vec::new();
    for (position, node) in graph.nodes().iter().enumerate() {
        let Node::Kernel(kernel) = node;
        for &held in &kernel.inputs {
            if held == value {
                read.push(position);
            }
        }
        match kernel.count {
            Count::Value(extent) | Count::Device { value: extent, .. } if extent == value => {
                read.push(position);
            }
            _ => {}
        }
    }
    read
}

/// The graph the merges make: the survivor at its own position, nothing where a producer was.
///
/// # Invariant
/// Values are renumbered by rebuilding in evaluation order, because `push` allocates ids
/// consecutively from `input_count()`. A value the merge deletes has no new id, so a read or a
/// return that still names one answers `None` rather than a wrong id.
///
/// **A fan-out's outputs are named, not counted**: the survivor fills the first consumer's graph
/// values and every consumer after it.
fn rebuild(graph: &Graph, starts: &[usize], merges: &HashMap<usize, Merge>) -> Option<Graph> {
    let absorbed: HashSet<usize> = merges
        .values()
        .flat_map(|merge| merge.absorbed.iter().copied())
        .collect();
    let inputs = graph.input_count();
    let mut rebuilt = Graph::with_inputs(inputs);
    let mut map: Vec<Option<GraphValueId>> = vec![None; graph.value_count()];
    for (value, slot) in map.iter_mut().enumerate().take(inputs) {
        *slot = Some(value);
    }
    for (position, node) in graph.nodes().iter().enumerate() {
        if absorbed.contains(&position) {
            continue;
        }
        let Node::Kernel(kernel) = node;
        let (fragment, reads, outputs) = match merges.get(&position) {
            Some(merge) => (merge.fragment.clone(), &merge.inputs, merge.outputs.clone()),
            None => (
                kernel.fragment.clone(),
                &kernel.inputs,
                (starts[position]..starts[position] + kernel.fragment.outputs).collect(),
            ),
        };
        let count = mapped_count(kernel.count, &map)?;
        let reads: Vec<GraphValueId> = reads
            .iter()
            .map(|value| map[*value])
            .collect::<Option<Vec<_>>>()?;
        let first = rebuilt
            .push(
                Node::Kernel(KernelNode {
                    fragment: fragment.clone(),
                    inputs: reads,
                    count,
                }),
                fragment.outputs,
            )
            .ok()?;
        for (ordinal, value) in outputs.iter().copied().enumerate() {
            map[value] = Some(first + ordinal);
        }
    }
    let returns = graph.returns()?;
    let returns: Vec<GraphValueId> = returns
        .iter()
        .map(|value| map[*value])
        .collect::<Option<Vec<_>>>()?;
    rebuilt.returning(returns).ok()?;
    Some(rebuilt)
}

/// An extent in the rebuilt graph's numbering.
fn mapped_count(count: Count, map: &[Option<GraphValueId>]) -> Option<Count> {
    match count {
        Count::Constant(count) => Some(Count::Constant(count)),
        Count::Value(value) => Some(Count::Value(map[value]?)),
        Count::Device { value, index } => Some(Count::Device {
            value: map[value]?,
            index,
        }),
    }
}

/// The licence a pack gave this region, as the report names it.
///
/// # Invariant
/// The region's **own hash** is in the sentence, because that is what the pack's `fuse` matched:
/// a runner reading the report can tell which region the pack fired for, exactly as the tiling
/// pass's own line names the node schema its pack fired for.
fn licensed_by(licensed: &Licence<'_>, region: &SchemaHash) -> String {
    match licensed {
        Licence::Floor => "licensed by the floor: every node of the region claims Reduction and \
                          nothing else"
            .to_string(),
        Licence::Pack(pack) => format!(
            "licensed by pack \"{}\", whose `fuse` names this region's schema {region}",
            pack.name()
        ),
    }
}

/// One merged region's line, naming both nodes, the absorbed input, the licence and the
/// unregistered survivor.
///
/// # Invariant
/// The licence is named in the line and not left to the graph: a runner reading a report has to
/// know whether a pack's `fuse` decided this merge or the engine's own proof did, because the
/// two are not the same claim about the same region. A fan-out names **every** node and every
/// absorbed position, because it is a shape the chain's line would under-report.
fn merged_line(index: usize, merge: &Merge, licensed: &Licence<'_>, region: &SchemaHash) -> String {
    let licence = licensed_by(licensed, region);
    let unregistered = "the merged fragment is unregistered, so it carries no claim and a later \
                        digest-keyed pass cannot name it";
    if merge.absorbed.len() == 1 {
        return format!(
            "region {index}: nodes {} and {} merged into one node at {} (the survivor), absorbing \
             input position {} of node {}, {licence}; {unregistered}",
            merge.absorbed[0],
            merge.survivor,
            merge.survivor,
            merge.absorbed_input[0],
            merge.survivor,
        );
    }
    let nodes = merge.absorbed.iter().copied().chain([merge.survivor]);
    let absorbing = merge
        .absorbed_input
        .iter()
        .zip(nodes.clone().skip(1))
        .map(|(position, node)| format!("input position {position} of node {node}"))
        .collect::<Vec<_>>()
        .join(" and ");
    format!(
        "region {index}: nodes {} merged into one node at {} (the survivor), absorbing \
         {absorbing}, {licence}; {unregistered}",
        join(nodes),
        merge.survivor,
    )
}

/// Positions as one sentence's list of them.
fn join(positions: impl Iterator<Item = usize>) -> String {
    positions
        .map(|position| position.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// One refused region's line, naming the nodes it holds and the check that refused it.
fn refused_line(index: usize, positions: &[usize], name: &str, message: &str) -> String {
    refusal_line(
        format!("region {index} (nodes {})", join(positions.iter().copied())),
        name,
        message,
    )
}

/// One node's line, for a fragment no row of the join holds.
///
/// # Invariant
/// The join's own miss in **this** pass's voice: the refusal and its sentence are the ones the
/// tiling pass gives that same node, so the two passes cannot answer a miss differently, while
/// the check's name is what tells their lines apart in a chain — this pass names every check it
/// refuses by.
fn unclaimed(position: usize) -> String {
    let refusal = SchemaRefusal::NotClaimed { node: position };
    refusal_line(
        format!("node {position}"),
        refusal.name(),
        &refusal.message(),
    )
}

/// A refusal as one line: what it is about, the check that refused it, and that check's sentence.
fn refusal_line(subject: String, name: &str, message: &str) -> String {
    format!("{subject}: refused: {name}: {message}")
}

/// The claimed kernels a module's own nodes name, as the derivation reads them.
///
/// # Invariant
/// The id is read from a node whose own value names a kernel, never re-derived from the source: a
/// kernel *value* is a `Copy` id, so the fragment the registry holds beside it is the only
/// compiled body there is.
fn instances(module: &Module<LangProgram>) -> Vec<Instance> {
    let mut instances = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for (id, _node) in module.nodes.iter() {
        let Ok(kernel) = kernel_id(module, id) else {
            continue;
        };
        if !seen.insert(kernel) {
            continue;
        }
        let (Some(claim), Ok(fragment)) = (claim_of(kernel), fragment_of(kernel)) else {
            continue;
        };
        instances.push(Instance {
            kernel,
            claim,
            fragment,
        });
    }
    instances
}

/// The join every registered kernel answers, for a module installed before its own evaluation.
///
/// # Invariant
/// The registry is append-only, so this is the same set the module would have named once its
/// kernels were evaluated; it is keyed the same way, so the two joins are one join.
fn registry_instances() -> Vec<Instance> {
    claimed_kernels()
        .into_iter()
        .map(|(kernel, claim, fragment)| Instance {
            kernel,
            claim,
            fragment,
        })
        .collect()
}
