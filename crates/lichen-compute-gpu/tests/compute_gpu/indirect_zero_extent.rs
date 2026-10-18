//! A device-written extent of zero dispatches nothing, and refuses nothing.
//!
//! # Invariant
//! A **host-known** zero extent is refused by name; the count a kernel wrote is not, because the
//! host never reads it. So a step gated by an end-of-sequence flag is expressible at the cost of
//! the schedule it already had. What is **not** guaranteed is that step's output.

use super::common;
use super::device_extent::{COUNTED, LANES, counting_chain, release_all, resident, words};

use lichen_graph_ir::{Policy, Runner, Value};

/// The counting chain with nothing passing its predicate, so the second node's extent is `0`.
#[test]
fn a_device_written_zero_extent_dispatches_nothing_and_refuses_nothing() {
    let test = "a_device_written_zero_extent_dispatches_nothing_and_refuses_nothing";
    let Some(context) = common::context(test) else {
        return;
    };
    let graph = counting_chain(true);
    let none_passing: Vec<i64> = vec![0; COUNTED];
    let before = (context.submissions(), context.fence_waits());
    let values = Runner::new(&context, Policy::Batch)
        .run(&graph, vec![Value::host(none_passing)])
        .unwrap_or_else(|refusal| panic!("a device-written zero extent runs: {refusal}"));
    let counted = (
        context.submissions() - before.0,
        context.fence_waits() - before.1,
    );

    let written = words(
        context
            .fetch(resident(&values[1]), 1)
            .expect("the counting kernel's own output comes off the device"),
    );
    assert_eq!(
        written[0], 0,
        "the counting kernel wrote the zero the second node's extent was read from"
    );
    assert_eq!(
        counted,
        (1, 1),
        "and a gated step costs the schedule it already had: one submission and one fence wait"
    );

    let answered = words(
        context
            .fetch(resident(&values[2]), LANES as usize)
            .expect("the allocation comes off the device"),
    );
    assert_eq!(
        answered.len(),
        LANES as usize,
        "the second node is allocated for the counting buffer's length, padded to a workgroup, \
         and the elements are not asserted: no workgroup ran, so those bytes name no computation"
    );
    release_all(&context, &values);
}
