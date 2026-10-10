//! The set value `set{a, b, …}` and its membership read. See
//! `docs/notes/operator-polymorphism.md` §3.2.

use lichen_lowlevel::{AnyNodeId, LowShape, Module, Program, ValueExt};

use crate::program::ValueType;
use crate::shape::{array_items, low_type_of};

/// The set whose **value** is `value`, its members in source order; `None`
/// when `value` is not an array.
pub fn members<P: Program>(module: &Module<P>, value: AnyNodeId) -> Option<Vec<AnyNodeId>>
where
    P::Value: ValueType,
{
    // SAFETY: `value` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { array_items(module, value) }?;
    Some(items.iter().map(|item| item.node).collect())
}

/// Whether `value` is a member of the set whose value is `set`. See
/// `docs/notes/operator-polymorphism.md` §3.2.
///
/// # Invariant
/// A member matches by the class it denotes when it denotes one, and by the
/// language's own value equality otherwise: `value_eq` compares an array by
/// handle, so a class node from another module needs the structural test.
pub fn contains<P: Program>(module: &Module<P>, set: AnyNodeId, value: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    members(module, set).is_some_and(|members| {
        members
            .into_iter()
            .any(|member| same_member(module, member, value))
    })
}

/// Whether a member and the tested value denote the same class, when both
/// denote one; [`ValueExt::value_eq`] otherwise.
fn same_member<P: Program>(module: &Module<P>, member: AnyNodeId, value: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    match (known_shape(module, member), known_shape(module, value)) {
        (Some(member), Some(value)) => member == value,
        _ => same_value(module, member, value),
    }
}

/// [`ValueExt::value_eq`] over two nodes' values; `false` when either has none.
fn same_value<P: Program>(module: &Module<P>, left: AnyNodeId, right: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    match (module.node_value(left), module.node_value(right)) {
        (Some(left), Some(right)) => left.value_eq(&right),
        _ => false,
    }
}

/// [`low_type_of`]'s answer for a type value, `None` when it is undecided.
fn known_shape<P: Program>(module: &Module<P>, node: AnyNodeId) -> Option<LowShape>
where
    P::Value: ValueType,
{
    let shape = low_type_of(module, node);
    shape.is_known().then_some(shape)
}
