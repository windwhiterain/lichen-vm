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

use lichen_lowlevel::{AnyNodeId, LowShape, Module, Program};

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

/// Whether `class`'s class is a member of the set whose value is `set` — the
/// membership test [`crate::program::TypeOperator::InDomain`] runs, and the one
/// a class domain is read with.
///
/// Both sides are *structural*: a member is matched through [`low_type_of`], so
/// two structurally identical class nodes from different modules match and no
/// node identity is involved.  A class or member the low type vocabulary cannot
/// classify is not a member — the conservative answer, and the one that keeps a
/// domain from admitting a type nobody can classify (a `string` answers
/// `Unknown` and is therefore refused, which is `add "a" "b"`).
pub fn contains<P: Program>(module: &Module<P>, set: AnyNodeId, class: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(class) = known_shape(module, class) else {
        return false;
    };
    members(module, set).is_some_and(|members| {
        members
            .into_iter()
            .any(|member| known_shape(module, member).is_some_and(|member| member == class))
    })
}

/// [`low_type_of`]'s answer for a type value, `None` when it is undecided.
fn known_shape<P: Program>(module: &Module<P>, node: AnyNodeId) -> Option<LowShape>
where
    P::Value: ValueType,
{
    let shape = low_type_of(module, node);
    shape.is_known().then_some(shape)
}
