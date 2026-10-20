//! A service admits, retires and advances sequences by slot, and its cache never leaves the device.

mod common;

use common::{APPEND, read};
use lichen_serve::{Service, Session};

/// Slots are taken and given back, and a retired sequence's cache waits where it was.
///
/// # Invariant
/// Three slots of four positions of two columns: slot `s` owns cache words `8s` to `8s + 7` and its
/// position `p` is words `8s + 2p` and `8s + 2p + 1`. Every fresh word below is a distinct pair, so
/// the assertion says which sequence wrote where and nothing else could have.
#[test]
fn a_service_admits_retires_and_advances_by_slot() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_service_admits_retires_and_advances_by_slot: no device");
        return;
    };
    let mut service = Service::open(&session, APPEND, 3, 4, 2)
        .unwrap_or_else(|refusal| panic!("the service opens: {refusal}"));
    assert_eq!(
        service.lanes(),
        24,
        "three slots of four positions of two columns"
    );
    assert_eq!(service.active(), 0, "a batch begins empty");
    for expected in 0..3 {
        assert_eq!(
            service.admit(),
            Some(expected),
            "the free slots are handed out in order"
        );
    }
    assert_eq!(service.active(), 3, "three sequences are live");
    assert_eq!(
        service.admit(),
        None,
        "and a fourth sequence has no slot to take"
    );

    let stepped = service
        .step(
            &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0],
            &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0],
        )
        .unwrap_or_else(|refusal| panic!("the first step runs: {refusal}"));
    assert_eq!(stepped.advanced, 3, "all three sequences advanced");
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "three sequences are one submission"
    );
    let keys = read(
        &session,
        service.keys().expect("the first step seeded the cache"),
    );
    assert_eq!(
        [keys[0], keys[1], keys[8], keys[9], keys[16], keys[17]],
        [10.0, 11.0, 12.0, 13.0, 14.0, 15.0],
        "each sequence wrote its own word at position zero of its own slots"
    );

    service.retire(1);
    assert_eq!(service.active(), 2, "one sequence retired");
    let stepped = service
        .step(
            &[20.0, 21.0, 22.0, 23.0, 24.0, 25.0],
            &[20.0, 21.0, 22.0, 23.0, 24.0, 25.0],
        )
        .unwrap_or_else(|refusal| panic!("the second step runs: {refusal}"));
    assert_eq!(stepped.advanced, 2, "only the live sequences advanced");
    let keys = read(
        &session,
        service.keys().expect("the second step wrote a cache"),
    );
    assert_eq!(
        [keys[2], keys[3]],
        [20.0, 21.0],
        "the first sequence took its second position"
    );
    assert_eq!(
        [keys[18], keys[19]],
        [24.0, 25.0],
        "and so did the third, in its own slots"
    );
    assert_eq!(
        [keys[8], keys[9]],
        [12.0, 13.0],
        "while the retired sequence's slots still hold what it wrote, which no reader reaches"
    );

    assert_eq!(
        service.admit(),
        Some(1),
        "the retired slot is free for a new sequence"
    );
    let stepped = service
        .step(
            &[30.0, 31.0, 32.0, 33.0, 34.0, 35.0],
            &[30.0, 31.0, 32.0, 33.0, 34.0, 35.0],
        )
        .unwrap_or_else(|refusal| panic!("the third step runs: {refusal}"));
    assert_eq!(stepped.advanced, 3, "the new sequence is live");
    let keys = read(
        &session,
        service.keys().expect("the third step wrote a cache"),
    );
    assert_eq!(
        [keys[8], keys[9]],
        [32.0, 33.0],
        "the new sequence writes at position zero of the slot it was given"
    );
    assert_eq!(
        [keys[4], keys[5]],
        [30.0, 31.0],
        "and the resident sequence kept advancing, from the cache it never had to upload"
    );
    assert_eq!([keys[20], keys[21]], [34.0, 35.0], "as did the third");
    println!("three slots admitted, one retired and reused, at one submission a step");
}
