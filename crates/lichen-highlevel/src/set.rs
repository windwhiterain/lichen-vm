//! The **set**: `set{a, b, …}` — the members are the value, the set kind is the
//! type.
//!
//! A set's *value* is its members: an ordinary array node, exactly the node an
//! array literal's elements make.  Nothing tags it, so nothing about the value
//! says "set" — the **type** does.  A set instance is typed
//! `[members, [[element type], [TypeSet, Type]]]`, whose shape is the element
//! type *alone* (a set has no length: `set{a}` and `set{a, b}` share one type),
//! and that is what separates the set kind from `array<T, n>`.
//!
//! Two consequences, and both are why the design is this way:
//!
//! - **A set is not an array.**  [`crate::checker`]'s index read pins its
//!   container's type to a fresh array type, so `s[i]` on a set is refused, and
//!   a set can never flow into an `array<T, n>` parameter.  A tagged *value*
//!   (`[tag, members]`) would instead have needed a guard at every read: a set
//!   reaching an `array`-typed parameter would read the tag as element 0 inside
//!   the callee, where no value-level guard can see it.
//! - **The class domain is one of these.**  `Num = set{Int, Float}` written in
//!   `lichen-std` is the same node shape a contract's own condition uses today
//!   (`docs/notes/operator-polymorphism.md` §3), so the contract can move out of
//!   Rust without changing the graph.
//!
//! A **class domain** is a set of type values, and a reader that must commit to
//! one class takes the **first member**: `set{Int, Float}` prefers `Int`, the
//! arithmetic operators' historical default.  That is a convention of the
//! *reading*, deliberately not an element of the encoding — nothing about a set
//! privileges a member, and an explicit preferred member would be a second
//! concept to learn for one reader's benefit.

use lichen_lowlevel::{AnyNodeId, LowShape, Module, Program, ValueExt};

use crate::program::ValueType;
use crate::shape::{array_items, low_type_of};

/// The members of the set whose **value** is `value`, in source order.
///
/// `None` when `value` is not an array, so not a set's value — a set's
/// membership is decided by its type, and this reader is for a caller that
/// already knows it holds one (a class domain).
pub fn members<P: Program>(module: &Module<P>, value: AnyNodeId) -> Option<Vec<AnyNodeId>>
where
    P::Value: ValueType,
{
    // SAFETY: `value` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { array_items(module, value) }?;
    Some(items.iter().map(|item| item.node).collect())
}

/// Whether `value` is a member of the set whose value is `set` — the membership
/// test [`crate::program::TypeOperator::InDomain`] runs, which is both the
/// class-domain read and the source form's meaning (`value @in set`,
/// `docs/notes/operator-polymorphism.md` §3).
///
/// A member is matched **by the class it denotes when it denotes one, and by
/// the language's own value equality otherwise** — the one rule that answers
/// both readings of a set:
///
/// - **A set of type values** (`Num = set{Int, Float}`) is compared
///   structurally, through [`low_type_of`], so a class node out of another
///   module matches by its shape and no node identity is involved.  That is why
///   this reader exists at all: [`ValueExt::value_eq`] compares array
///   *handles*, so two spellings of `Int` from different modules are unequal
///   under it, and an `==`-based membership test would refuse a valid class.
/// - **A set of ordinary values** (`set{1, 2}`) is compared with
///   [`ValueExt::value_eq`], which for a machine scalar *is* the value itself —
///   so `2 @in set{1, 2}` holds and `3 @in set{1, 2}` does not.  A member the
///   low type vocabulary cannot classify stays on this side too, which is what
///   keeps a `string` type a non-member of a class domain (`add "a" "b"`).
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

/// Whether a set's member and the tested value are the same member: the class
/// each denotes when **both** denote one, and [`ValueExt::value_eq`] otherwise.
fn same_member<P: Program>(module: &Module<P>, member: AnyNodeId, value: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    match (known_shape(module, member), known_shape(module, value)) {
        (Some(member), Some(value)) => member == value,
        _ => same_value(module, member, value),
    }
}

/// [`ValueExt::value_eq`] over two nodes' values; `false` when either has none
/// (an unbound cell, or a node that is not a value at all).
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
