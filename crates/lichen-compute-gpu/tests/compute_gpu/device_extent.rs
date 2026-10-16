//! A node dispatched over a count the device wrote, on real hardware.
//!
//! # Invariant
//! The count in this file is **produced on the device** and never crosses to the host on the
//! way to the dispatch that reads it: the runner resolves `Count::Device` to a buffer address
//! and an element, and the backend hands the queue an indirect command. What the host knows
//! about the answer is therefore evidence rather than input.

use super::common;

use lichen_compute_gpu::GpuContext;
use lichen_graph_ir::{Count, Graph, KernelNode, Node, Policy, Runner, Value};
use lichen_kernel_ir::{
    Barrier, Br, BufferSlot, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr,
    KernelRoles, KernelShape, LaneHeight, LaneWidth, ResidentId, ScalarClass, ScalarData, Slice,
    Space, TargetFacts, Terminator,
};

/// The lane count one workgroup runs, which is also what a fragment declares.
const LANES: u32 = 64;
/// How many elements the counting kernel reads: two per lane, one workgroup.
const COUNTED: usize = LANES as usize * 2;
/// The elements that pass `in[i] > 0`, which is what the count comes to.
const PASSING: i64 = 40;

/// One kernel writes how many of its elements pass a predicate.
///
/// # Invariant
/// **The count is on the device**: lane `i` stages `in[i] > 0` and `in[i + 64] > 0` into its own
/// slice slot, and the fold over the 64 slots is what the kernel writes to `out[0]`. The
/// predicate is a `Select` over the comparison and not the comparison itself: the fold adds what
/// a lane stages, and a `Gt` yields `0`/`1` rather than a mask.
fn counts_the_passing_elements() -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let lane = body.add_param(entry);

    let zero = body.add_const(entry, ScalarClass::Int, 0);
    let one = body.add_const(entry, ScalarClass::Int, 1);
    let width = body.add_const(entry, ScalarClass::Int, i64::from(LANES));
    // `in[i]` and `in[i + 64]`: the two steps of the tile, one workgroup wide.
    let second = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![lane, width],
        vec![ScalarClass::Int],
    );
    let first_element = body.add_op(
        entry,
        KernelInstr::BufferReadCall(ScalarClass::Int),
        vec![zero, lane],
        vec![ScalarClass::Int],
    );
    let first_kept = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Gt),
        vec![first_element, zero],
        vec![ScalarClass::Int],
    );
    let first = body.add_op(
        entry,
        KernelInstr::Select,
        vec![one, zero, first_kept],
        vec![ScalarClass::Int],
    );
    let second_element = body.add_op(
        entry,
        KernelInstr::BufferReadCall(ScalarClass::Int),
        vec![zero, second],
        vec![ScalarClass::Int],
    );
    let second_kept = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Gt),
        vec![second_element, zero],
        vec![ScalarClass::Int],
    );
    let latter = body.add_op(
        entry,
        KernelInstr::Select,
        vec![one, zero, second_kept],
        vec![ScalarClass::Int],
    );
    let pair = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![first, latter],
        vec![ScalarClass::Int],
    );
    let slot = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![lane, width],
        vec![ScalarClass::Int],
    );
    let write = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![zero, slot, pair],
        vec![ScalarClass::Int],
    );

    // The fold over the slice: counter `0 .. 64`, each slot read once.
    let header = body.add_block();
    let counter = body.add_param(header);
    let carried = body.add_param(header);
    let address = body.add_param(header);
    let step = body.add_block();
    let next = body.add_param(step);
    let current = body.add_param(step);
    let moved = body.add_param(step);
    let merged = body.add_block();
    let total = body.add_param(merged);
    let index = body.add_op(
        header,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![address, width],
        vec![ScalarClass::Int],
    );
    let read = body.add_op(
        header,
        KernelInstr::SliceReadCall(ScalarClass::Int),
        vec![zero, index, write],
        vec![ScalarClass::Int],
    );
    let reached = body.add_op(
        header,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Lt),
        vec![counter, width],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        entry,
        Terminator::Br(Br {
            target: header,
            args: vec![zero, zero, zero],
        }),
    );
    body.set_terminator(
        header,
        Terminator::CondBr {
            cond: reached,
            if_true: Br {
                target: step,
                args: vec![counter, carried, address],
            },
            if_false: Br {
                target: merged,
                args: vec![carried],
            },
        },
    );
    let combined = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![current, read],
        vec![ScalarClass::Int],
    );
    let advanced = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![next, one],
        vec![ScalarClass::Int],
    );
    let advanced_address = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![moved, one],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        step,
        Terminator::Br(Br {
            target: header,
            args: vec![advanced, combined, advanced_address],
        }),
    );
    let output = body.add_const(entry, ScalarClass::Int, 0);
    let writer = body.add_block();
    let written = body.add_param(writer);
    // Every lane reaches its write; only lane zero's is the count.
    let is_first = body.add_op(
        writer,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Eq),
        vec![lane, output],
        vec![ScalarClass::Int],
    );
    let zero = body.add_const(writer, ScalarClass::Int, 0);
    let published = body.add_op(
        writer,
        KernelInstr::Select,
        vec![written, zero, is_first],
        vec![ScalarClass::Int],
    );
    body.add_op(
        writer,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![output, lane, published],
        Vec::new(),
    );
    body.set_terminator(
        writer,
        Terminator::Return {
            values: vec![output],
        },
    );
    body.set_terminator(
        merged,
        Terminator::Br(Br {
            target: writer,
            args: vec![total],
        }),
    );
    body.barriers.push(Barrier { write, read });

    let fragment = KernelFragment {
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
            lane_width: LaneWidth::new(LANES),
            lane_height: LaneHeight::new(1),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: LANES,
            space: Space::Shared,
        }],
        marks: Vec::new(),
    };
    fragment
        .body
        .validate()
        .unwrap_or_else(|broken| panic!("the counting body is well formed: {broken}"));
    fragment
}

/// `out[i] = 2 · in[i] + 1` — the node whose extent is read on the device.
///
/// # Invariant
/// It does not read its extent leaf, and that is what makes it eligible: an indirect dispatch
/// fixes no extent, so a body that read the leaf would be running on a number the host does not
/// have. Every element it writes is `2 · in[i] + 1`, so the tail past the real extent is told
/// apart from the answer by its value rather than by its being unwritten.
fn doubles_then_one() -> KernelFragment {
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
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)), // in[i]
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 2)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Mul)),
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
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(LANES),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// The two-node graph: count, then dispatch over the count.
///
/// # Invariant
/// `device_held` is the whole difference between the two runs this file makes. When it is set the
/// second node's count names the counting node's output and element `0`, and **nothing** in the
/// graph knows what the number is: the second node's allocation is the whole counting buffer, so
/// the extent the dispatch ran over can only have come from the device.
fn counting_chain(device_held: bool) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let counted = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: counts_the_passing_elements(),
                inputs: vec![0],
                count: Count::Constant(COUNTED),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    let over = match device_held {
        true => Count::Device {
            value: counted,
            index: 0,
        },
        false => Count::Constant(COUNTED),
    };
    let answered = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: doubles_then_one(),
                inputs: vec![0],
                count: over,
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![answered])
        .expect("a value the graph defines");
    graph
}

/// `COUNTED` elements, of which the first `PASSING` are non-zero.
fn data() -> Vec<i64> {
    (0..COUNTED)
        .map(|index| {
            if (index as i64) < PASSING {
                index as i64 + 1
            } else {
                0
            }
        })
        .collect()
}

/// The graph's answer, as the `i64` elements this integer fragment produces.
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

fn resident(value: &Value<'_>) -> ResidentId {
    match value.slot().expect("a dispatch's output names a buffer") {
        BufferSlot::Resident(id) => id,
        BufferSlot::Host(_) => panic!("a dispatch's output is a buffer, not host data"),
    }
}

fn release_all(context: &GpuContext, values: &[Value<'_>]) {
    for value in values {
        if let Ok(BufferSlot::Resident(id)) = value.slot() {
            context.release(id);
        }
    }
}

/// A dispatch whose extent is a number a kernel wrote, on real hardware.
///
/// # Invariant
/// The count is `PASSING` because that is how many elements are non-zero, and it is read back
/// only as evidence — the dispatch was recorded before it came off the device. The answer is
/// `2i + 3` over the real extent and `1` over the surplus lanes, so a dispatch over the
/// counting buffer's whole length would be a second workgroup.
#[test]
fn a_node_dispatched_over_a_device_written_count_answers_at_that_count() {
    let Some(context) =
        common::context("a_node_dispatched_over_a_device_written_count_answers_at_that_count")
    else {
        return;
    };
    let graph = counting_chain(true);
    let values = Runner::new(&context, Policy::Serial)
        .run(&graph, vec![Value::host(data())])
        .unwrap_or_else(|refusal| panic!("the counting chain runs: {refusal}"));

    let counted = words(
        context
            .fetch(resident(&values[1]), 1)
            .expect("the counting kernel's own output comes off the device"),
    );
    assert_eq!(
        counted[0], PASSING,
        "the counting kernel counted the non-zero elements, so element 0 is the number the \
         second dispatch was recorded over"
    );
    // One workgroup, because `PASSING` is inside a single one: the surplus lanes write too.
    let answered = words(
        context
            .fetch(resident(&values[2]), LANES as usize)
            .expect("the answer comes off the device"),
    );
    let expected: Vec<i64> = (0..LANES as i64)
        .map(|index| {
            if index < PASSING {
                2 * (index + 1) + 1
            } else {
                1
            }
        })
        .collect();
    assert_eq!(
        answered, expected,
        "the second dispatch covered [0, {PASSING}) because the extent was the number in the \
         first node's output, and the rest is the rounding's surplus over zero input"
    );
    release_all(&context, &values);
}

/// The counterfactual: the same graph, dispatched over everything the counting buffer holds.
///
/// # Invariant
/// The only change is the second node's count, from the device's number to the buffer's own
/// length — so the two answers differ **only** if the first run really read the count. It is two
/// workgroups against one, and the numbers inside are the same either way, so the length is the
/// whole of the evidence.
#[test]
fn dispatching_the_same_node_over_the_whole_buffer_answers_differently() {
    let Some(context) =
        common::context("dispatching_the_same_node_over_the_whole_buffer_answers_differently")
    else {
        return;
    };
    let graph = counting_chain(false);
    let values = Runner::new(&context, Policy::Serial)
        .run(&graph, vec![Value::host(data())])
        .unwrap_or_else(|refusal| panic!("the whole-buffer chain runs: {refusal}"));

    let answered = words(
        context
            .fetch(resident(&values[2]), COUNTED)
            .expect("the answer comes off the device"),
    );
    assert_eq!(
        answered.len(),
        COUNTED,
        "a dispatch over the whole counting buffer is two workgroups, where the device's own \
         count of {PASSING} is one"
    );
    assert_eq!(
        answered[COUNTED - 1],
        1,
        "the surplus workgroup wrote over zero input: {answered:?}"
    );
    assert_eq!(
        answered[PASSING as usize - 1],
        2 * PASSING + 1,
        "the real extent is the same under both counts, so only the length moves: {answered:?}"
    );
    release_all(&context, &values);
}
