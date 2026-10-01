use std::{alloc::Layout, ptr};

use crate::{
    AnyHandle, ArrayItem, BlockId, Handle, LowValue, Module, NodeId, Program, TableItem,
    ValueExt as _,
};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// The items of `node`'s array value, if it has one.
    ///
    /// A node's home block is alive exactly while the node is: dropping a
    /// block removes its nodes, and indexing a removed `NodeId` panics, so a
    /// reachable node always has its arena alive.
    ///
    /// # Safety
    ///
    /// As [`AnyHandle<[ArrayItem]>::items`], which states the contract: the
    /// returned slice points into the array payload's home arena, so the
    /// caller must keep `node` reachable — its home block alive — for as long
    /// as the slice is read.  [`Module::drop_block`] releasing the block's
    /// `Bump` is what invalidates it.
    pub unsafe fn array_items(&self, node: NodeId) -> Option<&'static [ArrayItem]> {
        let value = self.nodes[node].value?;
        let LowValue::Array(array) = value.as_enum()? else {
            return None;
        };
        // SAFETY: the caller upholds this method's `# Safety`; `array` is the
        // live payload of a node whose home block has not been released, so
        // the same obligation covers handing its slice out here.
        Some(unsafe { array.items() })
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

    /// Copy `items` into `block.arena` and return the handle pointing at the
    /// copy — the payload a **program-specific** (ext) value carries.
    ///
    /// The two allocators above are the structural payloads the lowlevel itself
    /// defines.  This is the same bump allocation for a vocabulary's own
    /// payload type: a value that answers [`ValueExt::is_handle`] stores its
    /// data here, and the crate's copy path relocates it like any other payload
    /// ([`Self::copy_ext`]), so the lifetime is the block's, not a registry's.
    pub fn alloc_payload<T: Copy>(&self, items: &[T], block: BlockId) -> AnyHandle<[T]> {
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
        // `ValueExt::alignment` is required to be a power of two and the byte
        // length has to fit the layout; either violation is a broken value
        // vocabulary, not a runtime condition this signature can report.
        let layout = Layout::from_size_align(old.len(), P::Value::alignment()).expect(
            "ValueExt::alignment() must be a power of two and the payload byte length must fit the layout",
        );
        let dst = arena.alloc_layout(layout);
        // SAFETY: `old` is the live payload of `value` — its pointer and byte
        // length agree by the `ValueExt::handle` contract — and `dst` is a
        // fresh bump allocation of exactly that many bytes, so the source and
        // destination ranges cannot overlap.
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
