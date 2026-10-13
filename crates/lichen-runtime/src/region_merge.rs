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

use lichen_graph_ir::{Count, Graph, KernelNode, Node, ValueId as GraphValueId};
use lichen_kernel_ir::{
    ComposeRefusal, KernelFragment, Producer, ScalarClass, compose, compose_fan_in, compose_fan_out,
};

use crate::fusion_claim::{
    Instance, PortKind, REDUCTION, RegionSchema, SchemaHash, SchemaRefusal, derive_regions,
    offsets, region_positions,
};
use crate::graph_pass::{GraphPass, LaunchExtent};
use crate::kernel_registry::claimed_kernels;
use crate::placement_pack::{ConstraintRefusal, Fusion, PlacementPack};

/// The pass that merges the regions `instances` names, under `packs`.
///
/// # Invariant
/// The one spelling of the construction, and it takes only what a pass reads: a caller that
/// has a [`Instance`] list — from its own module, or from the registry — installs exactly this
/// pass, so a caller that wraps it wraps what the installer runs.
pub fn region_merge_pass(instances: Vec<Instance>, packs: &[PlacementPack]) -> Box<dyn GraphPass> {
    Box::new(RegionMerge {
        instances,
        packs: packs.to_vec(),
    })
}

/// The pass: the claimed kernels it was built with, joined to a graph node by its fragment
/// digest.
struct RegionMerge {
    /// One instance per claimed kernel the caller named, empty when it named none.
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
    /// The graph positions of the deleted nodes: every node of the region but the survivor.
    absorbed: Vec<usize>,
    /// The graph position of the node whose body survives: the region's last node.
    survivor: usize,
    /// One link per absorbed input, in the order the reader's own input is read in.
    links: Vec<Link>,
    /// The graph values the merged fragment's outputs fill, in the order the composition wrote them.
    outputs: Vec<GraphValueId>,
    /// The fragment the surviving node holds.
    fragment: KernelFragment,
    /// The surviving node's inputs: the first node's, then each later node's but the absorbed ones.
    inputs: Vec<GraphValueId>,
}

/// One edge of a region the merge inlines: `from`'s output reaches `to` at `input`.
///
/// # Invariant
/// **One link per absorbed input**, in (reader, input ordinal) order, so a node reading two
/// producers is two links rather than one choice between them, and the position the merged line
/// reports a link at is the reader's own input ordinal.
#[derive(Debug, Clone, Copy)]
struct Link {
    /// The graph position whose output the edge carries.
    from: usize,
    /// The graph position that reads it.
    to: usize,
    /// The reading node's input ordinal the output arrives at.
    input: usize,
}

/// The three shapes one composition inlines.
///
/// # Invariant
/// **One producer, one consumer at a time**: a `compose` inlines a consumer *of one producer*, so
/// a region is either a chain — every node reading the one before it — a fan-out, every node
/// reading the first, or a **fan-in**, that chain with a node reading an earlier producer as well.
/// A tree that is none of these names no single code the composition serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Every node but the first reads the first.
    FanOut,
    /// Every node but the first reads the one before it, and no node reads two producers.
    Chain,
    /// Every node but the first reads the one before it, and some node reads an earlier one too.
    FanIn,
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
    /// The region is neither a chain of consumers, a fan-in of one, nor one producer's consumers.
    RegionIsNotAPair {
        /// How many instances the region holds.
        nodes: usize,
    },
    /// One producer's output reaches the consumer at more than one input position.
    ///
    /// # Invariant
    /// A merge binds **one producer's value to one input slot**, so a producer reaching one reader
    /// twice has no second slot in the merged fragment: the two reads would answer one value, and a
    /// fan-in is admitted only where every input has a producer of its own.
    AbsorbedEdgeIsNotOne {
        /// How many of the consumer's input positions hold that one producer's output.
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
    /// reductions is refused here and not there, because the floor's own proof is the shape it can
    /// serve and a fan-out is not it — a fan-in **is**, once every one of its inputs is named.
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
                "the consumer reads one producer's output at {inputs} of its input position(s), and \
                 one value has one slot in the merged fragment"
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
/// The floor's licence is the reduction **shape this pass can serve**. A region two edges
/// **leave** towards two readers is a fan-out, attempted only under a pack's `fuse`. A
/// region two edges **arrive at** is that shape exactly when **every one of those inputs is
/// named and proven**: a [`Between`](crate::fusion_claim::Between) per arrival, each over a
/// slot of its own.
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
    // The shape first: a fan-out is refused by the shape's own name, not by its interfaces.
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
/// **A chain, a fan-in of it, or one producer's consumers**, in evaluation order, and every
/// condition below is the pair's own: the shape is decided first, then one absorbed position per
/// edge, and a merge that slipped past either would be a node reading a buffer the program never
/// wired to it — the union cannot make one, and `PortsDoNotCorrespond` is where that stops.
fn merge_region(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
) -> Result<Merge, MergeRefusal> {
    let shape = shape(graph, starts, positions)?;
    // Two or more nodes, so a survivor and a first node are both named.
    let survivor = *positions
        .last()
        .expect("a shape is decided over two or more");
    let links = links(graph, starts, positions)?;
    let (outputs, deleted) = written_and_deleted(graph, starts, positions, &links, shape);
    let Some(returns) = graph.returns() else {
        return Err(MergeRefusal::GraphReturnsEveryValue {
            value: absorbed_value(graph, &links[0]),
        });
    };
    if let Some(value) = deleted.iter().find(|value| returns.contains(value)) {
        return Err(MergeRefusal::ProducedValueIsReturned { value: *value });
    }
    // A deleted value is read at exactly the links that absorb it, and at no other node.
    for value in deleted {
        let read = readers(graph, value);
        let expected: Vec<usize> = links
            .iter()
            .filter(|link| absorbed_value(graph, link) == value)
            .map(|link| link.to)
            .collect();
        if read != expected {
            return Err(MergeRefusal::ProducedValueIsReadElsewhere {
                value,
                node: read.first().copied().unwrap_or(links[0].to),
            });
        }
    }
    // The first node's extent is the merged node's, and the first that differs from it is the one
    // named.
    let Node::Kernel(first_node) = &graph.nodes()[positions[0]];
    if let Some(disagreeing) = positions[1..]
        .iter()
        .find(|position| !extents_correspond(first_node.count, count(graph, **position)))
    {
        return Err(MergeRefusal::ExtentsDiffer {
            producer: first_node.count,
            consumer: count(graph, *disagreeing),
        });
    }
    // The union: the first node's inputs, then each later node's but what a producer fills.
    let absorbed: Vec<Vec<usize>> = absorbed_positions(positions, &links);
    let mut inputs: Vec<GraphValueId> = first_node.inputs.clone();
    let mut ports = first_node.fragment.inputs;
    for (index, position) in positions.iter().enumerate().skip(1) {
        let Node::Kernel(node) = &graph.nodes()[*position];
        inputs.extend(
            node.inputs
                .iter()
                .copied()
                .enumerate()
                .filter(|(ordinal, _)| !absorbed[index].contains(ordinal))
                .map(|(_, value)| value),
        );
        ports = ports
            .saturating_add(node.fragment.inputs)
            .saturating_sub(absorbed[index].len());
    }
    for link in &links {
        let Node::Kernel(node) = &graph.nodes()[link.to];
        let output = absorbed_value(graph, link) - starts[link.from];
        match (
            fragment_at(graph, link.from)
                .output_classes
                .get(output)
                .copied(),
            node.fragment.input_classes.get(link.input).copied(),
        ) {
            (Some(produced), Some(expected)) if produced == expected => {}
            (Some(produced), Some(expected)) => {
                return Err(MergeRefusal::ClassesDisagree { produced, expected });
            }
            (produced, expected) => {
                return Err(MergeRefusal::ClassIsNotStated { produced, expected });
            }
        }
    }
    // Past the merged ports, a position names another body's buffer: the union cannot fix it.
    if inputs.len() > ports {
        return Err(MergeRefusal::PortsDoNotCorrespond {
            position: ports,
            producer: inputs[ports],
            merged: None,
        });
    }
    let fragment = composed(graph, positions, shape, &links)
        .map_err(|refusal| MergeRefusal::CompositionRefused { refusal })?;
    Ok(Merge {
        absorbed: positions[..positions.len() - 1].to_vec(),
        survivor,
        links,
        outputs,
        fragment,
        inputs,
    })
}

/// The shape the region's edges make, or the refusal naming what they do not.
///
/// # Invariant
/// **One producer, one consumer at a time**, read off the edges and not off the node count: a
/// chain is every node reading the one before it, a fan-in is that chain with a node reading an
/// earlier producer as well, and a fan-out is every node reading the first. A node that reaches
/// a region value as its **extent** reads nothing at an input position.
fn shape(graph: &Graph, starts: &[usize], positions: &[usize]) -> Result<Shape, MergeRefusal> {
    let refused = MergeRefusal::RegionIsNotAPair {
        nodes: positions.len(),
    };
    if positions.len() < 2 {
        return Err(refused);
    }
    let chain = positions[1..]
        .iter()
        .enumerate()
        .all(|(index, position)| reads(graph, starts, positions[index], *position));
    if chain {
        // **A node reading two producers is a fan-in of the chain**: one reading one is the
        // chain itself.
        let several = positions
            .iter()
            .any(|position| absorbed_count(graph, starts, positions, *position) > 1);
        return Ok(if several { Shape::FanIn } else { Shape::Chain });
    }
    let fan_out = positions[1..]
        .iter()
        .all(|position| reads(graph, starts, positions[0], *position));
    fan_out.then_some(Shape::FanOut).ok_or(refused)
}

/// The input positions one node fills from a producer of the region.
///
/// # Invariant
/// **Sorted by the input ordinal**, which is the order the composition counts them in when it
/// shifts a surviving read, so the two cannot disagree about how many absorbed inputs sit below one.
fn absorbed_positions(positions: &[usize], links: &[Link]) -> Vec<Vec<usize>> {
    let mut absorbed: Vec<Vec<usize>> = vec![Vec::new(); positions.len()];
    for link in links {
        let at = positions.iter().position(|position| *position == link.to);
        if let Some(at) = at {
            absorbed[at].push(link.input);
        }
    }
    for ordinals in &mut absorbed {
        ordinals.sort_unstable();
    }
    absorbed
}

/// How many of a node's input positions hold a value the region produced.
fn absorbed_count(graph: &Graph, starts: &[usize], positions: &[usize], position: usize) -> usize {
    let Node::Kernel(node) = &graph.nodes()[position];
    node.inputs
        .iter()
        .filter(|value| {
            positions
                .iter()
                .any(|earlier| *earlier < position && produced_by(graph, starts, *earlier, **value))
        })
        .count()
}

/// Whether the node at `position` produces `value`.
fn produced_by(graph: &Graph, starts: &[usize], position: usize, value: GraphValueId) -> bool {
    let Node::Kernel(node) = &graph.nodes()[position];
    let produced = starts[position]..starts[position] + node.fragment.outputs;
    produced.contains(&value)
}

/// The region's edges, one per absorbed input, in (reader, input ordinal) order.
///
/// # Invariant
/// **One absorbed position per edge**, each naming the producer whose value fills it, and read off
/// the graph's own table rather than off the shape: a node that reads a producer twice names no
/// slot to inline, and the region is refused by that name rather than merged against either. The
/// shape has already been decided, so every edge here is one the composition can serve.
fn links(graph: &Graph, starts: &[usize], positions: &[usize]) -> Result<Vec<Link>, MergeRefusal> {
    let mut links: Vec<Link> = Vec::with_capacity(positions.len().saturating_sub(1));
    for (index, &to) in positions.iter().enumerate().skip(1) {
        let Node::Kernel(node) = &graph.nodes()[to];
        let read: Vec<(usize, usize)> = node
            .inputs
            .iter()
            .enumerate()
            .filter_map(|(ordinal, value)| {
                let from = region_producer(graph, starts, positions, index, *value)?;
                Some((ordinal, from))
            })
            .collect();
        // **One value per producer**: two inputs taking one output ask one value for two slots.
        let repeated = read
            .iter()
            .enumerate()
            .find(|(at, (_, from))| read[..*at].iter().any(|(_, other)| other == from));
        if repeated.is_some() {
            return Err(MergeRefusal::AbsorbedEdgeIsNotOne { inputs: 2 });
        }
        links.extend(
            read.into_iter()
                .map(|(input, from)| Link { from, to, input }),
        );
    }
    links.sort_by_key(|link| (link.to, link.input));
    Ok(links)
}

/// The region-local position of the node that produced `value` for the consumer at `consumer`.
fn region_producer(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
    consumer: usize,
    value: GraphValueId,
) -> Option<usize> {
    let mut found = None;
    for (index, position) in positions.iter().enumerate() {
        if index >= consumer {
            break;
        }
        if produced_by(graph, starts, *position, value) {
            found = Some(index);
        }
    }
    found
}

/// The graph value one link carries: what its reader holds at the absorbed position.
fn absorbed_value(graph: &Graph, link: &Link) -> GraphValueId {
    let Node::Kernel(node) = &graph.nodes()[link.to];
    node.inputs[link.input]
}

/// The graph values the merged fragment fills, and the ones the merge deletes.
///
/// # Invariant
/// **A fan-out writes every consumer's outputs; a chain and a fan-in write the survivor's**:
/// `compose_fan_out` sums the consumers' outputs, `compose` and `compose_fan_in` declare one.
/// Every other value a region node produces is deleted, and one that is returned or read
/// elsewhere is what the two refusals below name.
fn written_and_deleted(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
    links: &[Link],
    shape: Shape,
) -> (Vec<GraphValueId>, Vec<GraphValueId>) {
    let produced =
        |position: usize| starts[position]..starts[position] + fragment_at(graph, position).outputs;
    match shape {
        Shape::FanOut => (
            links.iter().flat_map(|link| produced(link.to)).collect(),
            produced(positions[0]).collect(),
        ),
        Shape::Chain | Shape::FanIn => (
            produced(positions[positions.len() - 1]).collect(),
            positions[..positions.len() - 1]
                .iter()
                .flat_map(|&position| produced(position))
                .collect(),
        ),
    }
}

/// The fragment the region's links compose into one.
///
/// # Invariant
/// **A fan-out is one `compose_fan_out`, a chain one `compose` per link, and a fan-in one
/// `compose_fan_in` over the region's own bodies**: from the second link on, the producer is the
/// fragment the last step composed, so every condition `compose` states is asked again of it.
/// A fan-in's bodies are the region's **own**, since each link names a different producer.
fn composed(
    graph: &Graph,
    positions: &[usize],
    shape: Shape,
    links: &[Link],
) -> Result<KernelFragment, ComposeRefusal> {
    let first = fragment_at(graph, positions[0]);
    match shape {
        Shape::FanOut => compose_fan_out(
            first,
            &links
                .iter()
                .map(|link| (fragment_at(graph, link.to), link.input as u32))
                .collect::<Vec<_>>(),
        ),
        Shape::Chain => links.iter().try_fold(first.clone(), |merged, link| {
            compose(&merged, fragment_at(graph, link.to), link.input as u32)
        }),
        Shape::FanIn => {
            let steps: Vec<Producer<'_>> = positions
                .iter()
                .map(|position| Producer {
                    fragment: fragment_at(graph, *position),
                    // **The producer is the body's index in this list**, so the list is
                    // topologically ordered.
                    absorbs: links
                        .iter()
                        .filter(|link| link.to == *position)
                        .map(|link| {
                            (
                                positions
                                    .iter()
                                    .position(|earlier| *earlier == link.from)
                                    .expect("a link's producer is a position of the region"),
                                link.input,
                            )
                        })
                        .collect(),
                })
                .collect();
            compose_fan_in(&steps)
        }
    }
}

/// The extent the node at `position` dispatches over.
fn count(graph: &Graph, position: usize) -> Count {
    let Node::Kernel(node) = &graph.nodes()[position];
    node.count
}

/// The fragment the node at `position` dispatched.
fn fragment_at(graph: &Graph, position: usize) -> &KernelFragment {
    let Node::Kernel(node) = &graph.nodes()[position];
    &node.fragment
}

/// Whether the node at `consumer` reads a value the node at `producer` produced, at an input.
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
                    scalar: kernel
                        .scalar
                        .map(|scalar| scalar.map_value(|v| map[v]))
                        .flatten(),
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
/// know whether a pack's `fuse` decided this merge or the engine's own proof did. A region of
/// more than two nodes names **every** node and **every** absorbed position, each with the node
/// it is an input of rather than by its rank.
fn merged_line(index: usize, merge: &Merge, licensed: &Licence<'_>, region: &SchemaHash) -> String {
    let licence = licensed_by(licensed, region);
    let unregistered = "the merged fragment is unregistered, so it carries no claim and a later \
                        digest-keyed pass cannot name it";
    if merge.absorbed.len() == 1 && merge.links.len() == 1 {
        return format!(
            "region {index}: nodes {} and {} merged into one node at {} (the survivor), absorbing \
             input position {} of node {}, {licence}; {unregistered}",
            merge.absorbed[0], merge.survivor, merge.survivor, merge.links[0].input, merge.survivor,
        );
    }
    let nodes = merge.absorbed.iter().copied().chain([merge.survivor]);
    let absorbing = merge
        .links
        .iter()
        .map(|link| format!("input position {} of node {}", link.input, link.to))
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
