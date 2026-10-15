//! The graph: nodes, and the edges between them.

use lichen_kernel_ir::{KernelFragment, ScalarClass, ScalarLeaf};

use crate::refusal::FragmentShape;

/// One value a node produces, named by its position in the graph's value table.
///
/// # Invariant
/// A number, not a reference: the graph is built by recording an evaluation that already
/// happened, and by then every operand is a `NodeId` the VM allocated — a stable integer
/// that outlives the arena a reference would point into.
pub type ValueId = usize;

/// A DAG of kernel launches and host calls.
///
/// # Invariant
/// Nodes are in evaluation order and a node's inputs are always values earlier nodes
/// produced, so the vector *is* a topological order: there is no sort, no cycle check and
/// no way to write a cycle, which is why nothing at run time checks for one.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    nodes: Vec<Node>,
    /// How many values the graph takes as arguments.
    inputs: usize,
    /// How many values exist in total: the inputs plus everything the nodes produced.
    ///
    /// # Invariant
    /// An edge naming a value at or past this is caught by [`Self::push`], not by an
    /// out-of-bounds index at run time.
    values: usize,
    /// Which values the function this graph was compiled from returns.
    ///
    /// # Invariant
    /// `None` is "nobody has said", not "returns nothing": a caller reads it as take
    /// everything, and an empty `Vec` is the real answer for a unit. Meaning it by
    /// forgetting to record would be indistinguishable from meaning it.
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
    /// # Invariant
    /// This is the function's own return, which is why a runner does not choose one: a
    /// graph whose tail is dead cannot express its return if "the tail" is the rule.
    /// Recording twice is refused rather than merged — the second opinion quietly winning
    /// is a wrong answer wearing a working graph.
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
    /// # Invariant
    /// A caller reading `None` takes every value the graph has, which is the same answer a
    /// recorded return gives when the function returns all of them.
    pub fn returns(&self) -> Option<&[ValueId]> {
        self.returns.as_deref()
    }

    /// Append a node, and name the values it produces.
    ///
    /// # Invariant
    /// The rest are consecutive after the returned first, so three outputs are `first`,
    /// `first + 1`, `first + 2`. Two things are refused here rather than at run time: an
    /// edge naming a value this graph has not produced — how a cycle would be written —
    /// and an output count that disagrees with the node.
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

    /// Replace a kernel node's `fragment`, leaving the rest of the graph as it was.
    ///
    /// # Invariant
    /// A replacement's `inputs`, `outputs` and `param_shape.flat_arity()` must equal the
    /// node's: those are the facts the graph's edges, its value numbering and a launch's
    /// scalar leaves were built against, and every other field is the body's own. Nothing
    /// else is touched, so the value count, the return, the edges and the node order are
    /// the same afterwards.
    pub fn replace_kernel(
        &mut self,
        node: usize,
        fragment: KernelFragment,
    ) -> Result<(), crate::GraphRefusal> {
        use crate::GraphRefusal;
        let nodes = self.nodes.len();
        let held = self
            .nodes
            .get_mut(node)
            .ok_or(GraphRefusal::NoSuchNode { node, nodes })?;
        let kind = held.kind();
        let Some(slot) = held.fragment_mut() else {
            return Err(GraphRefusal::NotAKernelNode { node, found: kind });
        };
        let (had, got) = (FragmentShape::of(slot), FragmentShape::of(&fragment));
        if had != got {
            return Err(GraphRefusal::ShapeDisagrees { node, had, got });
        }
        *slot = fragment;
        Ok(())
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
/// # Invariant
/// One kind, and the enum is where a second would go: the host call that used to be the
/// other was a bare `fn` pointer, which made a user closure inexpressible. A closure is a
/// compiled artifact like any other, so it lowers to a fragment and is dispatched like
/// one, and one kind of node means one set of rules about a node's environment.
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

    /// What kind of node this is, for a refusal that has to name it.
    fn kind(&self) -> &'static str {
        match self {
            Node::Kernel(_) => "a kernel node",
        }
    }

    /// The fragment this node dispatches, if this kind of node dispatches one.
    ///
    /// # Invariant
    /// An `Option` because the node enum is where a second kind would go: a kind that
    /// carries no fragment is refused by name rather than being unreachable.
    fn fragment_mut(&mut self) -> Option<&mut KernelFragment> {
        match self {
            Node::Kernel(kernel) => Some(&mut kernel.fragment),
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
    /// The runtime scalars beside the extent, in the parameter's own scalar-leaf order.
    ///
    /// # Invariant
    /// Exactly [`KernelFragment::runtime_scalars`] entries, leaf `k` being the parameter's
    /// `(k + 1)`-th scalar leaf: the extent is the [`Self::count`] and the index is the
    /// lane's. **How many** leaves a parameter may declare is `record_launch`'s rule and
    /// not this field's.
    pub scalars: Vec<Scalar>,
}

/// One runtime scalar a dispatch fixes beside its extent.
///
/// # Invariant
/// The same two arms [`Count`] has for a host-held number, and for the same reason: a
/// scalar is routinely read from the graph's parameter, and collapsing the literal into
/// an edge would invent a value the graph never had. There is no device arm, because a
/// scalar is read when the node runs and not out of a buffer on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scalar {
    /// A number the build already had.
    Constant(ScalarLeaf),
    /// A value in the table, read when the node runs.
    Value(ValueId),
}

impl Scalar {
    /// This scalar with a value edge renumbered by `map`, and `None` where the map
    /// cannot place it.
    ///
    /// # Invariant
    /// A constant is its own renumbering: a literal the build had names no value, so
    /// there is nothing a rebuilt graph's numbering could move.
    pub fn map_value(self, map: impl Fn(usize) -> Option<usize>) -> Option<Self> {
        match self {
            Self::Constant(leaf) => Some(Self::Constant(leaf)),
            Self::Value(value) => map(value).map(Self::Value),
        }
    }

    /// The class this scalar's parameter leaf declared, `ordinal` counting the node's
    /// runtime scalars from zero.
    ///
    /// # Invariant
    /// Read from the fragment rather than carried beside the value: the class is a
    /// property of the parameter field the leaf sits at. `ordinal + 1` because leaf `0`
    /// of `param_shape` is the extent, which is the count rather than one of these, and
    /// `ordinal` is below `runtime_scalars` because [`crate::Runner`] refuses a node
    /// whose list is another length.
    pub fn class(&self, fragment: &KernelFragment, ordinal: usize) -> ScalarClass {
        fragment.param_shape.leaf_classes()[ordinal + 1]
    }
}

/// The extent of a dispatch: a number the build already had, or one the value table
/// holds.
///
/// # Invariant
/// A count is a value, because a program's count is: a kernel's extent routinely depends
/// on data, and a graph that could only take a build-time count would be rebuilt for every
/// run. `Constant` is not a wart — it is the case where the build already had the answer,
/// and collapsing it into a value would mean inventing a node that produces a number for
/// free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// `[0, count)`, decided while the graph was built.
    Constant(usize),
    /// A value in the table, read when the node runs.
    ///
    /// # Invariant
    /// Never pending: a number is not produced by a device, so a count edge owes no wait.
    Value(ValueId),
    /// One element of a buffer value in the table, read on the device when the node runs.
    ///
    /// # Invariant
    /// **Where the number is, not the number**: the node is dispatched without the host ever
    /// reading it, which is the whole point of this arm — the other two name a number the host
    /// has. `index` is the element, because a counting kernel writes its count beside whatever
    /// else it produced rather than instead of it.
    Device { value: ValueId, index: usize },
}
