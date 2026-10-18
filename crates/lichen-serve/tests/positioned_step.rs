//! Each sequence appends at its own position, read from one `Int` buffer, in one dispatch.

mod common;

use common::{APPEND, caches, floats, positions, read};
use lichen_compute::compute::graph::RunArgument;
use lichen_serve::Session;

/// Three sequences at three different positions each write their own slot in one dispatch.
///
/// # Invariant
/// The cache is three sequences of four positions of two columns, so sequence `s` owns slots `8s` to
/// `8s + 7` and its position `p` is slots `8s + 2p` and `8s + 2p + 1`. The three sequences are at
/// positions `2`, `0` and `1` — not the same one — so the assertion says each wrote at its own
/// position and left the positions it had not reached alone.
#[test]
fn each_sequence_appends_at_its_own_position() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED each_sequence_appends_at_its_own_position: no device");
        return;
    };
    let prepared = session
        .prepare(APPEND)
        .unwrap_or_else(|refusal| panic!("the per-position append prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is the append and the scale"
    );

    let empty = [0.0f32; 24];
    let first = prepared
        .step(vec![
            RunArgument::Count(24),
            floats(&empty),
            floats(&empty),
            floats(&[10.0, 11.0, 12.0, 13.0, 14.0, 15.0]),
            floats(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            positions(&[2, 0, 1]),
        ])
        .unwrap_or_else(|refusal| panic!("the first step runs: {refusal}"));
    assert_eq!(
        (first.cost.submissions, first.cost.fence_waits),
        (1, 1),
        "three sequences at three positions appended in one submission"
    );
    let (keys, values) = caches(&first);
    let written = read(&session, keys);
    assert_eq!(
        [written[4], written[5]],
        [10.0, 11.0],
        "the first sequence is at position 2, which is its slots 4 and 5"
    );
    assert_eq!(
        [written[8], written[9]],
        [12.0, 13.0],
        "the second sequence is at position 0, which is its slots 0 and 1, told as 8 and 9"
    );
    assert_eq!(
        [written[18], written[19]],
        [14.0, 15.0],
        "and the third is at position 1, its slots 2 and 3, told as 18 and 19"
    );
    assert_eq!(
        [written[0], written[1], written[10], written[11]],
        [0.0; 4],
        "the positions those sequences had not reached are still what they were"
    );
    assert_eq!(
        [read(&session, values)[4], read(&session, values)[8]],
        [1.0, 3.0],
        "the value cache followed the key cache, at each sequence's own position"
    );

    let second = prepared
        .step(vec![
            RunArgument::Count(24),
            RunArgument::Resident(keys),
            RunArgument::Resident(values),
            floats(&[20.0, 21.0, 22.0, 23.0, 24.0, 25.0]),
            floats(&[7.0, 8.0, 9.0, 10.0, 11.0, 12.0]),
            positions(&[3, 1, 2]),
        ])
        .unwrap_or_else(|refusal| panic!("the second step runs: {refusal}"));
    let (next_keys, next_values) = caches(&second);
    let written = read(&session, next_keys);
    assert_eq!(
        [written[6], written[7]],
        [20.0, 21.0],
        "the first sequence advanced to position 3 and kept position 2"
    );
    assert_eq!(
        [written[4], written[5]],
        [10.0, 11.0],
        "which is still where it was"
    );
    assert_eq!(
        [written[10], written[11]],
        [22.0, 23.0],
        "the second sequence advanced to position 1 inside its own slots"
    );
    for resident in [keys, values, next_keys, next_values] {
        session.release(resident);
    }
    println!("three sequences at three different positions appended in one dispatch");
}
