//! The region merge pass: a producer→consumer region becomes one node.
//! See docs/notes/graph-fusion.md §5 and §8.
//!
//! # Invariant
//! The pass merges and never reorders: the surviving node stands at the region's last position,
//! every other value keeps its order, and a region the pass declines changes nothing.

use std::collections::{HashMap, HashSet};
use std::fmt;

use lichen_compute::compute::{GraphPass, LaunchExtent, claimed_kernels, set_graph_pass};
use lichen_graph_ir::{Count, Graph, KernelNode, Node, ValueId as GraphValueId};
use lichen_kernel_ir::{
    ComposeRefusal, KernelBody, KernelFragment, KernelInstr, ScalarClass, ValueDef, ValueId,
    compose,
};
use lichen_lowlevel::Module;

use crate::fusion_claim::{
    Instance, claim_of, derive_regions, fragment_of, kernel_id, offsets, region_positions,
};
use crate::program::LangProgram;

/// Install the region merge pass for `module`, replacing any previous pass.
pub fn install_region_merge(module: &Module<LangProgram>) {
    set_graph_pass(region_merge_pass(module));
}

/// The pass `install_region_merge` installs, for a caller that wants to wrap it.
pub fn region_merge_pass(module: &Module<LangProgram>) -> Box<dyn GraphPass> {
    Box::new(RegionMerge {
        instances: instances(module),
    })
}

/// The pass: a module's claimed kernels, joined to a graph node by its fragment digest.
struct RegionMerge {
    /// One instance per claimed kernel the module's own values name.
    instances: Vec<Instance>,
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
    /// The surviving node's inputs, with the absorbed position removed.
    inputs: Vec<GraphValueId>,
}

impl GraphPass for RegionMerge {
    /// Merge every two-node region whose legality conditions hold.
    ///
    /// # Invariant
    /// A region is looked at once and reported once, merged or refused. The graph handed over is
    /// the recording every time, because a rewrite is stored under its own digest.
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
                Ok(_) => match merge_region(graph, &starts, positions) {
                    Ok(merge) => (merged_line(index, &merge), Some(merge)),
                    Err(refusal) => (
                        refused_line(index, positions, refusal.name(), &refusal.message()),
                        None,
                    ),
                },
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
    /// The consumer reads the absorbed output where the survivor cannot compute it.
    ReadAddressIsNotComputable {
        /// The read instruction.
        read: ValueId,
        /// The address it reads, as a report names it.
        address: String,
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
    /// A position the producer reads is not the buffer the merged fragment puts there.
    PortsDoNotCorrespond {
        /// The producer's input position.
        position: usize,
        /// The value the producer reads there.
        producer: GraphValueId,
        /// The value the merged fragment puts there, if it has that many inputs.
        merged: Option<GraphValueId>,
    },
    /// `compose` refused the two fragments.
    CompositionRefused {
        /// What `compose` refused.
        refusal: ComposeRefusal,
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
            MergeRefusal::ReadAddressIsNotComputable { .. } => "ReadAddressIsNotComputable",
            MergeRefusal::ClassesDisagree { .. } => "ClassesDisagree",
            MergeRefusal::ClassIsNotStated { .. } => "ClassIsNotStated",
            MergeRefusal::PortsDoNotCorrespond { .. } => "PortsDoNotCorrespond",
            MergeRefusal::CompositionRefused { refusal } => refusal.name(),
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
            MergeRefusal::ReadAddressIsNotComputable { read, address } => write!(
                formatter,
                "the consumer reads the absorbed buffer at {address} ({read:?}), and a survivor \
                 computes only the constant 0 or its own invocation index"
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
                "the producer reads graph value {producer} at input position {position}, and the \
                 merged fragment puts {merged:?} there"
            ),
            MergeRefusal::CompositionRefused { refusal } => write!(formatter, "{refusal}"),
        }
    }
}

/// The merge one region's two nodes decide, or why they do not.
///
/// # Invariant
/// Every condition is checked before `compose` reads either body: a merge that slipped past one
/// would be a node whose body reads a buffer the program never wired to it.
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
    check_addresses(&consumer_node.fragment, absorbed as u32)?;
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
    let inputs: Vec<GraphValueId> = consumer_node
        .inputs
        .iter()
        .enumerate()
        .filter(|(ordinal, _)| *ordinal != absorbed)
        .map(|(_, value)| *value)
        .collect();
    for (position, value) in producer_node.inputs.iter().enumerate() {
        if inputs.get(position) != Some(value) {
            return Err(MergeRefusal::PortsDoNotCorrespond {
                position,
                producer: *value,
                merged: inputs.get(position).copied(),
            });
        }
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

/// Check every read of the absorbed buffer addresses a position the survivor can compute.
///
/// # Invariant
/// `compose` binds the producer's written value wherever the consumer read the absorbed input, so a
/// read at another address answers a lane's own value where the program asked for another. The
/// invocation-index arm is a licence no program cashes: an elementwise producer's forwarded value
/// is lane-dependent, so `compose` refuses that merge as `ForwardedValueIsLaneDependent`.
fn check_addresses(fragment: &KernelFragment, absorbed: u32) -> Result<(), MergeRefusal> {
    let index = fragment.body.parameters().last().copied();
    for block in &fragment.body.blocks {
        for &value in &block.instrs {
            let Some(ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                args,
                ..
            }) = fragment.body.values.get(value.0 as usize)
            else {
                continue;
            };
            let [position, address] = args[..] else {
                continue;
            };
            if !is_int_constant(&fragment.body, position, absorbed) {
                continue;
            }
            if is_int_constant(&fragment.body, address, 0) || Some(address) == index {
                continue;
            }
            return Err(MergeRefusal::ReadAddressIsNotComputable {
                read: value,
                address: describe_address(&fragment.body, address),
            });
        }
    }
    Ok(())
}

/// Whether `value` is the integer constant `wanted`.
fn is_int_constant(body: &KernelBody, value: ValueId, wanted: u32) -> bool {
    matches!(
        body.values.get(value.0 as usize),
        Some(ValueDef::Instr {
            op: KernelInstr::Const(ScalarClass::Int, bits),
            ..
        }) if *bits as u64 == u64::from(wanted)
    )
}

/// The address a refused read names, as a report reads it.
fn describe_address(body: &KernelBody, address: ValueId) -> String {
    match body.values.get(address.0 as usize) {
        Some(ValueDef::Instr {
            op: KernelInstr::Const(ScalarClass::Int, bits),
            ..
        }) => format!("the constant {bits}"),
        _ => format!("{address:?}"),
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

/// One merged region's line, naming both nodes, the absorbed input and the unregistered survivor.
fn merged_line(index: usize, merge: &Merge) -> String {
    format!(
        "region {index}: nodes {} and {} merged into one node at {} (the survivor), absorbing \
         input position {} of node {}; the merged fragment is unregistered, so it carries no claim \
         and a later digest-keyed pass cannot name it",
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
