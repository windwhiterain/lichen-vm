use bumpalo::Bump;
use slotmap::{SlotMap, new_key_type};
use stacksafe::stacksafe;
use std::alloc::Layout;
use std::ptr;

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
    USize(usize),
    None,
    Parameterized,
}

#[derive(Clone, Copy)]
pub enum Operator<P: Program> {
    Ext(P::Operator),
    Index,
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

pub struct Function {
    pub block: BlockId,
}

impl Function {
    pub const PARAMETER_IDX: usize = 0;
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
        });
        self.blocks[block].nodes.push(id);
        id
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
        let value = self.move_to_parent(root);
        self.release_block(block);
        value
    }

    /// - Nodes are added to parent block.
    /// - Allocations are copied.  
    /// - Nodes value update to the copied allocations.
    #[stacksafe]
    fn move_to_parent(&mut self, node: NodeId) -> Value<P> {
        let value = self.nodes[node].value.unwrap();
        let block = self.nodes[node].block;
        let parent = self.blocks[block].parent;
        let Some(parent) = parent else {
            return value;
        };
        self.nodes[node].block = parent;
        self.blocks[parent].nodes.push(node);
        let value = match value {
            Value::Array(array) => {
                let ids = unsafe { &*array };
                for &id in ids {
                    let child_block = self.nodes[id].block;
                    if child_block != block {
                        continue;
                    }
                    self.move_to_parent(id);
                }
                let slice = self.blocks[parent].arena.alloc_slice_copy(ids);
                Value::Array(ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()))
            }
            Value::Ext(mut ext) => {
                if !ext.is_ptr() {
                    return Value::Ext(ext);
                }
                let old = ext.ptr();
                let layout = Layout::from_size_align(old.len(), P::Value::alignment()).unwrap();
                let dst = self.blocks[parent].arena.alloc_layout(layout);
                unsafe { ptr::copy_nonoverlapping(old as *const u8, dst.as_ptr(), old.len()) };
                ext.set_ptr(ptr::slice_from_raw_parts(dst.as_ptr(), old.len()));
                Value::Ext(ext)
            }
            value => value,
        };
        self.nodes[node].value = Some(value);
        value
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
