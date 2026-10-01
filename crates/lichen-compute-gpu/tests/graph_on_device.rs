//! A graph of kernel nodes, run on a real device.
//!
//! The stub in `lichen-graph-ir` proves the scheduling. This proves the two fit:
//! that a `ParallelBackend` is enough to run a graph, and that the numbers a
//! graph produces are the numbers the same fragments produce outside one.
//!
//! The graph here is a chain, and the chain is the whole claim: **the second
//! dispatch records against a value the device may not be writing.** That is only
//! sound because the recording happened first and the submission behind it was
//! recorded first too, so nothing reads the value until the run settles it. A
//! graph that read it early would be a wrong answer rather than a slow one, which
//! is why the schedule is asserted in the stub and only the arithmetic is here.

use lichen_compute_gpu::GpuContext;
use lichen_graph_ir::{Count, Graph, KernelNode, Node, Policy, Runner, Value};
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
/// combines a dispatch's result with something the caller supplied.
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

/// Two dispatches, the second over the first's output and the caller's input.
///
/// **The second node records against a resident id the device has not finished
/// writing**, which is the whole reason a graph is not a loop. Nothing reads it:
/// the run settles every submission before it hands an id back.
fn chained(count: usize) -> Graph {
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
    graph
        .push(
            Node::Kernel(KernelNode {
                fragment: sums(),
                inputs: vec![first, 1],
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
}

#[test]
fn a_graph_of_dispatches_computes_the_same_numbers() {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");
    let count = 100;
    let data: Vec<i64> = (0..count as i64).collect();
    let other: Vec<i64> = (0..count as i64).map(|value| value % 7).collect();

    // adds(data), then adds that to `other`.
    let expected: Vec<i64> = (0..count as i64)
        .map(|value| (2 * value + 1) + (value % 7) + 1)
        .collect();

    for policy in [Policy::Serial, Policy::Async] {
        let graph = chained(count);
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

/// A dispatch whose extent is one of the graph's own values, on real hardware.
///
/// **The same graph runs at a different extent**, which is the reason a count is
/// a value rather than a build-time number. A graph that could only be given a
/// count the builder knew would have to be rebuilt per run, and rebuilding a
/// graph per run is the same as not having one. The device is what makes this
/// worth checking here rather than in the stub: the count decides how many
/// elements are allocated, dispatched and read back, so a count that were read
/// from the wrong slot would show up as a wrong *length* and not merely a wrong
/// number.
#[test]
fn an_extent_that_is_one_of_the_graphs_own_values_runs_at_that_extent() {
    let context = GpuContext::new().expect("a Vulkan device with shaderInt64 is available");

    let mut graph = Graph::with_inputs(2);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: adds(),
                // Input 0 is the data; value 1 is the count. The two are told
                // apart by their role, and the graph never says which slot is
                // which — only the recording and the caller do.
                inputs: vec![0],
                count: Count::Value(1),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");

    for count in [1_i64, 37, 100] {
        // Longer than the count, so a device that dispatched over the whole
        // buffer would answer with elements past the extent rather than merely
        // the right ones in the wrong place.
        let data: Vec<i64> = (0..128).collect();
        let out = Runner::new(&context, Policy::Async)
            .run(&graph, vec![Value::host(data), Value::int(count)])
            .unwrap_or_else(|refusal| panic!("a run over {count} element(s): {refusal}"));
        let returned = graph.returns().expect("the return was recorded")[0];
        let id = resident(&out[returned]);
        assert_eq!(
            context
                .fetch(id, count as usize)
                .expect("the answer comes off the device"),
            (0..count).map(|value| 2 * value + 1).collect::<Vec<i64>>(),
            "the dispatch covered [0, {count}) because the count was the graph's \
             second argument, not because the builder knew it"
        );
        release_all(&context, &out);
    }
}
