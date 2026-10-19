//! Probe: an `External` operator calls the entry the host installed.
//!
//! Prints; asserts nothing.

use lichen_lowlevel::{AnyNodeId, ArrayItem, LowOperator, LowValue, Module, Operation};

use crate::*;

/// Adds one to the operand array's second element, through the module the entry
/// is handed.
///
/// # Safety
/// [`LowOperator::External`]'s entry contract: the module, operand and out
/// pointers name this build's `TestProgram` types.
unsafe extern "C" fn add_one(
    module: *mut std::ffi::c_void,
    operand: *const std::ffi::c_void,
    out: *mut std::ffi::c_void,
) {
    let module = unsafe { &mut *(module as *mut Module<TestProgram>) };
    let operand = unsafe { &*(operand as *const TestValue) };
    let mut result: TestValue = LowValue::USize(0).into();
    if let Some(LowValue::Array(array)) = AsEnum::<LowValue>::as_enum(operand) {
        let items = unsafe { array.items() };
        if let Some(LowValue::USize(n)) = items
            .first()
            .and_then(|item| module.node_value(item.node))
            .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
        {
            result = LowValue::USize(n + 1).into();
        }
    }
    unsafe { (out as *mut TestValue).write(result) };
}

#[test]
fn probe_external_operator() {
    let mut module = Module::<TestProgram>::new();
    let block = module.add_block(None);
    let seven: TestValue = LowValue::USize(7).into();
    let seven_node = module.add_node(block, None, Some(seven));
    let one: TestValue = LowValue::USize(1).into();
    let one_node = module.add_node(block, None, Some(one));
    let items = [
        ArrayItem::new(AnyNodeId::Dynamic(seven_node)),
        ArrayItem::new(AnyNodeId::Dynamic(one_node)),
    ];
    let handle = module.alloc_array(&items, block);
    let array: TestValue = LowValue::Array(handle.into()).into();
    let operand = module.add_node(block, None, Some(array));
    let external = |module: &mut Module<TestProgram>| {
        module.add_node(
            block,
            Some(Operation {
                operator: TestOperator::from(LowOperator::External {
                    library: 7,
                    instance: 0,
                }),
                operand: Some(operand),
            }),
            None,
        )
    };

    // A node runs once, so the missing entry and the installed one need two nodes.
    let missing = external(&mut module);
    println!(
        "without an entry: {:?}",
        module.evaluate_node_deep(missing, None)
    );

    module.install_external(7, 0, add_one);
    let node = external(&mut module);
    println!(
        "with the entry: {:?}",
        module.evaluate_node_deep(node, None)
    );
    println!(
        "extension diagnostics: {:?}",
        module
            .extension_diagnostics
            .iter()
            .map(|entry| entry.message.clone())
            .collect::<Vec<_>>()
    );
}
