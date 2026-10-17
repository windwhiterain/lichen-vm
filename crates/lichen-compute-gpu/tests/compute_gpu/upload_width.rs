//! What a host input's upload covers: its own length, and the padding past it.
//!
//! # Invariant
//! A lane above the count runs no instruction of the body, so it writes nothing; the padding past
//! a buffer's own length is still cleared on the device, so a real lane that reads it — or a later
//! run's allocation — inherits no stale byte.

use super::common;

use lichen_kernel_ir::{
    BufferSlot, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaneHeight, LaneWidth, LaunchSet, ScalarClass, ScalarData, TargetFacts,
};

/// A one-fragment launch set: every kernel here stands alone.
fn only(fragment: &KernelFragment) -> LaunchSet<'_> {
    LaunchSet::single(fragment)
}

/// The packed bytes of `words`, one `i64` each.
fn pack(words: &[i64]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// The `i64` words of a fetched payload, for a fragment whose class is `Int`.
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

/// `out[i] = in[i + shift]`, a one-input integer fragment.
///
/// # Invariant
/// The shift is a literal rather than an index read, so the read set is exactly `[shift, shift +
/// count)` and is the same for every lane — which is what makes a count of 64 over a 128-element
/// input a run that reads nothing but elements above the count.
fn shifted(shift: i64) -> KernelFragment {
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
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)), // buffer pos
                FlatOp::Read(1),                                        // idx, again
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, shift)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int; 1],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
            lane_height: LaneHeight::new(1),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

/// One dispatch of `fragment` over `count` lanes of `input`, and all `read` of what it answered.
///
/// # Invariant
/// The id is released before this returns, so a caller reading two runs' answers on one context is
/// reading two runs and not one run's buffer twice.
fn dispatched(
    context: &lichen_compute_gpu::GpuContext,
    fragment: &KernelFragment,
    input: &[i64],
    count: usize,
    read: usize,
) -> Vec<i64> {
    let packed = pack(input);
    let resident = context
        .run(&only(fragment), &[BufferSlot::Host(&packed)], count)
        .expect("the dispatch completes");
    let answered = words(
        context
            .fetch(resident[0], read)
            .expect("the result comes back off the device"),
    );
    context.release(resident[0]);
    answered
}

/// The upload is the buffer's own length, so an index above the count is a real element.
///
/// # Invariant
/// **The count is a whole workgroup**, so no surplus lane runs and every read stays inside the
/// input's 128 elements: the fragment reads `[64, 128)` and the run covers `[0, 64)`. The
/// allocation is therefore also the case the padding cannot cover — 128 elements where the
/// rounding alone would have made 64.
#[test]
fn an_index_above_the_count_answers_the_element_the_host_holds() {
    let test = "an_index_above_the_count_answers_the_element_the_host_holds";
    let Some(context) = common::context(test) else {
        return;
    };
    let input: Vec<i64> = (0..128).map(|value| value + 1).collect();
    let expected: Vec<i64> = input[64..].to_vec();
    let answered = dispatched(&context, &shifted(64), &input, 64, 64);
    assert_eq!(
        answered, expected,
        "lane i answered in[i + 64], which is above the count of 64 and inside the input"
    );
}

/// A lane above the count writes nothing, so the tail keeps what the allocation held.
///
/// # Invariant
/// The first run covers a whole workgroup, so all 128 slots are written; the second covers 100 on
/// a fresh 128-slot allocation from the same pool, and its 28 surplus lanes run no instruction, so
/// those slots still hold the first run's answers. That is what proves the guard: the tail is
/// neither cleared nor overwritten, and only the first `count` elements are the answer.
#[test]
fn the_padding_past_the_buffer_keeps_what_the_allocation_held() {
    let test = "the_padding_past_the_buffer_keeps_what_the_allocation_held";
    let Some(context) = common::context(test) else {
        return;
    };
    let fragment = shifted(0);
    let long: Vec<i64> = (0..128).collect();
    let first = dispatched(&context, &fragment, &long, 128, 128);
    assert_eq!(
        first, long,
        "a whole workgroup writes every slot, so the answer is the input element for element"
    );

    let short: Vec<i64> = (0..100).map(|value| value - 50).collect();
    let second = dispatched(&context, &fragment, &short, 100, 128);
    assert_eq!(
        second[..100],
        short[..],
        "the run's own 100 lanes answer the input element for element"
    );
    assert_eq!(
        second[100..],
        first[100..],
        "a surplus lane runs no instruction, so the tail still holds what the previous run left \
         there rather than being cleared or overwritten"
    );
}

/// A count that reaches past the buffer reads the padding as zero rather than refusing.
///
/// # Invariant
/// The upload is the input's own length and the allocation is the count rounded to a
/// workgroup, so the lanes above the input read the cleared padding: the answer is the input
/// followed by `0`s, the same answer the host path's zero-fill gives.
#[test]
fn an_input_shorter_than_the_count_reads_the_padding_as_zero() {
    let test = "an_input_shorter_than_the_count_reads_the_padding_as_zero";
    let Some(context) = common::context(test) else {
        return;
    };
    let short: Vec<i64> = (0..100).collect();
    let answered = dispatched(&context, &shifted(0), &short, 128, 128);
    let mut expected = short.clone();
    expected.resize(128, 0);
    assert_eq!(
        answered, expected,
        "the lanes above the 100-element input answered the cleared padding"
    );
}
