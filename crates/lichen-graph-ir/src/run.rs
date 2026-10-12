//! Running a graph, and the one thing a graph is allowed to decide.

use lichen_kernel_ir::ParallelBackend;

use crate::graph::{Count, Graph, Node};
use crate::refusal::GraphRefusal;
use crate::value::Value;

/// When the host submits, and when it waits.
///
/// # Invariant
/// The only knob, deliberately not in the graph: a graph says what precedes what, never
/// when the host may notice a finish, or correctness would depend on where somebody put a
/// keyword and the builder and the runner would disagree about the same graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Submit and wait for every node, one at a time.
    ///
    /// # Invariant
    /// What a plain kernel run does today: it buys nothing, and it is here so a graph run
    /// with no choice behaves exactly as the un-graphed code did.
    Serial,
    /// Submit a node as soon as it is recordable, and wait only where data is needed.
    ///
    /// # Invariant
    /// The submissions are the same count as [`Self::Serial`] — nothing is fused away — but
    /// the host is not idle between them. What it collects is bounded, and the bound was
    /// measured: one submission's device time, no more.
    Async,
    /// Record every dispatchable node into one submission and wait once.
    ///
    /// # Invariant
    /// Refused: the backend contract has no way to ask for several dispatches in one command
    /// buffer — `submit` records one run and hands it over — so a `Batch` a runner accepted
    /// could only run as something else, reporting a number for a schedule nobody asked for.
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

/// The index range one node dispatches over, and where the number behind it is.
///
/// # Invariant
/// Two arms and no middle: a node's extent is either a number this host holds or one it does
/// not, and a runner that could not say which would be the thing that decides. [`Self::allocated`]
/// is the length every run is sized for, which for a device-held number is a bound rather than
/// the count.
enum Extent {
    /// `[0, count)`, a number the host holds.
    Host(usize),
    /// `[0, device)`, a number only the device holds.
    Device(lichen_kernel_ir::DeviceExtent),
}

impl Extent {
    /// How many elements a run over this extent is allocated for.
    ///
    /// # Invariant
    /// The device arm's `capacity`, because the number deciding how much of those buffers the
    /// shader reaches is not one this host has — which is also what the returned ids' count
    /// means, so a fetch cannot be given a length the allocation does not cover.
    fn allocated(&self) -> usize {
        match self {
            Extent::Host(count) => *count,
            Extent::Device(device) => device.capacity,
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

    /// Run `graph` over `inputs`, and hand back every value the graph has.
    ///
    /// # Invariant
    /// The result is the whole value table — the inputs, then everything the nodes
    /// produced — not the last node's outputs: what the function returns is
    /// [`Graph::returns`]' answer. The inputs are taken, because a `Value` can be a
    /// submission and a wait consumes it. Every value is settled before this returns, so
    /// every id names a buffer the device has written.
    pub fn run(
        &self,
        graph: &Graph,
        inputs: Vec<Value<'backend>>,
    ) -> Result<Vec<Value<'backend>>, GraphRefusal> {
        if let Some(refusal) = self.policy.refusal() {
            return Err(refusal);
        }
        if inputs.len() != graph.input_count() {
            return Err(GraphRefusal::RunArity {
                wanted: graph.input_count(),
                got: inputs.len(),
            });
        }
        let mut values: Vec<Value<'backend>> = inputs;

        for (index, node) in graph.nodes().iter().enumerate() {
            match node {
                Node::Kernel(kernel) => {
                    // The given buffers are checked against the count the fragment
                    // reads, derived by the emitter.

                    // Not `param_shape`: a parallel fragment's shape is `(config,
                    // index)` however many buffers it reads.
                    let wanted = kernel.fragment.inputs;
                    if kernel.inputs.len() != wanted {
                        return Err(GraphRefusal::InputArity {
                            node: index,
                            wanted,
                            got: kernel.inputs.len(),
                        });
                    }
                    let mut slots = Vec::with_capacity(kernel.inputs.len());
                    for &value in &kernel.inputs {
                        // Assembled here: `slot` says what the value is, and only this
                        // place knows who wanted it.
                        slots.push(
                            values
                                .get(value)
                                .ok_or(GraphRefusal::UnknownValue { node: index, value })?
                                .slot()
                                .map_err(|found| GraphRefusal::NotBufferData {
                                    node: index,
                                    value,
                                    found,
                                })?,
                        );
                    }
                    if let Some((first, other)) = ragged_host(&slots) {
                        return Err(GraphRefusal::RaggedInputs {
                            node: index,
                            first,
                            other,
                        });
                    }
                    // The count is read after the buffers, so a swapped node is told
                    // which role it got wrong.

                    // The extent conversion happens here, not in `as_number`: it
                    // belongs to whoever dispatches over `[0, count)`.
                    let extent = match kernel.count {
                        Count::Constant(count) => Extent::Host(count),
                        Count::Value(value) => {
                            let number = values
                                .get(value)
                                .ok_or(GraphRefusal::UnknownValue { node: index, value })?
                                .as_number()
                                .map_err(|found| GraphRefusal::CountNotANumber {
                                    node: index,
                                    value,
                                    found,
                                })?;
                            Extent::Host(
                                usize::try_from(number)
                                    .map_err(|_| GraphRefusal::CountNegative { number })?,
                            )
                        }
                        // The number stays where the device wrote it: the node is
                        // dispatched over a buffer element and the host reads nothing.
                        Count::Device { value, index } => {
                            let counted = values
                                .get(value)
                                .ok_or(GraphRefusal::UnknownValue { node: index, value })?;
                            let buffer = counted.device_buffer().map_err(|found| {
                                GraphRefusal::ExtentNotADevice {
                                    node: index,
                                    value,
                                    found,
                                }
                            })?;
                            Extent::Device(lichen_kernel_ir::DeviceExtent {
                                count: buffer,
                                index,
                                // Every element the value holds: how many of them the
                                // device will use is its own news.
                                capacity: counted.count().unwrap_or(0),
                            })
                        }
                    };

                    // A node's kernel is its own launch set, callees included.
                    let launch = lichen_kernel_ir::LaunchSet::single(&kernel.fragment);
                    let classes = &kernel.fragment.output_classes;
                    let produced = match self.policy {
                        Policy::Serial => {
                            let ids = match extent {
                                Extent::Host(count) => self
                                    .backend
                                    .run(&launch, &slots, count)
                                    .map_err(|reason| GraphRefusal::Backend {
                                        what: "dispatching a kernel node",
                                        reason,
                                    })?,
                                Extent::Device(device) => self
                                    .backend
                                    .run_over_device_extent(&launch, &slots, device)
                                    .map_err(|reason| GraphRefusal::Backend {
                                        what: "dispatching a kernel node over a device-held count",
                                        reason,
                                    })?,
                            };
                            Value::device_all(ids, extent.allocated(), classes)
                        }
                        _ => {
                            let Extent::Host(count) = extent else {
                                return Err(GraphRefusal::ExtentIsNotInFlight { node: index });
                            };
                            let submission =
                                self.backend
                                    .submit(&launch, &slots, count)
                                    .map_err(|reason| GraphRefusal::Backend {
                                        what: "submitting a kernel node",
                                        reason,
                                    })?;
                            let ids = submission.outputs().to_vec();
                            Value::pending_all(submission, ids, count, classes)
                        }
                    };
                    values.extend(produced);
                }
            }
        }

        for value in &mut values {
            value.settle()?;
        }
        Ok(values)
    }
}

/// Two host inputs of different payload lengths, if the slots have any.
///
/// # Invariant
/// A resident slot has no length to disagree with, so this is about host slots. The
/// comparison is on payload bytes, the one length a slot can answer without the fragment:
/// two slots of different classes with their width ratio between them compare equal here,
/// which the class check refuses before a dispatch.
fn ragged_host(slots: &[lichen_kernel_ir::BufferSlot<'_>]) -> Option<(usize, usize)> {
    let length = |slot: &lichen_kernel_ir::BufferSlot<'_>| match slot {
        lichen_kernel_ir::BufferSlot::Host(data) => Some(data.len()),
        lichen_kernel_ir::BufferSlot::Resident(_) => None,
    };
    let first = slots.iter().find_map(length)?;
    slots.iter().find_map(|slot| match length(slot) {
        Some(length) if length != first => Some((first, length)),
        _ => None,
    })
}
