//! Running a graph, and the one thing a graph is allowed to decide.

use lichen_kernel_ir::ParallelBackend;

use crate::graph::{Count, Graph, Node};
use crate::refusal::GraphRefusal;
use crate::value::Value;

/// When the host submits, and when it waits.
///
/// **This is the only knob, and it is deliberately not in the graph.** A graph
/// says what has to happen before what. It must never say when the host is
/// allowed to notice that something finished, because a graph that could name
/// its own synchronisation would be a graph whose correctness depended on where
/// somebody put a keyword — and the two answers to "may I read this yet" would
/// then disagree between the builder and the runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Submit and wait for every node, one at a time.
    ///
    /// What a plain kernel run does today. It buys nothing, and it is here so
    /// that the other two have something to be measured against and so that a
    /// graph run with no choice behaves exactly as the un-graphed code did.
    Serial,
    /// Submit a node as soon as it is recordable, and wait only where something
    /// needs the data.
    ///
    /// The submissions are the same count as [`Self::Serial`] — nothing is fused
    /// away — but the host is not idle between them. What it collects is bounded
    /// and the bound was **measured**, not assumed: one submission's device time,
    /// and no more. Host work in the gap is hidden up to that and fully exposed
    /// past it. See
    /// [compute-graph-jit.md](../../docs/notes/compute-graph-jit.md).
    Async,
    /// Record every dispatchable node into one submission and wait once.
    ///
    /// **Refused.** A backend can only do this if it can be handed several
    /// dispatches to put in one command buffer, and the backend contract has no
    /// way to ask for that — `submit` records one run and hands it over. Until
    /// the contract grows a fused-submission shape, a runner that accepted
    /// `Batch` could only run it as something else, and both things it could run
    /// it as report a number for a schedule nobody asked for.
    Batch,
}

impl Policy {
    /// What to say when this policy cannot be carried out.
    fn refusal(&self) -> Option<GraphRefusal> {
        match self {
            Policy::Serial | Policy::Async => None,
            Policy::Batch => Some(GraphRefusal::PolicyUnsupported {
                policy: "batch",
                reason: "the backend contract has no way to record several dispatches into one \
                         submission, so a batch run could only be a different schedule wearing \
                         its name",
            }),
        }
    }
}

/// Runs a graph against a backend, under one policy.
pub struct Runner<'backend> {
    backend: &'backend dyn ParallelBackend,
    policy: Policy,
}

impl<'backend> Runner<'backend> {
    pub fn new(backend: &'backend dyn ParallelBackend, policy: Policy) -> Self {
        Runner { backend, policy }
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Run `graph` over `inputs`, and hand back **every value the graph has**.
    ///
    /// The result is the whole value table, the inputs first and then everything
    /// the nodes produced, and not the last node's outputs, on purpose. What a
    /// caller returns from the function it compiled into this graph is the
    /// *function's* business, and the caller is the thing that knows it:
    /// [`Graph::returns`] is where that answer is recorded. A runner that picked
    /// a node would be making that decision for them, and a graph with a dead tail
    /// would be unable to express its own return at all.
    ///
    /// The inputs are **taken** rather than borrowed, and that is not a
    /// convenience: a `Value` can be a submission, and a submission is consumed
    /// by its wait, so there is no honest way to copy one. Taking them also says
    /// what is true — the runner owns the graph's arguments for the length of the
    /// run and hands back what it produced.
    ///
    /// Every value is settled before this returns, so every id handed back names
    /// a buffer the device has written. Returning with a submission still in
    /// flight would be handing the caller a handle to memory that is still
    /// moving, and the only way to notice would be to read it.
    pub fn run(
        &self,
        graph: &Graph,
        inputs: Vec<Value<'backend>>,
    ) -> Result<Vec<Value<'backend>>, GraphRefusal> {
        if let Some(refusal) = self.policy.refusal() {
            return Err(refusal);
        }
        if inputs.len() != graph.input_count() {
            return Err(GraphRefusal::InputArity {
                node: usize::MAX,
                wanted: graph.input_count(),
                got: inputs.len(),
            });
        }
        let mut values: Vec<Value<'backend>> = inputs;

        for (index, node) in graph.nodes().iter().enumerate() {
            match node {
                Node::Kernel(kernel) => {
                    // A parallel fragment's parameters are its input slots
                    // followed by the loop index, so its buffer arity is one less
                    // than its shape. Checked here rather than left to the
                    // backend, because the backend's refusal would be about a
                    // run and this is about a graph.
                    let wanted = kernel.fragment.param_shape.flat_arity().saturating_sub(1);
                    if kernel.inputs.len() != wanted {
                        return Err(GraphRefusal::InputArity {
                            node: index,
                            wanted,
                            got: kernel.inputs.len(),
                        });
                    }
                    let mut slots = Vec::with_capacity(kernel.inputs.len());
                    for value in &kernel.inputs {
                        slots.push(
                            values
                                .get(*value)
                                .ok_or(GraphRefusal::UnknownValue {
                                    node: index,
                                    value: *value,
                                })?
                                .slot()?,
                        );
                    }
                    if let Some((first, other)) = ragged_host(&slots) {
                        return Err(GraphRefusal::RaggedInputs {
                            node: index,
                            first,
                            other,
                        });
                    }
                    // The count is read **after** the buffers, and separately from
                    // them, so a node that swapped the two is told which of the two
                    // roles it got wrong rather than that a shape did not match.
                    let count = match kernel.count {
                        Count::Constant(count) => count,
                        Count::Value(value) => values
                            .get(value)
                            .ok_or(GraphRefusal::UnknownValue { node: index, value })?
                            .as_count()?,
                    };

                    let produced = match self.policy {
                        Policy::Serial => self
                            .backend
                            .run(&kernel.fragment, &slots, count)
                            .map(|ids| Value::device_all(ids, count))
                            .map_err(|reason| GraphRefusal::Backend {
                                what: "dispatching a kernel node",
                                reason,
                            })?,
                        _ => {
                            let submission = self
                                .backend
                                .submit(&kernel.fragment, &slots, count)
                                .map_err(|reason| GraphRefusal::Backend {
                                what: "submitting a kernel node",
                                reason,
                            })?;
                            let ids = submission.outputs().to_vec();
                            Value::pending_all(submission, ids, count)
                        }
                    };
                    values.extend(produced);
                }
                Node::Native(native) => {
                    // A host call reads host data, so every argument is settled
                    // **and fetched** first. Settling is a wait; fetching is a
                    // transfer. Both are the price of host logic in the middle of
                    // a data path, and the reason a native node whose inputs are
                    // already host-side is the shape worth having.
                    for value in &native.inputs {
                        let slot = values.get_mut(*value).ok_or(GraphRefusal::UnknownValue {
                            node: index,
                            value: *value,
                        })?;
                        slot.to_host(self.backend)?;
                    }
                    let arguments: Vec<&[i64]> = native
                        .inputs
                        .iter()
                        .map(|value| Ok(values[*value].as_host()?))
                        .collect::<Result<_, GraphRefusal>>()?;

                    let produced = (native.call)(&arguments);
                    if produced.len() != native.outputs {
                        return Err(GraphRefusal::NativeArity {
                            node: index,
                            wanted: native.outputs,
                            got: produced.len(),
                        });
                    }
                    values.extend(produced.into_iter().map(Value::host));
                }
            }
        }

        for value in &mut values {
            value.settle()?;
        }
        Ok(values)
    }
}

/// Two host inputs of different lengths, if the slots have any.
///
/// A resident slot has no length to disagree with — its own run's business — so
/// this can only ever be about the host slots, and it is checked here so the
/// refusal can name both lengths rather than leaving the backend to say
/// "input 1 is shorter than the count".
fn ragged_host(slots: &[lichen_kernel_ir::BufferSlot<'_>]) -> Option<(usize, usize)> {
    let first = slots.iter().find_map(|slot| match slot {
        lichen_kernel_ir::BufferSlot::Host(data) => Some(data.len()),
        lichen_kernel_ir::BufferSlot::Resident(_) => None,
    })?;
    slots.iter().find_map(|slot| match slot {
        lichen_kernel_ir::BufferSlot::Host(data) if data.len() != first => {
            Some((first, data.len()))
        }
        _ => None,
    })
}
