//! A graph with both kinds of node in it, run on a backend that is not a device.
//!
//! The stub computes on the host, so what is checked here is the **scheduling**
//! and not the arithmetic: that a native node is waited for before it reads, that
//! a kernel node is not, and that the value table stays aligned through both. The
//! arithmetic on a real device is `lichen-compute-gpu`'s business.

use std::sync::{Arc, Mutex};

use lichen_graph_ir::{
    Count, Graph, GraphRefusal, KernelNode, NativeNode, Node, Policy, Runner, Value,
};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape, ParallelBackend,
    Pending, ResidentId,
};

/// `out[i] = in[i] + in[i] + 1`, which is `adds` everywhere else in this tree.
fn fragment() -> KernelFragment {
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

/// A host call: `out[i] = in[i] * 10`.
fn times_ten(inputs: &[&[i64]]) -> Vec<Vec<i64>> {
    vec![inputs[0].iter().map(|value| value * 10).collect()]
}

/// A backend that keeps its buffers in host memory, and records **what the host
/// asked for and in what order**.
///
/// The order is the interesting part: it says whether a run waited when it did
/// not have to. `run` and `submit` are told apart because that is exactly what a
/// policy chooses between, and a stub that treated them alike could not tell the
/// difference.
#[derive(Clone, Default)]
struct Stub {
    /// One column per resident id, indexed by `id.0 - 1`.
    data: Arc<Mutex<Vec<Vec<i64>>>>,
    asked: Arc<Mutex<Vec<&'static str>>>,
}

impl Stub {
    fn new() -> Self {
        Stub::default()
    }

    /// What the host asked for, in order.
    fn asked(&self) -> Vec<&'static str> {
        self.asked.lock().unwrap().clone()
    }

    fn compute(&self, inputs: &[BufferSlot<'_>], count: usize) -> Vec<ResidentId> {
        let mut columns = Vec::with_capacity(inputs.len());
        for slot in inputs {
            match slot {
                BufferSlot::Host(data) => columns.push(data.to_vec()),
                BufferSlot::Resident(id) => {
                    columns.push(self.data.lock().unwrap()[id.0 as usize - 1].clone())
                }
            }
        }
        // Only the single-input fragment is ever dispatched here, so the kernel
        // is `adds` applied to the first column.
        let out: Vec<i64> = (0..count)
            .map(|i| columns[0][i] + columns[0][i] + 1)
            .collect();
        let mut data = self.data.lock().unwrap();
        data.push(out);
        vec![ResidentId(data.len() as u64)]
    }
}

/// A submission that has been recorded and not waited for.
struct StubPending {
    ids: Vec<ResidentId>,
    stub: Stub,
}

impl Pending for StubPending {
    fn outputs(&self) -> &[ResidentId] {
        &self.ids
    }

    fn wait(self: Box<Self>) -> Result<Vec<ResidentId>, String> {
        self.stub.asked.lock().unwrap().push("wait");
        Ok(self.ids)
    }
}

impl ParallelBackend for Stub {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn run(
        &self,
        _fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, String> {
        self.asked.lock().unwrap().push("run");
        Ok(self.compute(inputs, count))
    }

    fn submit<'backend>(
        &'backend self,
        _fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Box<dyn Pending + 'backend>, String> {
        self.asked.lock().unwrap().push("submit");
        Ok(Box::new(StubPending {
            ids: self.compute(inputs, count),
            stub: self.clone(),
        }))
    }

    fn fetch(&self, id: ResidentId, count: usize) -> Result<Vec<i64>, String> {
        self.asked.lock().unwrap().push("fetch");
        Ok(self.data.lock().unwrap()[id.0 as usize - 1][..count].to_vec())
    }

    fn release(&self, _id: ResidentId) {}
}

/// One dispatch, one host call over its result, one more dispatch.
///
/// Node 1 is the whole point: it reads a value the device may not have written,
/// so it is a demand point and forces the wait. Node 2 does not read anything —
/// it records against node 1's *output*, which is host data by then — so it
/// costs nothing to wait for.
fn mixed_graph(count: usize) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let first = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: fragment(),
                inputs: vec![0],
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("a node whose declared outputs match its body");
    let host = graph
        .push(
            Node::Native(NativeNode {
                call: times_ten,
                inputs: vec![first],
                outputs: 1,
            }),
            1,
        )
        .expect("a node whose declared outputs match its body");
    graph
        .push(
            Node::Kernel(KernelNode {
                fragment: fragment(),
                inputs: vec![host],
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("a node whose declared outputs match its body");
    graph
}

/// The result of `mixed_graph` at index `x`: adds, then ten times, then adds.
fn expected(count: usize) -> Vec<i64> {
    (0..count as i64).map(|x| 20 * (2 * x + 1) + 1).collect()
}

/// The resident id a run handed back, read through the only accessor that does
/// not require a backend.
fn resident(value: &Value<'_>) -> ResidentId {
    match value.slot().expect("a run's output names a buffer") {
        BufferSlot::Resident(id) => id,
        BufferSlot::Host(_) => panic!("a kernel node's output is a buffer, not host data"),
    }
}

#[test]
fn a_graph_of_both_node_kinds_computes_the_same_thing_under_both_policies() {
    let count = 8;
    let input: Vec<i64> = (0..count as i64).collect();

    for policy in [Policy::Serial, Policy::Async] {
        let stub = Stub::new();
        let graph = mixed_graph(count);
        let out = Runner::new(&stub, policy)
            .run(&graph, vec![Value::host(input.clone())])
            .unwrap_or_else(|refusal| panic!("{policy:?} runs: {refusal}"));

        assert_eq!(
            out.len(),
            4,
            "{policy:?}: the input plus one value per node. The runner hands back every value the \
             graph has rather than picking one, because what a function returns is the function's \
             business and it is recorded in the graph, not chosen here"
        );
        assert!(
            out.iter().all(Value::is_ready),
            "{policy:?}: every value is settled before the run hands it back, so an id never \
             names a buffer the device is still writing"
        );
        let id = resident(out.last().expect("the last node produced one"));
        assert_eq!(
            stub.fetch(id, count).expect("the answer comes back"),
            expected(count),
            "{policy:?}: adds, then ten times, then adds"
        );
    }
}

#[test]
fn an_async_graph_waits_exactly_where_something_needs_the_data() {
    let count = 8;
    let input: Vec<i64> = (0..count as i64).collect();

    let stub = Stub::new();
    let graph = mixed_graph(count);
    Runner::new(&stub, Policy::Async)
        .run(&graph, vec![Value::host(input)])
        .expect("the graph runs");

    // The whole schedule, asserted as a sequence rather than as counts, because
    // *where* the waits are is the claim and a count cannot say it.
    //
    //   submit         the first dispatch goes to the queue
    //   wait, fetch    the native node needs that dispatch's data, so it is a
    //                  demand point: the wait and the download are both forced
    //   submit         the second dispatch records against host data, so it
    //                  costs nothing to get here and **nothing waited for it**
    //   wait           the run hands back an id, and an id has to name a buffer
    //                  the device has written
    //
    // Three waits would be a scheduler that waited for everything, which is
    // serial with more steps. One wait would be a scheduler that let the native
    // node read a buffer the device had not written — a wrong answer rather than
    // a slow one, and the reason the pending state is in the value type at all.
    assert_eq!(
        stub.asked(),
        vec!["submit", "wait", "fetch", "submit", "wait"],
        "two waits, at the two demand points, and none of them before a dispatch \
         that did not need one"
    );
}

#[test]
fn a_serial_graph_never_submits_without_waiting() {
    let count = 8;
    let input: Vec<i64> = (0..count as i64).collect();

    let stub = Stub::new();
    let graph = mixed_graph(count);
    Runner::new(&stub, Policy::Serial)
        .run(&graph, vec![Value::host(input)])
        .expect("the graph runs");

    assert!(
        !stub.asked().contains(&"submit"),
        "serial is the plain kernel path, one run per node and a wait inside each: {:?}",
        stub.asked()
    );
}

#[test]
fn a_batch_policy_is_refused_by_name_rather_than_run_as_something_else() {
    let stub = Stub::new();
    let graph = mixed_graph(4);
    let refusal = Runner::new(&stub, Policy::Batch)
        .run(&graph, vec![Value::host(vec![0; 4])])
        .expect_err("batch is not something this contract can do");
    assert!(
        matches!(
            refusal,
            GraphRefusal::PolicyUnsupported {
                policy: "batch",
                ..
            }
        ),
        "a named refusal rather than a slower schedule: {refusal}"
    );
    assert!(
        stub.asked().is_empty(),
        "a refused policy touches the backend not at all: {:?}",
        stub.asked()
    );
}

#[test]
fn an_edge_to_a_value_that_does_not_exist_yet_is_refused_where_it_is_written() {
    let mut graph = Graph::with_inputs(1);
    let refusal = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: fragment(),
                // Value 7 has not been produced, and no cycle can be written
                // through this type — so this is the closest thing to one.
                inputs: vec![7],
                count: Count::Constant(4),
            }),
            1,
        )
        .expect_err("an edge past the end of the value table is a mistake in the graph");
    assert_eq!(
        refusal,
        GraphRefusal::EdgeBeforeItsProducer {
            node: 0,
            value: 7,
            defined: 1
        }
    );
}

#[test]
fn an_output_count_the_body_disagrees_with_is_refused_rather_than_misaligning_the_table() {
    let mut graph = Graph::with_inputs(1);
    let refusal = graph
        .push(
            Node::Native(NativeNode {
                call: times_ten,
                inputs: vec![0],
                outputs: 1,
            }),
            2,
        )
        .expect_err("two claimed outputs for a call that produces one");
    assert_eq!(
        refusal,
        GraphRefusal::OutputCount {
            node: 0,
            declared: 1,
            claimed: 2
        }
    );
}

/// `mixed_graph` with the source function's return recorded as its **first**
/// node's output, so the last two nodes are a dead tail.
///
/// This is the case the record exists for. "The tail" would answer this graph
/// with the wrong value, and a runner that picked the last node could not
/// express this function's return at all.
fn graph_with_a_dead_tail(count: usize) -> Graph {
    let mut graph = mixed_graph(count);
    // Value 0 is the input, so the first node's output is value 1.
    graph.returning(vec![1]).expect("a value the graph defines");
    graph
}

#[test]
fn a_dead_tail_still_returns_whatever_the_source_function_returned() {
    let count = 8;
    let input: Vec<i64> = (0..count as i64).collect();
    let stub = Stub::new();
    let graph = graph_with_a_dead_tail(count);

    let out = Runner::new(&stub, Policy::Async)
        .run(&graph, vec![Value::host(input)])
        .expect("the graph runs");
    let returned = graph.returns().expect("the return was recorded");

    assert_eq!(
        returned,
        [1],
        "the recorded return is the first node's output, and the runner did not overrule it"
    );
    // The dead tail still ran. A return is a *choice among the graph's values*,
    // not a truncation of it — which is the whole reason it is recorded rather
    // than read off the end.
    assert_eq!(
        stub.asked(),
        vec!["submit", "wait", "fetch", "submit", "wait"],
        "the same schedule as without a dead tail: the tail is not skipped, and the return does \
         not change what runs"
    );
    // The native node read value 1, so the runner brought it home in place and
    // the returned value is host data — the fetched answer, not an id.
    let Value::Host(first) = &out[returned[0]] else {
        panic!("the returned value was read by a native node, so it came home")
    };
    assert_eq!(
        *first,
        (0..count as i64).map(|x| 2 * x + 1).collect::<Vec<i64>>(),
        "which is the first node's answer and not the tail's"
    );
}

#[test]
fn a_graph_that_has_not_been_told_its_return_reports_none_rather_than_nothing() {
    let graph = mixed_graph(4);
    assert_eq!(
        graph.returns(),
        None,
        "None and an empty list are different answers: None means nobody has said, and a caller \
         reading it takes every value — while an empty list is a function that returns a unit"
    );
}

#[test]
fn a_return_recorded_twice_is_refused_rather_than_the_second_one_winning() {
    let mut graph = mixed_graph(4);
    graph.returning(vec![1, 2]).expect("the first answer");
    let refusal = graph
        .returning(vec![3])
        .expect_err("two opinions about what one function returns");
    assert_eq!(refusal, GraphRefusal::ReturnAlreadyRecorded { recorded: 2 });
    assert_eq!(
        graph.returns(),
        Some([1, 2].as_slice()),
        "and the first answer is the one that stands"
    );
}

#[test]
fn a_return_naming_a_value_the_graph_does_not_have_is_refused() {
    let mut graph = mixed_graph(4);
    let refusal = graph
        .returning(vec![99])
        .expect_err("a value past the end of the table");
    assert_eq!(
        refusal,
        GraphRefusal::UnknownValue {
            node: usize::MAX,
            value: 99
        }
    );
}

/// A graph whose extent is one of its arguments, which is the shape a count that
/// depends on data has.
fn counted_by_input() -> Graph {
    let mut graph = Graph::with_inputs(2);
    graph
        .push(
            Node::Kernel(KernelNode {
                fragment: fragment(),
                inputs: vec![0],
                count: Count::Value(1),
            }),
            1,
        )
        .expect("a node whose declared outputs match its body");
    graph
}

#[test]
fn a_count_can_be_one_of_the_graphs_own_values() {
    let stub = Stub::new();
    let graph = counted_by_input();
    // The count is an argument, so the same graph runs at a different extent
    // without being rebuilt — which is the whole reason a count is a value.
    for count in [1_i64, 3, 8] {
        let input: Vec<i64> = (0..count).collect();
        let out = Runner::new(&stub, Policy::Serial)
            .run(&graph, vec![Value::host(input), Value::int(count)])
            .unwrap_or_else(|refusal| panic!("a run over {count} element(s): {refusal}"));
        let id = resident(&out[2]);
        let computed = stub.fetch(id, count as usize).expect("a resident buffer");
        assert_eq!(
            computed,
            (0..count).map(|x| 2 * x + 1).collect::<Vec<i64>>(),
            "the dispatch covered [0, {count}) because the count was the graph's second argument"
        );
    }
}

#[test]
fn a_count_read_from_data_and_a_buffer_given_a_number_are_two_different_refusals() {
    // A count edge pointing at data. The node reads input 0, so that is the
    // buffer slot and value 1 is what the count is read from.
    let count_from_data = Runner::new(&Stub::new(), Policy::Serial)
        .run(
            &counted_by_input(),
            vec![Value::host(vec![1, 2, 3, 4]), Value::host(vec![4])],
        )
        .map(|_| ())
        .expect_err("a count read from data");
    assert_eq!(
        count_from_data,
        GraphRefusal::CountNotANumber { found: "host data" },
        "and the message says what the value was, so a caller can tell the edge \
         is wrong from the builder having written a number into a data slot"
    );

    // The same graph with a number in the buffer slot.
    let buffer_is_a_number = Runner::new(&Stub::new(), Policy::Serial)
        .run(&counted_by_input(), vec![Value::int(4), Value::int(4)])
        .map(|_| ())
        .expect_err("a buffer slot holding a number");
    assert_eq!(
        buffer_is_a_number,
        GraphRefusal::NotBufferData { found: "a number" },
        "which is a different message and not the same one twice: a number is \
         not a one-element buffer, and reading it as one would run the dispatch \
         against data nobody wrote"
    );
}

#[test]
fn a_negative_count_is_refused_rather_than_wrapped_into_an_enormous_extent() {
    let refusal = Runner::new(&Stub::new(), Policy::Serial)
        .run(
            &counted_by_input(),
            vec![Value::host(vec![1, 2]), Value::int(-1)],
        )
        .map(|_| ())
        .expect_err("a negative extent");
    assert_eq!(refusal, GraphRefusal::CountNegative { number: -1 });
    assert_eq!(
        refusal.to_string(),
        "a dispatch's count is -1, and a count cannot be negative.",
        "and it says so in a `usize` world where -1 would have become a very \
         large dispatch rather than a mistake"
    );
}

#[test]
fn a_native_value_names_its_kind_in_every_refusal_that_reads_one() {
    // The second inhabitant of `Native`, and the one the value table exists for:
    // a pointer to a value the host owns. Nothing produces one yet, so this is
    // only ever an argument — but both roles have to answer for it, and they have
    // to answer differently, because the two repairs are different.
    let pointer = || Value::native_pointer(7);
    let stub = Stub::new();

    // Asked for a count: the role was right and the value was not.
    let as_count = Runner::new(&stub, Policy::Serial)
        .run(
            &counted_by_input(),
            vec![Value::host(vec![1, 2, 3, 4]), pointer()],
        )
        .map(|_| ())
        .expect_err("a count read from a host-owned value");
    assert_eq!(
        as_count,
        GraphRefusal::CountNotANumber {
            found: "a value the host owns"
        }
    );

    // Asked for a buffer: the other role, refused by the other message.
    let as_buffer = Runner::new(&stub, Policy::Serial)
        .run(&counted_by_input(), vec![pointer(), Value::int(4)])
        .map(|_| ())
        .expect_err("a buffer slot holding a host-owned value");
    assert_eq!(
        as_buffer,
        GraphRefusal::NotBufferData {
            found: "a value the host owns"
        }
    );

    // And it is not the number's message either, which is the point of the kind:
    // both native values have no extent, but a caller who passed one knows which.
    let number_as_buffer = Runner::new(&Stub::new(), Policy::Serial)
        .run(&counted_by_input(), vec![Value::int(4), Value::int(4)])
        .map(|_| ())
        .expect_err("a buffer slot holding a number");
    assert_ne!(
        as_buffer, number_as_buffer,
        "a number and a host-owned value are the same category and different \
         mistakes, so they must not be reported as the same one"
    );

    // Ready, and without an extent. A native value is never pending because the
    // host made it, and a count of zero would be a length somebody could act on.
    let value = pointer();
    assert!(
        value.is_ready(),
        "the host made it, so there is no wait owed"
    );
    assert_eq!(value.count(), None, "and it has no extent to report");
}
