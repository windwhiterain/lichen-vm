//! The **class domain**: the set of scalar classes a contract admits, as a
//! *value*.
//!
//! `+`'s contract is `{Int, Float}`; the set is data — what a declaration names
//! and what a reader that needs the **candidates** consumes (the kernel, which
//! must answer an open domain with a class, and the diagnostics, which must name
//! the domain they refused).  The *check* is a refinement: a predicate on the
//! refined value required to evaluate to `1`
//! (`docs/notes/operator-polymorphism.md` §3).
//!
//! # Why the encoding has three elements
//!
//! A domain is `[TypeSet, [members], default]` — a tag, the member type values,
//! and the class a reader should fall back to.  The arity is load-bearing, not
//! cosmetic: [`crate::shape::node_holds_type`] decides "is this a type value" by
//! **silhouette** once tags fail, and `is_struct_marker_any` accepts any
//! two-element array as a struct marker.  A two-element `[TypeSet, [members]]`
//! is therefore read as a *kinded type expression* whose kind is a struct
//! marker, which is exactly the type-system membership this design exists to
//! avoid.  Three elements match none of the recognised shapes, so "not a type"
//! is a property of the representation.
//!
//! The domain is never a runtime value's type and never a member of a type
//! spine.  Nothing unifies against it: no rule is added to `unify_inner`.

use lichen_lowlevel::{AnyNodeId, LowShape, Module, NodeId, Program};

use crate::program::{Ctx, HighProgram, TypeValue, ValueType};
use crate::shape::{array_items, low_type_of};

/// Element 0 of a domain value — the tag.
const DOMAIN_TAG_SLOT: usize = 0;
/// Element 1 of a domain value — the member list.
const DOMAIN_MEMBERS_SLOT: usize = 1;
/// Element 2 of a domain value — the class a reader falls back to.
const DOMAIN_DEFAULT_SLOT: usize = 2;
/// The three elements [`Self::build`] lays out.
const DOMAIN_LEN: usize = 3;

/// A class domain value: `[TypeSet, [members], default]`.
///
/// The default is stated here rather than left to "the first member" because a
/// reader that has to pick a class (the kernel boundary) is making a choice the
/// declaration is the right place to record.
pub fn build<P: HighProgram>(ctx: &mut dyn Ctx<P>, members: &[NodeId], default: NodeId) -> NodeId
where
    P::Value: ValueType,
{
    let tag = ctx.value_node(P::Value::from(TypeValue::TypeSet));
    let members = ctx.array_node(members);
    ctx.array_node(&[tag, members, default])
}

/// Whether `node` holds a class domain.
pub fn is_domain<P: Program>(module: &Module<P>, node: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `node` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, node) }) else {
        return false;
    };
    items.len() == DOMAIN_LEN
        && module.node_value(items[DOMAIN_TAG_SLOT].node)
            == Some(P::Value::from(TypeValue::TypeSet))
}

/// The member type values of a class domain, or `None` when `node` is not one.
pub fn members<P: Program>(module: &Module<P>, node: AnyNodeId) -> Option<Vec<AnyNodeId>>
where
    P::Value: ValueType,
{
    if !is_domain(module, node) {
        return None;
    }
    // SAFETY: `node` is a live node of `module`, and `is_domain` just read its
    // three elements; the note on that read covers this one.
    let items = unsafe { array_items(module, node) }?;
    // SAFETY: the members slot of the live node `node`; the note above covers it.
    let members = unsafe { array_items(module, items[DOMAIN_MEMBERS_SLOT].node) }?;
    Some(members.iter().map(|item| item.node).collect())
}

/// The class a reader falls back to for `node`'s domain, or `None` when `node`
/// is not a domain.
pub fn default_class<P: Program>(module: &Module<P>, node: AnyNodeId) -> Option<AnyNodeId>
where
    P::Value: ValueType,
{
    if !is_domain(module, node) {
        return None;
    }
    // SAFETY: `node` is a live node of `module`; `is_domain` read the same
    // three-element array.
    let items = unsafe { array_items(module, node) }?;
    Some(items[DOMAIN_DEFAULT_SLOT].node)
}

/// Whether `class`'s class is a member of `domain` — the membership test
/// [`crate::program::TypeOperator::InDomain`] runs.
///
/// Both sides are *structural*: a member is matched through
/// [`low_type_of`], the same decode the kernel boundary reads a parameter's
/// domain with, so two structurally identical class nodes from different modules
/// match and no node identity is involved.  A class or member the low type
/// vocabulary cannot classify is not a member — the conservative answer, and the
/// one that keeps a domain from admitting a type nobody can classify (a
/// `string` answers `Unknown` and is therefore refused, which is §5's
/// `add "a" "b"`).
pub fn contains<P: Program>(module: &Module<P>, domain: AnyNodeId, class: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(class) = known_shape(module, class) else {
        return false;
    };
    members(module, domain).is_some_and(|members| {
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
