//! A service admits and retires sequences by slot.

use lichen_serve::Slots;

/// Slots are taken and given back, and a retired slot is the next admission's.
///
/// # Invariant
/// Three slots of four positions of two columns is the batch the service is opened with, so the free
/// slots are handed out in order and a fourth admission has nothing to hand back. A retirement frees
/// the slot a new admission takes. The bookkeeping is the host's own, so it needs no device: what a
/// step wrote into a slot is a reading of the device, and this test does not step.
#[test]
fn a_batch_admits_retires_and_reuses_its_own_slots() {
    let mut slots = Slots::new(3, 4);
    assert_eq!(
        slots.lanes(2),
        24,
        "three slots of four positions of two columns"
    );
    assert_eq!(slots.active(), 0, "a batch begins empty");
    for expected in 0..3 {
        assert_eq!(
            slots.admit(),
            Some(expected),
            "the free slots are handed out in order"
        );
    }
    assert_eq!(slots.active(), 3, "three sequences are live");
    assert_eq!(
        slots.admit(),
        None,
        "and a fourth sequence has no slot to take"
    );

    slots.retire(1);
    assert_eq!(slots.active(), 2, "one sequence retired");
    assert_eq!(
        slots.admit(),
        Some(1),
        "the retired slot is free for a new sequence"
    );
    println!("three slots admitted, one retired and reused");
}
