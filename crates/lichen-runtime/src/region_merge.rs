//! The region merge pass: a producer→consumer region becomes one node.
//! See docs/notes/graph-fusion.md §5 and §8.
//!
//! # Invariant
//! The pass merges and never reorders: an ordinary region's survivor stands at its last position,
//! a sibling region's at its first, and a declined region changes nothing. A pack decides which
//! regions are merged, never what a merge must satisfy: every legality condition refuses by name
//! under a pack as it does under the floor.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use lichen_graph_ir::{
    Count, Graph, KernelNode, Node, Scalar as GraphScalar, ValueId as GraphValueId,
};
use lichen_kernel_ir::{
    ComposeRefusal, KernelFragment, Producer, ScalarClass, Sibling, carried_producers, compose,
    compose_fan_in_staged, compose_fan_out, compose_siblings,
};

use crate::fusion_claim::{
    BRIDGED, Instance, PortKind, Region, RegionSchema, RegionShape, SchemaHash, SchemaRefusal,
    count_at, extents_correspond, merged_claim, offsets, ordinary_regions, producer_ordinal,
    sibling_regions,
};
use crate::fusion_file::{Budget, FusionPlan, Strategy};
use crate::graph_pass::{GraphPass, LaunchExtent};
use crate::kernel_registry::{claimed_kernels, intern_kernel};
use crate::merged_node;
use crate::placement_pack::{ConstraintRefusal, Fusion, PlacementPack};

/// The pass that merges the regions `instances` names, under `packs`.
///
/// # Invariant
/// The one spelling of the construction, and it takes only what a pass reads: a caller that
/// has a [`Instance`] list — from its own module, or from the registry — installs exactly this
/// pass, so a caller that wraps it wraps what the installer runs.
///
/// **The sibling derivation is the floor's own answer**: the shared producer stays materialized
/// and its consumers merge.
pub fn region_merge_pass(instances: Vec<Instance>, packs: &[PlacementPack]) -> Box<dyn GraphPass> {
    Box::new(RegionMerge {
        instances,
        packs: packs.to_vec(),
        strategy: None,
        budget: Budget::unlimited(),
        barriers: Vec::new(),
    })
}

/// The pass that merges the regions `instances` names, under `packs` and the `plan`'s policy.
///
/// # Invariant
/// The plan's force-merge overrides are installed beside the packs the runner handed over, so
/// a plan alone steers the merges it names. Its budget and its barriers are the policy the
/// pass reads, and the licence every merge still needs stands whatever they say.
pub fn region_merge_pass_with_plan(
    instances: Vec<Instance>,
    packs: &[PlacementPack],
    plan: &FusionPlan,
) -> Box<dyn GraphPass> {
    let mut installed = packs.to_vec();
    installed.extend(plan.overrides.iter().cloned());
    Box::new(RegionMerge {
        instances,
        packs: installed,
        strategy: Some(plan.strategy),
        budget: plan.budget,
        barriers: plan.barriers.clone(),
    })
}

/// The pass: the claimed kernels it was built with, joined to a graph node by its fragment
/// digest.
struct RegionMerge {
    /// One instance per claimed kernel the caller named, empty when it named none.
    instances: Vec<Instance>,
    /// The installed packs, in the order they are consulted.
    packs: Vec<PlacementPack>,
    /// The strategy a plan named, when one did; the floor's own answer otherwise.
    strategy: Option<Strategy>,
    /// The bounds the gate reads.
    budget: Budget,
    /// The regions a plan barred from merging.
    barriers: Vec<SchemaHash>,
}

/// What licensed one region's merge, as the report names it.
enum Licence<'a> {
    /// The engine's own proof: bridged interfaces, a shape the fold builds, one extent.
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
    /// The runtime scalars the merged node binds, in the merged fragment's own order.
    scalars: Vec<GraphScalar>,
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

/// One edge of the region's DAG, in region-local positions.
///
/// # Invariant
/// **The region's own edges**: `from` and `to` are the region's local positions and never the
/// graph's, so the fold reads the region's shape without the graph's numbering.
#[derive(Debug, Clone, Copy)]
struct Edge {
    /// The region-local position of the producer.
    from: usize,
    /// The region-local position of the reader.
    to: usize,
    /// The reader's input ordinal the value arrives at.
    input: usize,
}

/// One node of the region's DAG, as the fold reads it.
struct RegionNode<'a> {
    /// The fragment the node dispatched.
    fragment: &'a KernelFragment,
    /// The runtime scalars the node binds, in the fragment's own order.
    scalars: &'a [GraphScalar],
    /// The edges into this node from inside the region, in input-ordinal order.
    absorbed: Vec<Edge>,
    /// The edges out of this node to inside the region, in (reader, input) order.
    consumers: Vec<Edge>,
}

/// The fragment a fold produced, and the region's ports it reads and writes.
///
/// # Invariant
/// **The ports, the writes and the scalars are the region's own, in the fragment's own order**:
/// a port is a (node, input ordinal) the merged fragment reads, a write is a node whose value
/// the merged fragment holds, and a scalar is the number one body was launched with — so the
/// graph's values are renumbered from these lists alone, and each body binds its own scalars.
struct Folded {
    /// The merged fragment.
    fragment: KernelFragment,
    /// The ports the fragment reads, as (node, input ordinal), in port order.
    ports: Vec<(usize, usize)>,
    /// The nodes whose values the fragment writes, in output order.
    writes: Vec<usize>,
    /// The runtime scalars the merged node binds, in the merged fragment's own order.
    scalars: Vec<GraphScalar>,
    /// Whether the merged fragment keeps a store a body reads back.
    ///
    /// # Invariant
    /// **The composition's own answer is the fact**: the recomputing path copies the producer's
    /// value and keeps no store, and the staged path keeps the producer's store — so this is
    /// what says the merged node's read needs a barrier to reach it.
    keeps_store: bool,
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
        if let Some(strategy) = &self.strategy {
            lines.push(format!("strategy \"{}\"", strategy.as_str()));
        }
        let starts = offsets(graph);
        // **The strategy shapes the regions**: one groups, the other splits, and the floor's own
        // answer is the one that splits.
        let regions: Vec<Region> = match self.strategy.unwrap_or_default() {
            Strategy::Component => ordinary_regions(graph, instances),
            Strategy::Sibling => sibling_regions(graph, instances),
        };
        // A node no region holds is named too: "no fusible region" and "no claim" are two answers.
        for position in skipped(graph, &regions) {
            lines.push(unclaimed(position));
        }
        let mut decisions: Vec<Decision> = Vec::new();
        for (index, region) in regions.iter().enumerate() {
            let (line, merge) = match &region.schema {
                Err(refusal) => (
                    refused_line(index, &region.positions, refusal.name(), &refusal.message()),
                    None,
                ),
                Ok(schema) => {
                    // The licence says the merge may be *attempted*; every condition below
                    // stands whatever answered it.
                    match self
                        .gate(graph, &starts, &region.positions, &schema)
                        .and_then(|()| licence(graph, &schema, &region.positions, &self.packs))
                        .and_then(|licensed| {
                            merge_region(graph, &starts, &region.positions, &region.shape)
                                .map(|m| (licensed, m))
                        }) {
                        Ok((licensed, merge)) => (
                            merged_line(index, &merge, &licensed, &schema, &region.shape),
                            Some(merge),
                        ),
                        Err(refusal) => (
                            refused_line(
                                index,
                                &region.positions,
                                refusal.name(),
                                &refusal.message(),
                            ),
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

impl RegionMerge {
    /// The plan's own refusals, asked before any legality check.
    ///
    /// # Invariant
    /// **The gate may only decline.** A barrier and a budget say which regions are *attempted*,
    /// and a region they let through is still refused by every legality check that stands under
    /// them: a pack decides which regions are merged, never what a merge must satisfy.
    fn gate(
        &self,
        graph: &Graph,
        starts: &[usize],
        positions: &[usize],
        schema: &RegionSchema,
    ) -> Result<(), MergeRefusal> {
        if self.barriers.contains(&schema.hash) {
            return Err(MergeRefusal::RegionIsBarrier {
                region: schema.hash.clone(),
            });
        }
        let nodes = positions.len();
        let depth = region_depth(graph, starts, positions);
        if nodes > self.budget.max_region_nodes || depth > self.budget.max_region_depth {
            return Err(MergeRefusal::RegionOverBudget {
                nodes,
                depth,
                max_nodes: self.budget.max_region_nodes,
                max_depth: self.budget.max_region_depth,
            });
        }
        Ok(())
    }
}

/// The depth of the region's own DAG: the most nodes on one path inside it.
///
/// # Invariant
/// **The region's own edges**, read off the graph: a node that reads a body the region
/// produced continues that body's path, so the depth counts the nodes on the longest path and
/// a region of one node is one deep.
fn region_depth(graph: &Graph, starts: &[usize], positions: &[usize]) -> usize {
    let mut depths: Vec<usize> = vec![1; positions.len()];
    let mut deepest = 1;
    for (index, &position) in positions.iter().enumerate() {
        let Node::Kernel(node) = &graph.nodes()[position];
        for &value in &node.inputs {
            if let Some(producer) = region_producer(graph, starts, positions, index, value) {
                depths[index] = depths[index].max(depths[producer] + 1);
            }
        }
        deepest = deepest.max(depths[index]);
    }
    deepest
}

/// Why one region's two nodes did not become one.
///
/// # Invariant
/// One variant per fact the merge cannot decide for itself, and each names what it met: a decision
/// guessed here would be a node that runs and answers another program.
#[derive(Debug, Clone, PartialEq)]
pub enum MergeRefusal {
    /// The region holds one node, and a merge is one producer and one consumer.
    RegionIsNotAPair {
        /// How many instances the region holds.
        nodes: usize,
    },
    /// A node reads more producers than the fold composes.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::RegionIsNotAPair`] because it is a distinct fact: that one
    /// says the region holds no pair, this one says a node reads a third producer — and the
    /// fold's fan-in composes two producers into one consumer, which is all it proves today.
    FanInIsWiderThanTwo {
        /// How many producers reach the node.
        inputs: usize,
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
    /// The region's edges are not one of the shapes the fold builds.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::InterfacesAreNotProved`] because it is a distinct fact: that
    /// one is what a node **claims**, this one how the region's values **flow**. A region's edges
    /// that are none of the four the fold builds are refused here and not there, because the
    /// floor's own proof is the shape it can serve — a chain, a sibling, a fan-out and a fan-in
    /// are all shapes it serves.
    NotAReductionChain {
        /// The graph node whose edges are not a shape the fold builds.
        node: usize,
        /// A pack that named this region and whose constraints did not hold, when one was
        /// consulted.
        inert: Option<(String, ConstraintRefusal)>,
    },
    /// The region holds an interface whose merge this engine has no algebra for.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::NotAReductionChain`] because it is a distinct fact: that one
    /// is how the region's values **flow**, this one what a node **claims**. An interface the
    /// bridge does not compose is refused here and not there, whatever the region's shape is.
    InterfacesAreNotProved {
        /// The interfaces the region's nodes claim, other than one the bridge composes.
        interfaces: Vec<String>,
        /// The region's schema, which is what a pack's `fuse` must name.
        region: SchemaHash,
        /// A pack that named it and whose constraints did not hold, when one was consulted.
        inert: Option<(String, ConstraintRefusal)>,
    },
    /// The region is over the plan's budget.
    ///
    /// # Invariant
    /// A distinct arm from the legality refusals because it is not one: the budget says which
    /// regions are *attempted*, and the legality arms say what a merge must satisfy. The arm
    /// carries both sides of the comparison, so a report names what the region was and what the
    /// plan allowed.
    RegionOverBudget {
        /// How many nodes the region holds.
        nodes: usize,
        /// How deep the region's own DAG is.
        depth: usize,
        /// The most nodes the plan allows.
        max_nodes: usize,
        /// The deepest the plan allows.
        max_depth: usize,
    },
    /// The plan bars this region from merging.
    RegionIsBarrier {
        /// The region the plan bars.
        region: SchemaHash,
    },
    /// A node binds a different number of runtime scalars than its fragment declares.
    ///
    /// # Invariant
    /// A distinct arm from the legality refusals because it is about the region's own nodes: the
    /// merged fragment's parameter list is the union of the bodies' scalar leaves, and a body that
    /// binds another number has no leaf to give it.
    ScalarArityDiffers {
        /// The graph node whose scalars do not match its fragment.
        node: usize,
        /// How many the fragment declares.
        wanted: usize,
        /// How many the node binds.
        got: usize,
    },
    /// A branch of a sibling region does not read the shared producer's value.
    ///
    /// # Invariant
    /// A distinct arm from the legality refusals because it is about the region's own shape: a
    /// sibling merge reads the producer's value at one port, and a branch that does not read it
    /// has no claim on that port — so the merge would carry a port no body uses.
    TheProducerIsNotRead {
        /// The graph node that should have read the producer's value.
        node: usize,
    },
    /// The merged node would stand before a value it reads.
    ///
    /// # Invariant
    /// A distinct arm from the legality refusals because it is about the **placement** of the
    /// merged node: a sibling region's merged node stands at the region's first position, and a
    /// value the region reads that a node inside the region's own span produces would then be
    /// read before it was written.
    InputIsProducedAfterTheMerge {
        /// The value the merged node would read.
        value: GraphValueId,
        /// The node that produces it.
        node: usize,
    },
    /// The merged node would stand after a value it writes.
    ///
    /// # Invariant
    /// A distinct arm from the legality refusals because it is about the **placement** of the
    /// merged node: a sibling region's merged node stands at the region's first position, and a
    /// sink a node inside the region's own span reads would then be read before it was written.
    OutputIsReadBeforeTheMerge {
        /// The value the merged node would write.
        value: GraphValueId,
        /// The node that reads it.
        node: usize,
    },
    /// A staged read no barrier reaches, because the merged dispatch is wider than one lane group.
    ///
    /// # Invariant
    /// A distinct arm from the legality refusals because it is about the **staged body**: the
    /// merged node keeps the producer's store and the consumer reads it back, and the barrier
    /// that separates them orders the lanes of **one** group — a dispatch that spans several
    /// groups has a read that no barrier reaches, and the merged node would answer another
    /// program's value.
    StagedReadIsNotOrdered {
        /// How many lanes the merged node dispatches over, when the extent is a number.
        elements: Option<usize>,
        /// How many lanes one group holds.
        lanes: usize,
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
            MergeRefusal::FanInIsWiderThanTwo { .. } => "FanInIsWiderThanTwo",
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
            MergeRefusal::RegionOverBudget { .. } => "RegionOverBudget",
            MergeRefusal::RegionIsBarrier { .. } => "RegionIsBarrier",
            MergeRefusal::ScalarArityDiffers { .. } => "ScalarArityDiffers",
            MergeRefusal::TheProducerIsNotRead { .. } => "TheProducerIsNotRead",
            MergeRefusal::InputIsProducedAfterTheMerge { .. } => "InputIsProducedAfterTheMerge",
            MergeRefusal::OutputIsReadBeforeTheMerge { .. } => "OutputIsReadBeforeTheMerge",
            MergeRefusal::StagedReadIsNotOrdered { .. } => "StagedReadIsNotOrdered",
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
            MergeRefusal::FanInIsWiderThanTwo { inputs } => write!(
                formatter,
                "a node of the region reads {inputs} producers' outputs, and the fold composes a \
                 fan-in of two, so a wider one is not composed yet"
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
                    "graph node {node}'s edges inside the region make none of the shapes the fold \
                     builds (a chain, a sibling, a fan-out or a fan-in), so the floor's own proof \
                     cannot hold it, and a merge is licensed only by a pack whose `fuse` names this \
                     region's schema"
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
                    "the region's node(s) claim {}, and a merge is licensed by the proof this pass \
                     makes (every node an interface the bridge composes, a shape the fold builds, \
                     one extent) or by a pack whose `fuse` names this region's schema \
                     {region}: no installed pack asks for this region",
                    interfaces.join(", ")
                )?;
                write_inert(formatter, inert)
            }
            MergeRefusal::RegionOverBudget {
                nodes,
                depth,
                max_nodes,
                max_depth,
            } => write!(
                formatter,
                "the region holds {nodes} node(s) and its own DAG is {depth} deep, over the \
                 plan's budget of {max_nodes} node(s) and {max_depth} deep"
            ),
            MergeRefusal::RegionIsBarrier { region } => write!(
                formatter,
                "the plan bars the region {region} from merging, and a bar is the file's own \
                 refusal to merge it"
            ),
            MergeRefusal::ScalarArityDiffers { node, wanted, got } => write!(
                formatter,
                "graph node {node} binds {got} runtime scalar(s) and its fragment declares \
                 {wanted}, so it has no leaf to give the merged fragment's parameter list"
            ),
            MergeRefusal::TheProducerIsNotRead { node } => write!(
                formatter,
                "graph node {node} does not read the shared producer's value, so the sibling \
                 merge has no shared port to read"
            ),
            MergeRefusal::InputIsProducedAfterTheMerge { value, node } => write!(
                formatter,
                "graph value {value} is produced by node {node}, which the merged node would \
                 stand before, so it would read a value no node had written yet"
            ),
            MergeRefusal::OutputIsReadBeforeTheMerge { value, node } => write!(
                formatter,
                "graph value {value} is read by node {node}, which the merged node would stand \
                 after, so it would be read before it was written"
            ),
            MergeRefusal::StagedReadIsNotOrdered { elements, lanes } => write!(
                formatter,
                "the merged node keeps the producer's store and the consumer reads it back, and \
                 the barrier that separates them orders the lanes of one group of {lanes} — but \
                 the merged node dispatches over {extent}, so the read crosses a group and no \
                 barrier reaches it",
                extent = match elements {
                    Some(count) => format!("{count} element(s)"),
                    None => "an extent this reader cannot name".to_string(),
                }
            ),
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
/// The complement of the regions' own nodes, read off those groupings rather than off a second
/// join: the grouping is what decides a node carries no claim, so a node it skipped is exactly
/// a node no instance's fragment is, and the two answers cannot disagree.
fn skipped(graph: &Graph, regions: &[Region]) -> Vec<usize> {
    let mut grouped = vec![false; graph.len()];
    for region in regions {
        for &position in &region.positions {
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
/// The floor's licence is the **proof this pass makes of the region**: every node of it claims
/// an interface the bridge composes, its shape is one the fold builds, and it dispatches over
/// one extent. A pack whose `fuse` names the region's schema licenses what the floor cannot
/// prove, and every legality check stands under either.
fn licence<'a>(
    graph: &Graph,
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
    // **The interfaces**: every node of the region claims one the bridge composes.
    let unbridged: Vec<&str> = schema
        .nodes
        .iter()
        .map(|node| node.interface.as_str())
        .filter(|interface| !BRIDGED.contains(interface))
        .collect();
    if !unbridged.is_empty() {
        return Err(MergeRefusal::InterfacesAreNotProved {
            interfaces: unbridged.into_iter().map(str::to_string).collect(),
            region: schema.hash.clone(),
            inert,
        });
    }
    // **The shape**: one the fold builds — a chain, a sibling, a fan-out or a fan-in.
    if let Some(local) = schema.off_chain_node() {
        return Err(MergeRefusal::NotAReductionChain {
            node: positions[local],
            inert,
        });
    }
    // **The extent**: one kernel dispatches over one extent.
    region_extent(graph, positions)?;
    Ok(Licence::Floor)
}

/// The extent the region's nodes dispatch over, or the refusal that they do not share one.
///
/// # Invariant
/// **The first node's extent is the region's own**, and the first node that differs from it is
/// the one named: a merged node has one extent, so a region whose nodes disagree is not one
/// kernel.
fn region_extent(graph: &Graph, positions: &[usize]) -> Result<Count, MergeRefusal> {
    let Node::Kernel(first_node) = &graph.nodes()[positions[0]];
    if let Some(disagreeing) = positions[1..]
        .iter()
        .find(|position| !extents_correspond(first_node.count, count_at(graph, **position)))
    {
        return Err(MergeRefusal::ExtentsDiffer {
            producer: first_node.count,
            consumer: count_at(graph, *disagreeing),
        });
    }
    Ok(first_node.count)
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
/// **A fold over the region's DAG, in evaluation order**, and every condition below is the
/// region's own: the fold composes the region's bodies, then one absorbed position per edge is
/// checked — `PortsDoNotCorrespond` is where a missing port stops.
///
/// **The survivor is where the merged node stands**: an ordinary region's last position, and a
/// sibling region's first.
fn merge_region(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
    shape: &RegionShape,
) -> Result<Merge, MergeRefusal> {
    // Two or more nodes, so a survivor and a first node are both named.
    if positions.len() < 2 {
        return Err(MergeRefusal::RegionIsNotAPair {
            nodes: positions.len(),
        });
    }
    let survivor = match shape {
        RegionShape::Ordinary => *positions
            .last()
            .expect("a region is decided over two or more"),
        RegionShape::Sibling { .. } => *positions
            .first()
            .expect("a region is decided over two or more"),
    };
    // **A node binds as many runtime scalars as its fragment declares.**
    for &position in positions {
        let Node::Kernel(node) = &graph.nodes()[position];
        let wanted = node.fragment.runtime_scalars();
        if node.scalars.len() != wanted {
            return Err(MergeRefusal::ScalarArityDiffers {
                node: position,
                wanted,
                got: node.scalars.len(),
            });
        }
    }
    let links = links(graph, starts, positions)?;
    let Some(returns) = graph.returns() else {
        return Err(MergeRefusal::GraphReturnsEveryValue {
            value: absorbed_value(graph, &links[0]),
        });
    };
    let folded = composed(graph, starts, positions, &links, shape)?;
    // **A staged store is legal only where one lane group holds every lane**: the barrier
    // orders one group's lanes.
    if keeps_a_store(shape, &folded) {
        let lanes = folded.fragment.target.lane_width.lanes() as usize;
        let elements = match region_extent(graph, positions)? {
            Count::Constant(count) => Some(count),
            _ => None,
        };
        if elements.is_none_or(|count| count > lanes) {
            return Err(MergeRefusal::StagedReadIsNotOrdered { elements, lanes });
        }
    }
    // The values the merged fragment fills: the region's sinks, in the fold's own order.
    let (outputs, deleted) = written_and_deleted(graph, starts, positions, &folded.writes);
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
    region_extent(graph, positions)?;
    // The union the region's own edges name, against the ports the merged fragment holds.
    let absorbed: Vec<Vec<usize>> = absorbed_positions(positions, &links);
    let union: Vec<GraphValueId> = match shape {
        RegionShape::Ordinary => {
            let Node::Kernel(first_node) = &graph.nodes()[positions[0]];
            let mut union: Vec<GraphValueId> = first_node.inputs.clone();
            for (index, position) in positions.iter().enumerate().skip(1) {
                let Node::Kernel(node) = &graph.nodes()[*position];
                union.extend(
                    node.inputs
                        .iter()
                        .copied()
                        .enumerate()
                        .filter(|(ordinal, _)| !absorbed[index].contains(ordinal))
                        .map(|(_, value)| value),
                );
            }
            union
        }
        // **The shared value is one port**, however many branches read it.
        RegionShape::Sibling { producer, .. } => {
            let shared = produced(graph, starts, *producer).collect::<Vec<_>>();
            let mut union: Vec<GraphValueId> = Vec::new();
            for (index, position) in positions.iter().enumerate() {
                let Node::Kernel(node) = &graph.nodes()[*position];
                for (ordinal, value) in node.inputs.iter().copied().enumerate() {
                    if absorbed[index].contains(&ordinal) {
                        continue;
                    }
                    if shared.contains(&value) && union.contains(&value) {
                        continue;
                    }
                    union.push(value);
                }
            }
            union
        }
    };
    // The merged node reads the ports the fold holds, in the fold's own order.
    let inputs: Vec<GraphValueId> = folded
        .ports
        .iter()
        .map(|&(node, ordinal)| {
            let Node::Kernel(node) = &graph.nodes()[positions[node]];
            node.inputs[ordinal]
        })
        .collect();
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
    if union.len() > folded.fragment.inputs {
        return Err(MergeRefusal::PortsDoNotCorrespond {
            position: folded.fragment.inputs,
            producer: union[folded.fragment.inputs],
            merged: None,
        });
    }
    // **The merged node stands where its inputs are produced and its outputs are read.**
    if let RegionShape::Sibling { .. } = shape {
        for &value in &inputs {
            let Some(producer) = producer_of(graph, starts, value) else {
                continue;
            };
            if producer >= survivor {
                return Err(MergeRefusal::InputIsProducedAfterTheMerge {
                    value,
                    node: producer,
                });
            }
        }
        for &value in &outputs {
            if let Some(node) = readers(graph, value)
                .into_iter()
                .find(|node| *node < survivor)
            {
                return Err(MergeRefusal::OutputIsReadBeforeTheMerge { value, node });
            }
        }
    }
    Ok(Merge {
        absorbed: positions
            .iter()
            .copied()
            .filter(|position| *position != survivor)
            .collect(),
        survivor,
        links,
        outputs,
        fragment: folded.fragment,
        inputs,
        scalars: folded.scalars,
    })
}

/// Whether the merged fragment keeps a producer's store, so the consumer reads it back.
///
/// # Invariant
/// **The composition's own answer is the fact**: the staged path keeps the store and the
/// recomputing path does not, so the merged fragment's own answer is what says its read needs a
/// barrier to reach it. A sibling region's branches stand beside one another and keep no store,
/// so the question is not asked of one.
fn keeps_a_store(shape: &RegionShape, folded: &Folded) -> bool {
    let RegionShape::Ordinary = shape else {
        return false;
    };
    folded.keeps_store
}

/// The node that produced `value`, when one of the graph's nodes did.
fn producer_of(graph: &Graph, starts: &[usize], value: GraphValueId) -> Option<usize> {
    graph
        .nodes()
        .iter()
        .enumerate()
        .find_map(|(position, node)| {
            let Node::Kernel(kernel) = node;
            (value >= starts[position] && value < starts[position] + kernel.fragment.outputs)
                .then_some(position)
        })
}

/// The graph values the node at `position` produces.
fn produced(graph: &Graph, starts: &[usize], position: usize) -> std::ops::Range<GraphValueId> {
    starts[position]..starts[position] + fragment_at(graph, position).outputs
}

/// The fragment the region's nodes fold into, or the decision the fold cannot make.
///
/// # Invariant
/// **One fragment per region, and every body in it is one the region dispatched**: the fold
/// composes the region's own bodies and nothing else, so a merged node computes what the
/// region's nodes computed and no more.
fn composed(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
    links: &[Link],
    shape: &RegionShape,
) -> Result<Folded, MergeRefusal> {
    let nodes = region_nodes(graph, positions, links);
    match shape {
        RegionShape::Ordinary => {
            let members: Vec<usize> = (0..nodes.len()).collect();
            fold_branch(&nodes, &members)
        }
        RegionShape::Sibling { producer, branches } => {
            sibling_fold(graph, starts, positions, &nodes, *producer, branches)
        }
    }
}

/// The fragment the sibling region's branches fold into, or the decision the fold cannot make.
///
/// # Invariant
/// **Each branch is folded on its own, and the branches are merged into one fragment**: a
/// branch is a chain the producer's value enters at its head, and the merge reads that value
/// at one port however many branches read it.
fn sibling_fold(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
    nodes: &[RegionNode<'_>],
    producer: usize,
    branches: &[Vec<usize>],
) -> Result<Folded, MergeRefusal> {
    let local = |position: usize| positions.iter().position(|&held| held == position);
    let mut folded: Vec<Folded> = Vec::with_capacity(branches.len());
    // **The shared port of each branch**: the branch's head and the input the producer's
    // value reaches there.
    let mut shared: Vec<(usize, usize)> = Vec::with_capacity(branches.len());
    for branch in branches {
        let members: Vec<usize> = branch
            .iter()
            .filter_map(|&position| local(position))
            .collect();
        let Some(&root) = members.first() else {
            return Err(MergeRefusal::RegionIsNotAPair {
                nodes: members.len(),
            });
        };
        let branch_folded = fold_branch(nodes, &members)?;
        let Some(ordinal) = producer_ordinal(graph, starts, producer, positions[root]) else {
            return Err(MergeRefusal::TheProducerIsNotRead {
                node: positions[root],
            });
        };
        let Some(at) = branch_folded
            .ports
            .iter()
            .position(|&(member, held)| member == root && held == ordinal)
        else {
            return Err(MergeRefusal::TheProducerIsNotRead {
                node: positions[root],
            });
        };
        shared.push((root, at));
        folded.push(branch_folded);
    }
    // **The branches merge into one fragment.**
    let siblings: Vec<Sibling<'_>> = folded
        .iter()
        .zip(shared.iter())
        .map(|(branch, &(_, at))| Sibling {
            fragment: &branch.fragment,
            shared: at as u32,
        })
        .collect();
    let fragment = compose_siblings(&siblings)
        .map_err(|refusal| MergeRefusal::CompositionRefused { refusal })?;
    // **The merged ports**: the first branch's own, then each branch's but the shared one.
    let mut ports = folded[0].ports.clone();
    let mut writes = folded[0].writes.clone();
    let mut scalars = folded[0].scalars.clone();
    for (index, branch) in folded.iter().enumerate().skip(1) {
        let (root, at) = shared[index];
        ports.extend(
            branch
                .ports
                .iter()
                .copied()
                .filter(|&(member, held)| !(member == root && held == at)),
        );
        writes.extend(branch.writes.iter().copied());
        scalars.extend(branch.scalars.iter().copied());
    }
    Ok(Folded {
        fragment,
        ports,
        writes,
        scalars,
        keeps_store: false,
    })
}

/// The region's nodes, with the edges among them in region-local positions.
///
/// # Invariant
/// **One node per region position, in evaluation order**, and the edges are the region's own:
/// a link the graph holds between two of the region's nodes is an edge of the DAG, and a link
/// to a node outside it is not one.
fn region_nodes<'a>(graph: &'a Graph, positions: &[usize], links: &[Link]) -> Vec<RegionNode<'a>> {
    let mut nodes: Vec<RegionNode> = positions
        .iter()
        .map(|&position| RegionNode {
            fragment: fragment_at(graph, position),
            scalars: scalars_at(graph, position),
            absorbed: Vec::new(),
            consumers: Vec::new(),
        })
        .collect();
    let local = |position: usize| positions.iter().position(|&held| held == position);
    for link in links {
        let (Some(from), Some(to)) = (local(link.from), local(link.to)) else {
            continue;
        };
        let edge = Edge {
            from,
            to,
            input: link.input,
        };
        nodes[to].absorbed.push(edge);
        nodes[from].consumers.push(edge);
    }
    nodes
}

/// The fold of the branch `members` holds: its nodes composed into one fragment.
///
/// # Invariant
/// **Every member is folded once, in evaluation order, and the branch's fragment holds the
/// bodies it folded**: a node reading the body before it is a chain step, a node reading
/// several producers is a fan-in over the branch's own bodies, and a node several consumers
/// read splits the branch, each consumer taking the branch it owns.
fn fold_branch(nodes: &[RegionNode<'_>], members: &[usize]) -> Result<Folded, MergeRefusal> {
    let inside: HashSet<usize> = members.iter().copied().collect();
    let mut steps: Vec<Step> = Vec::new();
    let mut prefix: Vec<usize> = Vec::new();
    let mut head: Option<usize> = None;
    for &local in members {
        let node = &nodes[local];
        let absorbed: Vec<Edge> = node
            .absorbed
            .iter()
            .copied()
            .filter(|edge| inside.contains(&edge.from))
            .collect();
        if absorbed.len() > 2 {
            return Err(MergeRefusal::FanInIsWiderThanTwo {
                inputs: absorbed.len(),
            });
        }
        prefix.push(local);
        // **A node reading several producers is a fan-in**, and so is one reading a body that
        // is not the one before it.
        let fan_in = absorbed.len() >= 2 || (absorbed.len() == 1 && Some(absorbed[0].from) != head);
        if fan_in {
            steps.push(Step::FanIn {
                bodies: prefix.clone(),
            });
        } else if absorbed.len() == 1 {
            steps.push(Step::Chain {
                local,
                input: absorbed[0].input,
            });
        } else {
            steps.push(Step::Root { local });
        }
        head = Some(local);
        // **Several consumers split the branch**: each takes the branch it owns, and only
        // when those branches do not meet.
        let consumers: Vec<Edge> = node
            .consumers
            .iter()
            .copied()
            .filter(|edge| inside.contains(&edge.to))
            .collect();
        let reaches_of: Vec<Vec<usize>> = consumers
            .iter()
            .map(|edge| reaches(nodes, &inside, edge.to))
            .collect();
        let disjoint = reaches_of.iter().enumerate().all(|(one, held)| {
            reaches_of.iter().enumerate().all(|(other, other_held)| {
                one == other || held.iter().all(|m| !other_held.contains(m))
            })
        });
        if consumers.len() > 1 && disjoint {
            let producer = compose_steps(nodes, &steps)?;
            let mut claimed: Vec<usize> = Vec::new();
            let mut branches: Vec<(u32, Vec<usize>)> = Vec::with_capacity(consumers.len());
            for (edge, reached) in consumers.iter().zip(reaches_of.iter()) {
                let branch: Vec<usize> = reached
                    .iter()
                    .copied()
                    .filter(|local| !claimed.contains(local))
                    .collect();
                claimed.extend(branch.iter().copied());
                branches.push((edge.input as u32, branch));
            }
            let folded: Vec<Folded> = branches
                .iter()
                .map(|(_, branch)| fold_branch(nodes, branch))
                .collect::<Result<Vec<_>, MergeRefusal>>()?;
            return fan_out(&producer, &branches, &folded);
        }
    }
    compose_steps(nodes, &steps)
}

/// One step of a branch's fold.
///
/// # Invariant
/// **The branch's bodies in evaluation order**: a root begins a run, a chain step reads the
/// body before it, and a fan-in composes the branch's bodies up to and including itself.
enum Step {
    /// The branch's first body: nothing composes into it.
    Root {
        /// The region-local member.
        local: usize,
    },
    /// A chain step: the member reads the body before it.
    Chain {
        /// The region-local member.
        local: usize,
        /// The input ordinal the producer's value arrives at.
        input: usize,
    },
    /// A fan-in: the member reads several producers, and the branch's bodies compose into it.
    FanIn {
        /// The bodies the fan-in composes: the branch's members up to and including it.
        bodies: Vec<usize>,
    },
}

/// The fragment the branch's steps compose into.
///
/// # Invariant
/// **The chain steps wait for the fan-in that may follow them**: a fan-in composes the
/// branch's own bodies, so a chain step before it is a body of that fan-in and not a
/// composition of its own.
fn compose_steps(nodes: &[RegionNode<'_>], steps: &[Step]) -> Result<Folded, MergeRefusal> {
    let mut merged: Option<Folded> = None;
    let mut pending: Vec<(usize, usize)> = Vec::new();
    for step in steps {
        match step {
            Step::Root { local } => {
                merged = Some(Folded {
                    fragment: nodes[*local].fragment.clone(),
                    ports: own_ports(*local, &nodes[*local]),
                    writes: vec![*local],
                    scalars: own_scalars(&nodes[*local]),
                    keeps_store: false,
                });
                pending.clear();
            }
            Step::Chain { local, input } => pending.push((*local, *input)),
            Step::FanIn { bodies } => {
                merged = Some(fan_in(nodes, bodies)?);
                pending.clear();
            }
        }
    }
    // The chain steps that follow the last fan-in compose onto what came before.
    for (local, input) in pending {
        let node = &nodes[local];
        merged = Some(match merged {
            None => Folded {
                fragment: node.fragment.clone(),
                ports: own_ports(local, node),
                writes: vec![local],
                scalars: own_scalars(node),
                keeps_store: false,
            },
            Some(producer) => {
                let fragment = compose(&producer.fragment, node.fragment, input as u32)
                    .map_err(|refusal| MergeRefusal::CompositionRefused { refusal })?;
                // **The staged path keeps the producer's store**: the producer's outputs,
                // then the consumer's.
                let staged = fragment.outputs > node.fragment.outputs;
                let writes = if staged {
                    let mut writes = producer.writes.clone();
                    writes.push(local);
                    writes
                } else {
                    vec![local]
                };
                Folded {
                    fragment,
                    ports: chained_ports(&producer, local, node, input),
                    writes,
                    scalars: chained_scalars(&producer, node),
                    keeps_store: staged || producer.keeps_store,
                }
            }
        });
    }
    merged.ok_or(MergeRefusal::RegionIsNotAPair { nodes: steps.len() })
}

/// The fragment the fan-in's bodies compose into.
///
/// # Invariant
/// **The bodies are the branch's own, in evaluation order**: each names the producers it
/// absorbs, so the composition is not told an order it would have to take on trust.
///
/// **A producer whose value no address re-derives is staged**: its store answers the body that
/// reads it, so the merged fragment writes it too and the value is no longer deleted.
fn fan_in(nodes: &[RegionNode<'_>], bodies: &[usize]) -> Result<Folded, MergeRefusal> {
    let steps = fan_in_steps(nodes, bodies);
    // **The staged producers are the ones whose value no address re-derives**.
    let staged = carried_producers(&steps);
    let fragment = compose_fan_in_staged(&steps, &staged)
        .map_err(|refusal| MergeRefusal::CompositionRefused { refusal })?;
    let mut ports = Vec::new();
    for (index, &body) in bodies.iter().enumerate() {
        let absorbed: Vec<usize> = steps[index]
            .absorbs
            .iter()
            .map(|(_, input)| *input)
            .collect();
        ports.extend(
            (0..nodes[body].fragment.inputs)
                .filter(|ordinal| !absorbed.contains(ordinal))
                .map(|ordinal| (body, ordinal)),
        );
    }
    // **The staged producers' outputs are written, so they are no longer deleted**.
    let mut writes: Vec<usize> = staged.iter().map(|&producer| bodies[producer]).collect();
    writes.push(
        *bodies
            .last()
            .expect("a fan-in holds the body that reads them"),
    );
    let scalars: Vec<GraphScalar> = bodies
        .iter()
        .flat_map(|body| own_scalars(&nodes[*body]))
        .collect();
    Ok(Folded {
        fragment,
        ports,
        writes,
        scalars,
        keeps_store: !staged.is_empty(),
    })
}

/// The fragment the fan-out composes: the producer's body with each branch's inlined.
///
/// # Invariant
/// **One copy of the producer per branch**: the producer's body is the merged body's own, and
/// each branch reads the producer's value at its own port, which is where the producer's cone
/// is copied.
fn fan_out(
    producer: &Folded,
    branches: &[(u32, Vec<usize>)],
    folded: &[Folded],
) -> Result<Folded, MergeRefusal> {
    let inlined: Vec<(&KernelFragment, u32)> = folded
        .iter()
        .zip(branches.iter())
        .map(|(branch, (input, _))| (&branch.fragment, *input))
        .collect();
    let fragment = compose_fan_out(&producer.fragment, &inlined)
        .map_err(|refusal| MergeRefusal::CompositionRefused { refusal })?;
    let mut ports = producer.ports.clone();
    // **The staged path keeps the producer's store**, so the merged fragment writes it too.
    let kept = fragment.outputs
        > folded
            .iter()
            .map(|branch| branch.fragment.outputs)
            .sum::<usize>();
    let mut writes = if kept {
        producer.writes.clone()
    } else {
        Vec::new()
    };
    for ((input, members), branch) in branches.iter().zip(folded.iter()) {
        let root = members
            .first()
            .expect("a branch holds the consumer it starts at");
        ports.extend(
            branch
                .ports
                .iter()
                .copied()
                .filter(|&(member, ordinal)| !(member == *root && ordinal == *input as usize)),
        );
        writes.extend(branch.writes.iter().copied());
    }
    let mut scalars = producer.scalars.clone();
    for branch in folded {
        scalars.extend(branch.scalars.iter().copied());
    }
    Ok(Folded {
        fragment,
        ports,
        writes,
        scalars,
        keeps_store: kept || producer.keeps_store,
    })
}

/// The ports a body's own fragment reads.
fn own_ports(local: usize, node: &RegionNode<'_>) -> Vec<(usize, usize)> {
    (0..node.fragment.inputs)
        .map(|ordinal| (local, ordinal))
        .collect()
}

/// The runtime scalars a body's own node binds.
fn own_scalars(node: &RegionNode<'_>) -> Vec<GraphScalar> {
    node.scalars.to_vec()
}

/// The ports the merged fragment reads: the producer's own, then the consumer's but the one
/// the producer fills.
fn chained_ports(
    producer: &Folded,
    local: usize,
    node: &RegionNode<'_>,
    input: usize,
) -> Vec<(usize, usize)> {
    let mut ports = producer.ports.clone();
    ports.extend(
        (0..node.fragment.inputs)
            .filter(|ordinal| *ordinal != input)
            .map(|ordinal| (local, ordinal)),
    );
    ports
}

/// The runtime scalars a merged node binds: the producer's own, then the consumer's.
fn chained_scalars(producer: &Folded, node: &RegionNode<'_>) -> Vec<GraphScalar> {
    let mut scalars = producer.scalars.clone();
    scalars.extend(own_scalars(node));
    scalars
}

/// The fan-in's bodies: the branch's own fragments, each with the edges it absorbs.
///
/// # Invariant
/// **Evaluation order, and every index in a body's `absorbs` names an earlier entry**: a body
/// reads only what a body before it forwarded, so the list is topologically ordered by
/// construction and the composition is not told an order it would have to take on trust.
fn fan_in_steps<'a>(nodes: &'a [RegionNode<'a>], bodies: &[usize]) -> Vec<Producer<'a>> {
    bodies
        .iter()
        .enumerate()
        .map(|(index, &body)| {
            let absorbs: Vec<(usize, usize)> = nodes[body]
                .absorbed
                .iter()
                .filter(|edge| edge.to == body && bodies[..index].contains(&edge.from))
                .map(|edge| {
                    let producer = bodies[..index]
                        .iter()
                        .position(|earlier| *earlier == edge.from)
                        .expect("a producer is an earlier body of the fan-in");
                    (producer, edge.input)
                })
                .collect();
            Producer {
                fragment: nodes[body].fragment,
                absorbs,
            }
        })
        .collect()
}

/// The members of the branch that `root` reaches, in evaluation order.
///
/// # Invariant
/// **The branch's own nodes only**, so a consumer's branch is the part of the region it owns
/// and a node another branch owns is not folded twice.
fn reaches(nodes: &[RegionNode<'_>], inside: &HashSet<usize>, root: usize) -> Vec<usize> {
    let mut seen: Vec<usize> = vec![root];
    let mut index = 0;
    while index < seen.len() {
        let current = seen[index];
        index += 1;
        for edge in &nodes[current].consumers {
            if inside.contains(&edge.to) && !seen.contains(&edge.to) {
                seen.push(edge.to);
            }
        }
    }
    seen.sort_unstable();
    seen
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

/// Whether the node at `position` produces `value`.
fn produced_by(graph: &Graph, starts: &[usize], position: usize, value: GraphValueId) -> bool {
    let Node::Kernel(node) = &graph.nodes()[position];
    let produced = starts[position]..starts[position] + node.fragment.outputs;
    produced.contains(&value)
}

/// The region's edges, one per absorbed input, in (reader, input ordinal) order.
///
/// # Invariant
/// **One absorbed position per edge**, each naming the producer whose value fills it, and read
/// off the graph's own table rather than off the shape.
///
/// **Both ends are graph positions**, as [`Link`] states: the region's own order is not the
/// graph's, and an edge that named a rank would point at another node.
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
        links.extend(read.into_iter().map(|(input, from)| Link {
            from: positions[from],
            to,
            input,
        }));
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
/// **The region's sinks are written, and every other value the region produced is deleted**: a
/// sink's value is read outside the region or returned, so the merged fragment holds it, and a
/// value read only inside the region is the merge's to delete. One that is returned or read
/// elsewhere is what the two refusals name.
fn written_and_deleted(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
    writes: &[usize],
) -> (Vec<GraphValueId>, Vec<GraphValueId>) {
    let outputs: Vec<GraphValueId> = writes
        .iter()
        .flat_map(|&local| produced(graph, starts, positions[local]))
        .collect();
    let deleted: Vec<GraphValueId> = positions
        .iter()
        .flat_map(|&position| produced(graph, starts, position))
        .filter(|value| !outputs.contains(value))
        .collect();
    (outputs, deleted)
}

/// The fragment the node at `position` dispatched.
fn fragment_at(graph: &Graph, position: usize) -> &KernelFragment {
    let Node::Kernel(node) = &graph.nodes()[position];
    &node.fragment
}

/// The runtime scalars the node at `position` binds, in its fragment's own order.
fn scalars_at(graph: &Graph, position: usize) -> &[GraphScalar] {
    let Node::Kernel(node) = &graph.nodes()[position];
    &node.scalars
}

/// Every read of `value` in the graph, one entry per reading node.
///
/// # Invariant
/// **One entry per node, whatever it reads the value with**: a node reads a value as an input,
/// as the extent it dispatches over, or as one of its runtime scalars, and the merge deletes a
/// value only when nothing reads it.
fn readers(graph: &Graph, value: GraphValueId) -> Vec<usize> {
    let mut read = Vec::new();
    for (position, node) in graph.nodes().iter().enumerate() {
        let Node::Kernel(kernel) = node;
        let mut reads = false;
        for &held in &kernel.inputs {
            if held == value {
                reads = true;
            }
        }
        for scalar in &kernel.scalars {
            if let GraphScalar::Value(held) = scalar
                && *held == value
            {
                reads = true;
            }
        }
        match kernel.count {
            Count::Value(extent) | Count::Device { value: extent, .. } if extent == value => {
                reads = true;
            }
            _ => {}
        }
        if reads {
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
        let merged = merges.get(&position);
        let (fragment, reads, outputs, scalars) = match merged {
            Some(merge) => (
                merge.fragment.clone(),
                &merge.inputs,
                merge.outputs.clone(),
                merge.scalars.clone(),
            ),
            None => (
                kernel.fragment.clone(),
                &kernel.inputs,
                (starts[position]..starts[position] + kernel.fragment.outputs).collect(),
                kernel.scalars.clone(),
            ),
        };
        let count = mapped_count(kernel.count, &map)?;
        let reads: Vec<GraphValueId> = reads
            .iter()
            .map(|value| map[*value])
            .collect::<Option<Vec<_>>>()?;
        let scalars: Vec<GraphScalar> = scalars
            .iter()
            .map(|scalar| scalar.map_value(|v| map[v]))
            .collect::<Option<Vec<_>>>()?;
        let first = rebuilt
            .push(
                Node::Kernel(KernelNode {
                    fragment: fragment.clone(),
                    inputs: reads,
                    count,
                    scalars,
                }),
                fragment.outputs,
            )
            .ok()?;
        // **A merged fragment is registered with the conservative claim**, where it lands.
        if merged.is_some() {
            intern_kernel(fragment.clone(), Some(merged_claim()));
        }
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
/// pass's own line names the node schema its pack fired for. The floor's sentence names the
/// **interfaces the region's nodes claim**, and every one of them is one the bridge composes.
fn licensed_by(licensed: &Licence<'_>, schema: &RegionSchema) -> String {
    match licensed {
        Licence::Floor => format!(
            "licensed by the floor: every node of the region claims {} and nothing else",
            claimed_interfaces(schema)
        ),
        Licence::Pack(pack) => format!(
            "licensed by pack \"{}\", whose `fuse` names this region's schema {}",
            pack.name(),
            schema.hash
        ),
    }
}

/// The interfaces the region's own nodes claim, in name order, as one sentence's list of them.
///
/// # Invariant
/// **One name per interface, in name order**: a region holds two nodes claiming one interface
/// names it once, so the sentence says what the region's nodes claim and not how many they are.
fn claimed_interfaces(schema: &RegionSchema) -> String {
    let interfaces: BTreeSet<&str> = schema
        .nodes
        .iter()
        .map(|node| node.interface.as_str())
        .collect();
    interfaces.into_iter().collect::<Vec<_>>().join(" or ")
}

/// One merged region's line: the nodes, the absorbed input, the licence and the survivor's claim.
///
/// # Invariant
/// The licence is named in the line and not left to the graph: a runner reading a report has to
/// know whether a pack's `fuse` decided this merge or the engine's own proof did. A region of
/// more than two nodes names **every** node and **every** absorbed position, each with the node
/// it is an input of rather than by its rank.
fn merged_line(
    index: usize,
    merge: &Merge,
    licensed: &Licence<'_>,
    schema: &RegionSchema,
    shape: &RegionShape,
) -> String {
    let licence = licensed_by(licensed, schema);
    let registered = "the merged fragment is registered with the conservative claim `Opaque`: a \
                      pass reading the registry names it and refuses it by name, and no reader \
                      reads algebra from it";
    // **The digest is on the line**: the position is one run's, and the digest is the identity.
    let identity = format!(
        "the merged node's identity is {digest}",
        digest = merged_node::digest_of(&merge.fragment)
    );
    // **A sibling merge absorbs no input position**: its branches stand beside one another.
    if let RegionShape::Sibling { producer, .. } = shape {
        let nodes = merge.absorbed.iter().copied().chain([merge.survivor]);
        return format!(
            "region {index}: nodes {} merged into one node at {} (the survivor), the branches of \
             node {producer} reading its value at one port, {licence}; {registered}; {identity}",
            join(nodes),
            merge.survivor,
        );
    }
    if merge.absorbed.len() == 1 && merge.links.len() == 1 {
        return format!(
            "region {index}: nodes {} and {} merged into one node at {} (the survivor), absorbing \
             input position {} of node {}, {licence}; {registered}; {identity}",
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
         {absorbing}, {licence}; {registered}; {identity}",
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
