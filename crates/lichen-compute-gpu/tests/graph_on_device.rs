//! A graph with a native node in it, run on a real device.
//!
//! The stub in `lichen-graph-ir` proves the scheduling. This proves the two fit:
//! that a `ParallelBackend` is enough to run a graph, and that the numbers a
//! graph produces are the numbers the same fragment produces outside one.
//!
//! The graph is the shape the design says is worth having, and it is worth being
//! exact about why: **the native node's inputs are host data that existed before
//! the run.** It therefore never waits, and the dispatch recorded after it never
//! waits either. Putting a native node in the *middle* of a data path is the
//! other shape, and it is not this one — there the native node has to read a
//! buffer the device has not written yet, so it waits, and the whole point is
//! gone. Both are exercised below, because the difference between them is the
//! thing a graph has to get right.

use lichen_compute_gpu::GpuContext;
use lichen_graph_ir::{Count, Graph, KernelNode, NativeNode, Node, Policy, Runner, Value};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape, ResidentId,
};

/// `out[i] = in[i] + in[i] + 1`.
fn adds() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body: vec![
            KernelInstr::Const(0),
            KernelInstr::LocalGet(1),
            KernelInstr::Const(0),
            KernelInstr::LocalGet(1),
            KernelInstr::BufferReadCall,
            KernelInstr::Const(0),
            KernelInstr::LocalGet(1),
            KernelInstr::BufferReadCall,
            KernelInstr::Bin(KernelBin::Add),
            KernelInstr::Const(1),
            KernelInstr::Bin(KernelBin::Add),
            KernelInstr::BufferWriteCall,
            KernelInstr::Const(0),
        ],
        outputs: 1,
        results: 1,
        int_width: IntWidth::I64,
    }
}

/// `out[i] = a[i] + b[i] + 1` — the two-input shape a node takes when it
/// combines a dispatch's result with a host call's.
fn sums() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar,
            KernelShape::Scalar,
            KernelShape::Scalar,
        ]),
        body: vec![
            KernelInstr::Const(0),    // out_pos
            KernelInstr::LocalGet(2), // idx
            KernelInstr::Const(0),
            KernelInstr::LocalGet(2),
            KernelInstr::BufferReadCall, // a[i]
            KernelInstr::Const(1),
            KernelInstr::LocalGet(2),
            KernelInstr::BufferReadCall, // b[i]
            KernelInstr::Bin(KernelBin::Add),
            KernelInstr::Const(1),
            KernelInstr::Bin(KernelBin::Add),
            KernelInstr::BufferWriteCall,
            KernelInstr::Const(0),
        ],
        outputs: 1,
        results: 1,
        int_width: IntWidth::I64,
    }
}

/// A host call with no environment: `out[i] = in[i] * 10`.
///
/// A `fn`, so it **cannot capture** — which is the invariant the whole
/// scheduling argument rests on, and which is why the graph can promise this
/// node touches nothing the device is writing.
fn times_ten(inputs: &[&[i64]]) -> Vec<Vec<i64>> {
    vec![inputs[0].iter().map(|value| value * 10).collect()]
}

fn resident(value: &Value<'_>) -> ResidentId {
    match value.slot().expect("a dispatch's output names a buffer") {
        BufferSlot::Resident(id) => id,
        BufferSlot::Host(_) => panic!("a dispatch's output is a buffer, not host data"),
    }
}

/// Hand back every buffer the run produced, so the device memory is not left
/// resident for the context's own destructor to reclaim.
fn release_all(context: &GpuContext, values: &[Value<'_>]) {
    for value in values {
        if let Ok(BufferSlot::Resident(id)) = value.slot() {
            context.release(id);
        }
    }
}

/// Two dispatches with a host call over pre-existing data between them.
///
/// Both dispatches read **host** inputs, so neither is recording against a
/// pending value and the native node never forces a wait. The graph is: upload
/// and add, add ten times on the host, add again after uploading that.
fn side_graph(count: usize) -> Graph {
    let mut graph = Graph::with_inputs(2);
    let first = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: adds(),
                inputs: vec![0],
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("one output, one declared");
    let host = graph
        .push(
            Node::Native(NativeNode {
                call: times_ten,
                // Input 1: data that existed before the run, so this node has
                // nothing to wait for.
                inputs: vec![1],
                outputs: 1,
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .push(
            Node::Kernel(KernelNode {
                fragment: sums(),
                inputs: vec![first, host],
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
}

#[test]
fn a_graph_with_a_native_node_beside_its_dispatches_computes_the_same_numbers() {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = 100;
    let data: Vec<i64> = (0..count as i64).collect();
    let other: Vec<i64> = (0..count as i64).map(|value| value % 7).collect();

    // adds(data), then 10 * other on the host, then adds the two.
    let expected: Vec<i64> = (0..count as i64)
        .map(|value| {
            let first = 2 * value + 1;
            let host = 10 * (value % 7);
            first + host + 1
        })
        .collect();

    for policy in [Policy::Serial, Policy::Async] {
        let graph = side_graph(count);
        let out = Runner::new(&context, policy)
            .run(
                &graph,
                vec![Value::host(data.clone()), Value::host(other.clone())],
            )
            .unwrap_or_else(|refusal| panic!("{policy:?} runs the graph: {refusal}"));

        assert!(
            out.iter().all(Value::is_ready),
            "{policy:?}: every value settled"
        );
        let id = resident(out.last().expect("the last node produced one"));
        assert_eq!(
            context
                .fetch(id, count)
                .expect("the answer comes off the device"),
            expected,
            "{policy:?}: a graph produces what the same fragments produce outside one"
        );
        release_all(&context, &out);
    }
}

/// The same graph, but the native node reads a **dispatch's output**.
///
/// This is the shape the design warns about and it is worth being concrete about
/// why: the host call cannot read device memory, so the runner must wait and
/// fetch. The wait is not a bug in the scheduler — it is the cost of asking a
/// CPU for something a GPU has not finished, and no arrangement of submissions
/// removes it. What the runner guarantees is that it happens *before* the read
/// rather than being left to chance.
#[test]
fn a_native_node_reading_a_dispatches_output_waits_for_it_rather_than_guessing() {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = 100;
    let data: Vec<i64> = (0..count as i64).collect();

    let mut graph = Graph::with_inputs(1);
    let first = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: adds(),
                inputs: vec![0],
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .push(
            Node::Native(NativeNode {
                call: times_ten,
                inputs: vec![first],
                outputs: 1,
            }),
            1,
        )
        .expect("one output, one declared");
    let second = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: adds(),
                inputs: vec![2],
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("one output, one declared");
    // The function this graph stands in for returns the second dispatch, and
    // says so — which is what lets the test name the answer without counting
    // through the value table by hand.
    graph
        .returning(vec![second])
        .expect("a value the graph defines");

    let out = Runner::new(&context, Policy::Async)
        .run(&graph, vec![Value::host(data.clone())])
        .expect("the graph runs");

    let expected: Vec<i64> = (0..count as i64)
        .map(|value| 2 * (10 * (2 * value + 1)) + 1)
        .collect();
    let returned = graph.returns().expect("the return was recorded")[0];
    let id = resident(&out[returned]);
    assert_eq!(
        context
            .fetch(id, count)
            .expect("the answer comes off the device"),
        expected,
        "the native node's fetch waited, so what it read was what the dispatch wrote"
    );
    release_all(&context, &out);
}
