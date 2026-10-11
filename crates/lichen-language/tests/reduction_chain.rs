//! The declared-algebra chain, with the reduction derived and not stated.
//! See docs/notes/graph-fusion.md §3 and §5.

mod common;

use lichen_compute::compute::{
    FusedKernel, FusedRefusal, LandingRefusal, land_fused_kernel, write_fused_kernel,
};
use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X};
use lichen_fusion::{BinaryOp, Expr, Reduction};
use lichen_fusion_bridge::{Element, Folding, ReadRoles, projection};
use lichen_graph_ir::{Count, Graph, KernelNode, Node, Policy, Runner, Value};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBody, KernelFragment, KernelRoles, KernelShape, LaneWidth, Plan,
    PlanRefusal, ScalarClass, ScalarData, Slice, Space, Staging, TargetFacts, fragment_digest,
};
use lichen_language::fusion_claim::{claim_of, fragment_of, kernel_id, marked_value};
use lichen_language::program::LangProgram;
use lichen_lowlevel::Module;

/// The length one launch drives, chosen so every partial sum is an exact `f32`.
///
/// # Invariant
/// A whole number of workgroups, so a staged launch is legal, and small enough that every partial
/// any lane or the slice holds is an integer below `2^24`: the closed form `LENGTH(LENGTH + 1) / 2`
/// is `32896`, and the largest partial is that same number, so no rounding happens anywhere.
const LENGTH: u64 = 256;

/// The reduction the program's own marks declare, derived rather than stated.
///
/// # Invariant
/// No `Reduction` literal is an input here: the combine is the bridged shape of the marked
/// accumulation and the identity the bridged value of the marked seed, so what the test tiles is
/// the program's own algebra and never a copy of it (`docs/notes/graph-fusion.md` §3).
fn declared_reduction() -> (Reduction, KernelFragment) {
    let (module, _value, _root) = common::run(source());
    let (interface, fragment) = dispatched(&module);
    let combine = projection_of(&fragment, &interface, "combine");
    let identity = projection_of(&fragment, &interface, "identity");
    let combine = combine
        .as_ref()
        .unwrap_or_else(|refusal| panic!("the combine projects: {refusal}"));
    let identity = identity
        .as_ref()
        .unwrap_or_else(|refusal| panic!("the identity projects: {refusal}"));
    let operator = combine_operator(combine)
        .unwrap_or_else(|| panic!("the combine is one operator, not `{combine:?}`"));
    let identity = identity
        .as_constant()
        .unwrap_or_else(|| panic!("the identity is a literal, not `{identity:?}`"));
    (
        Reduction {
            combine: operator,
            identity,
        },
        fragment,
    )
}

/// The claim and fragment the registry holds for the kernel the program dispatched.
///
/// # Invariant
/// The id is read from the node whose own value names a kernel, and both facts are registry
/// reads: a kernel *value* carries neither, so nothing here re-derives a body.
fn dispatched(module: &Module<LangProgram>) -> (String, KernelFragment) {
    let id = module
        .nodes
        .iter()
        .find_map(|(id, _node)| kernel_id(module, id).ok())
        .expect("the program dispatched one parallel kernel");
    let claim = claim_of(id).expect("the kernel claims an interface");
    assert_eq!(
        claim.interface, "Reduction",
        "the claim the program wrote names the reduction interface"
    );
    let fragment = fragment_of(id).expect("the registry holds the dispatched fragment");
    (claim.interface, fragment)
}

/// One role's marked subexpression, bridged.
fn projection_of(fragment: &KernelFragment, interface: &str, role: &str) -> Result<Expr, String> {
    let marked = marked_value(fragment, interface, role).map_err(|refusal| refusal.message())?;
    projection(fragment, marked, ReadRoles::DATUM_ONLY).map_err(|refusal| refusal.message())
}

/// The operator a combine projection denotes, when it is exactly one two-operand operator.
fn combine_operator(expr: &Expr) -> Option<BinaryOp> {
    match expr {
        Expr::Binary(operator, _, _) => Some(*operator),
        _ => None,
    }
}

/// The program whose marks declare the algebra, over a **runtime** extent.
///
/// # Invariant
/// The fold's bound is `k.n`, a runtime value, so the definition pass leaves the nest undecided
/// and the emitter lowers it: only then is the marked seed visited and `identity` published
/// (`docs/notes/graph-fusion.md` §3). A literal bound loses both halves of the algebra.
fn source() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
In  = struct<.b (compute.Buf Float)>
Out = struct<.z (compute.Buf Float)>
Par = compute.P (compute.KT Float)(.I In, .O Out)
reduction = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1))) ~ "combine" ~ "accumulator"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, 0.0 ~ "identity")))
}
k = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu"
k
"#
}

/// The data one launch folds: `1 ..= LENGTH`, each an exact `f32`.
fn triangle() -> Vec<f32> {
    (1..=LENGTH).map(|value| value as f32).collect()
}

/// The hand-derived answer for a sum over `1 ..= LENGTH`.
///
/// # Invariant
/// `LENGTH` and `LENGTH + 1` are each exact in `f32` and the product is below `2^53`, so the closed
/// form is one exact `f32` however it is spelled.
fn closed_form() -> f32 {
    (LENGTH * (LENGTH + 1) / 2) as f32
}

/// A context on a device, or `None` when there is none here.
///
/// # Invariant
/// The skip is reported and never turned into a failure, the way every device-backed test in this
/// tree reports one.
fn device(test: &str) -> Option<GpuContext> {
    match GpuContext::new() {
        Ok(context) => Some(context),
        Err(reason) => {
            eprintln!("SKIPPED {test}: no device to run on: {reason}");
            None
        }
    }
}

/// The fragment a folded body is dispatched as.
///
/// # Invariant
/// A folded body's domain is `(extent, index)`, and the element class is the fold's own — the
/// buffer it reads and the output it writes are `Float`, while its addresses stay `Int`. `slices`
/// is exactly what a tiled fold stages: one element per lane in `Workgroup` storage.
fn body_fragment(body: KernelBody, slices: Vec<Slice>) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Float],
        output_classes: vec![ScalarClass::Float],
        result_classes: vec![ScalarClass::Float],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(LOCAL_SIZE_X),
        },
        slices,
        marks: Vec::new(),
    }
}

/// A graph whose one kernel node holds `fragment` and reads the graph's own input.
fn graph_of(fragment: KernelFragment) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment,
                inputs: vec![0],
                count: Count::Constant(LENGTH as usize),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    graph
}

/// The packed bytes of `elements`, one `f32` each.
fn pack(elements: &[f32]) -> Vec<u8> {
    elements
        .iter()
        .flat_map(|element| element.to_le_bytes())
        .collect()
}

/// Run `graph` over `data` and answer the node's first `LENGTH` elements as floats.
///
/// # Invariant
/// The class is checked: a fetch classed as integers from a float fragment would be the ABI reading
/// the buffer at the wrong width, which is the failure `ScalarData` exists to name.
fn run_graph(context: &GpuContext, graph: &Graph, data: &[f32], which: &str) -> Vec<f32> {
    let out = Runner::new(context, Policy::Serial)
        .run(
            graph,
            vec![Value::host_data(ScalarClass::Float, pack(data))],
        )
        .unwrap_or_else(|refusal| panic!("the {which} body is dispatched: {refusal}"));
    let returned = graph.returns().expect("the return was recorded")[0];
    let id = match out[returned]
        .slot()
        .expect("a dispatch's output names a buffer")
    {
        BufferSlot::Resident(id) => id,
        BufferSlot::Host(_) => panic!("a dispatch's output is a buffer, not host data"),
    };
    let answer = match context
        .fetch(id, LENGTH as usize)
        .expect("the folded answer comes off the device")
    {
        ScalarData::Float(elements) => elements,
        ScalarData::Int(elements) => panic!(
            "a float run's result came back as {} integer element(s)",
            elements.len()
        ),
    };
    for value in &out {
        if let Ok(BufferSlot::Resident(id)) = value.slot() {
            context.release(id);
        }
    }
    answer
}

/// The plan the tiled body is checked against.
///
/// # Invariant
/// The name is the body's own published mark and the width is the target's own lane count, so a
/// plan the fragment refutes is refused rather than reported as applied.
fn plan_of() -> Plan {
    Plan {
        staging: vec![Staging {
            name: "tile".to_string(),
            space: Space::Shared,
            group_width: LOCAL_SIZE_X,
        }],
    }
}

/// The text with the staged name replaced, so a plan names a mark the body never published.
///
/// # Invariant
/// The plan's own `name` is the one field rewritten and the mark's `text` is left as it was: one
/// name is published and the other is not, so the refusal is about the plan's name and nothing
/// else. The plan's field is the one followed by its space, which is what tells the two apart.
fn unpublished(artifact: &str) -> String {
    let plan_field = "\"name\": \"tile\",\n        \"space\"";
    assert_eq!(
        artifact.matches(plan_field).count(),
        1,
        "the artifact spells the plan's staged name in the field before its space, and once"
    );
    artifact.replace(
        plan_field,
        "\"name\": \"tile-unpublished\",\n        \"space\"",
    )
}

/// The fragment digest of node `node`.
fn digest_of(graph: &Graph, node: usize) -> u64 {
    let Node::Kernel(kernel) = &graph.nodes()[node];
    fragment_digest(&kernel.fragment)
}

/// The whole declared-algebra chain, on a device, with the algebra derived and not stated.
///
/// # Invariant
/// Nothing here states the algebra: the `Reduction` is asserted as an **output** of the program's
/// own marks, the tiled body is built from that derived algebra, the body travels as an artifact
/// and lands in the graph node the runner reads, and the answer after landing is the device's.
#[test]
fn the_declared_algebra_licenses_tiling_lands_in_a_graph_and_runs() {
    let (reduction, _fragment) = declared_reduction();
    assert_eq!(
        reduction,
        Reduction {
            combine: BinaryOp::Add,
            identity: 0.0,
        },
        "the program's own marks declare the sum its body folds"
    );
    let Some(context) = device("the_declared_algebra_licenses_tiling_lands_in_a_graph_and_runs")
    else {
        return;
    };

    // The transform bakes `LENGTH` into the body and the tiled body never reads its extent.
    let folding = Folding::new(
        &reduction,
        LENGTH,
        Element::Buffer { position: 0 },
        ScalarClass::Float,
    )
    .unwrap_or_else(|refusal| {
        panic!("the derived declaration tiles {LENGTH} element(s): {refusal}")
    });
    let tiled = folding.tiled(LOCAL_SIZE_X, 0).unwrap_or_else(|refusal| {
        panic!("the derived declaration tiles at {LOCAL_SIZE_X}: {refusal}")
    });
    tiled
        .body()
        .validate()
        .unwrap_or_else(|broken| panic!("the tiled body is well formed: {broken}"));
    assert_eq!(
        tiled.body().barriers.len(),
        1,
        "the tiled body records the write-to-read pair its barrier separates"
    );

    let data = triangle();
    let before = body_fragment(folding.per_lane(), Vec::new());
    before
        .body
        .validate()
        .unwrap_or_else(|broken| panic!("the per-lane body is well formed: {broken}"));
    let mut graph = graph_of(before);

    // Before landing the node holds the per-lane fold: `out[0]` is one element, not the sum.
    let per_lane = run_graph(&context, &graph, &data, "per-lane");
    assert_eq!(
        per_lane[0], 1.0,
        "the per-lane body's first lane folds one element before anything lands"
    );
    assert_ne!(
        per_lane[0],
        closed_form(),
        "the node's pre-landing answer is not the closed form, so the landing is observable"
    );

    // The artifact: the tiled body, the claim the program wrote, and the plan it was checked
    // against.
    let plan = plan_of();
    let mut fragment = body_fragment(
        tiled.body().clone(),
        vec![Slice {
            class: ScalarClass::Float,
            count: tiled.staged_count(),
            space: Space::Shared,
        }],
    );
    fragment.marks = tiled.marks("tile");
    fragment
        .check_plan(&plan)
        .unwrap_or_else(|refusal| panic!("the fragment stages what the plan says: {refusal}"));
    let artifact = write_fused_kernel(&FusedKernel {
        fragment,
        claim: None,
        plan: plan.clone(),
    });
    land_fused_kernel(&mut graph, 0, &artifact).unwrap_or_else(|refusal| {
        panic!("the tiled artifact lands in the graph's node: {refusal}")
    });

    // The landed body is the one that ran, and it answers the closed form at the Float class.
    let after = run_graph(&context, &graph, &data, "landed");
    assert_ne!(
        after[0], per_lane[0],
        "the answer changed with the node's body, so the landed body is the one that ran"
    );
    assert_eq!(
        after[0],
        closed_form(),
        "the workgroup's {LOCAL_SIZE_X} lane(s) tile {LENGTH} element(s) of a runtime-extent fold \
         to the hand-derived closed form"
    );

    // A plan naming a mark the fragment never published, refused before the graph is touched.
    let mut mutated = graph.clone();
    let refusal = land_fused_kernel(&mut mutated, 0, &unpublished(&artifact))
        .expect_err("a plan naming a mark the fragment never published is refused");
    assert!(
        matches!(
            &refusal,
            LandingRefusal::Read(FusedRefusal::PlanRefused {
                refusal: PlanRefusal::UnknownStagedValue { name },
            }) if name == "tile-unpublished"
        ),
        "the refusal is the plan check, and it carries the name it could not resolve: {refusal}"
    );
    assert!(
        refusal.message().contains("tile-unpublished"),
        "the refusal names the entry: {refusal}"
    );
    // The graph's shape and its node's body: a refused landing leaves all three alone.
    assert_eq!(
        (mutated.len(), mutated.value_count(), digest_of(&mutated, 0)),
        (graph.len(), graph.value_count(), digest_of(&graph, 0)),
        "a refused landing leaves the graph's node count, its value table and its node's fragment \
         as they were"
    );
}
