//! A fused kernel's artifact, landed into a recorded graph node and run there.
//! See `docs/notes/graph-fusion.md` §5.

use super::common;
use super::warp_exchange::sums_a_staged_tile;

use lichen_compute_gpu::{GpuContext, SUBGROUP_WIDTH};
use lichen_graph_ir::{Count, Graph, GraphRefusal, KernelNode, Node, Policy, Runner, Value};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelFragment, KernelRoles, KernelShape, LaneHeight, LaneWidth, Plan,
    ScalarClass, ScalarData, Slice, Space, TargetFacts, fragment_digest,
};
use lichen_runtime::{
    ARTIFACT_FORMAT_VERSION, DeviceFacts, DeviceFactsSource, DeviceRefusal, FusedKernel,
    LandingRefusal, LoadRefusal, land_fused_kernel, load_fused_kernel, write_fused_kernel,
};

/// A one-node graph over `fragment`, whose return is that node's output.
///
/// # Invariant
/// One kernel node and a recorded return, so the landing has edges, a value count and a
/// return to leave alone, and the run reads back the node the landing changed.
fn graph_over(fragment: KernelFragment, count: usize) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment,
                inputs: vec![0],
                count: Count::Constant(count),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    graph
}

/// The fragment the graph's one node holds.
fn node_fragment(graph: &Graph) -> &KernelFragment {
    match &graph.nodes()[0] {
        Node::Kernel(kernel) => &kernel.fragment,
    }
}

/// The tiled body's fragment for a sum over `1..=tile`, built as `graph_on_device` builds it.
///
/// # Invariant
/// The body is the transform's own `Folding::tiled`, so the artifact a caller lands is
/// the acceptance case's and its answer is that case's closed form.
fn tiled_fragment(tile: u32) -> KernelFragment {
    let body = lichen_fusion_bridge::Folding::new(
        &lichen_fusion::Reduction {
            combine: lichen_fusion::BinaryOp::Add,
            identity: 0.0,
        },
        lichen_fusion_bridge::Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .expect("a sum folds")
    .tiled(tile, 0)
    .expect("a tile folds")
    .into_body();
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(tile),
            lane_height: LaneHeight::new(1),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: tile,
            space: Space::Shared,
        }],
        marks: Vec::new(),
    }
}

/// The per-lane body's fragment for the same reduction, which is what the node holds first.
///
/// # Invariant
/// `out[i]` is the fold over `[0, i + 1]`, so lane 0 answers one element while the tiled
/// body answers the whole tile: the two answers differ at a lane, which is what makes the
/// landed body observable.
fn per_lane_fragment(tile: u32) -> KernelFragment {
    let body = lichen_fusion_bridge::Folding::new(
        &lichen_fusion::Reduction {
            combine: lichen_fusion::BinaryOp::Add,
            identity: 0.0,
        },
        lichen_fusion_bridge::Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .expect("a sum folds")
    .per_lane();
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(tile),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// The artifact a caller lands: the tiled fragment as its text.
fn tiled_artifact(tile: u32) -> String {
    write_fused_kernel(&FusedKernel {
        fragment: tiled_fragment(tile),
        claim: None,
        plan: Plan::default(),
    })
}

/// Run `graph` over `data` and answer the words its recorded return names.
fn settled(
    context: &GpuContext,
    graph: &Graph,
    data: &[i64],
    count: usize,
    policy: Policy,
) -> Vec<i64> {
    let out = Runner::new(context, policy)
        .run(graph, vec![Value::host(data.to_vec())])
        .unwrap_or_else(|refusal| panic!("{policy:?} runs the graph: {refusal}"));
    let returned = graph.returns().expect("the return was recorded")[0];
    let id = match out[returned]
        .slot()
        .expect("a dispatch's output names a buffer")
    {
        BufferSlot::Resident(id) => id,
        BufferSlot::Host(_) => panic!("a dispatch's output is a buffer, not host data"),
    };
    let answer = words(
        context
            .fetch(id, count)
            .expect("the answer comes off the device"),
    );
    release_all(context, &out);
    answer
}

#[test]
fn a_landed_tiled_artifact_is_the_body_that_runs() {
    let Some(context) = common::context("a_landed_tiled_artifact_is_the_body_that_runs") else {
        return;
    };
    let tile = lichen_compute_gpu::LOCAL_SIZE_X;
    let count = tile as usize;
    let data: Vec<i64> = (1..=count as i64).collect();
    let text = tiled_artifact(tile);

    let mut graph = graph_over(per_lane_fragment(tile), count);
    let nodes = graph.len();
    let values = graph.value_count();
    let returns = graph.returns().map(<[usize]>::to_vec);
    let before = fragment_digest(node_fragment(&graph));

    let per_lane = settled(&context, &graph, &data, count, Policy::Async);

    let landed = land_fused_kernel(&mut graph, 0, &text)
        .unwrap_or_else(|refusal| panic!("the tiled artifact lands: {refusal}"));
    assert_eq!(
        landed.fragment.outputs, 1,
        "the landed kernel is the artifact's"
    );

    let tiled = settled(&context, &graph, &data, count, Policy::Async);

    // The oracle the acceptance case derives by hand: a sum over `1..=count`.
    let expected = count as i64 * (count as i64 + 1) / 2;
    assert_eq!(
        tiled,
        vec![expected; count],
        "every lane answers the whole tile's sum after the landing"
    );
    assert_ne!(
        tiled, per_lane,
        "the node answered something else before the landing, so the landed body is the \
         one that ran"
    );
    assert_eq!(
        per_lane[0], data[0],
        "the per-lane body's first lane folded its first element alone"
    );
    assert_ne!(
        fragment_digest(node_fragment(&graph)),
        before,
        "the node's fragment is the landed one"
    );
    assert_eq!(graph.len(), nodes, "landing adds no node");
    assert_eq!(graph.value_count(), values, "landing renumbers no value");
    assert_eq!(
        graph.returns().map(<[usize]>::to_vec),
        returns,
        "the recorded return is untouched"
    );
}

#[test]
fn a_landing_onto_a_node_the_graph_does_not_have_is_refused() {
    let Some(context) = common::context("a_landing_onto_a_node_the_graph_does_not_have_is_refused")
    else {
        return;
    };
    let tile = lichen_compute_gpu::LOCAL_SIZE_X;
    let count = tile as usize;
    let mut graph = graph_over(per_lane_fragment(tile), count);
    let before = fragment_digest(node_fragment(&graph));

    // The graph is run on the device first, so the refusal below is on the device path.
    let answered = settled(
        &context,
        &graph,
        &(1..=count as i64).collect::<Vec<i64>>(),
        count,
        Policy::Async,
    );
    assert_eq!(
        answered[0], 1,
        "the per-lane body answers its first element before the refusal"
    );

    let refusal = land_fused_kernel(&mut graph, 1, &tiled_artifact(tile))
        .expect_err("a one-node graph has no node 1");

    assert_eq!(
        refusal,
        LandingRefusal::Graph(GraphRefusal::NoSuchNode { node: 1, nodes: 1 }),
        "the refusal names the index and the node count it met"
    );
    assert_eq!(
        fragment_digest(node_fragment(&graph)),
        before,
        "nothing moved"
    );
}

/// The artifact a caller lands, at a format version this build does not read.
///
/// # Invariant
/// **The version is the only thing changed**, and it is changed in the text the writer
/// produced rather than in a second hand-written artifact, so a refusal cannot be about a
/// body that differs from the one the happy path lands.
const ANOTHER_FORMAT: u32 = ARTIFACT_FORMAT_VERSION + 1;

/// [`tiled_artifact`] naming [`ANOTHER_FORMAT`] instead of this build's.
///
/// # Invariant
/// A replacement, not a rewrite: what the reader parses is the same artifact with one
/// number changed, which is what makes the refusal attributable to that number alone.
fn at_another_format(text: &str) -> String {
    text.replace(
        &format!("\"version\": {ARTIFACT_FORMAT_VERSION}"),
        &format!("\"version\": {ANOTHER_FORMAT}"),
    )
}

/// The loader's one call, against the three lines it replaces.
///
/// # Invariant
/// **One graph shape, one data set, one device**, and each path starts from the same
/// unlanded node, so the two answers are one measurement rather than two and a difference
/// could only be the loader. The submission counter is read either side, so "the device
/// ran it" is a number rather than a claim.
#[test]
fn an_artifact_loaded_in_one_call_answers_what_the_direct_path_answers() {
    let Some(context) =
        common::context("an_artifact_loaded_in_one_call_answers_what_the_direct_path_answers")
    else {
        return;
    };
    let tile = lichen_compute_gpu::LOCAL_SIZE_X;
    let count = tile as usize;
    let data: Vec<i64> = (1..=count as i64).collect();
    let text = tiled_artifact(tile);
    let before = context.submissions();

    let mut loaded = graph_over(per_lane_fragment(tile), count);
    load_fused_kernel(&text, &mut loaded, 0, &context)
        .unwrap_or_else(|refusal| panic!("the tiled artifact loads: {refusal}"));
    let one_call = settled(&context, &loaded, &data, count, Policy::Async);

    let mut direct = graph_over(per_lane_fragment(tile), count);
    land_fused_kernel(&mut direct, 0, &text)
        .unwrap_or_else(|refusal| panic!("the tiled artifact lands: {refusal}"));
    let three_lines = settled(&context, &direct, &data, count, Policy::Async);

    // The oracle the acceptance case derives by hand: a sum over `1..=count`.
    let expected = count as i64 * (count as i64 + 1) / 2;
    assert_eq!(
        one_call,
        vec![expected; count],
        "the loaded body is the tile's own sum, not the node's"
    );
    assert_eq!(
        one_call, three_lines,
        "the one call answers what read-then-land answers, lane for lane"
    );
    assert!(
        context.submissions() > before,
        "the device dispatched both graphs: {} submission(s) where {} stood",
        context.submissions(),
        before
    );
}

/// An artifact in another format is refused by name, with nothing dispatched.
///
/// # Invariant
/// The graph is run **first**, so "nothing was recorded or submitted" is a claim about a
/// device that had been dispatching rather than about a counter that had never moved, and
/// the node's digest is read either side because a loader that parsed a text it cannot
/// honour and then failed would leave a graph nobody asked it to change.
#[test]
fn an_artifact_naming_another_format_is_refused_before_the_device_is_asked() {
    let Some(context) =
        common::context("an_artifact_naming_another_format_is_refused_before_the_device_is_asked")
    else {
        return;
    };
    let tile = lichen_compute_gpu::LOCAL_SIZE_X;
    let count = tile as usize;
    let mut graph = graph_over(per_lane_fragment(tile), count);
    settled(
        &context,
        &graph,
        &(1..=count as i64).collect::<Vec<i64>>(),
        count,
        Policy::Async,
    );
    let before = (
        context.submissions(),
        context.fence_waits(),
        fragment_digest(node_fragment(&graph)),
    );

    let refusal = load_fused_kernel(
        &at_another_format(&tiled_artifact(tile)),
        &mut graph,
        0,
        &context,
    )
    .expect_err("an artifact this build does not read is not read at all");

    assert_eq!(refusal.name(), "VersionMismatch", "the gate is named");
    assert_eq!(
        refusal,
        LoadRefusal::VersionMismatch {
            found: ANOTHER_FORMAT,
            wanted: ARTIFACT_FORMAT_VERSION,
        },
        "and it carries both versions rather than only saying it did not match"
    );
    assert_eq!(
        (
            context.submissions(),
            context.fence_waits(),
            fragment_digest(node_fragment(&graph)),
        ),
        before,
        "nothing was recorded, nothing was submitted, and the node is untouched"
    );
}

/// A device whose subgroups are narrower than the artifact's `Warp` slice.
///
/// # Invariant
/// **Hand-made, and said so**: this machine's own device answers 32, so the case is stated
/// rather than waited for — a table of numbers is exactly what a `DeviceFactsSource` is,
/// and no stub backend is involved, so the refusal is the loader's and not a stub's.
struct NarrowSubgroups;

impl DeviceFactsSource for NarrowSubgroups {
    fn device_facts(&self) -> DeviceFacts {
        DeviceFacts {
            int_bits: 64,
            max_workgroup_lanes: 1024,
            max_workgroup_lanes_y: 1024,
            subgroup_width: Some(16),
            max_shared_bytes: 48 * 1024,
            max_storage_buffers: 4,
        }
    }
}

/// A `Warp` slice one subgroup too wide is refused by name, before the emitter sees it.
///
/// # Invariant
/// The same refusal [`super::warp_exchange`] pins at the emitter, one gate earlier: the
/// emitter refuses when a slice access is reached, the loader refuses before any dispatch
/// happens at all, and both name the count and the width so a report says which expr
/// disagreed.
#[test]
fn a_warp_slice_wider_than_this_devices_subgroup_is_refused_by_name() {
    let warp = sums_a_staged_tile(Space::Warp, SUBGROUP_WIDTH);
    let mut graph = graph_over(warp.clone(), lichen_compute_gpu::LOCAL_SIZE_X as usize);
    let text = write_fused_kernel(&FusedKernel {
        fragment: warp,
        claim: None,
        plan: Plan::default(),
    });

    let refusal = load_fused_kernel(&text, &mut graph, 0, &NarrowSubgroups)
        .expect_err("a 32-element warp slice cannot be honoured by a 16-lane subgroup");

    assert_eq!(refusal.name(), "Device", "the gate is named");
    assert_eq!(
        refusal,
        LoadRefusal::Device(DeviceRefusal::SubgroupWidthMismatch {
            count: SUBGROUP_WIDTH,
            subgroup_width: 16,
        }),
        "the refusal carries the slice's count and the device's subgroup width"
    );
}

/// A fetched payload as the `i64` elements these integer fragments produce.
///
/// # Invariant
/// The class is checked: a fetch classed as floats from an integer fragment would be the
/// ABI reading the buffer at the wrong width.
fn words(data: ScalarData) -> Vec<i64> {
    match data {
        ScalarData::Int(elements) => elements,
        ScalarData::Float(elements) => {
            panic!(
                "an integer run's result came back as {} float element(s)",
                elements.len()
            )
        }
    }
}

/// Hand back every buffer the run produced.
fn release_all(context: &GpuContext, values: &[Value<'_>]) {
    for value in values {
        if let Ok(BufferSlot::Resident(id)) = value.slot() {
            context.release(id);
        }
    }
}
