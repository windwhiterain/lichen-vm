use std::{alloc::Layout, ptr};

use crate::{
    AnyHandle, ArrayItem, BlockId, Handle, LowValue, Module, NodeId, Program, TableItem,
    ValueExt as _,
};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// The items of `node`'s array value, if it has one.  The slice lives in
    /// `node`'s home block.
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
        // SAFETY: the caller's `# Safety` holds — `array` is a node's live,
        // unreleased payload.
        Some(unsafe { array.items() })
    }

    /// Copy `items` into `block.arena` and return the [`LowValue::Array`] payload
    /// handle.
    pub fn alloc_array(&self, items: &[ArrayItem], block: BlockId) -> AnyHandle<[ArrayItem]> {
        let slice = self.blocks[block].arena.alloc_slice_copy(items);
        AnyHandle::Dynamic(Handle(ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )))
    }

    /// Copy `items` into `block.arena` and return the [`LowValue::Table`] payload
    /// handle.
    pub fn alloc_table(&self, items: &[TableItem], block: BlockId) -> AnyHandle<[TableItem]> {
        let slice = self.blocks[block].arena.alloc_slice_copy(items);
        AnyHandle::Dynamic(Handle(ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )))
    }

    /// Copy `items` into `block.arena` for an **ext** value; the handle's lifetime is
    /// the block's.  See [`Self::copy_ext`].
    pub fn alloc_payload<T: Copy>(&self, items: &[T], block: BlockId) -> AnyHandle<[T]> {
        let slice = self.blocks[block].arena.alloc_slice_copy(items);
        AnyHandle::Dynamic(Handle(ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )))
    }

    /// Copy `value` into `block.arena`; only a handle-carrying ext value relocates.
    pub(super) fn copy_ext(&self, mut value: P::Value, block: BlockId) -> P::Value {
        let arena = &self.blocks[block].arena;
        if !value.is_handle() {
            return value;
        }
        let old = value.handle();
        // Either violation is a broken value vocabulary, not a condition this
        // signature can report.
        let layout = Layout::from_size_align(old.len(), P::Value::alignment()).expect(
            "ValueExt::alignment() must be a power of two and the payload byte length must fit the layout",
        );
        let dst = arena.alloc_layout(layout);
        // SAFETY: `old` is a live payload and `dst` a fresh bump allocation of
        // `old.len()` bytes; they cannot overlap.
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
