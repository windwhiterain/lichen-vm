//! The class pass: the classes a pack states land on the nodes it names.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! **The pass rewrites classes and nothing else.** A node's body, its algebra and a region's
//! legality are the merge pass's own: a class pack states a precision and never answers a
//! check, so a region a class pack names is merged or refused as it would be without the pack.

use std::collections::HashMap;

use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::{KernelClaim, Plan, ScalarClass, fragment_digest};

use crate::automatic_tiling::schema_of;
use crate::fused::{FusedKernel, land_fused_kernel, landing_reason, write_fused_kernel};
use crate::fusion_claim::{Instance, SchemaHash};
use crate::graph_pass::{GraphPass, LaunchExtent};
use crate::kernel_registry::{claimed_kernels, intern_kernel};
use crate::placement_pack::PlacementPack;

/// The pass that lands the classes the packs state, under `packs`.
///
/// # Invariant
/// The one spelling of the construction, and it takes only what a pass reads: the claimed
/// kernels the caller names and the packs the runner installed, so a caller that wraps it
/// wraps what the installer runs.
pub fn class_pass(instances: Vec<Instance>, packs: &[PlacementPack]) -> Box<dyn GraphPass> {
    Box::new(AutomaticClasses {
        rows: rows_of(instances),
        packs: packs.to_vec(),
    })
}

/// One row per claimed kernel, keyed by the fragment's digest.
fn rows_of(instances: Vec<Instance>) -> HashMap<u64, Row> {
    let mut rows: HashMap<u64, Row> = HashMap::new();
    for instance in instances {
        rows.insert(
            fragment_digest(&instance.fragment),
            Row {
                claim: instance.claim.clone(),
                schema: schema_of(instance.kernel, &instance.claim, &instance.fragment)
                    .ok()
                    .map(|schema| schema.hash()),
            },
        );
    }
    rows
}

/// The rows every registered kernel answers, for a module installed before its own evaluation.
///
/// # Invariant
/// The registry is append-only, so this is the same set the module would have named once its
/// kernels were evaluated; it is keyed the same way, so the two joins are one join.
fn registry_rows() -> HashMap<u64, Row> {
    rows_of(
        claimed_kernels()
            .into_iter()
            .map(|(kernel, claim, fragment)| Instance {
                kernel,
                claim,
                fragment,
            })
            .collect(),
    )
}

/// One claimed kernel, as the class pass reads it: the claim it carries and the schema it derives.
///
/// # Invariant
/// **The claim is the kernel's own and the schema is the one that claim derives**: a pack that
/// names a schema matches the node whose schema is that one, and a claim the derivation refuses
/// states no schema at all — so a pack that names one does not reach the node.
struct Row {
    /// The interface the kernel claims.
    claim: KernelClaim,
    /// The schema the claim derives, or `None` when it derives none.
    schema: Option<SchemaHash>,
}

/// The pass: the classes the packs state, landed on the nodes they name.
struct AutomaticClasses {
    /// One row per claimed kernel, keyed by the fragment's digest.
    rows: HashMap<u64, Row>,
    /// The installed packs, in the order they are consulted.
    packs: Vec<PlacementPack>,
}

impl GraphPass for AutomaticClasses {
    /// Land the classes the packs state, and report the node each pack reached.
    ///
    /// # Invariant
    /// **The graph handed over is the recording every time**, because a rewrite is stored under
    /// its own digest, so a run of the same program is classed and reported again.
    ///
    /// A pack with no class reaches no node, and a node no pack names keeps the fragment the
    /// program wrote.
    fn apply(
        &self,
        graph: &Graph,
        _extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let deferred;
        let rows = if self.rows.is_empty() {
            deferred = registry_rows();
            &deferred
        } else {
            &self.rows
        };
        let mut rewritten = graph.clone();
        let mut landed = false;
        for (position, node) in graph.nodes().iter().enumerate() {
            let Node::Kernel(kernel) = node;
            let row = rows.get(&fragment_digest(&kernel.fragment));
            let Some(pack) = self.pack_for(position, row) else {
                continue;
            };
            let (line, changed) = classed(
                &mut rewritten,
                position,
                row.map(|row| &row.claim),
                pack.class(),
                pack.classes(),
            );
            lines.push(line);
            landed = landed || changed;
        }
        landed.then_some(rewritten)
    }
}

impl AutomaticClasses {
    /// The pack that states a class for the node at `position`.
    ///
    /// # Invariant
    /// **The first pack that names the node and states a class fires**, and a pack's own node
    /// list is empty for every node: a pack that names no node states its class for the whole
    /// graph. A pack that names a schema matches the node whose schema is that one, and a node
    /// no claim registers states no schema — so such a pack does not reach it.
    fn pack_for(&self, position: usize, row: Option<&Row>) -> Option<&PlacementPack> {
        self.packs.iter().find(|pack| {
            let node_match = pack.nodes().is_empty() || pack.nodes().contains(&position);
            let schema_match = match pack.schema() {
                None => true,
                Some(hash) => row.and_then(|row| row.schema.as_ref()) == Some(hash),
            };
            node_match && schema_match && (pack.class().is_some() || !pack.classes().is_empty())
        })
    }
}

/// Land a node at the classes its pack states: a whole-node class, or named input slots.
///
/// # Invariant
/// The body is the program's and only the precision is the pack's; a node with no claim carries
/// none, which is what lets a class pack reach a composed graph the claim join refuses.
///
/// **The landed kernel is registered under its claim**, so the registry's join finds the node
/// the class landed on, and the merge composes the branches it holds with the classes they
/// were given.
fn classed(
    graph: &mut Graph,
    position: usize,
    claim: Option<&KernelClaim>,
    single: Option<ScalarClass>,
    classes: &[(usize, ScalarClass)],
) -> (String, bool) {
    let Node::Kernel(node) = &graph.nodes()[position];
    let mut fragment = node.fragment.clone();
    if let Some(class) = single {
        fragment.states_the_class(class);
    }
    for (slot, class) in classes {
        fragment.states_input_class(*slot, *class);
    }
    let artifact = write_fused_kernel(&FusedKernel {
        fragment,
        claim: claim.cloned(),
        plan: Plan::default(),
    });
    match land_fused_kernel(graph, position, &artifact) {
        Ok(kernel) => {
            intern_kernel(kernel.fragment.clone(), kernel.claim.clone());
            let stated: Vec<String> = match single {
                Some(class) => vec![format!("{class:?}")],
                None => classes
                    .iter()
                    .map(|(slot, class)| format!("slot {slot}: {class:?}"))
                    .collect(),
            };
            (
                format!("node {position}: the pack states {}", stated.join(", ")),
                true,
            )
        }
        Err(refusal) => (
            format!("node {position}: refused: {}", landing_reason(&refusal)),
            false,
        ),
    }
}
