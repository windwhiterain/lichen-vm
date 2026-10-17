//! What a host input's upload covers: its own length, and the padding past it.
//!
//! # Invariant
//! An index the body reads above the count answers the element the host really holds, and the
//! padding past the buffer's own length is still cleared on the device, so the surplus lanes of
//! the last workgroup read a defined `0` and a later run's allocation inherits no stale byte.

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

/// The padding past the buffer's own length is still cleared, and a run above the count reads it.
///
/// # Invariant
/// Two runs on one context, both at a count of 100 and so both allocated 128 elements, which is
/// what puts the second on the first's buffer: without the clear its surplus lanes would answer the
/// first run's bytes. The first run fills the allocation, so its surplus lanes read real elements —
/// and the second's read `0` in exactly the places the first answered them.
#[test]
fn the_padding_past_the_buffer_is_cleared_and_read_as_zero() {
    let test = "the_padding_past_the_buffer_is_cleared_and_read_as_zero";
    let Some(context) = common::context(test) else {
        return;
    };
    let fragment = shifted(0);
    let long: Vec<i64> = (0..128).collect();
    let first = dispatched(&context, &fragment, &long, 100, 128);
    assert_eq!(
        first[100..],
        [
            100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
            117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127
        ],
        "a surplus lane answered the real element above the count, not a zero"
    );

    let short: Vec<i64> = (0..100).map(|value| value - 50).collect();
    let second = dispatched(&context, &fragment, &short, 100, 128);
    let mut expected = short.clone();
    expected.resize(128, 0);
    assert_eq!(
        second, expected,
        "the padding past the 100-element input is the first run's allocation, so it must be \
         cleared rather than left holding the bytes the first run wrote there"
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
