//! The region schema derived from a recorded graph.
//! See docs/notes/graph-fusion.md §4.

use std::collections::{BTreeSet, HashMap};
use std::fmt::{self, Write as _};

use lichen_fusion::{Expr, Reduction, Variable, normalize};
use lichen_fusion_bridge::{BridgeRefusal, ReadRoles, projection};
use lichen_graph_ir::{Count, Graph, Node, ValueId as GraphValueId};
use lichen_kernel_ir::{KernelClaim, KernelFragment, KernelId, ScalarClass, fragment_digest};
use lichen_utils::hash::{Hash, hex, sha256};

use super::marked_value;

/// One kernel a program dispatched, as the kernel registry holds it.
///
/// # Invariant
/// A graph node holds a fragment clone and no kernel id, and a claim lives in the registry
/// keyed by id, so a derivation joins the two on the fragment itself: two instances sharing
/// one fragment with different claims are refused rather than guessed.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// The kernel's registry id.
    pub kernel: KernelId,
    /// The interface the kernel was compiled for.
    pub claim: KernelClaim,
    /// The fragment the dispatcher compiled, with its marks published in it.
    pub fragment: KernelFragment,
}

/// A region's content hash, as `sha256:<hex>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SchemaHash(Hash);

impl SchemaHash {
    /// The digest's hex, without the algorithm prefix.
    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// The hash a `sha256:<64 hex>` spelling names, or `None` when the text is not one.
    ///
    /// # Invariant
    /// The one spelling both sides of a pack agree on: a reader takes the text as it is printed,
    /// so a wrong digit count or a non-hex character is `None` rather than a shorter digest.
    pub fn parse(text: &str) -> Option<SchemaHash> {
        let digits = text.strip_prefix("sha256:")?;
        if digits.len() != 64 {
            return None;
        }
        let mut bytes: Hash = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(digits.as_bytes().chunks(2)) {
            let pair = std::str::from_utf8(pair).ok()?;
            *byte = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(SchemaHash(bytes))
    }
}

impl fmt::Display for SchemaHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "sha256:{}", hex(&self.0))
    }
}

/// Which side of the region boundary a port is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortKind {
    /// A buffer the region reads and no node in it produced.
    Input,
    /// A buffer the region writes and no node in it reads.
    Output,
    /// The extent a node dispatches over.
    Extent,
}

/// One value crossing the region boundary.
///
/// # Invariant
/// A port is named by the region-local node and the ordinal inside it, never by the graph's
/// own value number: `value` is one run's numbering and is reported, not hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    /// Which side of the boundary this is.
    pub kind: PortKind,
    /// The region-local position of the node the port belongs to.
    pub node: usize,
    /// The buffer ordinal, or `0` for an extent.
    pub position: usize,
    /// The element class, for a buffer port.
    pub class: Option<ScalarClass>,
    /// The graph value that crosses, where one does.
    pub value: Option<GraphValueId>,
    /// The extent, when the build already knew it.
    pub count: Option<usize>,
}

/// One role of an interface instance, in both forms a reader needs.
///
/// # Invariant
/// [`Self::canonical`] is [`Self::expression`] normalized once, here: §4's hash is over the
/// canonical form, and §6's abstraction must read the tree the logic wrote, so neither form
/// stands for the other.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleProjection {
    /// The role the claim names.
    pub role: String,
    /// The expression the marked subexpression reads, before canonicalization.
    pub expression: Expr,
    /// The same expression's canonical form.
    pub canonical: Expr,
}

/// One interface instance of a region.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaNode {
    /// The region-local position, in evaluation order.
    pub position: usize,
    /// The kernel's registry id.
    pub kernel: KernelId,
    /// The interface the kernel claims.
    pub interface: String,
    /// Every role the claim requires, in name order.
    pub projections: Vec<RoleProjection>,
}

impl SchemaNode {
    /// The one projection a role names, as the marked code reads.
    pub fn projection(&self, role: &str) -> Option<&Expr> {
        self.projections
            .iter()
            .find(|projection| projection.role == role)
            .map(|projection| &projection.expression)
    }

    /// The one projection a role names, in canonical form.
    pub fn canonical(&self, role: &str) -> Option<&Expr> {
        self.projections
            .iter()
            .find(|projection| projection.role == role)
            .map(|projection| &projection.canonical)
    }

    /// The canonical text of this node alone, without its position.
    ///
    /// # Invariant
    /// One node, one pack: the position is deliberately absent, so a plan written for a node
    /// serves it wherever a region puts it. [`RegionSchema::canonical_form`] writes the position
    /// and then exactly this text, so the two granularities cannot drift.
    pub fn canonical_form(&self) -> String {
        let mut text = String::new();
        self.write_form(&mut text);
        text
    }

    /// The hash of [`Self::canonical_form`], as `sha256:<hex>` prints.
    ///
    /// # Invariant
    /// The text is the one a report prints, so a pack author recomputes this from what they see
    /// (`docs/notes/graph-fusion.md` §4).
    pub fn hash(&self) -> SchemaHash {
        SchemaHash(sha256(self.canonical_form().as_bytes()))
    }

    /// This node's form, with no position of its own, appended to `text`.
    fn write_form(&self, text: &mut String) {
        let _ = writeln!(text, "interface {}", self.interface);
        for projection in &self.projections {
            let _ = write!(text, "role {} ", projection.role);
            write_canonical(&projection.canonical, text);
            text.push('\n');
        }
    }

    /// The reduction this node's `combine` and `identity` projections declare.
    ///
    /// # Invariant
    /// One role each, read as §6's engine reads them: the combine's operator is the one the
    /// body wrote, and the identity is read from the canonical form, where a spelled
    /// `0.0 - 1000000.0` is the literal it denotes.
    pub fn reduction(&self) -> Result<Reduction, SchemaRefusal> {
        if self.interface != REDUCTION {
            return Err(SchemaRefusal::UnknownInterface {
                node: self.position,
                interface: self.interface.clone(),
            });
        }
        let combine = self
            .projection(COMBINE)
            .ok_or_else(|| SchemaRefusal::MissingRole {
                node: self.position,
                role: COMBINE.to_string(),
            })?;
        let identity = self
            .canonical(IDENTITY)
            .ok_or_else(|| SchemaRefusal::MissingRole {
                node: self.position,
                role: IDENTITY.to_string(),
            })?;
        let Expr::Binary(operator, _, _) = combine else {
            return Err(SchemaRefusal::NotACombine {
                node: self.position,
                projection: combine.clone(),
            });
        };
        let Some(identity) = identity.as_constant() else {
            return Err(SchemaRefusal::NotAnIdentity {
                node: self.position,
                projection: identity.clone(),
            });
        };
        Ok(Reduction {
            combine: *operator,
            identity,
        })
    }

    /// The elementwise function this node's `element` projection declares.
    ///
    /// # Invariant
    /// The one role, read as §6's engine reads a role: the canonical form, so a report and a
    /// pack that names the function name the same one. A node that declares this interface
    /// states a function and nothing more — whether these operations become one kernel is a
    /// pack's decision, so this reading licenses nothing.
    pub fn elementwise(&self) -> Result<&Expr, SchemaRefusal> {
        if self.interface != ELEMENTWISE {
            return Err(SchemaRefusal::UnknownInterface {
                node: self.position,
                interface: self.interface.clone(),
            });
        }
        let element = self
            .canonical(ELEMENT)
            .ok_or_else(|| SchemaRefusal::MissingRole {
                node: self.position,
                role: ELEMENT.to_string(),
            })?;
        if !reads_a_datum(element) {
            return Err(SchemaRefusal::NotAnElementFunction {
                node: self.position,
                projection: element.clone(),
            });
        }
        Ok(element)
    }
}

/// Whether a projection is a function of a datum.
///
/// # Invariant
/// A datum is a buffer read and the roles name every read: `d` is the node's own datum, and
/// `r` the producer's value arriving at the input ordinal the region's edge found. So the
/// function is one over whichever of the two its body read, and never over a loop's carried
/// value — which is `Reduction`'s own element, not an elementwise one.
fn reads_a_datum(element: &Expr) -> bool {
    let variables = element.variables();
    !variables.contains(&Variable::CONSUMER_STATE)
        && (variables.contains(&Variable::DATUM) || variables.contains(&Variable::PRODUCER))
}

/// The analyzable code one instance feeds the next through.
///
/// # Invariant
/// The element is the consumer's `element` projection, both forms of it: the engine abstracts
/// over the expression as written, and the hash is over the canonical one.
#[derive(Debug, Clone, PartialEq)]
pub struct Between {
    /// The region-local position of the instance that produces the value.
    pub producer: usize,
    /// The region-local position of the instance that reads it.
    pub consumer: usize,
    /// The consumer's input ordinal the producer's value arrives at.
    pub input: usize,
    /// The consumer's `element` role as the marked code reads.
    pub element: Expr,
    /// The same expression's canonical form.
    pub canonical: Expr,
}

/// One fusible region: its instances, the code between them, and its boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionSchema {
    /// The interface instances, in evaluation order.
    pub nodes: Vec<SchemaNode>,
    /// The code connecting adjacent instances, one entry per edge.
    pub between: Vec<Between>,
    /// The values crossing the region boundary.
    pub ports: Vec<Port>,
    /// The content hash of the canonical form.
    pub hash: SchemaHash,
}

impl RegionSchema {
    /// The canonical text the hash is taken over.
    ///
    /// # Invariant
    /// Printing it and hashing it are the same text, so a pack author recomputes the hash from
    /// what a report shows. The interfaces, their canonical projections, the between code and
    /// the ports' roles and classes are in it; kernel ids, graph value numbers, a constant
    /// extent's value and every raw fragment body are out. A node is its position and then the
    /// node's own form.
    pub fn canonical_form(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(text, "region {}", self.nodes.len());
        for node in &self.nodes {
            let _ = write!(text, "node {} ", node.position);
            node.write_form(&mut text);
        }
        for between in &self.between {
            let _ = write!(
                text,
                "between {} {} input {} ",
                between.producer, between.consumer, between.input
            );
            write_canonical(&between.canonical, &mut text);
            text.push('\n');
        }
        for port in &self.ports {
            let class = port
                .class
                .map_or("none".to_string(), |class| format!("{class:?}"));
            let _ = writeln!(
                text,
                "port {:?} {} {} {class}",
                port.kind, port.node, port.position
            );
        }
        text
    }
}

/// Why a region has no schema.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaRefusal {
    /// A node's fragment matches two instances whose claims differ.
    AmbiguousClaim {
        /// The graph node.
        node: usize,
        /// The interfaces that matched.
        claims: Vec<String>,
    },
    /// A node's fragment is in no row of a digest-keyed join: nothing registered a claim for it.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::AmbiguousClaim`] because it is a distinct answer: that one is a
    /// join that found two claims and this is a join that found none. A composed fragment is this
    /// arm, because composition registers neither body it joins.
    NotClaimed {
        /// The graph node whose fragment no row holds.
        node: usize,
    },
    /// A claimed role has no mark in the kernel's body.
    RoleNotMarked {
        /// The region-local position of the node.
        node: usize,
        /// The role with no mark.
        role: String,
    },
    /// A role's marked subexpression did not become one symbolic expression.
    ProjectionNotBridged {
        /// The region-local position of the node.
        node: usize,
        /// The role that did not bridge.
        role: String,
        /// What the bridge refused.
        cause: BridgeRefusal,
    },
    /// A canonical form outgrew the normalizer's budget.
    NotCanonical {
        /// The region-local position of the node.
        node: usize,
        /// The role that did not canonicalize.
        role: String,
        /// The budget it exceeded, in nodes.
        budget: usize,
    },
    /// A node's role reaches another node's output, and this reader names no producer.
    ///
    /// # Invariant
    /// One variant per party, not one shared sentence: a reader that knows the producer reports
    /// the edge at [`crate::automatic_tiling`], and this arm is what a reader that does not says.
    BeyondANode {
        /// The position the refusal is read at, region-local or a kernel id.
        node: usize,
        /// The roles whose code reaches a producer the reader cannot name.
        roles: Vec<String>,
    },
    /// A graph node reads a value another node produced, and the reader has no producer for it.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::BeyondANode`] because it is a distinct question and it carries a
    /// distinct fact: that one is the claim's shape and its roles, this one the graph's own edge
    /// and the value it names. A reader with a graph says this; one with only a fragment cannot.
    ReadsAnotherNode {
        /// The graph node whose input is another node's output.
        node: usize,
        /// The value in the graph's table it reads.
        value: usize,
    },
    /// The region's instances are not one producer-to-consumer chain.
    NotAReductionChain {
        /// The graph node whose edges are not a chain's.
        node: usize,
    },
    /// A consumer declares no `element` role for the code between the two instances.
    NoElementRole {
        /// The region-local position of the producer.
        producer: usize,
        /// The region-local position of the consumer.
        consumer: usize,
    },
    /// The consumer's element does not read the producer's value.
    ElementWithoutProducer {
        /// The region-local position of the producer.
        producer: usize,
        /// The region-local position of the consumer.
        consumer: usize,
        /// The element as it bridged.
        element: Expr,
    },
    /// An edge names a value no earlier node of the region produced.
    ///
    /// # Invariant
    /// A recorded graph cannot express one — [`Graph::push`] refuses an edge before its
    /// producer — so this guards a region reader that does not lean on that.
    EdgeOutOfOrder {
        /// The region-local position of the node that reads it.
        consumer: usize,
        /// The value it reads.
        value: GraphValueId,
    },
    /// A node claims an interface this reader has no algebra for.
    UnknownInterface {
        /// The region-local position of the node.
        node: usize,
        /// The interface it claims.
        interface: String,
    },
    /// A role a known interface requires is absent from the instance.
    MissingRole {
        /// The region-local position of the node.
        node: usize,
        /// The role that is missing.
        role: String,
    },
    /// A node's `combine` projection is not one binary operator.
    NotACombine {
        /// The region-local position of the node.
        node: usize,
        /// The projection as it bridged.
        projection: Expr,
    },
    /// A node's `identity` projection is not a literal.
    NotAnIdentity {
        /// The region-local position of the node.
        node: usize,
        /// The projection as it bridged.
        projection: Expr,
    },
    /// A node's `element` projection is not a function of a datum.
    NotAnElementFunction {
        /// The region-local position of the node.
        node: usize,
        /// The projection as it bridged.
        projection: Expr,
    },
}

impl SchemaRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// The refusal's name, for a report that has to say which check refused a region.
    ///
    /// # Invariant
    /// One variant, one name: a report that printed the sentence alone could not be matched
    /// against the variant a caller reads, so the two would drift.
    pub fn name(&self) -> &'static str {
        match self {
            SchemaRefusal::AmbiguousClaim { .. } => "AmbiguousClaim",
            SchemaRefusal::NotClaimed { .. } => "NotClaimed",
            SchemaRefusal::RoleNotMarked { .. } => "RoleNotMarked",
            SchemaRefusal::ProjectionNotBridged { .. } => "ProjectionNotBridged",
            SchemaRefusal::NotCanonical { .. } => "NotCanonical",
            SchemaRefusal::BeyondANode { .. } => "BeyondANode",
            SchemaRefusal::ReadsAnotherNode { .. } => "ReadsAnotherNode",
            SchemaRefusal::NotAReductionChain { .. } => "NotAReductionChain",
            SchemaRefusal::NoElementRole { .. } => "NoElementRole",
            SchemaRefusal::ElementWithoutProducer { .. } => "ElementWithoutProducer",
            SchemaRefusal::EdgeOutOfOrder { .. } => "EdgeOutOfOrder",
            SchemaRefusal::UnknownInterface { .. } => "UnknownInterface",
            SchemaRefusal::MissingRole { .. } => "MissingRole",
            SchemaRefusal::NotACombine { .. } => "NotACombine",
            SchemaRefusal::NotAnIdentity { .. } => "NotAnIdentity",
            SchemaRefusal::NotAnElementFunction { .. } => "NotAnElementFunction",
        }
    }
}

impl fmt::Display for SchemaRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaRefusal::AmbiguousClaim { node, claims } => write!(
                formatter,
                "graph node {node} holds a fragment two of this program's kernels share, and \
                 they claim different interfaces ({}), so the node's instance cannot be named",
                claims.join(", ")
            ),
            SchemaRefusal::NotClaimed { node } => write!(
                formatter,
                "no claim is registered for the fragment at {node}, so it is in no row of the \
                 digest join and no algebra can be derived for it; a composed fragment is exactly \
                 this, because composition registers neither body it joins"
            ),
            SchemaRefusal::RoleNotMarked { node, role } => write!(
                formatter,
                "the instance at {node} claims a `{role}` role and its body carries no mark for it"
            ),
            SchemaRefusal::ProjectionNotBridged { node, role, cause } => write!(
                formatter,
                "the instance at {node}'s `{role}` projection did not bridge: {cause}"
            ),
            SchemaRefusal::NotCanonical { node, role, budget } => write!(
                formatter,
                "the instance at {node}'s `{role}` projection has no canonical form within \
                 {budget} nodes"
            ),
            SchemaRefusal::BeyondANode { node, roles } => write!(
                formatter,
                "the instance at {node} claims the role(s) {} beyond `combine` and `identity`, \
                 whose code reads the value another node produced, and this reader names no producer",
                roles.join(", ")
            ),
            SchemaRefusal::ReadsAnotherNode { node, value } => write!(
                formatter,
                "the instance at {node} reads value {value}, which another node of the graph \
                 produced, so the value it reads as its own datum is not its own"
            ),
            SchemaRefusal::NotAReductionChain { node } => write!(
                formatter,
                "graph node {node}'s edges inside the region do not make one \
                 producer-to-consumer chain"
            ),
            SchemaRefusal::NoElementRole { producer, consumer } => write!(
                formatter,
                "the instance at {consumer} reads the one at {producer} and declares no \
                 `element` role, so the code between them is not a projection of either"
            ),
            SchemaRefusal::ElementWithoutProducer {
                producer,
                consumer,
                element,
            } => write!(
                formatter,
                "the instance at {consumer} reads the one at {producer} and its element \
                 `{element}` does not read the producer's value"
            ),
            SchemaRefusal::EdgeOutOfOrder { consumer, value } => write!(
                formatter,
                "the instance at {consumer} reads value {value}, which no earlier node of the \
                 region produced"
            ),
            SchemaRefusal::UnknownInterface { node, interface } => write!(
                formatter,
                "the instance at {node} claims {interface}, which this reader has no algebra for"
            ),
            SchemaRefusal::MissingRole { node, role } => write!(
                formatter,
                "the instance at {node} has no `{role}` projection"
            ),
            SchemaRefusal::NotACombine { node, projection } => write!(
                formatter,
                "the instance at {node} declares the combine `{projection}`, which is not one \
                 binary operator"
            ),
            SchemaRefusal::NotAnIdentity { node, projection } => write!(
                formatter,
                "the instance at {node} declares the identity `{projection}`, which is not a \
                 literal"
            ),
            SchemaRefusal::NotAnElementFunction { node, projection } => write!(
                formatter,
                "the instance at {node} declares the elementwise function `{projection}`, which \
                 is not a function of any datum: it reads none, or it reads a loop's carried value"
            ),
        }
    }
}

/// The elementwise interface's name: a body that computes one function of its own datum per index.
pub const ELEMENTWISE: &str = "Elementwise";
/// The reduction interface's name.
pub const REDUCTION: &str = "Reduction";
/// The reduction's combine role.
const COMBINE: &str = "combine";
/// The reduction's identity role.
const IDENTITY: &str = "identity";
/// The reduction's element role: the code between two instances.
const ELEMENT: &str = "element";

/// Derive one schema per fusible region of `graph`, in evaluation order.
///
/// # Invariant
/// A node whose fragment matches no instance carries no claim and is a boundary. Every other
/// node belongs to exactly one region, and a region either yields a schema or one refusal
/// naming its cause: a claimed role is never dropped to make a region assemble.
pub fn derive_regions(
    graph: &Graph,
    instances: &[Instance],
) -> Vec<Result<RegionSchema, SchemaRefusal>> {
    let registry = Registry::new(instances);
    let starts = offsets(graph);
    region_positions(graph, instances)
        .iter()
        .map(|positions| region_schema(graph, &starts, &registry, positions))
        .collect()
}

/// Which graph positions form each fusible region, in evaluation order.
///
/// # Invariant
/// The grouping [`derive_regions`] derives schemas for, and it is exposed because a rewrite has to
/// name the graph nodes a region holds: a schema names its instances by kernel id, and a recorded
/// node holds no kernel id.
pub fn region_positions(graph: &Graph, instances: &[Instance]) -> Vec<Vec<usize>> {
    let registry = Registry::new(instances);
    let starts = offsets(graph);
    let mut regions: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for (position, _) in graph.nodes().iter().enumerate() {
        if registry
            .matched(fragment_at(graph, position))
            .is_unclaimed()
        {
            if !current.is_empty() {
                regions.push(std::mem::take(&mut current));
            }
            continue;
        }
        let joined = current
            .iter()
            .any(|earlier| feeds(graph, &starts, *earlier, position));
        if !current.is_empty() && !joined {
            regions.push(std::mem::take(&mut current));
        }
        current.push(position);
    }
    if !current.is_empty() {
        regions.push(current);
    }
    regions
}

/// The schema one region's nodes derive, or the refusal that names why they do not.
fn region_schema(
    graph: &Graph,
    starts: &[usize],
    registry: &Registry<'_>,
    positions: &[usize],
) -> Result<RegionSchema, SchemaRefusal> {
    let mut matched: Vec<&Instance> = Vec::with_capacity(positions.len());
    for &position in positions {
        match registry.matched(fragment_at(graph, position)) {
            Match::One(instance) => matched.push(instance),
            Match::Ambiguous => {
                return Err(SchemaRefusal::AmbiguousClaim {
                    node: position,
                    claims: registry.claims(fragment_at(graph, position)),
                });
            }
            Match::NotClaimed => {
                // A region holds only nodes `derive_regions` matched, so this is unreachable.
                return Err(SchemaRefusal::NotAReductionChain { node: position });
            }
        }
    }
    let edges = region_edges(graph, starts, positions)?;
    let mut nodes = Vec::with_capacity(positions.len());
    for (local, instance) in matched.iter().enumerate() {
        let reads = edges
            .iter()
            .find(|edge| edge.consumer == local)
            .map_or(ReadRoles::DATUM_ONLY, |edge| {
                ReadRoles::consumer(edge.input)
            });
        let mut projections = Vec::with_capacity(instance.claim.roles.len());
        for role in sorted_roles(&instance.claim) {
            let marked = marked_value(&instance.fragment, &instance.claim.interface, &role)
                .map_err(|_| SchemaRefusal::RoleNotMarked {
                    node: local,
                    role: role.clone(),
                })?;
            let expression = projection(&instance.fragment, marked, reads).map_err(|cause| {
                SchemaRefusal::ProjectionNotBridged {
                    node: local,
                    role: role.clone(),
                    cause,
                }
            })?;
            let canonical = normalize(&expression).ok_or_else(|| SchemaRefusal::NotCanonical {
                node: local,
                role: role.clone(),
                budget: lichen_fusion::NORMALIZATION_BUDGET,
            })?;
            projections.push(RoleProjection {
                role,
                expression,
                canonical,
            });
        }
        nodes.push(SchemaNode {
            position: local,
            kernel: instance.kernel,
            interface: instance.claim.interface.clone(),
            projections,
        });
    }
    let mut between = Vec::with_capacity(edges.len());
    for edge in &edges {
        let Some(element) = nodes[edge.consumer].projection(ELEMENT) else {
            return Err(SchemaRefusal::NoElementRole {
                producer: edge.producer,
                consumer: edge.consumer,
            });
        };
        if !element.variables().contains(&Variable::PRODUCER) {
            return Err(SchemaRefusal::ElementWithoutProducer {
                producer: edge.producer,
                consumer: edge.consumer,
                element: element.clone(),
            });
        }
        let canonical = nodes[edge.consumer]
            .canonical(ELEMENT)
            .expect("a role's two forms are read together");
        between.push(Between {
            producer: edge.producer,
            consumer: edge.consumer,
            input: edge.input,
            element: element.clone(),
            canonical: canonical.clone(),
        });
    }
    let ports = region_ports(graph, starts, positions);
    let mut region = RegionSchema {
        nodes,
        between,
        ports,
        hash: SchemaHash([0; 32]),
    };
    region.hash = hash_of(&region);
    Ok(region)
}

/// One edge inside a region, in region-local positions.
#[derive(Debug, Clone, Copy)]
struct Edge {
    producer: usize,
    consumer: usize,
    input: usize,
}

/// The producer-to-consumer edges among a region's nodes.
///
/// # Invariant
/// One producer and one consumer per node: a node fed by two instances of its own region, or
/// feeding two, is not the producer→code→consumer shape §6 matches and is refused by name.
/// A region's first node is that shape with no incoming edge, so nothing here reads its own
/// element — only a consumer's element must name the producer, which is [`region_schema`]'s.
fn region_edges(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
) -> Result<Vec<Edge>, SchemaRefusal> {
    let nodes = graph.nodes();
    let mut edges: Vec<Edge> = Vec::new();
    let mut consumers: Vec<usize> = vec![0; positions.len()];
    for (consumer, &position) in positions.iter().enumerate() {
        let Node::Kernel(node) = &nodes[position];
        for (input, value) in node.inputs.iter().copied().enumerate() {
            let Some(producer) = region_producer(graph, starts, positions, value) else {
                continue;
            };
            if producer >= consumer {
                return Err(SchemaRefusal::EdgeOutOfOrder { consumer, value });
            }
            consumers[consumer] += 1;
            edges.push(Edge {
                producer,
                consumer,
                input,
            });
        }
    }
    if let Some(edge) = edges.iter().find(|edge| {
        edges
            .iter()
            .filter(|other| other.producer == edge.producer)
            .count()
            > 1
    }) {
        return Err(SchemaRefusal::NotAReductionChain {
            node: positions[edge.producer],
        });
    }
    if let Some(consumer) = consumers.iter().position(|count| *count > 1) {
        return Err(SchemaRefusal::NotAReductionChain {
            node: positions[consumer],
        });
    }
    Ok(edges)
}

/// The region-local position of the node that produced `value`, if one of them did.
fn region_producer(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
    value: GraphValueId,
) -> Option<usize> {
    positions
        .iter()
        .position(|position| produces(graph, starts, *position, value))
}

/// Whether the node at `position` produces `value`.
fn produces(graph: &Graph, starts: &[usize], position: usize, value: GraphValueId) -> bool {
    let Node::Kernel(node) = &graph.nodes()[position];
    value >= starts[position] && value < starts[position] + node.fragment.outputs
}

/// The first value each node produces, the graph's own inputs coming first.
pub(crate) fn offsets(graph: &Graph) -> Vec<usize> {
    let mut next = graph.input_count();
    let mut starts = Vec::with_capacity(graph.len());
    for node in graph.nodes() {
        starts.push(next);
        next += node.outputs();
    }
    starts
}

/// The fragment a graph node dispatched.
fn fragment_at(graph: &Graph, position: usize) -> &KernelFragment {
    let Node::Kernel(node) = &graph.nodes()[position];
    &node.fragment
}

/// How a graph node's fragment matched the instances a program dispatched.
enum Match<'a> {
    /// No instance's fragment is this node's: the node carries no claim.
    NotClaimed,
    /// Exactly one instance's fragment is this node's.
    One(&'a Instance),
    /// Two instances share this fragment and claim different interfaces.
    Ambiguous,
}

impl Match<'_> {
    fn is_unclaimed(&self) -> bool {
        matches!(self, Match::NotClaimed)
    }
}

/// The instances a graph's nodes are matched against, keyed by the fragment digest.
///
/// # Invariant
/// The join is exact fragment equality, because a recorded node holds a fragment clone and
/// nothing else; the digest is only the bucket, and every candidate is compared in full.
struct Registry<'a> {
    by_digest: HashMap<u64, Vec<&'a Instance>>,
}

impl<'a> Registry<'a> {
    fn new(instances: &'a [Instance]) -> Registry<'a> {
        let mut by_digest: HashMap<u64, Vec<&'a Instance>> = HashMap::new();
        for instance in instances {
            by_digest
                .entry(fragment_digest(&instance.fragment))
                .or_default()
                .push(instance);
        }
        Registry { by_digest }
    }

    fn matched(&self, fragment: &KernelFragment) -> Match<'a> {
        let Some(candidates) = self.by_digest.get(&fragment_digest(fragment)) else {
            return Match::NotClaimed;
        };
        let mut matched = candidates
            .iter()
            .copied()
            .filter(|instance| instance.fragment == *fragment);
        let Some(first) = matched.next() else {
            return Match::NotClaimed;
        };
        if matched.all(|other| other.claim == first.claim) {
            Match::One(first)
        } else {
            Match::Ambiguous
        }
    }

    /// The interfaces that matched an ambiguous fragment.
    fn claims(&self, fragment: &KernelFragment) -> Vec<String> {
        let mut claims: Vec<String> = self
            .by_digest
            .get(&fragment_digest(fragment))
            .into_iter()
            .flatten()
            .filter(|instance| instance.fragment == *fragment)
            .map(|instance| instance.claim.interface.clone())
            .collect();
        claims.sort();
        claims.dedup();
        claims
    }
}

/// Whether the node at `producer` produces a value the node at `consumer` reads.
fn feeds(graph: &Graph, starts: &[usize], producer: usize, consumer: usize) -> bool {
    let Node::Kernel(node) = &graph.nodes()[consumer];
    let reads = node
        .inputs
        .iter()
        .copied()
        .chain(extent_of(graph, consumer));
    reads
        .into_iter()
        .any(|value| produces(graph, starts, producer, value))
}

/// The value a node's extent reads, when it reads one.
fn extent_of(graph: &Graph, position: usize) -> Option<GraphValueId> {
    let Node::Kernel(node) = &graph.nodes()[position];
    match node.count {
        Count::Value(value) => Some(value),
        Count::Device { value, .. } => Some(value),
        Count::Constant(_) => None,
    }
}

/// The values crossing a region's boundary.
///
/// # Invariant
/// An input no region node produced is a boundary input, an output no region node reads is a
/// boundary output, and an extent the region does not compute is a boundary extent.
fn region_ports(graph: &Graph, starts: &[usize], positions: &[usize]) -> Vec<Port> {
    let nodes = graph.nodes();
    let mut read: BTreeSet<GraphValueId> = BTreeSet::new();
    for &position in positions {
        let Node::Kernel(node) = &nodes[position];
        read.extend(node.inputs.iter().copied());
        read.extend(extent_of(graph, position));
    }
    let mut ports = Vec::new();
    for (local, &position) in positions.iter().enumerate() {
        let Node::Kernel(node) = &nodes[position];
        for (ordinal, value) in node.inputs.iter().copied().enumerate() {
            if region_producer(graph, starts, positions, value).is_none() {
                ports.push(Port {
                    kind: PortKind::Input,
                    node: local,
                    position: ordinal,
                    class: node.fragment.input_classes.get(ordinal).copied(),
                    value: Some(value),
                    count: None,
                });
            }
        }
        match node.count {
            Count::Constant(count) => ports.push(Port {
                kind: PortKind::Extent,
                node: local,
                position: 0,
                class: None,
                value: None,
                count: Some(count),
            }),
            Count::Value(value) | Count::Device { value, .. } => {
                if region_producer(graph, starts, positions, value).is_none() {
                    ports.push(Port {
                        kind: PortKind::Extent,
                        node: local,
                        position: 0,
                        class: None,
                        value: Some(value),
                        count: None,
                    });
                }
            }
        }
        for ordinal in 0..node.fragment.outputs {
            let value = starts[position] + ordinal;
            if !read.contains(&value) {
                ports.push(Port {
                    kind: PortKind::Output,
                    node: local,
                    position: ordinal,
                    class: node.fragment.output_classes.get(ordinal).copied(),
                    value: Some(value),
                    count: None,
                });
            }
        }
    }
    ports
}

/// A claim's roles, in name order.
///
/// # Invariant
/// Name order, not the payload's: a claim spells a set of roles, so two spellings of one
/// interface must not be two schemas.
fn sorted_roles(claim: &KernelClaim) -> Vec<String> {
    let mut roles = claim.roles.clone();
    roles.sort();
    roles
}

/// The content hash of a region's canonical form.
fn hash_of(region: &RegionSchema) -> SchemaHash {
    SchemaHash(sha256(region.canonical_form().as_bytes()))
}

/// One expression in the prefix form the hash is taken over.
///
/// # Invariant
/// Fully parenthesized and prefix, so the text is a function of the tree alone: no precedence
/// or rendering rule can make two different canonical forms print the same.
fn write_canonical(expression: &Expr, text: &mut String) {
    match expression {
        Expr::Constant(value) => {
            let _ = write!(text, "{value:?}");
        }
        Expr::Variable(variable) => text.push_str(variable.name()),
        Expr::Unary(operator, operand) => {
            text.push('(');
            text.push_str(operator.name());
            text.push(' ');
            write_canonical(operand, text);
            text.push(')');
        }
        Expr::Binary(operator, left, right) => {
            text.push('(');
            text.push_str(operator.name());
            text.push(' ');
            write_canonical(left, text);
            text.push(' ');
            write_canonical(right, text);
            text.push(')');
        }
    }
}
