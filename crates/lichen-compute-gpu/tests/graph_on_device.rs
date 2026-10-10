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

mod common;

use lichen_compute_gpu::{GpuContext, RunError};
use lichen_graph_ir::{Count, Graph, KernelNode, Node, Policy, Runner, Value};
use lichen_kernel_ir::{
    BufferSlot, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaunchSet, ResidentId, ScalarClass,
};

/// `out[i] = in[i] + in[i] + 1`.
fn adds() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(1),                                        // the write's index
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // cfg_pos, the *input* space
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),       // cfg_pos again
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        int_width: IntWidth::I64,
    }
}

/// `out[i] = a[i] + b[i] + 1` — the two-input shape a node takes when it
/// combines a dispatch's result with something the caller supplied.
///
/// **Two leaves, however many inputs there are**: a graph dispatch pushes the
/// launch extent and the index and nothing else, so the second input is reached
/// through a read's own position rather than through a further parameter.  A
/// fragment that declares a runtime scalar beside the extent is refused by name
/// (`RunError::ScalarsNotPushed`) rather than dispatched with the argument
/// missing — `docs/notes/compute-runtime-scalars.md` §3.
fn sums() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(1),                                        // idx
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // a[i]
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // b[i]
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 2,
        outputs: 1,
        input_classes: vec![ScalarClass::Int, ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        int_width: IntWidth::I64,
    }
}

/// The same shape with a **runtime scalar** declared beside the extent and the
/// index: the parameter a dispatch cannot carry, kept here so the refusal that
/// names it has something to refuse.
fn with_a_runtime_scalar() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            3,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // out_pos
                FlatOp::Read(2),                                        // idx
                FlatOp::Read(1),                                        // the runtime scalar
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                FlatOp::Read(2),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        int_width: IntWidth::I64,
    }
}

fn resident(value: &Value<'_>) -> ResidentId {
    match value.slot().expect("a dispatch's output names a buffer") {
        BufferSlot::Resident(id) => id,
        BufferSlot::Host(_) => panic!("a dispatch's output is a buffer, not host data"),
    }
}

/// A fetched payload as the `i64` elements these integer fragments produce.
///
/// The class is checked rather than assumed: a fetch that came back classed as
/// floats from an integer fragment would be the ABI reading the buffer at the
/// wrong width, which is a wrong number rather than a failed shape.
fn words(data: lichen_kernel_ir::ScalarData) -> Vec<i64> {
    match data {
        lichen_kernel_ir::ScalarData::Int(elements) => elements,
        lichen_kernel_ir::ScalarData::Float(elements) => {
            panic!(
                "an integer run's result came back as {} float element(s)",
                elements.len()
            )
        }
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
    let Some(context) = common::context("a_graph_of_dispatches_computes_the_same_numbers") else {
        return;
    };
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
            words(
                context
                    .fetch(id, count)
                    .expect("the answer comes off the device")
            ),
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
    let Some(context) =
        common::context("an_extent_that_is_one_of_the_graphs_own_values_runs_at_that_extent")
    else {
        return;
    };

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
            words(
                context
                    .fetch(id, count as usize)
                    .expect("the answer comes off the device")
            ),
            (0..count).map(|value| 2 * value + 1).collect::<Vec<i64>>(),
            "the dispatch covered [0, {count}) because the count was the graph's \
             second argument, not because the builder knew it"
        );
        release_all(&context, &out);
    }
}

/// A parameter that declares a runtime scalar is **refused by name** on this
/// path, not dispatched with the argument missing.
///
/// The graph node is where the limit is sharpest: a `KernelNode` carries the
/// extent and its inputs, and the push constant carries the extent alone, so a
/// third leaf has nowhere to arrive — the CPU path passes the whole leaf list
/// and this one cannot (`docs/notes/compute-runtime-scalars.md` §3).  Pinned
/// here so the refusal stays named: the alternative is every lane computing from
/// the wrong value.
#[test]
fn a_parameter_with_a_runtime_scalar_is_refused_by_name() {
    let Some(context) = common::context("a_parameter_with_a_runtime_scalar_is_refused_by_name")
    else {
        return;
    };
    let data = vec![0u8; 8 * 8];
    let refusal = context
        .run(
            &LaunchSet::single(&with_a_runtime_scalar()),
            &[BufferSlot::Host(&data)],
            8,
        )
        .expect_err("a runtime scalar has no push constant to arrive in");
    assert_eq!(refusal, RunError::ScalarsNotPushed { leaves: 3 });
    assert!(
        refusal.to_string().contains("3 leaf/leaves"),
        "the message names how many leaves it saw: {refusal}"
    );
}
