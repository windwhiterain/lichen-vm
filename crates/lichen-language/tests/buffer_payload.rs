//! A `Buffer` is a `Copy` handle into the block arena, re-homed on every move
//! (`D15`). See compute-buffer-wrapper.md.

use lichen_compute::ComputeValue;
use lichen_kernel_ir::ScalarClass;
use lichen_language::program::{LangProgram, LangValue};
use lichen_lowlevel::{AnyHandle, AnyNodeId, Module, Program, ValueExt};
use lichen_utils::extend::AsEnum;

/// The payload's address, for the "did it move" assertion.
fn payload_address(value: &LangValue) -> usize {
    let compute = AsEnum::<ComputeValue>::as_enum(value).expect("the node holds a compute value");
    match compute {
        ComputeValue::Buffer(AnyHandle::Dynamic(handle), _) => {
            handle.as_ptr() as *const u8 as usize
        }
        other => panic!("expected a buffer payload, got {other:?}"),
    }
}

/// The payload's elements: packed bytes, [`ScalarClass::byte_width`] per
/// element, decoded at the class carried.
fn payload_items(value: &LangValue) -> Vec<i64> {
    let compute = AsEnum::<ComputeValue>::as_enum(value).expect("the node holds a compute value");
    match compute {
        ComputeValue::Buffer(AnyHandle::Dynamic(handle), class) => {
            // SAFETY: the value was read out of the module on a live borrow, so
            // its payload's home block is alive for this read.
            let bytes = unsafe { &*handle.as_ptr() };
            bytes
                .chunks_exact(class.byte_width())
                .map(|element| match class {
                    ScalarClass::Int => i64::from_le_bytes(element.try_into().unwrap_or_default()),
                    ScalarClass::Float => {
                        i64::from(u32::from_le_bytes(element.try_into().unwrap_or_default()))
                    }
                })
                .collect()
        }
        other => panic!("expected a buffer payload, got {other:?}"),
    }
}

#[test]
fn a_collected_payload_is_relocated_when_its_block_is_released() {
    let mut module = Module::<LangProgram>::new();
    let root = module.add_block(None);
    let child = module.add_block(Some(root));

    // A buffer value in the child block, its payload in that block's arena.
    let words: Vec<u8> = [10_i64, 20, 30]
        .into_iter()
        .flat_map(i64::to_le_bytes)
        .collect();
    let payload = module.alloc_payload(&words, child);
    let node = module.add_node(
        child,
        None,
        Some(<LangProgram as Program>::Value::from(ComputeValue::Buffer(
            payload,
            ScalarClass::Int,
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

    // Vacate the child: its subtree moves up and the `Bump` is released, so every
    // payload homed there must move.
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
