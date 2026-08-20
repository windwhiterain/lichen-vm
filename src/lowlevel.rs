use bumpalo::Bump;
use slotmap::{SecondaryMap, SlotMap, new_key_type};
use std::alloc::Layout;
use std::ptr;

pub trait Program: Sized + Copy {
    type Value: ValueExt;
    type Operator: OperatorExt<Self>;
}

pub trait ValueExt: Copy {
    fn is_ptr() -> bool;
    /// avaliable if `self::is_ptr()`
    fn ptr(&self) -> *const [u8] {
        unreachable!()
    }
    /// avaliable if `self::is_ptr()`
    fn set_ptr(&mut self, _ptr: *const [u8]) {
        unreachable!()
    }
    /// avaliable if `self::is_ptr()`
    fn alignment() -> usize {
        unreachable!()
    }
}

pub trait OperatorExt<P: Program>: Copy {
    fn run(
        &self,
        operand: Value<P>,
        block: &mut Block,
        values: &SlotMap<NodeId, Option<Value<P>>>,
    ) -> Value<P>;
}

#[derive(Clone, Copy)]
pub enum Value<P: Program> {
    Ext(P::Value),
    Array(*const [NodeId]),
    USize(usize),
    None,
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

pub struct Moduele<P: Program> {
    pub values: SlotMap<NodeId, Option<Value<P>>>,
    pub operations: SecondaryMap<NodeId, Option<Operation<P>>>,
    /// which block the node lives in.
    /// # Contract
    /// - only one node can be referenced from parent block
    pub block_ids: SecondaryMap<NodeId, BlockId>,
    pub blocks: SlotMap<BlockId, Block>,
}

impl<P: Program> Default for Moduele<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Moduele<P> {
    pub fn new() -> Self {
        Moduele {
            values: SlotMap::with_key(),
            operations: SecondaryMap::new(),
            block_ids: SecondaryMap::new(),
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
        let id = self.values.insert(value);
        self.operations.insert(id, operation);
        self.block_ids.insert(id, block);
        self.blocks[block].nodes.push(id);
        id
    }

    /// If `id` lives in a child of `referer`, its a block root, [`Self::evaluate_block`] will be called on it.
    pub fn evaluate_node(&mut self, id: NodeId, referer: Option<BlockId>) -> Value<P> {
        let block = self.block_ids[id];
        if let Some(referer) = referer && self.blocks[block].parent == Some(referer) {
            self.evaluate_block(block, id);
            return self.values[id].unwrap();
        }
        if let Some(value) = self.values[id] {
            return value;
        }
        let operation = self.operations[id].unwrap();
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
                    Some(operand) => self.evaluate_node_deep(operand, Some(block)),
                    None => Value::None,
                };
                let Moduele { values, blocks, .. } = self;
                ext.run(operand, &mut blocks[block], values)
            }
        };
        self.values[id] = Some(value);
        value
    }

    /// Run [`Self::evaluate_node`] for all nodes in the reachable subtree of `id`. 
    pub fn evaluate_node_deep(&mut self, id: NodeId, current: Option<BlockId>) -> Value<P> {
        let value = self.evaluate_node(id, current);
        if let Value::Array(array) = value {
            let block = self.block_ids[id];
            for &id in unsafe { &*array } {
                self.evaluate_node_deep(id, Some(block));
            }
        }
        value
    }

    fn evaluate_block(&mut self, id: BlockId, root: NodeId) -> Value<P> {
        debug_assert_eq!(self.block_ids[root], id);
        self.evaluate_node_deep(root, None);
        let value = self.move_to_parent(root);
        self.release_block(id);
        value
    }

    /// - nodes are added to parent block.
    /// - allocations are copied.  
    /// - nodes value update to the copied allocations. 
    fn move_to_parent(&mut self, node: NodeId) -> Value<P> {
        let value = self.values[node].unwrap();
        let block = self.block_ids[node];
        let parent = self.blocks[block].parent;
        let Some(parent) = parent else{
            return value;
        };
        self.block_ids[node] = parent;
        self.blocks[parent].nodes.push(node);
        let value = match value {
            Value::Array(array) => {
                let ids = unsafe { &*array };
                for &id in ids {
                    let child_block = self.block_ids[id];
                    if child_block != block{
                        continue;
                    }
                    self.move_to_parent(id);
                }
                let slice = self.blocks[parent].arena.alloc_slice_copy(ids);
                Value::Array(ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()))
            }
            Value::Ext(mut ext) => {
                if !P::Value::is_ptr() {
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
        self.values[node] = Some(value);
        value
    }

    fn release_block(&mut self, block: BlockId) {
        // release children
        let children = std::mem::take(&mut self.blocks[block].children);
        for child in children {
            if self.blocks.contains_key(child) {
                self.release_block(child);
            }
        }
        // remove nodes
        let nodes = std::mem::take(&mut self.blocks[block].nodes);
        for node in nodes {
            if self.block_ids.get(node) == Some(&block) {
                self.values.remove(node);
                self.operations.remove(node);
                self.block_ids.remove(node);
            }
        }
        // remove self
        self.blocks.remove(block);
    }
}
