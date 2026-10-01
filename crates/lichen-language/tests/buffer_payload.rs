//! A compute buffer's payload lives in the **block arena**, and this is the
//! contract that makes that safe (`D15`).
//!
//! A `Buffer` value is a `Copy` handle, like the lowlevel's own array and table
//! payloads. The crate's copy path re-homes a payload whenever the value moves
//! to another block — an apply clone, a garbage collect — and it can only do
//! that for a value whose [`ValueExt::is_handle`] answers `true`, which the
//! *composed* union must dispatch to its leaves. So this pins the three links
//! the design rests on at once: the leaf answers, the composition dispatches,
//! and the relocation actually moves the payload rather than leaving a handle
//! pointing into a block that is about to be released.
//!
//! It is a pointer comparison, not a content read, on purpose: reading through
//! a dangling handle is undefined behaviour that frequently *appears* to work
//! (freed bump memory is still mapped), so a test that only checked the values
//! would pass while the payload was dangling.

use lichen_compute::ComputeValue;
use lichen_language::program::{LangProgram, LangValue};
use lichen_lowlevel::{AnyHandle, AnyNodeId, Module, Program, ValueExt};
use lichen_utils::extend::AsEnum;

/// The payload's address, for the "did it move" assertion.
fn payload_address(value: &LangValue) -> usize {
    let compute = AsEnum::<ComputeValue>::as_enum(value).expect("the node holds a compute value");
    match compute {
        ComputeValue::Buffer(AnyHandle::Dynamic(handle)) => handle.as_ptr() as *const i64 as usize,
        other => panic!("expected a buffer payload, got {other:?}"),
    }
}

/// The payload's elements.
fn payload_items(value: &LangValue) -> Vec<i64> {
    let compute = AsEnum::<ComputeValue>::as_enum(value).expect("the node holds a compute value");
    match compute {
        ComputeValue::Buffer(AnyHandle::Dynamic(handle)) => {
            // SAFETY: the value was read out of the module on a live borrow, so
            // its payload's home block is alive for this read.
            unsafe { (*handle.as_ptr()).to_vec() }
        }
        other => panic!("expected a buffer payload, got {other:?}"),
    }
}

#[test]
fn a_collected_payload_is_relocated_when_its_block_is_released() {
    let mut module = Module::<LangProgram>::new();
    let root = module.add_block(None);
    let child = module.add_block(Some(root));

    // A buffer value in the child block, its payload in the child's arena.
    let payload = module.alloc_payload(&[10_i64, 20, 30], child);
    let node = module.add_node(
        child,
        None,
        Some(<LangProgram as Program>::Value::from(ComputeValue::Buffer(
            payload,
        ))),
    );
    let before = module.node_value(AnyNodeId::Dynamic(node));
    let before = before.expect("the node holds the buffer");
    assert!(
        before.is_handle(),
        "the composed value must report the leaf's payload — the copy path \
         consults this on the union, so a union that declares itself inert \
         leaves the payload unrelocated"
    );
    let address_before = payload_address(&before);
    assert_eq!(payload_items(&before), vec![10, 20, 30]);

    // Vacate the child: its reachable subtree moves to the parent and the
    // child's `Bump` is released, so every payload homed there must be
    // re-allocated — or the handle now points at freed arena memory.
    let moved = module.garbage_collect(node).expect("the node moves");
    assert_eq!(
        module.node_block(node),
        root,
        "the node moved to the parent"
    );

    let address_after = payload_address(&moved);
    assert_ne!(
        address_before, address_after,
        "the payload must be re-allocated in the destination block — an \
         unchanged address means the freed block's arena is still what the \
         handle reads, which is a dangling payload"
    );
    assert_eq!(
        payload_items(&moved),
        vec![10, 20, 30],
        "the relocation must carry the elements, not just the pointer"
    );
}
