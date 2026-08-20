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
    fn run(&self, operand: Value<P>, block: &mut Block) -> Value<P>;
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
    pub operand: Option<usize>,
}

pub struct Moduele<P: Program> {
    pub values: SlotMap<NodeId, Option<Value<P>>>,
    pub operations: SecondaryMap<NodeId, Option<Operation<P>>>,
    /// # Contract
    /// only one node can be referenced from parent block
    pub block_ids: SecondaryMap<NodeId, usize>,
    pub blocks: Vec<Block>,
}

new_key_type! {pub struct NodeId;}

pub struct Block {
    pub arena: Bump,
    pub parent: Option<usize>,
}

impl<P: Program> Block<P> {
    pub const RETURN_ID: usize = 0;
    /// make return value lives in `parent`.
    pub fn run(&mut self, parent: &mut Self) -> Value<P> {
        let value = self.run_node_deep(Some(Self::RETURN_ID));
        let mut remap = Vec::new();
        self.compact_value(value, &parent.arena, &mut parent.values, &mut remap)
    }
    /// make `value` lives in parent.
    fn compact_value(
        &self,
        value: Value<P>,
        arena: &Bump,
        values: &mut Vec<Option<Value<P>>>,
        remap: &mut Vec<Option<usize>>,
    ) -> Value<P> {
        match value {
            Value::Array(array) => {
                let ids = unsafe { &*array }
                    .iter()
                    .map(|&id| self.compact_node(id, arena, values, remap));
                let slice = arena.alloc_slice_fill_iter(ids);
                Value::Array(ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()))
            }
            Value::Ext(mut ext) => {
                // Relocate an arena-allocated payload: copy the pointed-to
                // bytes into parent's arena, preserving the payload's
                // alignment, and rebuild the value with the relocated pointer.
                if !P::Value::is_ptr() {
                    return Value::Ext(ext);
                }
                let old = ext.ptr();
                let layout = Layout::from_size_align(old.len(), P::Value::alignment()).unwrap();
                let dst = arena.alloc_layout(layout);
                unsafe { ptr::copy_nonoverlapping(old as *const u8, dst.as_ptr(), old.len()) };
                ext.set_ptr(ptr::slice_from_raw_parts(dst.as_ptr(), old.len()));
                Value::Ext(ext)
            }
            value => value,
        }
    }
    /// make node `id` lives in parent
    fn compact_node(
        &self,
        id: usize,
        arena: &Bump,
        values: &mut Vec<Option<Value<P>>>,
        remap: &mut Vec<Option<usize>>,
    ) -> usize {
        if let Some(entry) = remap.get(id) {
            if let Some(new) = *entry {
                return new;
            }
        }
        let value = self.values[id].unwrap();
        let compacted = self.compact_value(value, arena, values, remap);
        values.push(Some(compacted));
        let new = values.len() - 1;
        if remap.len() <= id {
            remap.resize(id + 1, None);
        }
        remap[id] = Some(new);
        new
    }
    fn run_node(&mut self, id: Option<usize>) -> Value<P> {
        let Some(id) = id else { return Value::None };
        if let Some(value) = self.values[id] {
            return value;
        }
        let operation = self.operations[id].unwrap();
        let value = match operation.operator {
            Operator::Ext(ext) => ext.run(self.run_node_deep(operation.operand), self),
            Operator::Block(block) => unsafe { &mut *block }.run(self),
            Operator::Index => {
                let Value::Array(array) = self.run_node(Some(operation.operand.unwrap())) else {
                    unreachable!()
                };
                let operands = unsafe { &*array };
                let Value::USize(index) = self.run_node(Some(operands[1])) else {
                    unreachable!()
                };
                let Value::Array(array) = self.run_node(Some(operands[0])) else {
                    unreachable!()
                };
                let array = unsafe { &*array };
                self.run_node(Some(array[index]))
            }
        };
        self.values[id] = Some(value);
        value
    }
    fn run_node_deep(&mut self, id: Option<usize>) -> Value<P> {
        let value = self.run_node(id);
        match value {
            Value::Array(array) => {
                for id in unsafe { &*array }.iter().copied() {
                    self.run_node_deep(Some(id));
                }
            }
            _ => (),
        }
        value
    }
}
