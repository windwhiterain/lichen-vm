//! The automatic tiling pass: the plan's first consumer.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! A declared reduction licenses a tiled fold, so the program's own algebra decides the schedule,
//! and an installed pack matching a node's schema **steers** the placement over the engine's own
//! choice. A node the pass cannot read stays per-lane rather than tiled on a guess.
//!
//! A node reading another node's output is unreadable: [`NODE_LOCAL_ROLES`] is all its marks say.

use std::collections::HashMap;

use lichen_fusion::{Reduction, normalize};
use lichen_fusion_bridge::{Element, Folding, ReadRoles, projection};
use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::{
    KernelClaim, KernelFragment, KernelId, Plan, Slice, Space, Staging, fragment_digest,
};

use crate::fused::{
    FusedKernel, FusedRefusal, LandingRefusal, land_fused_kernel, write_fused_kernel,
};
use crate::fusion_claim::{
    DeclaredFold, REDUCTION, RoleProjection, SchemaHash, SchemaNode, SchemaRefusal, marked_value,
};
use crate::graph_pass::{GraphPass, LaunchExtent};
use crate::kernel_registry::claimed_kernels;
use crate::placement_pack::{ConstraintRefusal, PlacementPack};

/// The roles a node whose algebra this pass can derive for itself declares.
///
/// # Invariant
/// The algebra is `combine` and `identity` and nothing else: a claim that declares a further role
/// describes code between two nodes — the `element` role — which this pass has no graph to name a
/// producer with, so the node is refused rather than tiled on the algebra of its own datum reads.
const NODE_LOCAL_ROLES: [&str; 2] = ["combine", "identity"];

/// The row a fragment digest joins to: the program's own instance, or why it has no algebra.
///
/// # Invariant
/// The refusal is **stored**, not recomputed, so one node's marks are read once and a graph run
/// reports what the derivation found rather than re-deriving it every time.
type Row = Result<Instance, SchemaRefusal>;

/// The pass that tiles the nodes `kernels` names, under `packs`.
///
/// # Invariant
/// The one spelling of the construction, and it takes only what a pass reads: a caller that has
/// the registered kernels — from its own module, or from the registry — installs exactly this
/// pass, so a caller that wraps it wraps what the installer runs.
pub fn tiling_pass(
    kernels: Vec<(KernelId, KernelClaim, KernelFragment)>,
    packs: &[PlacementPack],
) -> Box<dyn GraphPass> {
    let mut instances: HashMap<u64, Row> = HashMap::new();
    for (id, claim, fragment) in kernels {
        insert(&mut instances, id, claim, fragment);
    }
    Box::new(AutomaticTiling {
        instances,
        packs: packs.to_vec(),
    })
}

/// One kernel as the derivation reads it: the claim, and the algebra its own marks declare.
#[derive(Clone)]
struct Instance {
    /// The interface the kernel claims.
    claim: KernelClaim,
    /// The node's own schema, which is the key a placement pack matches.
    schema: SchemaNode,
    /// The algebra the fragment's `combine` and `identity` marks declare.
    reduction: Reduction,
}

/// What the installed packs have to say about one node.
#[derive(Clone, Copy)]
enum Decision<'a> {
    /// This pack places the node and its constraints hold: it fires.
    Fires(&'a PlacementPack, &'a Staging),
    /// This pack's hash is the node's and a constraint did not hold: it is inert.
    Inert(&'a PlacementPack, ConstraintRefusal),
    /// No installed pack's hash is the node's.
    Unmatched,
}

/// One registered kernel, before its algebra is derived.
struct Kernel {
    /// The kernel's registry id, which the derivation's refusal names a position against.
    id: KernelId,
    /// The interface the kernel was compiled for.
    claim: KernelClaim,
    /// The fragment the dispatcher compiled.
    fragment: KernelFragment,
}

/// The pass: a digest-keyed join from a node's fragment to the algebra it declares, with the
/// packs a runner installed.
struct AutomaticTiling {
    /// One row per kernel the caller registered, whether or not its algebra is readable.
    instances: HashMap<u64, Row>,
    /// The installed packs, in the order they are consulted.
    packs: Vec<PlacementPack>,
}

impl GraphPass for AutomaticTiling {
    /// Tile every node whose own marks declare a reduction the launch can serve.
    ///
    /// # Invariant
    /// Every node is looked at, so every node has one line: a row is tiled or refused, and a
    /// fragment in no row is refused by name and left **alone**.
    ///
    /// The graph handed over is the **recording** every time, because a rewrite is stored under
    /// its own digest, so a run of the same program is tiled and reported again.
    ///
    /// An **empty** join is re-read from the registry on each run.
    ///
    /// # Safety
    /// A row is not enough to tile: a node whose input is a value another node produced reads that
    /// output as a datum, so the edge refuses it and its body is left alone.
    fn apply(
        &self,
        graph: &Graph,
        extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let mut rewritten = graph.clone();
        let deferred;
        let instances = if self.instances.is_empty() {
            deferred = registry_instances();
            &deferred
        } else {
            &self.instances
        };
        // The rows are read out first, so the graph is free to be rewritten.
        let inputs = graph.input_count();
        let mut tiled: Vec<(usize, Instance, DeclaredFold)> = Vec::new();
        for (position, node) in graph.nodes().iter().enumerate() {
            let Node::Kernel(kernel) = node;
            let digest = fragment_digest(&kernel.fragment);
            let Some(row) = instances.get(&digest) else {
                lines.push(unclaimed(position));
                continue;
            };
            match row {
                Ok(instance) => match beyond_a_node(kernel.inputs.iter().copied(), inputs) {
                    Some(value) => lines.push(format!(
                        "node {position}: refused: {}",
                        SchemaRefusal::ReadsAnotherNode {
                            node: position,
                            value,
                        }
                        .message()
                    )),
                    // The row is the node's own algebra; folding it is what the claim must license.
                    None => match instance.schema.the_fold() {
                        Ok(fold) => tiled.push((position, instance.clone(), fold)),
                        Err(refusal) => lines.push(named(position, &refusal)),
                    },
                },
                Err(refusal) => {
                    lines.push(format!("node {position}: refused: {}", refusal.message()));
                }
            }
        }
        for (position, instance, fold) in tiled {
            lines.push(tile(
                &mut rewritten,
                position,
                &instance,
                &fold,
                extent(position),
                &self.packs,
            ));
        }
        Some(rewritten)
    }
}

/// The first input of a node that no graph input carries, when it reads one.
///
/// # Invariant
/// A graph numbers the values it takes as arguments from zero and everything its nodes produce
/// after them, so an input at or above the count is another node's output — the value the
/// node-local derivation reads as a datum.
fn beyond_a_node(inputs: impl Iterator<Item = usize>, graph_inputs: usize) -> Option<usize> {
    inputs.into_iter().find(|value| *value >= graph_inputs)
}

/// One node's line, for a fragment no row of the join holds.
///
/// # Invariant
/// A refusal and not a derivation: there is no claim to read roles from and no algebra to
/// normalize, so the pass reports the join's own miss and tiles nothing, and a composed fragment
/// is left exactly as the merge left it. The sentence alone is what tells this pass's node lines
/// from the merge pass's `region N …` ones; [`named`] is the one line that also prints a name.
fn unclaimed(position: usize) -> String {
    format!(
        "node {position}: refused: {}",
        SchemaRefusal::NotClaimed { node: position }.message()
    )
}

/// One node's line, for a refusal the pass states **by the check's own name**.
///
/// # Invariant
/// The name as well as the sentence, for the reason [`landing_reason`] gives: the report is the
/// only place a caller reads which check refused a node, and this one tells an operator author
/// what their claim has to describe, which the projection alone does not.
fn named(position: usize, refusal: &SchemaRefusal) -> String {
    format!(
        "node {position}: refused: {}: {}",
        refusal.name(),
        refusal.message()
    )
}

/// What the installed packs have to say about one node.
///
/// # Invariant
/// A hash that matches and a constraint that does not is **inert** rather than a refusal: the
/// engine's own choice answers, which is the floor under every pack. Packs are consulted in
/// order, so two packs for one schema are decided by the runner's installation order.
fn decision<'a>(
    packs: &'a [PlacementPack],
    hash: &SchemaHash,
    fragment: &KernelFragment,
) -> Decision<'a> {
    let mut inert = None;
    for pack in packs.iter().filter(|pack| pack.places(hash)) {
        match pack.holds_for(fragment) {
            Ok(()) => return Decision::Fires(pack, staging_of(pack)),
            Err(refusal) => {
                if inert.is_none() {
                    inert = Some((pack, refusal));
                }
            }
        }
    }
    match inert {
        Some((pack, refusal)) => Decision::Inert(pack, refusal),
        None => Decision::Unmatched,
    }
}

/// The one value a firing pack stages, which is where its space, width and name come from.
///
/// # Invariant
/// [`PlacementPack::places`] already refused a pack that stages nothing, so this entry exists
/// for every pack a caller can be handed as a [`Decision::Fires`].
fn staging_of(pack: &PlacementPack) -> &Staging {
    pack.plan()
        .staging
        .first()
        .expect("a pack that places a node stages a value for it")
}

/// Tile `position`, or say by name why its launch cannot be served.
///
/// # Invariant
/// Nothing lands when a bound is missing or the staging would not be a whole group: a launch the
/// dispatcher would refuse is declined here rather than left to die with the tile in the graph.
///
/// The **datum** is `fold`'s and not the raw buffer, because it is what the claim said it does to
/// the datum before joining it, and the tiled body applies it per element.
fn tile(
    graph: &mut Graph,
    position: usize,
    instance: &Instance,
    fold: &DeclaredFold,
    extent: Option<usize>,
    packs: &[PlacementPack],
) -> String {
    let Node::Kernel(node) = &graph.nodes()[position];
    let lane_width = node.fragment.target.lane_width.lanes();
    let hash = instance.schema.hash();
    let decision = decision(packs, &hash, &node.fragment);
    // A pack's width is what the tiling is built at, so a width the target lacks is `check_plan`'s.
    let group_width = match decision {
        Decision::Fires(_, staging) => staging.group_width,
        _ => lane_width,
    };
    let Some(count) = extent else {
        return declined(
            position,
            &hash,
            decision,
            "its launch extent is not readable here, and a tiled fold is built for a whole \
             number of tiles",
        );
    };
    if count % group_width as usize != 0 {
        return declined(
            position,
            &hash,
            decision,
            &format!(
                "its launch covers {count} element(s) and a group tiles {group_width}, so the \
                 staging would not be a whole workgroup and the dispatcher refuses it"
            ),
        );
    }
    let Some(class) = node.fragment.input_classes.first().copied() else {
        return declined(
            position,
            &hash,
            decision,
            "a fold over no input buffer has nothing to fold",
        );
    };
    let folding = match Folding::new(
        &instance.reduction,
        Element::Function {
            position: 0,
            function: Box::new(fold.datum.clone()),
        },
        class,
    ) {
        Ok(folding) => folding,
        Err(refusal) => return declined(position, &hash, decision, &refusal.to_string()),
    };
    let tiled = match folding.tiled(group_width, 0) {
        Ok(tiled) => tiled,
        Err(refusal) => return declined(position, &hash, decision, &refusal.to_string()),
    };
    let mut fragment = node.fragment.clone();
    fragment.body = tiled.body().clone();
    let (space, plan, marks) = match decision {
        Decision::Fires(pack, staging) => (
            staging.space,
            pack.plan().clone(),
            // The plan's own staging name is the mark the body publishes, so `check_plan` finds it.
            tiled.marks(&staging.name),
        ),
        // The floor publishes no mark: a name is the logic's and a plan is a pack's.
        _ => (Space::Shared, Plan::default(), Vec::new()),
    };
    fragment.slices = vec![Slice {
        class,
        // A slice's count is the body's; a plan's group width is the plan's
        // (`docs/notes/shared-memory-and-barriers.md` §5).
        count: tiled.staged_count(),
        space,
    }];
    fragment.marks = marks;
    let artifact = write_fused_kernel(&FusedKernel {
        fragment,
        claim: Some(instance.claim.clone()),
        plan,
    });
    if let Err(refusal) = land_fused_kernel(graph, position, &artifact) {
        return declined(position, &hash, decision, &landing_reason(&refusal));
    }
    match decision {
        Decision::Fires(pack, staging) => format!(
            "node {position}: pack \"{}\" fired for schema {hash}: its plan stages `{}` in \
             {:?} over {} lane(s)",
            pack.name(),
            staging.name,
            staging.space,
            group_width,
        ),
        Decision::Inert(pack, refusal) => format!(
            "node {position}: pack \"{}\" was consulted and its constraints did not hold ({}), \
             so the automatic floor tiled: its own marks declare `{}` over {lane_width} lane(s)",
            pack.name(),
            refusal.message(),
            instance.reduction.combine.name(),
        ),
        Decision::Unmatched => format!(
            "node {position}: no pack matched its schema {hash}, so the automatic floor tiled: \
             its own marks declare `{}` over {lane_width} lane(s)",
            instance.reduction.combine.name(),
        ),
    }
}

/// Why a tiled body did not land, with the plan check's own name where one refused it.
///
/// # Invariant
/// A plan refusal is named as well as described, because the report is the only place a runner
/// reads which check refused a pack: a sentence alone could not be matched to the variant.
fn landing_reason(refusal: &LandingRefusal) -> String {
    match refusal {
        LandingRefusal::Read(FusedRefusal::PlanRefused { refusal }) => {
            format!("{}: {}", refusal.name(), refusal.message())
        }
        other => format!("the tiled body did not land: {}", other.message()),
    }
}

/// One node's line, for a tiling the pass declined to build or to land.
///
/// # Invariant
/// Every refusal names the pack that was consulted, so a runner reading a report knows whether a
/// pack steered the node or the engine's own choice did.
fn declined(position: usize, hash: &SchemaHash, decision: Decision<'_>, reason: &str) -> String {
    match decision {
        Decision::Fires(pack, _) => format!(
            "node {position}: pack \"{}\" was refused: {reason}",
            pack.name()
        ),
        Decision::Inert(pack, refusal) => format!(
            "node {position}: pack \"{}\" was consulted and its constraints did not hold ({}), \
             so the automatic floor refused: {reason}",
            pack.name(),
            refusal.message(),
        ),
        Decision::Unmatched => format!(
            "node {position}: no pack matched its schema {hash}, so the automatic floor \
             refused: {reason}"
        ),
    }
}

/// The join every registered kernel answers, for a module installed before its own evaluation.
///
/// # Invariant
/// The registry is append-only, so this is the same set the module would have named once its
/// kernels were evaluated; it is keyed the same way, so the two joins are one join.
fn registry_instances() -> HashMap<u64, Row> {
    let mut instances: HashMap<u64, Row> = HashMap::new();
    for (kernel, claim, fragment) in claimed_kernels() {
        insert(&mut instances, kernel, claim, fragment);
    }
    instances
}

/// Put one kernel's row in the join, refused or not.
///
/// # Invariant
/// The refusal is a row like any other: a fragment whose own marks do not declare the algebra
/// its claim asks for is reported by name, never silently left per-lane. The digest is the
/// bucket and the row is the join, so a fragment two claims share is **one** row, and the
/// registry's order — id order — decides which of them it is.
fn insert(
    instances: &mut HashMap<u64, Row>,
    id: KernelId,
    claim: KernelClaim,
    fragment: KernelFragment,
) {
    let digest = fragment_digest(&fragment);
    instances.insert(
        digest,
        derive(&Kernel {
            id,
            claim,
            fragment,
        }),
    );
}

/// The algebra a fragment's own marks declare, or why the node-local reading is not its algebra.
///
/// # Invariant
/// The projections are the fragment's own, as §6's engine reads them: a role the body does not
/// mark is refused by name, and a marked subexpression the bridge cannot read carries its cause.
///
/// The refusal's `node` is the kernel **id**; the run report prepends the graph node. A role
/// outside [`NODE_LOCAL_ROLES`] is beyond a node, and another interface is not read at all.
fn derive(kernel: &Kernel) -> Row {
    if kernel.claim.interface != REDUCTION {
        return Err(SchemaRefusal::UnknownInterface {
            node: kernel.id,
            interface: kernel.claim.interface.clone(),
        });
    }
    let roles = roles(&kernel.claim);
    let mut projections = Vec::with_capacity(roles.len());
    for role in roles.iter().cloned() {
        let marked = match marked_value(&kernel.fragment, &kernel.claim.interface, &role) {
            Ok(marked) => marked,
            Err(_) => {
                return Err(SchemaRefusal::RoleNotMarked {
                    node: kernel.id,
                    role,
                });
            }
        };
        let expression = match projection(&kernel.fragment, marked, ReadRoles::DATUM_ONLY) {
            Ok(expression) => expression,
            Err(cause) => {
                return Err(SchemaRefusal::ProjectionNotBridged {
                    node: kernel.id,
                    role,
                    cause,
                });
            }
        };
        let canonical = match normalize(&expression) {
            Some(canonical) => canonical,
            None => {
                return Err(SchemaRefusal::NotCanonical {
                    node: kernel.id,
                    role,
                    budget: lichen_fusion::NORMALIZATION_BUDGET,
                });
            }
        };
        projections.push(RoleProjection {
            role,
            expression,
            canonical,
        });
    }
    // A consumer's read is another node's output, which no node-local derivation can name.
    let beyond: Vec<String> = roles
        .iter()
        .filter(|role| !NODE_LOCAL_ROLES.contains(&role.as_str()))
        .cloned()
        .collect();
    if !beyond.is_empty() {
        return Err(SchemaRefusal::BeyondANode {
            node: kernel.id,
            roles: beyond,
        });
    }
    let schema = SchemaNode {
        position: kernel.id,
        kernel: kernel.id,
        interface: kernel.claim.interface.clone(),
        projections,
    };
    let reduction = schema.reduction()?;
    Ok(Instance {
        claim: kernel.claim.clone(),
        schema,
        reduction,
    })
}

/// A claim's roles, in name order.
///
/// # Invariant
/// Name order, not the payload's: a claim spells a set of roles, so two spellings of one
/// interface must be two schemas rather than two orders of one.
fn roles(claim: &KernelClaim) -> Vec<String> {
    let mut roles = claim.roles.clone();
    roles.sort();
    roles
}
