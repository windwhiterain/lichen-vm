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

use lichen_compute::compute::{GraphPass, LaunchExtent, claimed_kernels, set_graph_pass};
use lichen_graph_ir::{Count, Graph, KernelNode, Node, ValueId as GraphValueId};
use lichen_kernel_ir::{ComposeRefusal, KernelFragment, ScalarClass, compose};
use lichen_lowlevel::Module;

use crate::fusion_claim::{
    Instance, PortKind, REDUCTION, RegionSchema, SchemaHash, claim_of, derive_regions, fragment_of,
    kernel_id, offsets, region_positions,
};
use crate::placement_pack::{ConstraintRefusal, Fusion, PlacementPack};
use crate::program::LangProgram;

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
    /// The graph position of the deleted node: the producer.
    absorbed: usize,
    /// The graph position of the node whose body survives: the consumer.
    survivor: usize,
    /// The consumer input position the absorbed output arrived at.
    absorbed_input: usize,
    /// The fragment the surviving node holds.
    fragment: KernelFragment,
    /// The surviving node's inputs: the producer's, then the consumer's but the absorbed one.
    inputs: Vec<GraphValueId>,
}

impl GraphPass for RegionMerge {
    /// Merge every two-node region whose legality conditions hold.
    ///
    /// # Invariant
    /// A region is looked at once and reported once, merged or refused, and a merged region's line
    /// names the licence that answered for it. The graph handed over is the recording every time,
    /// because a rewrite is stored under its own digest.
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
                    match licence(&schema, &self.packs).and_then(|licensed| {
                        merge_region(graph, &starts, positions).map(|m| (licensed, m))
                    }) {
                        Ok((licensed, merge)) => {
                            (merged_line(index, &merge, &licensed), Some(merge))
                        }
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
                if let Some((pack, refusal)) = inert {
                    write!(
                        formatter,
                        " (the pack \"{pack}\" named it and its constraints did not hold: {})",
                        refusal.message()
                    )?;
                }
                Ok(())
            }
        }
    }
}

/// The algebra this pass must be able to prove before it merges a region, or the pack that
/// decided it does.
///
/// # Invariant
/// The floor's licence is the reduction chain: a claim states what a kernel computes, not that
/// inlining pays, and whether it pays is a machine fact. Any other interface — `Elementwise`
/// today — is attempted **only** where a pack's `fuse` names the region's hash.
///
/// A pack changes which regions are attempted, never what must hold: every refusal in
/// [`merge_region`] stands under one.
fn licence<'a>(
    schema: &RegionSchema,
    packs: &'a [PlacementPack],
) -> Result<Licence<'a>, MergeRefusal> {
    let interfaces: BTreeSet<&str> = schema
        .nodes
        .iter()
        .map(|node| node.interface.as_str())
        .filter(|interface| *interface != REDUCTION)
        .collect();
    if interfaces.is_empty() {
        return Ok(Licence::Floor);
    }
    let class = first_input_class(schema);
    let mut inert = None;
    for pack in packs {
        let asks = pack.fuse().is_some_and(|directive| {
            directive.region() == Some(&schema.hash) && directive.form() == Fusion::OneKernel
        });
        if !asks {
            continue;
        }
        match pack.holds_for_class(class) {
            Ok(()) => return Ok(Licence::Pack(pack)),
            Err(refusal) => {
                if inert.is_none() {
                    inert = Some((pack.name().to_string(), refusal));
                }
            }
        }
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

/// The merge one region's two nodes decide, or why they do not.
///
/// # Invariant
/// Every condition is checked before `compose` reads either body: a merge that slipped past one
/// would be a node whose body reads a buffer the program never wired to it. The one check the
/// union cannot make for itself is `PortsDoNotCorrespond`: the merged node's inputs **are** the
/// union, so a buffer past the merged fragment's ports is a node whose ordinals name a different
/// number of buffers than its body reads ports for, and wiring it would answer one buffer as
/// another.
fn merge_region(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
) -> Result<Merge, MergeRefusal> {
    let [producer, consumer] = positions else {
        return Err(MergeRefusal::RegionIsNotAPair {
            nodes: positions.len(),
        });
    };
    let (producer, consumer) = (*producer, *consumer);
    let Node::Kernel(producer_node) = &graph.nodes()[producer];
    let Node::Kernel(consumer_node) = &graph.nodes()[consumer];
    let produced = starts[producer]..starts[producer] + producer_node.fragment.outputs;
    let absorbed: Vec<usize> = consumer_node
        .inputs
        .iter()
        .enumerate()
        .filter(|(_, value)| produced.contains(value))
        .map(|(ordinal, _)| ordinal)
        .collect();
    let [absorbed] = absorbed[..] else {
        return Err(MergeRefusal::AbsorbedEdgeIsNotOne {
            inputs: absorbed.len(),
        });
    };
    let absorbed_value = consumer_node.inputs[absorbed];
    let Some(returns) = graph.returns() else {
        return Err(MergeRefusal::GraphReturnsEveryValue {
            value: absorbed_value,
        });
    };
    if let Some(value) = produced.clone().find(|value| returns.contains(value)) {
        return Err(MergeRefusal::ProducedValueIsReturned { value });
    }
    for value in produced.clone() {
        let read = readers(graph, value);
        let expected: &[usize] = if value == absorbed_value {
            &[consumer]
        } else {
            &[]
        };
        if read != expected {
            return Err(MergeRefusal::ProducedValueIsReadElsewhere {
                value,
                node: read.first().copied().unwrap_or(consumer),
            });
        }
    }
    if !extents_correspond(producer_node.count, consumer_node.count) {
        return Err(MergeRefusal::ExtentsDiffer {
            producer: producer_node.count,
            consumer: consumer_node.count,
        });
    }
    let output = absorbed_value - starts[producer];
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
    // **The merged node's inputs are the union**: the producer's inputs in their order, then the
    // consumer's surviving inputs in theirs, because the merged fragment's own ports are that
    // union and a node is wired to the ports its body reads.
    let inputs: Vec<GraphValueId> = producer_node
        .inputs
        .iter()
        .copied()
        .chain(
            consumer_node
                .inputs
                .iter()
                .enumerate()
                .filter(|(ordinal, _)| *ordinal != absorbed)
                .map(|(_, value)| *value),
        )
        .collect();
    // A buffer past the merged fragment's ports is one the merged node is handed at a position
    // its body reads as the other body's: the ordinals name a different number of buffers than
    // the bodies read ports for, which is the mismatch the union cannot paper over.
    let ports = producer_node
        .fragment
        .inputs
        .saturating_add(consumer_node.fragment.inputs)
        .saturating_sub(1);
    if inputs.len() > ports {
        return Err(MergeRefusal::PortsDoNotCorrespond {
            position: ports,
            producer: inputs[ports],
            merged: None,
        });
    }
    let fragment = compose(
        &producer_node.fragment,
        &consumer_node.fragment,
        absorbed as u32,
    )
    .map_err(|refusal| MergeRefusal::CompositionRefused { refusal })?;
    Ok(Merge {
        absorbed: producer,
        survivor: consumer,
        absorbed_input: absorbed,
        fragment,
        inputs,
    })
}

/// Whether the two nodes dispatch over one extent, as `Count` states it.
fn extents_correspond(producer: Count, consumer: Count) -> bool {
    match (producer, consumer) {
        (Count::Constant(producer), Count::Constant(consumer)) => producer == consumer,
        (Count::Value(producer), Count::Value(consumer)) => producer == consumer,
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
        if kernel.count == Count::Value(value) {
            read.push(position);
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
fn rebuild(graph: &Graph, starts: &[usize], merges: &HashMap<usize, Merge>) -> Option<Graph> {
    let absorbed: HashSet<usize> = merges.values().map(|merge| merge.absorbed).collect();
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
        let (fragment, reads) = match merges.get(&position) {
            Some(merge) => (merge.fragment.clone(), &merge.inputs),
            None => (kernel.fragment.clone(), &kernel.inputs),
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
        for ordinal in 0..fragment.outputs {
            map[starts[position] + ordinal] = Some(first + ordinal);
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
    }
}

/// One merged region's line, naming both nodes, the absorbed input, the licence and the
/// unregistered survivor.
///
/// # Invariant
/// The licence is named in the line and not left to the graph: a runner reading a report has to
/// know whether a pack's `fuse` decided this merge or the engine's own proof did, because the
/// two are not the same claim about the same region.
fn merged_line(index: usize, merge: &Merge, licensed: &Licence<'_>) -> String {
    let licence = match licensed {
        Licence::Floor => "licensed by the floor: every node of the region claims Reduction and \
                          nothing else"
            .to_string(),
        Licence::Pack(pack) => format!(
            "licensed by pack \"{}\", whose `fuse` names this region's schema",
            pack.name()
        ),
    };
    format!(
        "region {index}: nodes {} and {} merged into one node at {} (the survivor), absorbing \
         input position {} of node {}, {licence}; the merged fragment is unregistered, so it \
         carries no claim and a later digest-keyed pass cannot name it",
        merge.absorbed, merge.survivor, merge.survivor, merge.absorbed_input, merge.survivor,
    )
}

/// One refused region's line, naming the nodes it holds and the check that refused it.
fn refused_line(index: usize, positions: &[usize], name: &str, message: &str) -> String {
    let nodes = positions
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("region {index} (nodes {nodes}): refused: {name}: {message}")
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
