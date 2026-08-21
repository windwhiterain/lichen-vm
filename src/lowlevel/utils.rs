use std::{alloc::Layout, ptr};

use crate::lowlevel::{BlockId, Module, NodeId, Program, Value, ValueExt as _};

impl<P: Program> Module<P> {
    /// Copy `nodes` into `block.arena` and return the new `nodes`.
    pub(super) fn copy_nodes(&self, nodes: &[NodeId], block: BlockId) -> *const [NodeId] {
        let slice = self.blocks[block].arena.alloc_slice_copy(nodes);
        ptr::slice_from_raw_parts(slice.as_ptr(), slice.len())
    }

    /// Copy `ext` into `block.arena` and return the new `ext`.
    pub(super) fn copy_ext(&self, mut ext: P::Value, block: BlockId) -> Value<P> {
        let arena = &self.blocks[block].arena;
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
