use std::{alloc::Layout, ptr};

use crate::{BlockId, Handle, LowValue, Module, NodeId, Program, ValueExt as _};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// The ids of `node`'s array value, if it has one.
    ///
    /// # Safety
    /// The returned slice points into the arena of the node's home block.
    /// A node's home block is alive exactly while the node is: dropping a
    /// block removes its nodes, and indexing a removed `NodeId` panics — so
    /// a reachable node always has its arena alive, and the slice is valid
    /// for the lifetime of `&self`.
    pub fn array_ids(&self, node: NodeId) -> Option<&[NodeId]> {
        let value = self.nodes[node].value?;
        let LowValue::Array(ptr) = value.as_enum()? else {
            return None;
        };
        Some(unsafe { &*ptr })
    }

    /// Copy `nodes` into `block.arena` and return the new `nodes`.
    pub(super) fn copy_nodes(&self, nodes: &[NodeId], block: BlockId) -> *const [NodeId] {
        let slice = self.blocks[block].arena.alloc_slice_copy(nodes);
        ptr::slice_from_raw_parts(slice.as_ptr(), slice.len())
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
        unsafe { ptr::copy_nonoverlapping(old.0 as *const u8, dst.as_ptr(), old.len()) };
        value.set_handle(Handle(ptr::slice_from_raw_parts(dst.as_ptr(), old.len())));
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
