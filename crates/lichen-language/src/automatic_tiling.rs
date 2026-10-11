//! The automatic tiling pass: the plan's first consumer.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! A declared reduction is what licenses a tiled fold, so the program that wrote the algebra
//! decides the schedule: no count and no operator is an input here. A node the pass cannot read
//! stays per-lane rather than being tiled on a guess.

use std::collections::HashMap;

use lichen_compute::compute::{
    FusedKernel, GraphPass, LaunchExtent, claimed_kernels, land_fused_kernel, set_graph_pass,
    write_fused_kernel,
};
use lichen_fusion::{Reduction, normalize};
use lichen_fusion_bridge::{Element, Folding, ReadRoles, projection};
use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::{
    KernelClaim, KernelFragment, KernelId, Plan, Slice, Space, fragment_digest,
};
use lichen_lowlevel::Module;

use crate::fusion_claim::{
    RoleProjection, SchemaNode, SchemaRefusal, claim_of, fragment_of, kernel_id, marked_value,
};
use crate::program::LangProgram;

/// The staged value's name for the tiling pass's own marks.
///
/// # Invariant
/// The plan that would name it is a pack's artifact, so this is a mark the fragment publishes
/// and nothing reads: the pass's plan is empty and an empty plan names no staged value.
const TILE: &str = "tile";

/// The row a fragment digest joins to: the program's own instance, or why it has no algebra.
///
/// # Invariant
/// The refusal is **stored**, not recomputed, so one node's marks are read once and a graph run
/// reports what the derivation found rather than re-deriving it every time.
type Row = Result<Instance, SchemaRefusal>;

/// Install the automatic tiling pass for `module`.
///
/// # Invariant
/// The join is built from the module's own kernel identities: a graph node holds a fragment
/// clone and no kernel id, so the digest is the only key the two share.
///
/// A kernel is registered while the module **evaluates**, so a module installed before its own
/// run resolves an empty join; the pass then asks the registry at the first run.
pub fn install_tiling(module: &Module<LangProgram>) {
    set_graph_pass(Box::new(AutomaticTiling {
        instances: instances(module),
    }));
}

/// One kernel as the derivation reads it: the claim, and the algebra its own marks declare.
#[derive(Clone)]
struct Instance {
    /// The interface the kernel claims.
    claim: KernelClaim,
    /// The algebra the fragment's `combine` and `identity` marks declare.
    reduction: Reduction,
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

/// The pass: a digest-keyed join from a graph node's fragment to the algebra it declares.
struct AutomaticTiling {
    /// One row per kernel the module registered, whether or not its algebra is readable.
    instances: HashMap<u64, Row>,
}

impl GraphPass for AutomaticTiling {
    /// Tile every node whose own marks declare a reduction the launch can serve.
    ///
    /// # Invariant
    /// A node's fragment is looked up by digest, and a fragment with no row is left **alone**:
    /// that is what another program's kernel looks like.
    ///
    /// The graph handed over is the **recording** every time, because a rewrite is stored under
    /// its own digest, so a run of the same program is tiled and reported again.
    ///
    /// An **empty** join is re-read from the registry on each run.
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
        let mut tiled: Vec<(usize, Instance)> = Vec::new();
        for (position, node) in graph.nodes().iter().enumerate() {
            let Node::Kernel(kernel) = node;
            let digest = fragment_digest(&kernel.fragment);
            let Some(row) = instances.get(&digest) else {
                continue;
            };
            match row {
                Ok(instance) => tiled.push((position, instance.clone())),
                Err(refusal) => {
                    lines.push(format!("node {position}: refused: {}", refusal.message()));
                }
            }
        }
        for (position, instance) in tiled {
            lines.extend(tile(&mut rewritten, position, &instance, extent(position)));
        }
        Some(rewritten)
    }
}

/// Tile `position`, or say by name why its launch cannot be served.
///
/// # Invariant
/// Nothing lands when a bound is missing or the staging would not be a whole workgroup: a tiled
/// fold reads a **whole number of tiles**, so a launch the dispatcher would refuse is declined
/// here rather than left to die with the tile already in the graph.
fn tile(
    graph: &mut Graph,
    position: usize,
    instance: &Instance,
    extent: Option<usize>,
) -> Vec<String> {
    let Node::Kernel(node) = &graph.nodes()[position];
    let lane_width = node.fragment.target.lane_width.lanes();
    let Some(count) = extent else {
        return vec![refused(
            position,
            "its launch extent is not readable here, and a tiled fold is built for a whole \
             number of tiles",
        )];
    };
    if count % lane_width as usize != 0 {
        return vec![refused(
            position,
            &format!(
                "its launch covers {count} element(s) and a workgroup tiles {lane_width}, so the \
                 staging would not be a whole workgroup and the dispatcher refuses it"
            ),
        )];
    }
    let Some(class) = node.fragment.input_classes.first().copied() else {
        return vec![refused(
            position,
            "a fold over no input buffer has nothing to fold",
        )];
    };
    let folding = match Folding::new(&instance.reduction, Element::Buffer { position: 0 }, class) {
        Ok(folding) => folding,
        Err(refusal) => return vec![refused(position, &refusal.to_string())],
    };
    let tiled = match folding.tiled(lane_width, 0) {
        Ok(tiled) => tiled,
        Err(refusal) => return vec![refused(position, &refusal.to_string())],
    };
    let mut fragment = node.fragment.clone();
    fragment.body = tiled.body().clone();
    fragment.slices = vec![Slice {
        class,
        count: tiled.staged_count(),
        space: Space::Shared,
    }];
    fragment.marks = tiled.marks(TILE);
    let artifact = write_fused_kernel(&FusedKernel {
        fragment,
        claim: Some(instance.claim.clone()),
        plan: Plan::default(),
    });
    if let Err(refusal) = land_fused_kernel(graph, position, &artifact) {
        return vec![refused(
            position,
            &format!("the tiled body did not land: {}", refusal.message()),
        )];
    }
    vec![format!(
        "node {position}: tiled: its own marks declare `{}` over {lane_width} lane(s)",
        instance.reduction.combine.name()
    )]
}

/// One node's refusal, in the run report's voice.
fn refused(position: usize, reason: &str) -> String {
    format!("node {position}: refused: {reason}")
}

/// The join a module's own kernel identities answer, keyed by fragment digest.
///
/// # Invariant
/// The id is read from a node whose own value names a kernel, never re-derived from the source:
/// a kernel *value* is a `Copy` id, so the fragment the registry holds beside it is the only
/// compiled body there is.
fn instances(module: &Module<LangProgram>) -> HashMap<u64, Row> {
    let mut instances: HashMap<u64, Row> = HashMap::new();
    for (id, _node) in module.nodes.iter() {
        let Ok(kernel) = kernel_id(module, id) else {
            continue;
        };
        let (Some(claim), Ok(fragment)) = (claim_of(kernel), fragment_of(kernel)) else {
            continue;
        };
        insert(&mut instances, kernel, claim, fragment);
    }
    instances
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

/// The algebra a fragment's own marks declare, or the refusal that names the half missing.
///
/// # Invariant
/// The projections are the fragment's own, read the way §6's engine reads them: a role the body
/// does not mark is refused by name, and a marked subexpression the bridge cannot read carries
/// the bridge's own cause.
///
/// The refusal's own `node` is the kernel **id**, because that is what a schema names an
/// instance by; the run report prepends the graph node the row was reached from.
fn derive(kernel: &Kernel) -> Row {
    let mut projections = Vec::with_capacity(kernel.claim.roles.len());
    for role in roles(&kernel.claim) {
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
    let reduction = SchemaNode {
        position: kernel.id,
        kernel: kernel.id,
        interface: kernel.claim.interface.clone(),
        projections,
    }
    .reduction()?;
    Ok(Instance {
        claim: kernel.claim.clone(),
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
