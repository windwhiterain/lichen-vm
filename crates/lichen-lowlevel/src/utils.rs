use std::{alloc::Layout, ptr};

use crate::{BlockId, Module, NodeId, Handle, Program, Value, ValueExt as _};

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
        match self.nodes[node].value {
            Some(Value::Array(ptr)) => Some(unsafe { &*ptr }),
            _ => None,
        }
    }

    /// Copy `nodes` into `block.arena` and return the new `nodes`.
    pub(super) fn copy_nodes(&self, nodes: &[NodeId], block: BlockId) -> *const [NodeId] {
        let slice = self.blocks[block].arena.alloc_slice_copy(nodes);
        ptr::slice_from_raw_parts(slice.as_ptr(), slice.len())
    }

    /// Copy `ext` into `block.arena` and return the new `ext`.
    pub(super) fn copy_ext(&self, mut ext: P::Value, block: BlockId) -> Value<P> {
        let arena = &self.blocks[block].arena;
        if !ext.is_handle() {
            return Value::Ext(ext);
        }
        let old = ext.handle();
        let layout = Layout::from_size_align(old.len(), P::Value::alignment()).unwrap();
        let dst = arena.alloc_layout(layout);
        unsafe { ptr::copy_nonoverlapping(old.0 as *const u8, dst.as_ptr(), old.len()) };
        ext.set_handle(Handle(ptr::slice_from_raw_parts(dst.as_ptr(), old.len())));
        Value::Ext(ext)
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
