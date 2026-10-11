//! A graph of kernel nodes, run on a real device.
//!
//! # Invariant
//! The stub in `lichen-graph-ir` proves the scheduling; this proves the two fit, that a
//! `ParallelBackend` is enough to run a graph and that the numbers a graph produces are the numbers the
//! same fragments produce outside one. The graph is a chain, where the second dispatch records against
//! a value the device may not be writing.

mod common;

use lichen_compute_gpu::GpuContext;
use lichen_graph_ir::{Count, Graph, KernelNode, Node, Policy, Runner, Value};
use lichen_kernel_ir::{
    Barrier, Br, BufferSlot, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr,
    KernelRoles, KernelShape, LaneWidth, LaunchSet, ResidentId, ScalarClass, Slice, Space,
    TargetFacts, Terminator,
};

/// Whether the emitter's loop machinery carries a value across the backedge at all.
///
/// # Invariant
/// The body is the smallest loop with a carried accumulator: `n = 0; while n < 3 { n = n + 1 }`,
/// answered in `out[0]`. A run answering 3 says a carried block parameter survives the backedge;
/// anything else is the emitter's, not a tiling's.
#[test]
fn a_hand_built_loop_carries_its_accumulator_on_the_device() {
    let Some(context) = common::context("a_hand_built_loop_carries_its_accumulator_on_the_device")
    else {
        return;
    };
    let mut body = KernelBody::new();
    let entry = body.add_block();
    body.add_param(entry);
    body.add_param(entry);
    let stop = body.add_const(entry, ScalarClass::Int, 3);
    let zero = body.add_const(entry, ScalarClass::Int, 0);
    let one = body.add_const(entry, ScalarClass::Int, 1);
    let header = body.add_block();
    let counter = body.add_param(header);
    let value = body.add_param(header);
    let step = body.add_block();
    let next = body.add_param(step);
    let current = body.add_param(step);
    let merged = body.add_block();
    let result = body.add_param(merged);
    let reached = body.add_op(
        header,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Lt),
        vec![counter, stop],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        entry,
        Terminator::Br(Br {
            target: header,
            args: vec![zero, zero],
        }),
    );
    body.set_terminator(
        header,
        Terminator::CondBr {
            cond: reached,
            if_true: Br {
                target: step,
                args: vec![counter, value],
            },
            if_false: Br {
                target: merged,
                args: vec![value],
            },
        },
    );
    let combined = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![current, one],
        vec![ScalarClass::Int],
    );
    let advanced = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![next, one],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        step,
        Terminator::Br(Br {
            target: header,
            args: vec![advanced, combined],
        }),
    );
    let output = body.add_const(entry, ScalarClass::Int, 0);
    let writer = body.add_block();
    let written = body.add_param(writer);
    body.add_op(
        writer,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![output, zero, written],
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
            args: vec![result],
        }),
    );
    let fragment = KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 0,
        outputs: 1,
        input_classes: Vec::new(),
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    };
    let resident = context
        .run(&LaunchSet::single(&fragment), &[], 64)
        .unwrap_or_else(|refusal| panic!("a hand-built loop is dispatched: {refusal}"));
    let answer = words(context.fetch(resident[0], 1).expect("the answer comes back"));
    context.release(resident[0]);
    assert_eq!(
        answer[0], 3,
        "a carried accumulator counts three steps before its bound stops it"
    );
}

/// The folded per-lane body's two-step case, run on the device.
///
/// # Invariant
/// The body is built by the transform's own `Folding::per_lane`, so a run says whether *that*
/// construction carries: lane 0 runs once and answers `data[0]`, lane 1 runs twice and answers
/// `data[0] + data[1]`.
#[test]
fn a_two_step_folded_loop_carries_on_the_device() {
    let Some(context) = common::context("a_two_step_folded_loop_carries_on_the_device") else {
        return;
    };
    let body = lichen_fusion_bridge::Folding::new(
        &lichen_fusion::Reduction {
            combine: lichen_fusion::BinaryOp::Add,
            identity: 0.0,
        },
        2,
        lichen_fusion_bridge::Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .expect("a sum folds")
    .per_lane();
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
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    };
    let data: Vec<i64> = vec![10, 20];
    let packed: Vec<u8> = data.iter().flat_map(|word| word.to_le_bytes()).collect();
    let resident = context
        .run(
            &LaunchSet::single(&fragment),
            &[BufferSlot::Host(&packed)],
            2,
        )
        .unwrap_or_else(|refusal| panic!("a folded loop is dispatched: {refusal}"));
    let answer = words(context.fetch(resident[0], 2).expect("the answer comes back"));
    context.release(resident[0]);
    assert_eq!(
        answer,
        vec![10, 30],
        "a two-step fold answers its running total, not its first read"
    );
}

/// The folded **tiled** body's one-step case, run on the device.
///
/// # Invariant
/// The body is built by the transform's own `Folding::tiled`, so a run says whether the slice
/// path carries: `lane_width` elements, one per lane, and the workgroup folds the whole tile.
#[test]
fn a_one_step_tiled_loop_folds_the_tile_on_the_device() {
    let Some(context) = common::context("a_one_step_tiled_loop_folds_the_tile_on_the_device") else {
        return;
    };
    let tile = lichen_compute_gpu::LOCAL_SIZE_X;
    let body = lichen_fusion_bridge::Folding::new(
        &lichen_fusion::Reduction {
            combine: lichen_fusion::BinaryOp::Add,
            identity: 0.0,
        },
        u64::from(tile),
        lichen_fusion_bridge::Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .expect("a sum folds")
    .tiled(tile, 0)
    .expect("a tile folds")
    .into_body();
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
            lane_width: LaneWidth::new(tile),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: tile,
            space: Space::Shared,
        }],
        marks: Vec::new(),
    };
    let data: Vec<i64> = (1..=i64::from(tile)).collect();
    let packed: Vec<u8> = data.iter().flat_map(|word| word.to_le_bytes()).collect();
    let resident = context
        .run(
            &LaunchSet::single(&fragment),
            &[BufferSlot::Host(&packed)],
            tile as usize,
        )
        .unwrap_or_else(|refusal| panic!("a tiled loop is dispatched: {refusal}"));
    let answer = words(
        context
            .fetch(resident[0], tile as usize)
            .expect("the answer comes back"),
    );
    context.release(resident[0]);
    let expected: i64 = data.iter().sum();
    assert_eq!(
        answer[tile as usize - 1], expected,
        "the workgroup folds the whole tile to its sum"
    );
}

/// The slice fold alone: `slot[lane] = lane + 1`, barrier, then each lane sums every slot.
///
/// # Invariant
/// The smallest body that reads a slice the way the tiling does — the read index is a **modulo of
/// the loop's carried slot** — so a run separates the slice's addressing from the fold's.
#[test]
fn the_slice_fold_alone_sums_every_slot_on_the_device() {
    let Some(context) = common::context("the_slice_fold_alone_sums_every_slot_on_the_device") else {
        return;
    };
    let tile = lichen_compute_gpu::LOCAL_SIZE_X;
    let mut body = KernelBody::new();
    let entry = body.add_block();
    body.add_param(entry);
    let lane = body.add_param(entry);
    let width = body.add_const(entry, ScalarClass::Int, i64::from(tile));
    let one = body.add_const(entry, ScalarClass::Int, 1);
    let zero = body.add_const(entry, ScalarClass::Int, 0);
    let slot = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![lane, width],
        vec![ScalarClass::Int],
    );
    let written = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![lane, one],
        vec![ScalarClass::Int],
    );
    let ordinal = body.add_const(entry, ScalarClass::Int, 0);
    let write = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![ordinal, slot, written],
        vec![ScalarClass::Int],
    );
    // The read fold: counter 0 .. width, address `slot + counter` taken modulo the width.
    let header = body.add_block();
    let counter = body.add_param(header);
    let value = body.add_param(header);
    let address = body.add_param(header);
    let step = body.add_block();
    let next = body.add_param(step);
    let current = body.add_param(step);
    let carried_address = body.add_param(step);
    let merged = body.add_block();
    let result = body.add_param(merged);
    let index = body.add_op(
        header,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![address, width],
        vec![ScalarClass::Int],
    );
    let read = body.add_op(
        header,
        KernelInstr::SliceReadCall(ScalarClass::Int),
        vec![ordinal, index, write],
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
                args: vec![counter, value, address],
            },
            if_false: Br {
                target: merged,
                args: vec![value],
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
    let moved = body.add_op(
        step,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![carried_address, one],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        step,
        Terminator::Br(Br {
            target: header,
            args: vec![advanced, combined, moved],
        }),
    );
    let after = body.add_block();
    let left = body.add_param(after);
    body.set_terminator(
        merged,
        Terminator::Br(Br {
            target: after,
            args: vec![result],
        }),
    );
    let position = body.add_const(entry, ScalarClass::Int, 0);
    body.add_op(
        after,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![position, lane, left],
        Vec::new(),
    );
    body.set_terminator(
        after,
        Terminator::Return {
            values: vec![position],
        },
    );
    body.barriers.push(Barrier { write, read });
    let fragment = KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 0,
        outputs: 1,
        input_classes: Vec::new(),
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(tile),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: tile,
            space: Space::Shared,
        }],
        marks: Vec::new(),
    };
    fragment
        .body
        .validate()
        .unwrap_or_else(|broken| panic!("the slice fold is well formed: {broken}"));
    let resident = context
        .run(&LaunchSet::single(&fragment), &[], tile as usize)
        .unwrap_or_else(|refusal| panic!("the slice fold is dispatched: {refusal}"));
    let answer = words(
        context
            .fetch(resident[0], tile as usize)
            .expect("the answer comes back"),
    );
    context.release(resident[0]);
    let expected: i64 = (1..=i64::from(tile)).sum();
    assert_eq!(
        answer[tile as usize - 1], expected,
        "every lane sums every slot: {answer:?}"
    );
}

/// The folded **tiled** body's two-step case: does the per-lane fold take its second step?
///
/// # Invariant
/// `lane i` folds `data[i]` and `data[i + 64]`, so its partial is `2i + 3` and the workgroup's
/// total is their sum; a body that takes one step per lane answers the first step's sum instead.
#[test]
fn a_two_step_tiled_loop_folds_the_tile_on_the_device() {
    let Some(context) = common::context("a_two_step_tiled_loop_folds_the_tile_on_the_device") else {
        return;
    };
    let tile = lichen_compute_gpu::LOCAL_SIZE_X;
    let steps = 2;
    let body = lichen_fusion_bridge::Folding::new(
        &lichen_fusion::Reduction {
            combine: lichen_fusion::BinaryOp::Add,
            identity: 0.0,
        },
        tile as u64 * steps,
        lichen_fusion_bridge::Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .expect("a sum folds")
    .tiled(tile, 0)
    .expect("a tile folds")
    .into_body();
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
            lane_width: LaneWidth::new(tile),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: tile,
            space: Space::Shared,
        }],
        marks: Vec::new(),
    };
    let data: Vec<i64> = (0..(tile as i64 * steps as i64)).collect();
    let packed: Vec<u8> = data.iter().flat_map(|word| word.to_le_bytes()).collect();
    let resident = context
        .run(
            &LaunchSet::single(&fragment),
            &[BufferSlot::Host(&packed)],
            tile as usize,
        )
        .unwrap_or_else(|refusal| panic!("a two-step tiled loop is dispatched: {refusal}"));
    let answer = words(
        context
            .fetch(resident[0], tile as usize)
            .expect("the answer comes back"),
    );
    context.release(resident[0]);
    let expected: i64 = data.iter().sum();
    println!("two-step partials: {answer:?} expected total {expected}");
    assert_eq!(
        answer[tile as usize - 1], expected,
        "the workgroup folds both steps of the tile"
    );
}

/// Whether a **per-lane** fold takes two steps on the device.
///
/// # Invariant
/// The same `Loop` the tiled body's first fold uses, with `stop` two instead of one: `lane 0`
/// answers `data[0] + data[1]` and a body that stopped after one step answers `data[0]`.
#[test]
fn a_two_step_per_lane_fold_carries_on_the_device() {
    let Some(context) = common::context("a_two_step_per_lane_fold_carries_on_the_device") else {
        return;
    };
    let tile = lichen_compute_gpu::LOCAL_SIZE_X;
    let body = lichen_fusion_bridge::Folding::new(
        &lichen_fusion::Reduction {
            combine: lichen_fusion::BinaryOp::Add,
            identity: 0.0,
        },
        tile as u64 * 2,
        lichen_fusion_bridge::Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .expect("a sum folds")
    .per_lane();
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
            lane_width: LaneWidth::new(tile),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    };
    let data: Vec<i64> = (0..(tile as i64 * 2)).collect();
    let packed: Vec<u8> = data.iter().flat_map(|word| word.to_le_bytes()).collect();
    let resident = context
        .run(
            &LaunchSet::single(&fragment),
            &[BufferSlot::Host(&packed)],
            (tile * 2) as usize,
        )
        .unwrap_or_else(|refusal| panic!("a two-step per-lane fold is dispatched: {refusal}"));
    let answer = words(
        context
            .fetch(resident[0], (tile * 2) as usize)
            .expect("the answer comes back"),
    );
    context.release(resident[0]);
    println!(
        "per-lane two-step: [1] {} [0] {} [2] {}",
        answer[1], answer[0], answer[2]
    );
    assert_eq!(
        answer[1], 1,
        "lane 1 folds two steps and answers their sum"
    );
}

/// `out[i] = in[i] + in[i] + 1`.
fn adds() -> KernelFragment {    KernelFragment {
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
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// `out[i] = a[i] + b[i] + 1` — the two-input shape a node takes.
///
/// # Invariant
/// Two leaves however many inputs there are: a graph dispatch pushes the extent and the index and
/// nothing else, so the second input is reached through a read's position. A fragment declaring a
/// runtime scalar beside the extent is refused by name rather than dispatched.
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
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// The same shape with a runtime scalar declared beside the extent and the index.
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
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
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
/// # Invariant
/// The class is checked: a fetch classed as floats from an integer fragment would be the ABI reading the
/// buffer at the wrong width.
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

/// Hand back every buffer the run produced.
fn release_all(context: &GpuContext, values: &[Value<'_>]) {
    for value in values {
        if let Ok(BufferSlot::Resident(id)) = value.slot() {
            context.release(id);
        }
    }
}

/// Two dispatches, the second over the first's output and the caller's input.
///
/// # Invariant
/// The second node records against a resident id the device has not finished writing, and nothing reads
/// it: the run settles every submission before it hands an id back.
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
/// # Invariant
/// The same graph runs at a different extent, which is why a count is a value rather than a build-time
/// number: a graph given only a count the builder knew would be rebuilt per run. The count decides how
/// many elements are allocated, dispatched and read back, so a wrong slot shows as a wrong length.
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
                // Input 0 is the data, value 1 is the count: the recording and the
                // caller tell them apart.
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
        // Longer than the count, so a whole-buffer dispatch would answer wrong.
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

/// A graph node's fragment may not declare a runtime scalar beside the extent.
///
/// # Invariant
/// A `KernelNode` carries the extent and its inputs, so the launch fixes one scalar leaf; a
/// fragment declaring a second would have every lane read a value nobody supplied. The
/// alternative is a number that is wrong rather than an answer that is missing.
#[test]
fn a_graph_node_carrying_a_runtime_scalar_is_refused_by_name() {
    let Some(context) =
        common::context("a_graph_node_carrying_a_runtime_scalar_is_refused_by_name")
    else {
        return;
    };
    let mut graph = Graph::with_inputs(1);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: with_a_runtime_scalar(),
                inputs: vec![0],
                count: Count::Constant(8),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    let data: Vec<i64> = vec![0; 8];
    let refusal = Runner::new(&context, Policy::Serial)
        .run(&graph, vec![Value::host(data)])
        .expect_err("the node's second leaf has no value in a graph");
    assert!(
        refusal.to_string().contains("runtime scalar"),
        "the message names what the launch did not supply: {refusal}"
    );
}

/// `slot[i % 64] = i + 1`; barrier; `out[i] = slot[(i + 1) % 64]`.
///
/// # Invariant
/// The reader names **another lane's** element: a lane that read its own slot would answer
/// `i + 1`, so the answer exists only if the workgroup's storage and the barrier both work.
/// `add_op` rather than `from_flat` because the read's version operand is appended while the
/// write that produces it comes first (`docs/notes/shared-memory-and-barriers.md` §4).
fn stages_between_lanes() -> KernelFragment {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);

    let sixty_four = body.add_const(entry, ScalarClass::Int, 64);
    let one = body.add_const(entry, ScalarClass::Int, 1);
    // The write: slice 0 holds `i + 1` at `i % 64`.
    let write_slot = body.add_const(entry, ScalarClass::Int, 0);
    let offset = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![index, sixty_four],
        vec![ScalarClass::Int],
    );
    let written = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![index, one],
        vec![ScalarClass::Int],
    );
    let write = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![write_slot, offset, written],
        vec![ScalarClass::Int],
    );
    // The read: the element one lane over, which this lane did not write.
    let read_slot = body.add_const(entry, ScalarClass::Int, 0);
    let next = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![index, one],
        vec![ScalarClass::Int],
    );
    let next_offset = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![next, sixty_four],
        vec![ScalarClass::Int],
    );
    let read = body.add_op(
        entry,
        KernelInstr::SliceReadCall(ScalarClass::Int),
        vec![read_slot, next_offset, write],
        vec![ScalarClass::Int],
    );
    body.barriers.push(Barrier { write, read });
    let position = body.add_const(entry, ScalarClass::Int, 0);
    body.add_op(
        entry,
        KernelInstr::BufferWriteCall(ScalarClass::Int),
        vec![position, index, read],
        Vec::new(),
    );
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![position],
        },
    );
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        // Nothing is bound in: the exchange is between the workgroup's own lanes.
        inputs: 0,
        outputs: 1,
        input_classes: Vec::new(),
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: 64,
            space: Space::Shared,
        }],
        marks: Vec::new(),
    }
}

/// A staged exchange answers the number derived by hand, on a real device.
///
/// # Invariant
/// This class has no CPU reference: `gpu_matches_cpu`'s interpreter refuses a staged body rather
/// than read a barrier as a no-op (§7 of the shared-memory note), so the closed form is the
/// oracle. Lane `g` reads what lane `(g + 1) % 64` wrote — `local + 1` — answering `g + 2`,
/// except at a workgroup's last lane, where the read wraps to `g - 62`.
#[test]
fn a_staged_exchange_answers_the_value_derived_by_hand() {
    let Some(context) = common::context("a_staged_exchange_answers_the_value_derived_by_hand")
    else {
        return;
    };
    // A workgroup multiple, so no surplus lane writes a slot a real lane reads.
    let count = 128usize;
    let mut graph = Graph::with_inputs(0);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: stages_between_lanes(),
                inputs: Vec::new(),
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    let returned = graph.returns().expect("the return was recorded")[0];

    let expected: Vec<i64> = (0..count as i64)
        .map(|lane| if lane % 64 == 63 { lane - 62 } else { lane + 2 })
        .collect();
    // A same-lane read would answer `lane + 1`: the cross-lane index is what this pins.
    assert_ne!(
        expected,
        (0..count as i64).map(|lane| lane + 1).collect::<Vec<i64>>()
    );

    for policy in [Policy::Serial, Policy::Async] {
        let out = Runner::new(&context, policy)
            .run(&graph, Vec::new())
            .unwrap_or_else(|refusal| panic!("{policy:?} runs a staged dispatch: {refusal}"));
        let id = resident(&out[returned]);
        assert_eq!(
            words(
                context
                    .fetch(id, count)
                    .expect("the staged answer comes off the device")
            ),
            expected,
            "{policy:?}: a barrier and a workgroup slot answer the closed form"
        );
        release_all(&context, &out);
    }
}

/// A staged launch at a count that is not a whole workgroup is refused by name.
///
/// # Invariant
/// The dispatch rounds up and runs the surplus lanes with zero-padded input, so on a staged body a
/// surplus lane's fabricated value reaches a real lane's answer with no diagnostic. The refusal
/// names the count and the width it is off (`docs/notes/shared-memory-and-barriers.md` §8).
#[test]
fn a_staged_launch_off_a_workgroup_multiple_is_refused_by_name() {
    let Some(context) =
        common::context("a_staged_launch_off_a_workgroup_multiple_is_refused_by_name")
    else {
        return;
    };
    let count = 100usize;
    let refusal = context
        .run(&LaunchSet::single(&stages_between_lanes()), &[], count)
        .expect_err("a staged launch off a whole workgroup is refused");
    let message = refusal.to_string();
    assert!(message.contains("100"), "names the count: {message}");
    assert!(message.contains("64"), "names the lane width: {message}");
}
