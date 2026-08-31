use std::{alloc::Layout, ptr};

use crate::{
    AnyHandle, ArrayItem, BlockId, Handle, LowValue, Module, NodeId, Program, TableItem,
    ValueExt as _,
};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// The items of `node`'s array value, if it has one.
    ///
    /// # Safety
    /// The returned slice points into the arena of the node's home block.
    /// A node's home block is alive exactly while the node is: dropping a
    /// block removes its nodes, and indexing a removed `NodeId` panics — so
    /// a reachable node always has its arena alive, and the slice is valid
    /// for the lifetime of `&self`.
    pub fn array_items(&self, node: NodeId) -> Option<&'static [ArrayItem]> {
        let value = self.nodes[node].value?;
        let LowValue::Array(array) = value.as_enum()? else {
            return None;
        };
        Some(array.items())
    }

    /// Copy `items` into `block.arena` and return the array handle pointing
    /// at the copy — the payload every [`LowValue::Array`] carries.
    pub fn alloc_array(&self, items: &[ArrayItem], block: BlockId) -> AnyHandle<[ArrayItem]> {
        let slice = self.blocks[block].arena.alloc_slice_copy(items);
        AnyHandle::Dynamic(Handle(ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )))
    }

    /// Copy `items` into `block.arena` and return the table handle pointing
    /// at the copy — the payload every [`LowValue::Table`] carries.
    pub fn alloc_table(&self, items: &[TableItem], block: BlockId) -> AnyHandle<[TableItem]> {
        let slice = self.blocks[block].arena.alloc_slice_copy(items);
        AnyHandle::Dynamic(Handle(ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )))
    }

    /// Copy `value` into `block.arena` and return the new `value`.  Only a
    /// handle-carrying program-specific value relocates; everything else is
    /// returned untouched.
    pub(super) fn copy_ext(&self, mut value: P::Value, block: BlockId) -> P::Value {
        let arena = &self.blocks[block].arena;
        if !value.is_handle() {
            return value;
        }
        let old = value.handle();
        let layout = Layout::from_size_align(old.len(), P::Value::alignment()).unwrap();
        let dst = arena.alloc_layout(layout);
        unsafe { ptr::copy_nonoverlapping(old.as_ptr(), dst.as_ptr(), old.len()) };
        value.set_handle(AnyHandle::Dynamic(Handle(ptr::slice_from_raw_parts(
            dst.as_ptr(),
            old.len(),
        ))));
        value
    }

    /// True if `block` is `ancestor` or a descendant of it.
    pub(super) fn descends_from(&self, mut block: BlockId, ancestor: BlockId) -> bool {
        loop {
            if block == ancestor {
                return true;
            }
            match self.blocks[block].parent {
                Some(parent) => block = parent,
                None => return false,
            }
        }
    }
}
