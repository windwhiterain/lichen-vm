//! A graph of kernel nodes, run on a backend that is not a device.
//!
//! The stub computes on the host, so what is checked here is the **scheduling**
//! and not the arithmetic. With one kind of node there is no mid-graph demand
//! point at all — nothing in a run reads a value on the host — so the schedule
//! this file pins down is the one that follows from that: a run submits each
//! node in turn and waits exactly once, at the end. The arithmetic on a real
//! device is `lichen-compute-gpu`'s business.

use std::sync::{Arc, Mutex};

use lichen_graph_ir::{Count, Graph, GraphRefusal, KernelNode, Node, Policy, Runner, Value};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBin, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    ParallelBackend, Pending, ResidentId, ScalarClass, ScalarData,
};

/// `out[i] = in[i] + in[i] + 1`, which is `adds` everywhere else in this tree.
fn fragment() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: vec![
            KernelInstr::Const(ScalarClass::Int, 0),
            KernelInstr::LocalGet(1),
            KernelInstr::Const(ScalarClass::Int, 0),
            KernelInstr::LocalGet(1),
            KernelInstr::BufferReadCall(ScalarClass::Int),
            KernelInstr::Const(ScalarClass::Int, 0),
            KernelInstr::LocalGet(1),
            KernelInstr::BufferReadCall(ScalarClass::Int),
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            KernelInstr::Const(ScalarClass::Int, 1),
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            KernelInstr::BufferWriteCall(ScalarClass::Int),
            KernelInstr::Const(ScalarClass::Int, 0),
        ]
        .into(),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        int_width: IntWidth::I64,
    }
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
                // A host slot is packed bytes, so the elements are decoded as the
                // `Int` words every fragment in this file declares rather than
                // taken as bytes.
                BufferSlot::Host(data) => {
                    columns.push(
                        data.chunks_exact(8)
                            .map(|word| i64::from_le_bytes(word.try_into().unwrap_or_default()))
                            .collect(),
                    );
                }
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

    /// A host stub's buffers hold integers, so it answers with the class it
    /// holds them as — the fetch's own class, which is what a real backend
    /// takes from the buffer it was asked about.
    fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, String> {
        self.asked.lock().unwrap().push("fetch");
        Ok(ScalarData::Int(
            self.data.lock().unwrap()[id.0 as usize - 1][..count].to_vec(),
        ))
    }

    fn release(&self, _id: ResidentId) {}
}

/// One dispatch, then a second over its output.
///
/// **The second node is what makes this a graph rather than a loop.** It records
/// against a value the device may not have written, which is only sound because
/// the recording happened first and the submission behind it was recorded first
/// too — so the chain is the shape the whole feature exists for, and it is worth
/// pinning even though it is two lines.
fn chain(count: usize) -> Graph {
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
    graph
        .push(
            Node::Kernel(KernelNode {
                fragment: fragment(),
                inputs: vec![first],
                count: Count::Constant(count),
            }),
            1,
        )
        .expect("a node whose declared outputs match its body");
    graph
}

/// The result of [`chain`] at index `x`: adds, then adds again.
fn expected(count: usize) -> Vec<i64> {
    (0..count as i64).map(|x| 4 * x + 3).collect()
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
fn a_chain_computes_the_same_thing_under_both_policies() {
    let count = 8;
    let input: Vec<i64> = (0..count as i64).collect();

    for policy in [Policy::Serial, Policy::Async] {
        let stub = Stub::new();
        let graph = chain(count);
        let out = Runner::new(&stub, policy)
            .run(&graph, vec![Value::host(input.clone())])
            .unwrap_or_else(|refusal| panic!("{policy:?} runs: {refusal}"));

        assert_eq!(
            out.len(),
            3,
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
            ScalarData::Int(expected(count)),
            "{policy:?}: adds, then adds"
        );
    }
}

#[test]
fn an_async_graph_of_kernels_waits_once_and_only_at_the_end() {
    let count = 8;
    let input: Vec<i64> = (0..count as i64).collect();

    let stub = Stub::new();
    Runner::new(&stub, Policy::Async)
        .run(&chain(count), vec![Value::host(input)])
        .expect("the graph runs");

    // The whole schedule, asserted as a sequence rather than as counts, because
    // *where* the waits are the claim and a count cannot say it.
    //
    //   submit   the first dispatch goes to the queue
    //   submit   the second records against a value the device may not have
    //            written yet, which is sound precisely because it does not read
    //   wait     the first submission is waited for
    //   wait     the second is
    //
    // **Both waits are at the end, and neither is before a dispatch, and that is
    // the whole measurement.** The host work between the two submissions is the
    // submission itself, so `hidden = min(host, device)` is `min(nothing,
    // device) = 0` and Async earns no milliseconds on a chain of pure kernels.
    // A wait in the middle would be the graph reaching for the one thing that
    // would make the overlap real, and there is nothing in a graph of kernels
    // that reaches.
    //
    // Two waits rather than one is not two rounds of synchronisation: a run that
    // has to hand back an id has to have *every* submission the device finished,
    // and two nodes submitted two things.
    assert_eq!(
        stub.asked(),
        vec!["submit", "submit", "wait", "wait"],
        "every submission waited for exactly once, and none of them before a dispatch \
         that did not need one"
    );
}

#[test]
fn a_serial_graph_never_submits_without_waiting() {
    let count = 8;
    let input: Vec<i64> = (0..count as i64).collect();

    let stub = Stub::new();
    Runner::new(&stub, Policy::Serial)
        .run(&chain(count), vec![Value::host(input)])
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
    let refusal = Runner::new(&stub, Policy::Batch)
        .run(&chain(4), vec![Value::host(vec![0; 4])])
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
            Node::Kernel(KernelNode {
                fragment: fragment(),
                inputs: vec![0],
                count: Count::Constant(4),
            }),
            2,
        )
        .expect_err("two claimed outputs for a fragment that writes one");
    assert_eq!(
        refusal,
        GraphRefusal::OutputCount {
            node: 0,
            declared: 1,
            claimed: 2
        }
    );
}

/// [`chain`] with the source function's return recorded as its **first** node's
/// output, so the last node is a dead tail.
///
/// This is the case the record exists for. "The tail" would answer this graph
/// with the wrong value, and a runner that picked the last node could not
/// express this function's return at all.
fn graph_with_a_dead_tail(count: usize) -> Graph {
    let mut graph = chain(count);
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
        vec!["submit", "submit", "wait", "wait"],
        "the same schedule as without a dead tail: the tail is not skipped, and the return does \
         not change what runs"
    );
    // The returned value is still a resident id, because nothing in the run ever
    // needed it on the host. Taking it home is the caller's choice, not the
    // runner's — which is the other half of why the return is recorded.
    assert_eq!(
        stub.fetch(resident(&out[returned[0]]), count)
            .expect("the returned value comes back"),
        ScalarData::Int((0..count as i64).map(|x| 2 * x + 1).collect::<Vec<i64>>()),
        "which is the first node's answer and not the tail's"
    );
}

#[test]
fn a_graph_that_has_not_been_told_its_return_reports_none_rather_than_nothing() {
    assert_eq!(
        chain(4).returns(),
        None,
        "None and an empty list are different answers: None means nobody has said, and a caller \
         reading it takes every value — while an empty list is a function that returns a unit"
    );
}

#[test]
fn a_return_recorded_twice_is_refused_rather_than_the_second_one_winning() {
    let mut graph = chain(4);
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
    let mut graph = chain(4);
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
            ScalarData::Int((0..count).map(|x| 2 * x + 1).collect::<Vec<i64>>()),
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
        GraphRefusal::CountNotANumber {
            node: 0,
            value: 1,
            found: "host data"
        },
        "and the message says what the value was, so a caller can tell the edge \
         is wrong from the builder having written a number into a data slot"
    );
    assert!(
        count_from_data.to_string().contains("node 0"),
        "and it names the node, because a refusal that does not is a sentence \
         about a graph of one node: {count_from_data}"
    );

    // The same graph with a number in the buffer slot.
    let buffer_is_a_number = Runner::new(&Stub::new(), Policy::Serial)
        .run(&counted_by_input(), vec![Value::int(4), Value::int(4)])
        .map(|_| ())
        .expect_err("a buffer slot holding a number");
    assert_eq!(
        buffer_is_a_number,
        GraphRefusal::NotBufferData {
            node: 0,
            value: 0,
            found: "a number"
        },
        "which is a different message and not the same one twice: a number is \
         not a one-element buffer, and reading it as one would run the dispatch \
         against data nobody wrote"
    );
    assert!(
        buffer_is_a_number.to_string().contains("value 0"),
        "**and the value, which is the other half of the same lookup.** The two \
         refusals above are told apart by the *role* — one edge wanted a count \
         and the other a buffer — so a reader has to be able to find both edges \
         by number: {buffer_is_a_number}"
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
        "a count is -1, and a count cannot be negative.",
        "and it says so in a `usize` world where -1 would have become a very \
         large dispatch rather than a mistake"
    );
    assert!(
        !refusal.to_string().contains("node"),
        "**and it names no node, on purpose.** The other two refusals in this \
         file are about a demand a node made; this one is about the number, and \
         a number is also what a function can *return*, which no node asked \
         for. Inventing a node here would name a place the mistake is not in"
    );
}

#[test]
fn a_number_is_ready_and_has_no_extent_because_it_is_neither() {
    // The two properties `Int` is refused for are also the two it is safe by, and
    // they are not the same fact. A number is never pending because a device
    // does not produce one, so there is no wait owed against it; and it has no
    // length, so a reported `0` would be a number a caller could dispatch over.
    let value = Value::int(4);
    assert!(
        value.is_ready(),
        "a device never produces one, so there is no submission behind it to wait for"
    );
    assert_eq!(
        value.count(),
        None,
        "and a number has no length, so reporting `0` would be one a caller could act on"
    );
}
