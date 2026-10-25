//! The region schema derived from a recorded graph.
//! See docs/notes/graph-fusion.md §4.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::{self, Write as _};

use lichen_fusion::{BinaryOp, Expr, FAN_IN_INPUTS, Reduction, Variable, normalize};
use lichen_fusion_bridge::{BridgeRefusal, ReadRoles, computes, folds, projection};
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
        for (byte, expr) in bytes.iter_mut().zip(digits.as_bytes().chunks(2)) {
            let expr = std::str::from_utf8(expr).ok()?;
            *byte = u8::from_str_radix(expr, 16).ok()?;
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
    /// One role each, read as §6's engine reads them. **The identity is the seed the body's own
    /// `~ "identity"` mark holds** — a [`KernelClaim`] names the role and never the value — read
    /// from the canonical form, where a spelled `((0.0 - 1.0) / 0.0)` is the `−∞` it denotes.
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

    /// The fold a node's `combine` declares, or the refusal that says it declares none.
    ///
    /// # Invariant
    /// The **canonical** form, and the combine must be one operator over the loop's carried state
    /// `t` and one datum, in either operand order. [`folds`] is the whole operator test and
    /// [`computes`] the whole datum test, so what this returns is exactly what a `Folding` can be
    /// built over, and what it refuses is a claim no tiled body would compute.
    pub fn the_fold(&self) -> Result<DeclaredFold, SchemaRefusal> {
        let combine = self
            .canonical(COMBINE)
            .ok_or_else(|| SchemaRefusal::MissingRole {
                node: self.position,
                role: COMBINE.to_string(),
            })?;
        let refused = |projection: &Expr| SchemaRefusal::CombineIsNotAFold {
            node: self.position,
            projection: projection.clone(),
        };
        let Expr::Binary(operator, left, right) = combine else {
            return Err(refused(&combine));
        };
        if !folds(*operator) {
            return Err(refused(&combine));
        }
        // **Exactly one occurrence of the state**: two states is not this loop's step.
        let datum = match (carries(left.as_ref()), carries(right.as_ref())) {
            (true, false) => right.as_ref(),
            (false, true) => left.as_ref(),
            _ => return Err(refused(&combine)),
        };
        if !computes(datum) {
            return Err(SchemaRefusal::DatumIsNotAReadableFunction {
                node: self.position,
                datum: datum.clone(),
            });
        }
        Ok(DeclaredFold {
            operator: *operator,
            datum: datum.clone(),
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

/// The fold a node's `combine` declares: the operator it joins with, and the datum it joins.
///
/// # Invariant
/// The datum is a **function** of the datum slot, not the slot itself, because that is what a
/// tiled body has to compute per element: a `combine` of `(+ t (* d d))` folds the squares, and a
/// pass that read the buffer raw would fold the values. Both pieces are what
/// [`Folding::new`] takes, so a caller that holds one needs no second reading of the claim.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredFold {
    /// The operator the loop applies to the state and the datum.
    pub operator: BinaryOp,
    /// The function the datum is computed by, in the symbolic language.
    pub datum: Expr,
}

/// Whether an expression is the loop's own carried state, and nothing else.
fn carries(expression: &Expr) -> bool {
    matches!(expression, Expr::Variable(variable) if *variable == Variable::CONSUMER_STATE)
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
    /// The region-local node whose edges are not a shape the fold builds, or `None` when they
    /// are.
    ///
    /// # Invariant
    /// **The four shapes the fold builds**: a **chain** (every node but the first reads the one
    /// before it), a **fan-in** (a node reads two producers), a **fan-out** (a producer feeds two
    /// consumers), and a **sibling** (no edge inside the region). Edges that are none of them are
    /// ones the fold composes no step for, and the node that does not read the one before it is
    /// the one this names.
    pub fn off_chain_node(&self) -> Option<usize> {
        let edges: Vec<(usize, usize)> = self
            .between
            .iter()
            .map(|edge| (edge.producer, edge.consumer))
            .collect();
        // **A sibling**: the branches stand beside one another, and no edge is inside the region.
        if edges.is_empty() {
            return None;
        }
        // **A chain**: every node but the first reads the one before it.
        if (1..self.nodes.len()).all(|local| edges.contains(&(local - 1, local))) {
            return None;
        }
        // **A fan-in**: a node reads two producers, each with a slot of its own.
        let mut arrivals = vec![0usize; self.nodes.len()];
        for &(_, consumer) in &edges {
            arrivals[consumer] += 1;
        }
        if arrivals.iter().any(|count| *count > 1) {
            return None;
        }
        // **A fan-out**: a producer feeds two consumers.
        let mut leaving = vec![0usize; self.nodes.len()];
        for &(producer, _) in &edges {
            leaving[producer] += 1;
        }
        if leaving.iter().any(|count| *count > 1) {
            return None;
        }
        // **None of the four**: the first node that does not read the one before it.
        (1..self.nodes.len()).find(|local| !edges.contains(&(local - 1, *local)))
    }

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
    /// One producer reaches one consumer at more than one input position.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::NotAReductionChain`] because it is a distinct fact and the
    /// two are decided by different evidence: that one is a node's edges, this one **two slots
    /// for one value**. The symbolic language names one producer once, so the second read would
    /// be either the same variable or a datum — and a fan-in is admitted only where every input
    /// has a name of its own.
    ProducerFeedsTwice {
        /// The region-local position of the producer whose output arrives twice.
        producer: usize,
        /// The region-local position of the consumer that reads it twice.
        consumer: usize,
    },
    /// One node reads more producers than the symbolic language names.
    InputsAreNotNamed {
        /// The region-local position of the consumer.
        node: usize,
        /// How many producers reach it.
        inputs: usize,
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
    /// A node's `combine` is one binary operator, but not over the accumulator and a datum.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::NotACombine`] because it is a distinct answer: that one says
    /// the combine is not one operator and this says it is one operator, but over something no
    /// fold can be built from — an operator that is not associative or has no instruction, or
    /// operands that are not the state and one datum.
    CombineIsNotAFold {
        /// The region-local position of the node.
        node: usize,
        /// The combine as it canonicalized.
        projection: Expr,
    },
    /// The combine's datum is not a function a kernel body computes.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::CombineIsNotAFold`] because it is a distinct answer: that one
    /// says the combine does not join the state to a datum, and this says it does but the datum it
    /// names is not one a body can apply — it reads a value the fold has no slot for, or an
    /// operator the kernel IR carries no instruction for.
    DatumIsNotAReadableFunction {
        /// The region-local position of the node.
        node: usize,
        /// The combine's datum as it canonicalized.
        datum: Expr,
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
            SchemaRefusal::ProducerFeedsTwice { .. } => "ProducerFeedsTwice",
            SchemaRefusal::InputsAreNotNamed { .. } => "InputsAreNotNamed",
            SchemaRefusal::NoElementRole { .. } => "NoElementRole",
            SchemaRefusal::ElementWithoutProducer { .. } => "ElementWithoutProducer",
            SchemaRefusal::EdgeOutOfOrder { .. } => "EdgeOutOfOrder",
            SchemaRefusal::UnknownInterface { .. } => "UnknownInterface",
            SchemaRefusal::MissingRole { .. } => "MissingRole",
            SchemaRefusal::NotACombine { .. } => "NotACombine",
            SchemaRefusal::CombineIsNotAFold { .. } => "CombineIsNotAFold",
            SchemaRefusal::DatumIsNotAReadableFunction { .. } => "DatumIsNotAReadableFunction",
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
            SchemaRefusal::ProducerFeedsTwice { producer, consumer } => write!(
                formatter,
                "the instance at {consumer} reads the one at {producer} at more than one input \
                 position, and the symbolic language names one producer once, so the second read \
                 has no slot of its own"
            ),
            SchemaRefusal::InputsAreNotNamed { node, inputs } => write!(
                formatter,
                "the instance at {node} reads {inputs} producers' outputs and this reader names \
                 one variable per input ordinal, at most {FAN_IN_INPUTS}"
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
            SchemaRefusal::CombineIsNotAFold { node, projection } => write!(
                formatter,
                "the instance at {node} declares the combine `{projection}`, which is not the \
                 fold itself: a schedule rebuilds the loop from the operator and the datum, so a \
                 claim it can honour has to name one operator the fold is performed with, over the \
                 loop's own carried state and one datum, and nothing else"
            ),
            SchemaRefusal::DatumIsNotAReadableFunction { node, datum } => write!(
                formatter,
                "the instance at {node}'s combine names the datum `{datum}`, which no kernel body \
                 computes: it reads a value the fold has no slot for, or an operator no lowered \
                 instruction carries, so a body that applied the raw buffer instead would answer a \
                 different fold"
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
/// The interfaces the symbolic bridge composes.
///
/// # Invariant
/// **A node that claims one of these is algebra the bridge reads, and one that claims another is
/// not**: the bridge reads a reduction's `combine` and `identity` and an elementwise's `element`,
/// so those are the two it composes — and `Opaque` is the name no reader derives algebra from.
pub const BRIDGED: &[&str] = &[REDUCTION, ELEMENTWISE];
/// The opaque interface's name: a merged fragment claims it, and no reader has algebra for it.
///
/// # Invariant
/// The name is one no reader can derive algebra from, so a fragment that claims it is **named and
/// refused** rather than read.
pub const OPAQUE: &str = "Opaque";
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
        .map(|positions| region_schema(graph, &starts, &registry, positions, &[]))
        .collect()
}

/// The shape of a region the fold reads.
///
/// # Invariant
/// **One shape per region, and the shape says how the fold composes it**: an ordinary region is
/// one body composed from the region's own nodes, and a sibling region is several branches
/// composed apart and merged into one body that reads the shared producer's value once.
#[derive(Debug, Clone, PartialEq)]
pub enum RegionShape {
    /// The ordinary shape: the region's nodes compose into one body.
    Ordinary,
    /// The sibling shape: the shared producer stays outside, and the branches compose apart.
    Sibling {
        /// The graph position of the shared producer, which the region does not hold.
        producer: usize,
        /// The branches, each a list of graph positions.
        branches: Vec<Vec<usize>>,
    },
}

/// One region the merge pass attempts: its nodes, its shape, and its schema.
///
/// # Invariant
/// **The positions are the region's own nodes in graph order**, and the schema is the one those
/// nodes derive, so a reader of a region never asks the graph a second time.
#[derive(Debug, Clone)]
pub struct Region {
    /// The region's nodes, in graph order.
    pub positions: Vec<usize>,
    /// The shape the fold reads.
    pub shape: RegionShape,
    /// The region's schema, or the refusal that names why it has none.
    pub schema: Result<RegionSchema, SchemaRefusal>,
}

/// The regions the ordinary rule derives, in evaluation order.
///
/// # Invariant
/// **One region per component of the claimed nodes**, as [`region_positions`] groups them, and
/// each region's schema is the one its own nodes derive.
pub fn ordinary_regions(graph: &Graph, instances: &[Instance]) -> Vec<Region> {
    let registry = Registry::new(instances);
    let starts = offsets(graph);
    region_positions(graph, instances)
        .into_iter()
        .map(|positions| {
            let schema = region_schema(graph, &starts, &registry, &positions, &[]);
            Region {
                positions,
                shape: RegionShape::Ordinary,
                schema,
            }
        })
        .collect()
}

/// The regions the sibling strategy derives, in evaluation order.
///
/// # Invariant
/// **A shared producer's consumers become one region and the producer stays outside it**: the
/// region is the union of those consumers' descendant subgraphs, and the producer keeps a
/// region of its own, so it stays materialized and its value is read once.
///
/// **The branches are grouped by the extent they dispatch over**, because one kernel dispatches
/// over one extent.
pub fn sibling_regions(graph: &Graph, instances: &[Instance]) -> Vec<Region> {
    let registry = Registry::new(instances);
    let starts = offsets(graph);
    // **The sibling rule's own component**: the run's connectivity, not the ordinary regions.
    let components = components(graph, &starts, claimed_run(graph, &registry), |_, _| true);
    let mut regions = Vec::new();
    for component in components {
        sibling_component(graph, &starts, &registry, component, &mut regions);
    }
    regions.sort_by_key(|region| region.positions.first().copied().unwrap_or(usize::MAX));
    regions
}

/// The regions one component of the claimed nodes yields under the sibling strategy.
///
/// # Invariant
/// **The shared producer is left outside**, in a region of its own, and the branches are the
/// regions beside it: the component's remaining nodes keep the ordinary rule, so nothing the
/// sibling rule does not claim is lost.
fn sibling_component(
    graph: &Graph,
    starts: &[usize],
    registry: &Registry<'_>,
    component: Vec<usize>,
    regions: &mut Vec<Region>,
) {
    let Some((producer, branches)) = shared_producer(graph, starts, &component) else {
        // **No shared producer**: the whole component keeps the ordinary rule, cut and all.
        for group in ordinary_components(graph, starts, component) {
            regions.push(ordinary_region(graph, starts, registry, group));
        }
        return;
    };
    // **The producer keeps a region of its own**: it stays materialized.
    regions.push(ordinary_region(graph, starts, registry, vec![producer]));
    // **The branches are grouped by the extent they dispatch over**, and only a group of two
    // or more is a sibling set.
    let mut claimed: Vec<usize> = Vec::new();
    for group in groups_by_extent(graph, &branches) {
        if group.len() < 2 {
            continue;
        }
        claimed.extend(group.iter().flatten().copied());
        regions.push(sibling_region(graph, starts, registry, producer, group));
    }
    // **What the sibling regions did not claim** keeps the ordinary rule, cut at the extents.
    let rest: Vec<usize> = component
        .iter()
        .copied()
        .filter(|position| *position != producer && !claimed.contains(position))
        .collect();
    for group in ordinary_components(graph, starts, rest) {
        regions.push(ordinary_region(graph, starts, registry, group));
    }
}

/// The ordinary rule's components: the run's, each cut where the extent changes.
///
/// # Invariant
/// **A cut is made at every data edge whose ends dispatch over different extents**: one merged
/// node dispatches over one extent, so what lies beyond the change is a region of its own.
fn ordinary_components(graph: &Graph, starts: &[usize], run: Vec<usize>) -> Vec<Vec<usize>> {
    components(graph, starts, run, extents_correspond)
}

/// One ordinary region: the nodes, and the schema they derive.
fn ordinary_region(
    graph: &Graph,
    starts: &[usize],
    registry: &Registry<'_>,
    positions: Vec<usize>,
) -> Region {
    let schema = region_schema(graph, starts, registry, &positions, &[]);
    Region {
        positions,
        shape: RegionShape::Ordinary,
        schema,
    }
}

/// One sibling region: the branches, and the schema they derive.
///
/// # Invariant
/// **The roots name the producer's value**: a branch's head reads the producer's output, and the
/// schema reads that arrival as the producer's own slot — so the region's code between the
/// producer and each branch says what the branch does with it.
fn sibling_region(
    graph: &Graph,
    starts: &[usize],
    registry: &Registry<'_>,
    producer: usize,
    branches: Vec<Vec<usize>>,
) -> Region {
    let mut positions: Vec<usize> = branches.iter().flatten().copied().collect();
    positions.sort_unstable();
    let mut roots: Vec<(usize, usize)> = Vec::with_capacity(branches.len());
    for branch in &branches {
        let Some(&root) = branch.first() else {
            continue;
        };
        let Some(local) = positions.iter().position(|held| *held == root) else {
            continue;
        };
        let Some(ordinal) = producer_ordinal(graph, starts, producer, root) else {
            continue;
        };
        roots.push((local, ordinal));
    }
    let schema = region_schema(graph, starts, registry, &positions, &roots);
    Region {
        positions,
        shape: RegionShape::Sibling { producer, branches },
        schema,
    }
}

/// The shared producer of a component, and the branches its consumers hold.
///
/// # Invariant
/// **The earliest node with several claimed consumers**: the branches are those consumers
/// themselves, and a consumer that read another would not be independent — the merge would read
/// one branch's value in another.
fn shared_producer(
    graph: &Graph,
    starts: &[usize],
    component: &[usize],
) -> Option<(usize, Vec<Vec<usize>>)> {
    let consumers = consumers_within(graph, starts, component);
    for (index, &position) in component.iter().enumerate() {
        let held = &consumers[index];
        if held.len() < 2 {
            continue;
        }
        let branches: Vec<Vec<usize>> = held
            .iter()
            .map(|consumer| vec![component[*consumer]])
            .collect();
        if branches_are_not_independent(graph, starts, &branches) {
            continue;
        }
        return Some((position, branches));
    }
    None
}

/// The claimed consumers of each node of a component, in region-local positions.
///
/// # Invariant
/// **A consumer is a node that reads what the node produced**, and the relation is read over the
/// component's own nodes: a node outside the component is not a consumer of one inside it.
fn consumers_within(graph: &Graph, starts: &[usize], component: &[usize]) -> Vec<Vec<usize>> {
    let mut producer_of: HashMap<GraphValueId, usize> = HashMap::with_capacity(component.len());
    for (index, &position) in component.iter().enumerate() {
        let Node::Kernel(node) = &graph.nodes()[position];
        for value in starts[position]..starts[position] + node.fragment.outputs {
            producer_of.insert(value, index);
        }
    }
    let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); component.len()];
    for (index, &position) in component.iter().enumerate() {
        let Node::Kernel(node) = &graph.nodes()[position];
        let read: Vec<GraphValueId> = node
            .inputs
            .iter()
            .copied()
            .chain(extent_of(graph, position))
            .collect();
        for value in read {
            if let Some(&producer) = producer_of.get(&value)
                && producer < index
            {
                consumers[producer].push(index);
            }
        }
    }
    consumers
}

/// Whether one branch reads a value another branch produced.
///
/// # Invariant
/// **A sibling merge composes the branches apart**, so a branch that read another would read a
/// value the merged body does not hold — the shape is not a sibling set at all.
fn branches_are_not_independent(graph: &Graph, starts: &[usize], branches: &[Vec<usize>]) -> bool {
    for (index, branch) in branches.iter().enumerate() {
        for &position in branch {
            let Node::Kernel(node) = &graph.nodes()[position];
            for value in node.inputs.iter().copied() {
                let Some(producer) = producer_position(graph, starts, value) else {
                    continue;
                };
                if branches
                    .iter()
                    .enumerate()
                    .any(|(other, other_branch)| other != index && other_branch.contains(&producer))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// The node that produced `value`, when one of the graph's nodes did.
fn producer_position(graph: &Graph, starts: &[usize], value: GraphValueId) -> Option<usize> {
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

/// The input ordinal the producer's value reaches at `consumer`.
pub(crate) fn producer_ordinal(
    graph: &Graph,
    starts: &[usize],
    producer: usize,
    consumer: usize,
) -> Option<usize> {
    let Node::Kernel(node) = &graph.nodes()[consumer];
    let Node::Kernel(source) = &graph.nodes()[producer];
    let first = starts[producer];
    let last = first + source.fragment.outputs;
    node.inputs
        .iter()
        .position(|value| *value >= first && *value < last)
}

/// The branches grouped by the extent they dispatch over, in the branches' own order.
fn groups_by_extent(graph: &Graph, branches: &[Vec<usize>]) -> Vec<Vec<Vec<usize>>> {
    let mut groups: Vec<(Count, Vec<Vec<usize>>)> = Vec::new();
    for branch in branches {
        let Some(&head) = branch.first() else {
            continue;
        };
        let Node::Kernel(node) = &graph.nodes()[head];
        let count = node.count;
        match groups.iter_mut().find(|(held, _)| *held == count) {
            Some((_, group)) => group.push(branch.clone()),
            None => groups.push((count, vec![branch.clone()])),
        }
    }
    groups.into_iter().map(|(_, group)| group).collect()
}

/// Which graph positions form each fusible region, in evaluation order.
///
/// # Invariant
/// A region is a **weakly-connected component** of the **claimed** nodes' producer/consumer
/// relation: a node joins an earlier claimed node's component, **fed by it or feeding it**, and
/// two producers one consumer reads are one component. An unclaimed node belongs to no region and
/// is not a run separator, so a claimed consumer may join a claimed producer across it.
///
/// # Invariant
/// **The relation keeps one extent**: a node joins a producer's component only when the two
/// dispatch over the same extent, so a chain is cut where the extent changes.
pub fn region_positions(graph: &Graph, instances: &[Instance]) -> Vec<Vec<usize>> {
    let registry = Registry::new(instances);
    let starts = offsets(graph);
    // **The ordinary rule cuts at the extent changes**: one merged node has one extent.
    components(
        graph,
        &starts,
        claimed_run(graph, &registry),
        extents_correspond,
    )
}

/// The claimed nodes of the graph, in evaluation order.
///
/// # Invariant
/// **A node no row of the join holds is in no region and separates no run**: the run is the
/// claimed nodes alone, so a claimed consumer may join a claimed producer across an unclaimed
/// node.
fn claimed_run(graph: &Graph, registry: &Registry<'_>) -> Vec<usize> {
    let mut run: Vec<usize> = Vec::new();
    for (position, _) in graph.nodes().iter().enumerate() {
        if registry
            .matched(fragment_at(graph, position))
            .is_unclaimed()
        {
            continue;
        }
        run.push(position);
    }
    run
}

/// The weakly-connected components of one run of claimed nodes, each in evaluation order.
///
/// # Invariant
/// **One component per expr the relation joins, in either direction**, and the run is walked
/// **backwards**, so a node joins the component of every producer it reads: a recorded graph cannot
/// express an edge before its producer, so one sweep is enough. A component is named by its
/// **earliest** node, which is what puts the components in evaluation order.
///
/// # Invariant
/// **A count edge joins whatever the two extents are** — the consumer's extent is the producer's
/// output — and a data edge joins when the rule says so: the ordinary rule asks the extents to
/// correspond, for one merged node dispatches over one extent.
fn components(
    graph: &Graph,
    starts: &[usize],
    run: Vec<usize>,
    data_edge_joins: impl Fn(Count, Count) -> bool,
) -> Vec<Vec<usize>> {
    if run.len() < 2 {
        return if run.is_empty() {
            Vec::new()
        } else {
            vec![run]
        };
    }
    let mut producer_of: HashMap<GraphValueId, usize> = HashMap::with_capacity(run.len());
    for (index, &position) in run.iter().enumerate() {
        let Node::Kernel(node) = &graph.nodes()[position];
        for value in starts[position]..starts[position] + node.fragment.outputs {
            producer_of.insert(value, index);
        }
    }
    let mut reach: Vec<usize> = (0..run.len()).collect();
    for (later, &position) in run.iter().enumerate().rev() {
        let Node::Kernel(node) = &graph.nodes()[position];
        for &value in &node.inputs {
            // **A data edge joins when the rule says so**: the ordinary rule asks the extents.
            if let Some(&earlier) = producer_of.get(&value)
                && earlier < later
                && data_edge_joins(count_at(graph, run[earlier]), node.count)
            {
                join(&mut reach, earlier, later);
            }
        }
        // **A count edge is not a data edge**: the consumer's extent is the producer's output.
        if let Some(value) = extent_of(graph, position)
            && let Some(&earlier) = producer_of.get(&value)
            && earlier < later
        {
            join(&mut reach, earlier, later);
        }
    }
    let mut grouped: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, &position) in run.iter().enumerate() {
        grouped
            .entry(root(&mut reach, index))
            .or_default()
            .push(position);
    }
    grouped.into_values().collect()
}

/// The component `node` belongs to, after the roots on the way are flattened.
fn root(reach: &mut Vec<usize>, mut node: usize) -> usize {
    while reach[node] != node {
        reach[node] = reach[reach[node]];
        node = reach[node];
    }
    node
}

/// The two components become one, and the earlier root is the one's name.
///
/// # Invariant
/// **The earlier root wins**, so a component is named by its earliest node and reading the
/// components by name is reading them in evaluation order.
fn join(reach: &mut Vec<usize>, one: usize, other: usize) {
    let (one, other) = (root(reach, one), root(reach, other));
    if one != other {
        reach[other] = one;
    }
}

/// The schema one region's nodes derive, or the refusal that names why they do not.
fn region_schema(
    graph: &Graph,
    starts: &[usize],
    registry: &Registry<'_>,
    positions: &[usize],
    roots: &[(usize, usize)],
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
    // Each node's roles are decided once, from the edges that arrive at it.
    let roles: Vec<ReadRoles> = (0..positions.len())
        .map(|consumer| {
            let root = roots
                .iter()
                .find(|(local, _)| *local == consumer)
                .map(|(_, ordinal)| *ordinal);
            roles_of(&edges, consumer, root)
        })
        .collect();
    let mut nodes = Vec::with_capacity(positions.len());
    for (local, instance) in matched.iter().enumerate() {
        let reads = roles[local];
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
        // **Every absorbed input's own slot**: a producer the element never names is refused.
        let slot = roles[edge.consumer]
            .producer_at(edge.input)
            .expect("an edge's own input is one of that consumer's named positions");
        if !element.variables().contains(&slot) {
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

/// The roles one node's projection is read under: every input a region's producer fills.
///
/// # Invariant
/// **In input-ordinal order**, so the slot a producer is read as is decided by the body's own
/// parameter order and not by the order the edges were met in.
///
/// **A root reads the producer's value at the slot the caller named**: the producer is outside
/// the region, so its arrival is not one of the region's own edges.
fn roles_of(edges: &[Edge], consumer: usize, root: Option<usize>) -> ReadRoles {
    let mut inputs: Vec<usize> = edges
        .iter()
        .filter(|edge| edge.consumer == consumer)
        .map(|edge| edge.input)
        .collect();
    inputs.sort_unstable();
    match root {
        Some(ordinal) => ReadRoles::consumer(ordinal),
        None => ReadRoles::consumers(&inputs)
            .expect("region_edges admits no consumer wider than the table"),
    }
}

/// The producer-to-consumer edges among a region's nodes.
///
/// # Invariant
/// **One edge per producer and consumer**: one producer reaching a consumer twice names
/// one value at two slots, and the symbolic language has one slot per producer, so the
/// region is refused by that name. Several **distinct** producers is a fan-in: each
/// arrival carries its own [`roles_of`] slot and its own [`Between`].
fn region_edges(
    graph: &Graph,
    starts: &[usize],
    positions: &[usize],
) -> Result<Vec<Edge>, SchemaRefusal> {
    let nodes = graph.nodes();
    let mut edges: Vec<Edge> = Vec::new();
    let mut arrivals: Vec<Vec<usize>> = vec![Vec::new(); positions.len()];
    for (consumer, &position) in positions.iter().enumerate() {
        let Node::Kernel(node) = &nodes[position];
        for (input, value) in node.inputs.iter().copied().enumerate() {
            let Some(producer) = region_producer(graph, starts, positions, value) else {
                continue;
            };
            if producer >= consumer {
                return Err(SchemaRefusal::EdgeOutOfOrder { consumer, value });
            }
            arrivals[consumer].push(producer);
            edges.push(Edge {
                producer,
                consumer,
                input,
            });
        }
    }
    for (consumer, producers) in arrivals.iter().enumerate() {
        if let Some(producer) = producers
            .iter()
            .enumerate()
            .find(|(ordinal, producer)| producers[..*ordinal].contains(producer))
            .map(|(_, producer)| *producer)
        {
            return Err(SchemaRefusal::ProducerFeedsTwice { producer, consumer });
        }
        if producers.len() > FAN_IN_INPUTS {
            return Err(SchemaRefusal::InputsAreNotNamed {
                node: consumer,
                inputs: producers.len(),
            });
        }
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

/// The value a node's extent reads, when it reads one.
fn extent_of(graph: &Graph, position: usize) -> Option<GraphValueId> {
    let Node::Kernel(node) = &graph.nodes()[position];
    match node.count {
        Count::Value(value) => Some(value),
        Count::Device { value, .. } => Some(value),
        Count::Constant(_) => None,
    }
}

/// The count a node dispatches over.
pub(crate) fn count_at(graph: &Graph, position: usize) -> Count {
    let Node::Kernel(node) = &graph.nodes()[position];
    node.count
}

/// Whether two counts are one extent, as `Count` states it.
///
/// # Invariant
/// **Two constants are one extent when they are equal, two values when they name the same
/// one, and two device counts when they name the same value and the same element**: a merged
/// node dispatches over one extent, so a rule that admitted a pair the merge cannot take
/// would only move the refusal later.
pub(crate) fn extents_correspond(producer: Count, consumer: Count) -> bool {
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
