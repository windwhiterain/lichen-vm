//! Running a graph, and the one thing a graph is allowed to decide.

use lichen_kernel_ir::{
    BatchExtent, BatchInput, BatchRefusal, BufferSlot, DeviceExtent, Dispatch, KernelFragment,
    LaunchSet, ParallelBackend, ResidentId, ScalarClass, ScalarLeaf,
};

use crate::graph::{Count, Graph, KernelNode, Node, Scalar};
use crate::refusal::GraphRefusal;
use crate::value::{BitsRefusal, Value};

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
    /// The submissions are **one** where [`Self::Serial`] is one per node, and the answers
    /// are the same numbers. A backend with no [`ParallelBackend::run_batch`] refuses by
    /// name rather than answering as some other schedule: a serial fallback is a schedule
    /// nobody asked for wearing this one's name.
    Batch,
}

/// Where one value of the table came from, which is what a later dispatch resolves it
/// against.
///
/// # Invariant
/// [`Self::Given`] covers everything the caller handed in — a number, host data, or a
/// buffer the backend already holds — and [`Self::Produced`] covers what a dispatch of
/// this run will issue. The arms differ in one fact only: whether the value has an address
/// this host can name yet.
#[derive(Clone, Copy)]
enum Source {
    /// The caller handed it in.
    Given,
    /// A dispatch of this run produces it.
    Produced { dispatch: usize, ordinal: usize },
}

/// One node resolved: what it dispatches and what it reads, before any of it has run.
///
/// # Invariant
/// Every field is a fact the graph itself states — a launch, an edge, an extent — and none
/// is an id a dispatch has yet issued, which is what lets a whole graph be resolved before
/// the first of it is recorded.
struct Planned<'a> {
    /// A node's kernel is its own launch set, callees included.
    launch: LaunchSet<'a>,
    inputs: Vec<BatchInput<'a>>,
    extent: BatchExtent,
    classes: &'a [ScalarClass],
    /// The element count each output of the node holds.
    counts: Vec<usize>,
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
        if inputs.len() != graph.input_count() {
            return Err(GraphRefusal::RunArity {
                wanted: graph.input_count(),
                got: inputs.len(),
            });
        }
        let mut values: Vec<Value<'backend>> = inputs;

        match self.policy {
            Policy::Serial | Policy::Async => self.run_node_by_node(graph, &mut values)?,
            Policy::Batch => self.run_in_one_submission(graph, &mut values)?,
        }

        for value in &mut values {
            value.settle()?;
        }
        Ok(values)
    }

    /// Dispatch each node as the walk reaches it.
    ///
    /// # Invariant
    /// One submission per node, whatever the policy: nothing is fused away, which is what
    /// makes the two the same count of submissions and a different host between them.
    fn run_node_by_node(
        &self,
        graph: &Graph,
        values: &mut Vec<Value<'backend>>,
    ) -> Result<(), GraphRefusal> {
        let mut sources = vec![Source::Given; values.len()];
        let mut allocated: Vec<usize> = Vec::new();
        let mut issued: Vec<Vec<ResidentId>> = Vec::new();

        for (index, node) in graph.nodes().iter().enumerate() {
            let Node::Kernel(kernel) = node;
            let planned = self.plan(index, kernel, values, &sources, &allocated)?;
            let slots = slots_of(&planned.inputs, &issued, index)?;
            let (produced, ids) = self.dispatch(index, &planned, &slots, &issued)?;
            allocated.push(planned.extent.allocated());
            issued.push(ids);
            sources.extend((0..produced.len()).map(|ordinal| Source::Produced {
                dispatch: index,
                ordinal,
            }));
            values.extend(produced);
        }
        Ok(())
    }

    /// Record every node into one submission and wait once, as
    /// [`ParallelBackend::run_batch`] does it.
    ///
    /// # Invariant
    /// Nothing is dispatched while the graph is being resolved, so the buffers one node
    /// reads another's output through are the ones the backend issues as it records: the
    /// value table is the one [`Self::run_node_by_node`] builds, from a single call.
    fn run_in_one_submission(
        &self,
        graph: &Graph,
        values: &mut Vec<Value<'backend>>,
    ) -> Result<(), GraphRefusal> {
        let mut sources = vec![Source::Given; values.len()];
        let mut allocated: Vec<usize> = Vec::new();
        let mut planned: Vec<Planned<'_>> = Vec::with_capacity(graph.nodes().len());

        for (index, node) in graph.nodes().iter().enumerate() {
            let Node::Kernel(kernel) = node;
            let plan = self.plan(index, kernel, values, &sources, &allocated)?;
            allocated.push(plan.extent.allocated());
            sources.extend((0..plan.classes.len()).map(|ordinal| Source::Produced {
                dispatch: index,
                ordinal,
            }));
            planned.push(plan);
        }

        let batch: Vec<Dispatch<'_>> = planned
            .iter()
            .map(|plan| Dispatch {
                launch: &plan.launch,
                inputs: &plan.inputs,
                extent: plan.extent,
            })
            .collect();
        let issued = self
            .backend
            .run_batch(&batch)
            .map_err(|refusal| match refusal {
                BatchRefusal::Cannot => GraphRefusal::PolicyUnsupported {
                    policy: "batch",
                    reason: BatchRefusal::CANNOT,
                },
                BatchRefusal::Refused(reason) => GraphRefusal::Backend {
                    what: "recording the graph in one submission",
                    reason,
                },
            })?;

        // Built before the table grows: a host input borrows it.
        let produced: Vec<Vec<Value<'backend>>> = planned
            .iter()
            .enumerate()
            .map(|(index, plan)| {
                // One list per dispatch, in order, one id per declared output: another
                // count would mis-number the value table.
                let ids = issued
                    .get(index)
                    .expect("a batch answers with one list of ids per dispatch, in order");
                assert_eq!(
                    ids.len(),
                    plan.classes.len(),
                    "a dispatch answers with one id per output its fragment declares"
                );
                Value::device_all(ids.clone(), &plan.counts, plan.classes)
            })
            .collect();
        values.extend(produced.into_iter().flatten());
        Ok(())
    }

    /// One node resolved against the value table, under any policy.
    ///
    /// # Invariant
    /// Nothing here dispatches, so the same answer serves every policy: a value this run
    /// produces is named by where it will be rather than by an id nobody has issued, and a
    /// count is read from the number or from the address of a device buffer.
    fn plan<'a>(
        &self,
        index: usize,
        kernel: &'a KernelNode,
        values: &'a [Value<'backend>],
        sources: &[Source],
        allocated: &[usize],
    ) -> Result<Planned<'a>, GraphRefusal> {
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
        let mut inputs = Vec::with_capacity(kernel.inputs.len());
        for &value in &kernel.inputs {
            // Assembled here: what the value is, and only this
            // place knows who wanted it.
            inputs.push(match sources.get(value) {
                Some(Source::Produced { dispatch, ordinal }) => BatchInput::Produced {
                    dispatch: *dispatch,
                    ordinal: *ordinal,
                },
                // A value the table does not hold is not in `sources`
                // either, so this arm answers for both.
                _ => match values
                    .get(value)
                    .ok_or(GraphRefusal::UnknownValue { node: index, value })?
                {
                    Value::Host { class, data } => BatchInput::Host(*class, data),
                    Value::Device { id, .. } | Value::Pending { id, .. } => {
                        BatchInput::Resident(*id)
                    }
                    other => {
                        return Err(GraphRefusal::NotBufferData {
                            node: index,
                            value,
                            found: other.state(),
                        });
                    }
                },
            });
        }
        // The count is read after the buffers, so a swapped node is told
        // which role it got wrong.

        // The extent conversion happens here, not in `as_number`: it
        // belongs to whoever dispatches over `[0, count)`.
        let extent = match kernel.count {
            Count::Constant(count) => BatchExtent::Host(count),
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
                BatchExtent::Host(
                    usize::try_from(number).map_err(|_| GraphRefusal::CountNegative { number })?,
                )
            }
            // The number stays where the device wrote it: the node is
            // dispatched over a buffer element and the host reads nothing.
            Count::Device { value, index } => match sources.get(value) {
                // Every element the value holds is the capacity.
                Some(Source::Produced { dispatch, ordinal }) => BatchExtent::Produced {
                    dispatch: *dispatch,
                    ordinal: *ordinal,
                    index,
                    capacity: allocated.get(*dispatch).copied().unwrap_or(0),
                },
                _ => {
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
                    BatchExtent::Resident {
                        count: buffer,
                        index,
                        capacity: counted.count().unwrap_or(0),
                    }
                }
            },
        };

        Ok(Planned {
            launch: LaunchSet::single(&kernel.fragment)
                .with_leaves(leaves_of(index, kernel, values)?),
            inputs,
            extent,
            classes: &kernel.fragment.output_classes,
            counts: output_counts(&kernel.fragment, extent.allocated()),
        })
    }

    /// Dispatch one resolved node, and hand back its values and their ids.
    ///
    /// # Invariant
    /// The policies differ in one call: [`Policy::Serial`] hands the ids over waited for
    /// and [`Policy::Async`] hands a submission back in flight. A device-held extent has
    /// one entry in the contract and it waits, so under [`Policy::Async`] it is refused
    /// rather than submitted as a stall wearing a submission's name.
    fn dispatch(
        &self,
        index: usize,
        planned: &Planned<'_>,
        slots: &[BufferSlot<'_>],
        issued: &[Vec<ResidentId>],
    ) -> Result<(Vec<Value<'backend>>, Vec<ResidentId>), GraphRefusal> {
        let classes = planned.classes;
        match self.policy {
            Policy::Serial => {
                let over_device = |device: DeviceExtent| {
                    self.backend
                        .run_over_device_extent(&planned.launch, slots, device)
                        .map_err(|reason| GraphRefusal::Backend {
                            what: "dispatching a kernel node over a device-held count",
                            reason,
                        })
                };
                let ids = match planned.extent {
                    BatchExtent::Host(count) => self
                        .backend
                        .run(&planned.launch, slots, count)
                        .map_err(|reason| GraphRefusal::Backend {
                            what: "dispatching a kernel node",
                            reason,
                        })?,
                    BatchExtent::Resident {
                        count,
                        index,
                        capacity,
                    } => over_device(DeviceExtent {
                        count,
                        index,
                        capacity,
                    })?,
                    BatchExtent::Produced {
                        dispatch,
                        ordinal,
                        index,
                        capacity,
                    } => over_device(DeviceExtent {
                        count: issued
                            .get(dispatch)
                            .and_then(|ids| ids.get(ordinal))
                            .copied()
                            .expect("a node reads only what a dispatch before it issued"),
                        index,
                        capacity,
                    })?,
                };
                Ok((
                    Value::device_all(ids.clone(), &planned.counts, classes),
                    ids,
                ))
            }
            _ => {
                let BatchExtent::Host(count) = planned.extent else {
                    return Err(GraphRefusal::ExtentIsNotInFlight { node: index });
                };
                let submission =
                    self.backend
                        .submit(&planned.launch, slots, count)
                        .map_err(|reason| GraphRefusal::Backend {
                            what: "submitting a kernel node",
                            reason,
                        })?;
                let ids = submission.outputs().to_vec();
                Ok((
                    Value::pending_all(submission, ids.clone(), &planned.counts, classes),
                    ids,
                ))
            }
        }
    }
}

/// The element count each output of a node holds.
///
/// # Invariant
/// **A declared extent is the length, and the launch count is the length when the declaration
/// states none**: a count is what a fetch reads, so it must be the buffer's own length and not
/// the number of lanes that wrote it — a kernel that writes a whole row per lane holds more
/// elements than it has lanes.
fn output_counts(fragment: &KernelFragment, launch: usize) -> Vec<usize> {
    let mut counts: Vec<usize> = fragment
        .roles
        .output_extents
        .iter()
        .map(|extent| extent.map(|extent| extent as usize).unwrap_or(launch))
        .collect();
    counts.resize(fragment.outputs, launch);
    counts
}

/// The runtime scalars a node fixes beside its extent, resolved against the value table.
///
/// # Invariant
/// **As many as [`KernelFragment::runtime_scalars`]**, and **each leaf read at its own
/// class**: a leaf declared `Float` whose value is an integer is
/// [`GraphRefusal::ScalarClassMismatch`], and so is the reverse, because the two are each
/// other's bits. This is where the rule is kept rather than in the recording, which cannot
/// see what the run will be handed.
fn leaves_of(
    node: usize,
    kernel: &KernelNode,
    values: &[Value<'_>],
) -> Result<Vec<ScalarLeaf>, GraphRefusal> {
    let wanted = kernel.fragment.runtime_scalars();
    if kernel.scalars.len() != wanted {
        return Err(GraphRefusal::ScalarArity {
            node,
            wanted,
            got: kernel.scalars.len(),
        });
    }
    kernel
        .scalars
        .iter()
        .enumerate()
        .map(|(ordinal, scalar)| {
            let class = scalar.class(&kernel.fragment, ordinal);
            match scalar {
                Scalar::Constant(leaf) => Ok(ScalarLeaf { class, ..*leaf }),
                Scalar::Value(value) => {
                    let bits = values
                        .get(*value)
                        .ok_or(GraphRefusal::UnknownValue {
                            node,
                            value: *value,
                        })?
                        .bits_at(class)
                        .map_err(|refused| match refused {
                            BitsRefusal::NotANumber(found) => GraphRefusal::ScalarNotANumber {
                                node,
                                value: *value,
                                found,
                            },
                            BitsRefusal::Class(found) => GraphRefusal::ScalarClassMismatch {
                                node,
                                value: *value,
                                leaf: class,
                                found,
                            },
                        })?;
                    Ok(ScalarLeaf { class, bits })
                }
            }
        })
        .collect()
}

/// The slots a resolved node's already-issued inputs are read through.
///
/// # Invariant
/// A [`BatchInput::Produced`] is a buffer this run recorded and waited for, and its id is
/// where it said it would be: the graph resolves an output to a position and this is the
/// one place that position becomes an address.
fn slots_of<'a>(
    inputs: &'a [BatchInput<'_>],
    issued: &'a [Vec<ResidentId>],
    node: usize,
) -> Result<Vec<BufferSlot<'a>>, GraphRefusal> {
    let issued = |dispatch: usize, ordinal: usize| -> Result<ResidentId, GraphRefusal> {
        issued
            .get(dispatch)
            .and_then(|ids| ids.get(ordinal))
            .copied()
            .ok_or(GraphRefusal::UnknownValue {
                node,
                value: ordinal,
            })
    };
    inputs
        .iter()
        .map(|input| match input {
            BatchInput::Host(_, data) => Ok(BufferSlot::Host(data)),
            BatchInput::Resident(id) => Ok(BufferSlot::Resident(*id)),
            BatchInput::Produced { dispatch, ordinal } => {
                issued(*dispatch, *ordinal).map(BufferSlot::Resident)
            }
        })
        .collect()
}
