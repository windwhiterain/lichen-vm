//! The graph: nodes, and the edges between them.

use lichen_kernel_ir::KernelFragment;

/// One value a node produces, named by its position in the graph's value table.
///
/// **A value number, not a reference.** Edges are numbers because the graph is
/// built by recording an evaluation that has already happened, and by then every
/// operand is a `NodeId` the VM allocated — a stable integer that will outlive
/// whatever arena the run lived in, which a reference into that arena would not.
pub type ValueId = usize;

/// A DAG of kernel launches and host calls.
///
/// Nodes are in evaluation order and a node's inputs are **always** values
/// produced by earlier nodes, so the vector *is* a topological order and there is
/// no sort, no cycle check and no way to write a cycle. A graph that is
/// malformed in the ways a graph can be malformed cannot be constructed through
/// this type, which is why nothing at run time has to check for one.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    nodes: Vec<Node>,
    /// How many values the graph takes as arguments.
    inputs: usize,
    /// How many values exist in total: the inputs, plus everything the nodes have
    /// produced. An edge naming a value at or past this is a mistake in whoever
    /// built the graph, and it is caught by [`Self::push`] rather than left to
    /// index out of bounds at run time.
    values: usize,
    /// Which values the function this graph was compiled from returns.
    ///
    /// [`None`] until [`Self::returning`] says so, and **`None` is not "returns
    /// nothing"** — it is "nobody has said", which a caller reads as *take
    /// everything*. The distinction matters because an empty `Vec` is a real
    /// answer (a function that returns a unit) and silently meaning it by
    /// forgetting to record would make a forgotten call indistinguishable from a
    /// deliberate one.
    returns: Option<Vec<ValueId>>,
}

impl Graph {
    /// A graph taking `inputs` values, producing nothing yet.
    pub fn with_inputs(inputs: usize) -> Self {
        Graph {
            nodes: Vec::new(),
            inputs,
            values: inputs,
            returns: None,
        }
    }

    /// How many values this graph takes as arguments.
    pub fn input_count(&self) -> usize {
        self.inputs
    }

    /// Record which values the source function returns.
    ///
    /// **This is the function's own return, and it is why a runner does not get
    /// to choose one.** The value a caller ends up with is a decision about what
    /// the program computes, not about what the last node happened to be — and a
    /// graph whose tail is dead cannot express its return at all if "the tail" is
    /// the rule. So the builder records it here, once, from the function it
    /// compiled.
    ///
    /// Recording twice is refused rather than merged: a lowering that answers this
    /// question twice has two opinions about what its own function returns, and
    /// the second one quietly winning is a wrong answer wearing a working graph.
    pub fn returning(&mut self, values: Vec<ValueId>) -> Result<(), crate::GraphRefusal> {
        use crate::GraphRefusal;
        if let Some(recorded) = &self.returns {
            return Err(GraphRefusal::ReturnAlreadyRecorded {
                recorded: recorded.len(),
            });
        }
        if let Some(bad) = values.iter().copied().find(|value| *value >= self.values) {
            return Err(GraphRefusal::UnknownValue {
                node: usize::MAX,
                value: bad,
            });
        }
        self.returns = Some(values);
        Ok(())
    }

    /// What the source function returns, or `None` if nobody recorded it.
    ///
    /// A caller reading `None` takes **every** value the graph has. That is the
    /// same answer a recorded return gives when the function returns all of them,
    /// so an unrecorded graph is the permissive one rather than a broken one.
    pub fn returns(&self) -> Option<&[ValueId]> {
        self.returns.as_deref()
    }

    /// Append a node, and name the values it produces.
    ///
    /// Returns the first of them; the rest are consecutive after it, so a node
    /// with three outputs yields `first`, `first + 1`, `first + 2`.
    ///
    /// Two things are refused here rather than at run time, because both are
    /// mistakes in the graph rather than in the program that built it and both
    /// are cheaper to find now: an **edge that names a value this graph has not
    /// produced yet**, which is how a cycle would be written, and an **output
    /// count that disagrees with the node** — a kernel produces
    /// `fragment.outputs`, so a caller that passes a different number is asking
    /// for value numbers that mean something else.
    pub fn push(&mut self, node: Node, outputs: usize) -> Result<ValueId, crate::GraphRefusal> {
        use crate::GraphRefusal;
        let declared = match &node {
            Node::Kernel(kernel) => kernel.fragment.outputs,
        };
        if declared != outputs {
            return Err(GraphRefusal::OutputCount {
                node: self.nodes.len(),
                declared,
                claimed: outputs,
            });
        }
        if let Some(bad) = node
            .inputs()
            .iter()
            .copied()
            .find(|value| *value >= self.values)
        {
            return Err(GraphRefusal::EdgeBeforeItsProducer {
                node: self.nodes.len(),
                value: bad,
                defined: self.values,
            });
        }
        let first = self.values;
        self.values += outputs;
        self.nodes.push(node);
        Ok(first)
    }

    /// The nodes, in evaluation order.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// How many values the graph defines in total.
    pub fn value_count(&self) -> usize {
        self.values
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// What one node of a graph is.
///
/// **One kind, and the enum is left as the place a second would go.** The graph
/// IR used to have two, a dispatch and a host call, and the host call was a bare
/// `fn` pointer. That is what made a user closure inexpressible, and it opened
/// a contradiction that outlived three attempts to settle it. The answer was not
/// a better pointer: it was that **a closure is a compiled artifact like any
/// other**, so it lowers to a fragment and is dispatched like one. One kind of
/// node means one set of rules about what a node's environment is, and nothing
/// a trait object could make unsound behind it.
#[derive(Debug, Clone)]
pub enum Node {
    /// A dispatch: one fragment over one index range.
    Kernel(KernelNode),
}

impl Node {
    /// The values this node reads, in the order its own kind numbers them.
    pub fn inputs(&self) -> &[ValueId] {
        match self {
            Node::Kernel(kernel) => &kernel.inputs,
        }
    }

    /// How many values this node produces.
    pub fn outputs(&self) -> usize {
        match self {
            Node::Kernel(kernel) => kernel.fragment.outputs,
        }
    }
}

/// A dispatch, over the buffers its inputs resolve to.
#[derive(Debug, Clone, PartialEq)]
pub struct KernelNode {
    pub fragment: KernelFragment,
    /// The values this reads, in the fragment's own input order.
    pub inputs: Vec<ValueId>,
    /// The index range `[0, count)`.
    pub count: Count,
}

/// The extent of a dispatch: a number the build already had, or one the value
/// table holds.
///
/// **A count is a value, because a program's count is.** How many elements a
/// kernel covers routinely depends on data — a length off a `collect`, a size
/// the host computed — and a graph that could only be given a build-time count
/// would have to be rebuilt for every run, which is the same as not having a
/// graph. [`crate::Value::Native`] is what makes the edge expressible.
///
/// `Constant` is not a wart on that. It is the one case where the build already
/// had the answer, and collapsing it into a value would mean inventing a **third
/// kind of node** — one that produces a number for free, doing no work — which
/// is exactly the kind of node the two-kind rule exists to keep out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// `[0, count)`, decided while the graph was built.
    Constant(usize),
    /// A value in the table, read when the node runs.
    ///
    /// **Never pending.** A number is not produced by a device, so a count edge
    /// never needs the wait that a buffer edge does not need but might.
    Value(ValueId),
}
