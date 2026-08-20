use bumpalo::Bump;
use slotmap::{SlotMap, new_key_type};
use stacksafe::stacksafe;
use std::alloc::Layout;
use std::collections::{HashMap, HashSet};
use std::ptr;

use crate::utils::disjoint::{self, DisjointNode, Meta};

pub trait Program: Sized + Copy {
    type Value: ValueExt;
    type Operator: OperatorExt<Self>;
}

pub trait ValueExt: Copy {
    fn is_ptr(&self) -> bool;
    /// Avaliable if [`Self::is_ptr()`].
    fn ptr(&self) -> *const [u8] {
        unreachable!()
    }
    /// Avaliable if [`Self::is_ptr()`].
    fn set_ptr(&mut self, _ptr: *const [u8]) {
        unreachable!()
    }
    /// Avaliable if [`Self::is_ptr()`].
    fn alignment() -> usize {
        unreachable!()
    }
}

pub trait OperatorExt<P: Program>: Copy {
    fn run(
        &self,
        operand: Value<P>,
        block: &mut Block,
        nodes: &SlotMap<NodeId, Node<P>>,
    ) -> Value<P>;
}

#[derive(Clone, Copy)]
pub enum Value<P: Program> {
    Ext(P::Value),
    Array(*const [NodeId]),
    Function(Function),
    USize(usize),
    None,
    Parameterized,
}

#[derive(Clone, Copy)]
pub enum Operator<P: Program> {
    Ext(P::Operator),
    Index,
    /// Call the function value at `operand[0]` with the argument node
    /// `operand[1]` (the operand is an array of two node ids), caching the
    /// result as this node's value.
    Call,
}

#[derive(Clone, Copy)]
pub struct Operation<P: Program> {
    pub operator: Operator<P>,
    pub operand: Option<NodeId>,
}

new_key_type! {pub struct NodeId;}
new_key_type! {pub struct BlockId;}

pub struct Block {
    pub arena: Bump,
    pub parent: Option<BlockId>,
    pub children: Vec<BlockId>,
    pub nodes: Vec<NodeId>,
}

impl Block {
    pub const RETURN_IDX: usize = 0;
}

#[derive(Clone, Copy)]
pub struct Function {
    pub nodes: *const [NodeId],
    pub r#return: NodeId,
    pub parameter: NodeId,
}

impl Function {
    /// Clone the body into `target` with `argument` bound to the parameter
    /// and return the evaluated result.
    ///
    /// The body is a static template: it is neither mutated nor consumed,
    /// so the function stays callable.  Because the body always exists, the
    /// clone only copies what depends on the parameter — unevaluated
    /// operation nodes and nodes with [`Node::parameterized_deep`] set —
    /// and references concrete nodes in place.  References to the parameter
    /// map to the argument node; the clone's return node is evaluated
    /// against the argument before returning.  `target` is the block the
    /// clone is created in — for a call operator, the call node's own
    /// block.  Only the return node is evaluated: container returns stay
    /// unevaluated so the caller can consume them lazily.
    pub fn call<P: Program>(
        &self,
        module: &mut Module<P>,
        target: BlockId,
        argument: NodeId,
    ) -> Value<P> {
        let nodes = unsafe { &*self.nodes };
        debug_assert!(nodes.contains(&self.r#return));
        debug_assert!(nodes.contains(&self.parameter));
        let clone = module.clone_reachable(self.r#return, target, self.parameter, argument, nodes);
        module.evaluate_node(clone, Some(target))
    }
}

pub struct Node<P: Program> {
    pub value: Option<Value<P>>,
    pub operation: Option<Operation<P>>,
    /// Which block the node lives in.
    /// # Contract
    /// - Only one node can be referenced from parent block
    /// - Referencing a node whose block was released is a panic
    pub block: BlockId,
    /// Detect circular recursion.
    pub visiting: bool,
    /// Any node in self's reachable subtree has a [`Value::Parameterized`], computed during [`Module::evaluate_node_deep`].
    pub parameterized_deep: Option<bool>,
    /// Disjoint-set metadata for node equality classes, maintained by
    /// [`Module::add_equality`] and [`Module::root_node`].
    pub set: Meta<NodeId>,
}

impl<P: Program> DisjointNode for Node<P> {
    type Key = NodeId;
    fn set(&self) -> &Meta<NodeId> {
        &self.set
    }
    fn set_mut(&mut self) -> &mut Meta<NodeId> {
        &mut self.set
    }
}

pub struct Module<P: Program> {
    pub nodes: SlotMap<NodeId, Node<P>>,
    pub blocks: SlotMap<BlockId, Block>,
}

impl<P: Program> Default for Module<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Module<P> {
    pub fn new() -> Self {
        Module {
            nodes: SlotMap::with_key(),
            blocks: SlotMap::with_key(),
        }
    }

    pub fn add_block(&mut self, parent: Option<BlockId>) -> BlockId {
        let id = self.blocks.insert(Block {
            arena: Bump::new(),
            parent,
            children: Vec::new(),
            nodes: Vec::new(),
        });
        if let Some(parent) = parent {
            self.blocks[parent].children.push(id);
        }
        id
    }

    pub fn add_node(
        &mut self,
        block: BlockId,
        operation: Option<Operation<P>>,
        value: Option<Value<P>>,
    ) -> NodeId {
        let id = self.nodes.insert(Node {
            value,
            operation,
            block,
            visiting: false,
            parameterized_deep: None,
            set: Meta::default(),
        });
        disjoint::make_set(&mut self.nodes, id);
        self.blocks[block].nodes.push(id);
        id
    }

    /// Create a node whose value is a [`Function`] over the template rooted
    /// at `ret` with parameter `param`.  `nodes` is the template's scope —
    /// the only nodes that may reference the parameter — and is copied into
    /// `block`'s arena so the function value relocates with the block.
    pub fn add_function(
        &mut self,
        block: BlockId,
        ret: NodeId,
        param: NodeId,
        nodes: &[NodeId],
    ) -> NodeId {
        let function = Function {
            nodes: self.copy_ids(block, nodes),
            r#return: ret,
            parameter: param,
        };
        self.add_node(block, None, Some(Value::Function(function)))
    }

    /// Declare that `a` and `b` are equal, merging their equivalence classes,
    /// and return the merged class's representative.
    ///
    /// Every node starts in its own singleton class; equality is transitive,
    /// so this is a disjoint-set union over nodes.  The class metadata lives
    /// in the nodes themselves, so evaluation and compaction leave classes
    /// intact — a class whose members were released with their block panics
    /// on the next access.
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        disjoint::union(&mut self.nodes, a, b)
    }

    /// Return the representative of `node`'s equivalence class, compacting
    /// the path from `node` to it so later calls are shorter.
    pub fn root_node(&mut self, node: NodeId) -> NodeId {
        disjoint::find(&mut self.nodes, node)
    }

    /// If `id` lives in a child of `referer`, its a block root, [`Self::evaluate_block`] will be called on it.
    pub fn evaluate_node(&mut self, node: NodeId, referer: Option<BlockId>) -> Value<P> {
        let block = self.nodes[node].block;
        debug_assert!(
            self.blocks.contains_key(block),
            "node {node:?} references released block {block:?}"
        );
        if let Some(referer) = referer
            && self.blocks[block].parent == Some(referer)
        {
            return self.evaluate_block(block, node);
        }
        if let Some(value) = self.nodes[node].value {
            return value;
        }
        if self.nodes[node].visiting {
            unreachable!("cycle detected: node {node:?} is being evaluated");
        }
        self.nodes[node].visiting = true;
        let operation = self.nodes[node].operation.unwrap();
        let value = match operation.operator {
            Operator::Index => {
                let Some(operands) = operation.operand else {
                    unreachable!("Index expects an operand array node")
                };
                let Value::Array(ptr) = self.evaluate_node(operands, Some(block)) else {
                    unreachable!("Index operand must be an array of [array, index]")
                };
                let operands = unsafe { &*ptr };
                let Value::USize(index) = self.evaluate_node(operands[1], Some(block)) else {
                    unreachable!("Index needs a USize index node")
                };
                let Value::Array(ptr) = self.evaluate_node(operands[0], Some(block)) else {
                    unreachable!("Index target must be an array")
                };
                let array = unsafe { &*ptr };
                self.evaluate_node(array[index], Some(block))
            }
            Operator::Ext(ext) => {
                let operand = match operation.operand {
                    Some(operand) => {
                        let value = self.evaluate_node_deep(operand, Some(block));
                        if self.nodes[operand].parameterized_deep.unwrap(){
                            Value::Parameterized
                        }else{
                            value
                        }
                    },
                    None => Value::None,
                };
                let Module { nodes, blocks, .. } = self;
                ext.run(operand, &mut blocks[block], nodes)
            }
            Operator::Call => {
                let Some(operands) = operation.operand else {
                    unreachable!("Call expects an operand array node")
                };
                let Value::Array(ptr) = self.evaluate_node(operands, Some(block)) else {
                    unreachable!("Call operand must be an array of [function, argument]")
                };
                let operands = unsafe { &*ptr };
                let Value::Function(function) = self.evaluate_node(operands[0], Some(block)) else {
                    unreachable!("Call target must be a function value")
                };
                function.call(self, block, operands[1])
            }
        };
        self.nodes[node].value = Some(value);
        self.nodes[node].visiting = false;
        value
    }

    /// Run [`Self::evaluate_node`] for all nodes in the reachable subtree of `id`.
    #[stacksafe]
    pub fn evaluate_node_deep(&mut self, node: NodeId, current: Option<BlockId>) -> Value<P> {
        let value = self.evaluate_node(node, current);
        if let Value::Array(array) = value {
            let block = self.nodes[node].block;
            for &id in unsafe { &*array } {
                self.evaluate_node_deep(id, Some(block));
            }
        }
        let parameterized = matches!(value, Value::Parameterized)
            || matches!(
                value,
                Value::Array(array)
                    if unsafe { &*array }.iter().any(|&id| self.nodes[id].parameterized_deep == Some(true))
            )
            || self.nodes[node].operation.is_some_and(|op| {
                op.operand.is_some_and(|operand| {
                    // Operands are static graph edges, not value-reachable, so a
                    // nested block release may have dropped the node by now.
                    self.nodes
                        .get(operand)
                        .is_some_and(|node| node.parameterized_deep == Some(true))
                })
            });
        self.nodes[node].parameterized_deep = Some(parameterized);
        value
    }

    fn evaluate_block(&mut self, block: BlockId, root: NodeId) -> Value<P> {
        debug_assert_eq!(self.nodes[root].block, block);
        self.evaluate_node_deep(root, None);
        let value = self.move_to_parent(root).expect("evaluated return node");
        self.release_block(block);
        value
    }

    /// Copy `ids` into `block`'s arena and return them as a value slice.
    fn copy_ids(&self, block: BlockId, ids: &[NodeId]) -> *const [NodeId] {
        let slice = self.blocks[block].arena.alloc_slice_copy(ids);
        ptr::slice_from_raw_parts(slice.as_ptr(), slice.len())
    }

    /// Copy `ext`'s payload bytes into `arena` and re-point it there,
    /// preserving the payload's alignment.
    fn copy_ext(mut ext: P::Value, arena: &Bump) -> Value<P> {
        if !ext.is_ptr() {
            return Value::Ext(ext);
        }
        let old = ext.ptr();
        let layout = Layout::from_size_align(old.len(), P::Value::alignment()).unwrap();
        let dst = arena.alloc_layout(layout);
        unsafe { ptr::copy_nonoverlapping(old as *const u8, dst.as_ptr(), old.len()) };
        ext.set_ptr(ptr::slice_from_raw_parts(dst.as_ptr(), old.len()));
        Value::Ext(ext)
    }

    /// Move a node into its parent block: re-point it, relocate any arena
    /// data it carries (array slices, function scopes, Ext payloads) into
    /// the parent's arena, and return the relocated value.
    ///
    /// The node may not hold a value yet (a static function template is
    /// unevaluated) — in that case only the block mapping is updated and
    /// `None` is returned.  Value edges are followed like the original
    /// contract: array elements and function scope members that still live
    /// in `block` move with it, so a function's template survives the
    /// closing block exactly like an array's elements.
    #[stacksafe]
    fn move_to_parent(&mut self, node: NodeId) -> Option<Value<P>> {
        let block = self.nodes[node].block;
        let parent = self.blocks[block].parent;
        let Some(parent) = parent else {
            return self.nodes[node].value;
        };
        self.nodes[node].block = parent;
        self.blocks[parent].nodes.push(node);
        let current = self.nodes[node].value;
        let value = current.map(|value| match value {
            Value::Array(array) => {
                let ids = unsafe { &*array };
                for &id in ids {
                    if self.nodes[id].block == block {
                        self.move_to_parent(id);
                    }
                }
                Value::Array(self.copy_ids(parent, ids))
            }
            Value::Function(function) => {
                // The template's nodes must outlive the closing block, so
                // the scope is mapped like an array slice: each member that
                // lives here moves into the parent.
                let ids = unsafe { &*function.nodes };
                for &id in ids {
                    if self.nodes[id].block == block {
                        self.move_to_parent(id);
                    }
                }
                Value::Function(Function {
                    nodes: self.copy_ids(parent, ids),
                    ..function
                })
            }
            Value::Ext(ext) => Self::copy_ext(ext, &self.blocks[parent].arena),
            value => value,
        });
        self.nodes[node].value = value;
        value
    }

    /// Clone the parameter-dependent parts of `root`'s reachable graph into
    /// `target`, mapping the parameter node onto `argument`, and return the
    /// clone of `root`.  `function_nodes` is the template's scope — the only
    /// nodes that may reference the parameter.  The body is a permanent
    /// template, so only unevaluated operation nodes and parameterized nodes
    /// are copied as fresh node ids; concrete nodes are referenced in place.
    /// The clone is unevaluated — its operand edges are carried along so it
    /// resolves once the argument is bound.
    fn clone_reachable(
        &mut self,
        root: NodeId,
        target: BlockId,
        param: NodeId,
        argument: NodeId,
        function_nodes: &[NodeId],
    ) -> NodeId {
        let members: HashSet<NodeId> = function_nodes.iter().copied().collect();
        let mut remap = HashMap::new();
        self.clone_node(root, target, param, argument, &members, &mut remap)
    }

    #[stacksafe]
    fn clone_node(
        &mut self,
        node: NodeId,
        target: BlockId,
        param: NodeId,
        argument: NodeId,
        members: &HashSet<NodeId>,
        remap: &mut HashMap<NodeId, NodeId>,
    ) -> NodeId {
        if node == param {
            return argument;
        }
        if let Some(&clone) = remap.get(&node) {
            return clone;
        }
        if !members.contains(&node) {
            return node; // outside the template scope — reference as-is
        }
        // The body always exists, so only the parts that depend on the
        // parameter need fresh nodes: unevaluated operation nodes (their
        // operand edges must be rewritten) and parameterized nodes (their
        // value or operand still embeds the marker).  A node with a concrete
        // evaluated value is baked — reference it in place.
        let (value, operation, parameterized_deep) = {
            let source = &self.nodes[node];
            (source.value, source.operation, source.parameterized_deep)
        };
        let depends_on_parameter =
            (value.is_none() && operation.is_some()) || parameterized_deep == Some(true);
        if !depends_on_parameter {
            return node;
        }
        // Reserve the clone id before recursing so diamonds resolve to one
        // clone and value cycles to the clone's own (still evaluating) id.
        let clone = self.add_node(target, None, None);
        remap.insert(node, clone);
        // A cached value on an operation node was computed against the
        // body's parameter and is stale once the argument is mapped in, so
        // such clones are left unevaluated — the kept operand chain
        // recomputes against the argument.  Constant nodes (no operation)
        // carry their remapped value.
        let value = if operation.is_some() {
            None
        } else {
            value.map(|value| {
                self.clone_value(value, target, param, argument, members, remap)
            })
        };
        let operation = operation.map(|operation| Operation {
            operand: operation
                .operand
                .map(|operand| self.clone_node(operand, target, param, argument, members, remap)),
            ..operation
        });
        self.nodes[clone].value = value;
        self.nodes[clone].operation = operation;
        clone
    }

    #[stacksafe]
    fn clone_value(
        &mut self,
        value: Value<P>,
        target: BlockId,
        param: NodeId,
        argument: NodeId,
        members: &HashSet<NodeId>,
        remap: &mut HashMap<NodeId, NodeId>,
    ) -> Value<P> {
        match value {
            Value::Array(array) => {
                let ids: Vec<NodeId> = unsafe { &*array }
                    .iter()
                    .map(|&id| self.clone_node(id, target, param, argument, members, remap))
                    .collect();
                Value::Array(self.copy_ids(target, &ids))
            }
            Value::Function(function) => {
                // A cloned function's scope is mapped like an array: every
                // member and both entry points are cloned into the target.
                let ids: Vec<NodeId> = unsafe { &*function.nodes }
                    .iter()
                    .map(|&id| self.clone_node(id, target, param, argument, members, remap))
                    .collect();
                Value::Function(Function {
                    nodes: self.copy_ids(target, &ids),
                    r#return: self.clone_node(function.r#return, target, param, argument, members, remap),
                    parameter: self.clone_node(function.parameter, target, param, argument, members, remap),
                })
            }
            Value::Ext(ext) => Self::copy_ext(ext, &self.blocks[target].arena),
            value => value,
        }
    }

    #[stacksafe]
    fn release_block(&mut self, block: BlockId) {
        let children = std::mem::take(&mut self.blocks[block].children);
        for child in children {
            if self.blocks.contains_key(child) {
                self.release_block(child);
            }
        }
        let nodes = std::mem::take(&mut self.blocks[block].nodes);
        for node in nodes {
            if self.nodes.get(node).is_some_and(|node| node.block == block) {
                self.nodes.remove(node);
            }
        }
        self.blocks.remove(block);
    }
}
