//! The single authority for the pair/type encoding. See
//! `docs/notes/checker-encoding-instability.md`.
//!
//! # Invariant
//! Every expression compiles to a `[value, type, attrs…]` pair, every kinded
//! type expression to `[shape, [marker, universe]]`, and every type spine
//! bottoms out at the self-referential universe `K = [Type, ↺]`.

use lichen_lowlevel::{
    AnyHandle, AnyNodeId, ArrayItem, LowShape, LowValue, Module, NodeId, Program, StaticNodeId,
    TableItem, UnifyStep,
};
use lichen_utils::extend::AsEnum;

use crate::attr::AttrSet;
use crate::ir::LocStep;
use crate::program::{HighProgram, ValueType};

// --- the kind-marker registry ---
//
// THE one list of the 9 kind markers; every other definition is macro-derived.

// Each entry: `{ variant doc, codec tag, display name, ValueType method, Ctx
// accessor }`.

// The codec tag is the compatibility contract with persisted artifacts: an
// existing tag must NEVER change.

// A new marker takes the next unused tag, deliberately NOT the next position.

// `TypeValue::TypeId` holds tag `8`, so a new marker starts at `9`.
// `KIND_MARKER_TAGS` checks both halves.

// A consumer macro receives the whole list; an optional `[ args… ]` group is
// forwarded verbatim for call-site context.
macro_rules! for_each_kind_marker {
    ($mac:ident $( [ $($args:tt)* ] )?) => {
        $mac! { $( [ $($args)* ] )?
            /// The `int` type constant — `USize` literals pair with `[int, K]`.
            TypeInt { 0, "int", int_marker, int_marker_node }
            /// The `float` type constant — `Float` literals pair with
            /// `[float, K]`.
            TypeFloat { 9, "float", float_marker, float_marker_node }
            /// The `string` type constant — the builtin immutable string
            /// value; `Str` literals pair with `[string, K]`.
            TypeString { 7, "string", string_marker, string_marker_node }
            /// The `Type` constant — the canonical universe node itself
            /// (`Type : Type`).
            TypeType { 1, "Type", type_marker, type_marker_node }
            /// The kind marker of function type expressions — the pair's
            /// second element is a `Function` value.
            TypeFunction { 2, "FunctionType", function_type_marker, function_type_marker_node }
            /// The kind marker of tuple type expressions — the shape is the
            /// element-type list.
            TypeTuple { 3, "TupleType", tuple_type_marker, tuple_type_marker_node }
            /// The kind marker of array type expressions — the shape is
            /// `[element type, length]`.
            TypeArray { 4, "ArrayType", array_type_marker, array_type_marker_node }
            /// The kind marker of struct type expressions: the nominal id bundled with
            /// the field-type list.
            TypeStruct { 5, "TypeStruct", type_struct_marker, type_struct_marker_node }
            /// The kind marker of table type expressions — the shape is
            /// `[key type, value type]`.
            TypeTable { 6, "TypeTable", table_type_marker, table_type_marker_node }
            /// The kind marker of set type expressions: the **element type alone**, so
            /// `set(a)` and `set(a, b)` share one type.
            TypeSet { 10, "SetType", set_type_marker, set_type_marker_node }
        }
    };
}
pub(crate) use for_each_kind_marker;
/// The registry's tags, so its compatibility contract is checked at compile time.
macro_rules! define_kind_marker_tags {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        const KIND_MARKER_TAGS: &[u32] = &[ $( $tag ),* ];
    };
}
for_each_kind_marker!(define_kind_marker_tags);

// A registry tag is unique, and none claims the codec's own `TypeId` tag.
const _: () = {
    let tags = KIND_MARKER_TAGS;
    let mut left = 0;
    while left < tags.len() {
        assert!(
            tags[left] != 8,
            "a kind marker claims the codec's TypeId tag"
        );
        let mut right = left + 1;
        while right < tags.len() {
            assert!(tags[left] != tags[right], "two kind markers claim one tag");
            right += 1;
        }
        left += 1;
    }
};

// --- pair layout: `[value, type, attrs…]`, one spelling of the slots ---------

/// Element 0 of an expression's `[value, type, attrs…]` pair — the value.
pub const PAIR_VALUE_SLOT: usize = 0;
/// Element 1 of an expression's `[value, type, attrs…]` pair — the type.
pub const PAIR_TYPE_SLOT: usize = 1;
/// The first attribute slot: the schema tail's attribute `i` sits at
/// `PAIR_ATTR_BASE + i` of the pair.
pub const PAIR_ATTR_BASE: usize = 2;

/// The pair slot of the schema tail's `index`-th attribute.
pub const fn attr_slot(index: usize) -> usize {
    PAIR_ATTR_BASE + index
}

/// The schema-tail index of pair attribute `slot` — the inverse of
/// [`attr_slot`].
pub const fn attr_index(slot: usize) -> usize {
    slot - PAIR_ATTR_BASE
}

/// The **element-slot width** of a compound type: the value, the type, and one
/// slot per attribute.
///
/// # Invariant
/// Every element of every compound kind has this width, so a written type's
/// slots and a value's unify however few positions either one spells. See
/// `docs/notes/compound-element-slots.md`.
pub fn slot_width<P: HighProgram>() -> usize {
    PAIR_ATTR_BASE + P::Attr::ORDER.len()
}

// --- kinded-type layout: `[shape, kind]` with kind `[marker, universe]` ------

/// Element 0 of a kinded type expression `[shape, kind]` — the shape, whose
/// meaning is the kind's.
pub const TYPE_SHAPE_SLOT: usize = 0;
/// Element 1 of a kinded type expression `[shape, kind]` — the kind
/// `[marker, universe]`.
pub const TYPE_KIND_SLOT: usize = 1;
/// Element 0 of a kind `[marker, universe]` — the kind marker.
pub const KIND_MARKER_SLOT: usize = 0;
/// Element 1 of a kind `[marker, universe]` — the universe the kind closes on.
pub const KIND_UNIVERSE_SLOT: usize = 1;

// --- struct marker: `[payload, TypeStruct]`, a normal type value -------------
//
// Its type read is the honest signal.

/// Element 0 of a struct marker — the marker's *value*: the payload
/// `[TypeId, names, names_in_order]`.
pub const STRUCT_MARKER_PAYLOAD_SLOT: usize = 0;
/// Element 1 of a struct marker — the `TypeStruct` atom: the honest signal.
pub const STRUCT_MARKER_TAG_SLOT: usize = 1;

/// Element 0 of a struct marker's payload `[TypeId, names, names_in_order]` —
/// the nominal type id.
pub const STRUCT_MARKER_ID_SLOT: usize = 0;
/// Element 1 of a struct marker's payload — the optional name→index table;
/// `LowValue::Error` when absent.
pub const STRUCT_MARKER_NAMES_SLOT: usize = 1;
/// Element 2 of a struct marker's payload — the field names in definition
/// order: the inverse of the name→index table.
pub const STRUCT_MARKER_NAMES_ORDER_SLOT: usize = 2;

// --- shape-half layout: each kind names its own shape positions --------------

/// Element 0 of an **array type**'s `[element type, length]` shape.
pub const ARRAY_TYPE_ELEMENT_SLOT: usize = 0;
/// Element 1 of an array type's shape — a `USize` value slot.
pub const ARRAY_TYPE_LENGTH_SLOT: usize = 1;
/// Element 0 of a **table type**'s `[key type, value type]` shape.
pub const TABLE_TYPE_KEY_SLOT: usize = 0;
/// Element 1 of a table type's shape.
pub const TABLE_TYPE_VALUE_SLOT: usize = 1;
/// Element 0 of a **function type**'s `[domain, codomain]` shape.
pub const FUNCTION_TYPE_DOMAIN_SLOT: usize = 0;
/// Element 1 of a function type's shape.
pub const FUNCTION_TYPE_CODOMAIN_SLOT: usize = 1;

/// The lazy index path from a struct **type** to its name table — the `a.x`
/// read walks it (`container_ty[1][0][0][1]`).
pub const STRUCT_TYPE_NAMES_PATH: [usize; 4] = [
    TYPE_KIND_SLOT,
    KIND_MARKER_SLOT,
    STRUCT_MARKER_PAYLOAD_SLOT,
    STRUCT_MARKER_NAMES_SLOT,
];

/// The lazy index path from a TypeStruct **kind** to its definition-order
/// names: the direct read of the kind node.
///
/// # Invariant
/// Reading *its* slot 1 of a callee's `[shape, kind]` term would pull the
/// shape half into the operand chain; that hazard needed the operand-forcing
/// pass, which is gone, so this path is kept as the direct read rather than
/// re-derived.
pub const STRUCT_KIND_NAMES_ORDER_PATH: [usize; 3] = [
    KIND_MARKER_SLOT,
    STRUCT_MARKER_PAYLOAD_SLOT,
    STRUCT_MARKER_NAMES_ORDER_SLOT,
];

/// The array items behind a dynamic node or a static ref — the raw read every
/// accessor here is built on.
///
/// # Safety
/// The caller must keep `id` reachable — its home block alive — for as long
/// as the returned slice is read: the contract
/// `AnyHandle::<[ArrayItem]>::items` states.
pub unsafe fn array_items<P: Program>(
    module: &Module<P>,
    id: AnyNodeId,
) -> Option<&'static [ArrayItem]>
where
    P::Value: AsEnum<LowValue>,
{
    let value = module.node_value(id)?;
    let LowValue::Array(array) = value.as_enum()? else {
        return None;
    };
    // SAFETY: as the `# Safety` contract states, `id`'s block outlives the read.
    Some(unsafe { array.items() })
}

/// The shape slot of a kinded type expression `[shape, kind]`. `None` when
/// `ty` is not a 2-element array.
pub fn shape_of<P: Program>(module: &Module<P>, ty: AnyNodeId) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    // SAFETY: `ty` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { array_items(module, ty) }?;
    if items.len() >= 2 {
        Some(items[TYPE_SHAPE_SLOT].node)
    } else {
        None
    }
}

/// The kind slot `[marker, universe]` of a kinded type expression. `None`
/// as [`shape_of`] is.
pub fn kind_of<P: Program>(module: &Module<P>, ty: AnyNodeId) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    // SAFETY: `ty` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { array_items(module, ty) }?;
    if items.len() >= 2 {
        Some(items[TYPE_KIND_SLOT].node)
    } else {
        None
    }
}

/// The marker slot of a kind `[marker, universe]`. `None` when `kind` is not
/// a 2-element array.
pub fn marker_of<P: Program>(module: &Module<P>, kind: AnyNodeId) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    // SAFETY: `kind` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { array_items(module, kind) }?;
    if items.len() >= 2 {
        Some(items[KIND_MARKER_SLOT].node)
    } else {
        None
    }
}

// --- universe recognition -----------------------------------------------------

/// Whether a static ref names the canonical universe — a 2-item
/// self-referential array, read by content.
pub fn is_static_universe<P: Program>(module: &Module<P>, sref: StaticNodeId) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `sref`'s payload is in a static module's arena, pinned while the
    // module stays registered.
    let Some(items) = (unsafe { array_items(module, AnyNodeId::Static(sref)) }) else {
        return false;
    };
    items.len() >= 2
        && module.node_value(items[PAIR_VALUE_SLOT].node) == Some(P::Value::type_marker())
        && matches!(items[PAIR_TYPE_SLOT].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
}

/// Whether `id` is the canonical universe: a dynamic node by equality class,
/// a static ref by content.
pub fn is_universe_any<P: Program>(module: &mut Module<P>, universe: NodeId, id: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    match id {
        AnyNodeId::Dynamic(node) => {
            module.equality_representative(node) == module.equality_representative(universe)
        }
        AnyNodeId::Static(sref) => is_static_universe(module, sref),
    }
}

// --- shape predicates: one `AnyNodeId` implementation per predicate ---------
//
// Each has a thin `NodeId` wrapper.

/// Whether `kind` is the kind expression `[marker, K]`.
pub fn kind_marker_is_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    kind: AnyNodeId,
    marker: P::Value,
) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `kind` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, kind) }) else {
        return false;
    };
    items.len() >= 2
        && module.node_value(items[KIND_MARKER_SLOT].node) == Some(marker)
        && is_universe_any(module, universe, items[KIND_UNIVERSE_SLOT].node)
}

/// [`kind_marker_is_any`] over a dynamic node.
pub fn kind_marker_is<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    kind: NodeId,
    marker: P::Value,
) -> bool
where
    P::Value: ValueType,
{
    kind_marker_is_any(module, universe, AnyNodeId::Dynamic(kind), marker)
}

/// Whether `ty` is a concrete **arrow-term** function type — what a written
/// `A -> B` lowers to.
pub fn is_arrow_type_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `ty` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, ty) }) else {
        return false;
    };
    items.len() >= 2
        && kind_marker_is_any(
            module,
            universe,
            items[TYPE_KIND_SLOT].node,
            P::Value::function_type_marker(),
        )
}

/// [`is_arrow_type_any`] over a dynamic node.
pub fn is_function_type<P: Program>(module: &mut Module<P>, universe: NodeId, ty: NodeId) -> bool
where
    P::Value: ValueType,
{
    is_arrow_type_any(module, universe, AnyNodeId::Dynamic(ty))
}

/// Whether `ty` is a struct type `[shape, [[payload, TypeStruct], K]]`, whose
/// marker carries that tag.
pub fn is_struct_type_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `ty` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, ty) }) else {
        return false;
    };
    if items.len() < 2 {
        return false;
    }
    // SAFETY: `items[TYPE_KIND_SLOT].node` is a live node of `module`; nothing
    // in this crate calls `Module::drop_block`.
    let Some(kind_items) = (unsafe { array_items(module, items[TYPE_KIND_SLOT].node) }) else {
        return false;
    };
    kind_items.len() >= 2
        && is_universe_any(module, universe, kind_items[KIND_UNIVERSE_SLOT].node)
        && is_struct_marker_any(module, kind_items[KIND_MARKER_SLOT].node)
}

/// Whether `ty` is a **tuple type** `[shape, [marker, K]]`, its kind carrying
/// the tuple marker.
pub fn is_tuple_type_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `ty` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, ty) }) else {
        return false;
    };
    items.len() >= 2
        && kind_marker_is_any(
            module,
            universe,
            items[TYPE_KIND_SLOT].node,
            P::Value::tuple_type_marker(),
        )
}

/// Whether `ty` is an **array type** `[shape, [marker, K]]`, its kind carrying
/// the array marker.
pub fn is_array_type_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `ty` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, ty) }) else {
        return false;
    };
    items.len() >= 2
        && kind_marker_is_any(
            module,
            universe,
            items[TYPE_KIND_SLOT].node,
            P::Value::array_type_marker(),
        )
}

/// An array type's **length**: its shape's second position, which is a count and
/// not an element slot.
pub fn array_length<P: Program>(module: &Module<P>, ty: TypeRef) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    let shape = field_list(module, ty)?;
    // SAFETY: `shape` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    Some(
        unsafe { array_items(module, shape) }?
            .get(ARRAY_TYPE_LENGTH_SLOT)?
            .node,
    )
}

/// Whether a value is a struct marker: the pair `[payload, TypeStruct]`.
///
/// # Invariant
/// The test is the **tag**, never the silhouette: the marker must be an
/// ordinary two-element `[value, type]` pair whose [`STRUCT_MARKER_TAG_SLOT`]
/// holds the `TypeStruct` atom. The payload is not inspected — its cells may
/// still be undecided while the marker is being built.
pub fn is_struct_marker_any<P: Program>(module: &Module<P>, marker: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `marker` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, marker) }) else {
        return false;
    };
    items.len() >= 2
        && module.node_value(items[STRUCT_MARKER_TAG_SLOT].node)
            == Some(P::Value::type_struct_marker())
}

/// Whether `ty` is a TypeStruct **kind** rather than a struct *type* — the
/// struct-kind requirement a field read's container states.
pub fn is_type_struct_kind_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `ty` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, ty) }) else {
        return false;
    };
    items.len() >= 2
        && is_universe_any(module, universe, items[KIND_UNIVERSE_SLOT].node)
        && is_struct_marker_any(module, items[KIND_MARKER_SLOT].node)
}

/// The pieces of a struct **type term** every reader needs: the universe slot,
/// the field list, and the name table.
///
/// # Invariant
/// The universe *gate* is the caller's: `struct_names_any` recognises it by
/// class equality and `field_names` by its self-referential cycle, and the walk
/// lives here so the two cannot drift.
fn struct_term_parts<P: Program>(
    module: &Module<P>,
    ty: AnyNodeId,
) -> Option<(AnyNodeId, AnyNodeId, AnyHandle<[TableItem]>)>
where
    P::Value: ValueType,
{
    // SAFETY: `ty` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { array_items(module, ty) }?;
    if items.len() < 2 {
        return None;
    }
    let shape = items[TYPE_SHAPE_SLOT].node;
    // SAFETY: `items[TYPE_KIND_SLOT].node` is a live node of `module`.
    let kind_items = unsafe { array_items(module, items[TYPE_KIND_SLOT].node) }?;
    if kind_items.len() < 2 {
        return None;
    }
    // The struct marker `[payload, TypeStruct]`; the tag is the honest signal.
    if !is_struct_marker_any(module, kind_items[KIND_MARKER_SLOT].node) {
        return None;
    }
    // SAFETY: `kind_items[KIND_MARKER_SLOT].node` is a live node of `module`.
    let marker_items = unsafe { array_items(module, kind_items[KIND_MARKER_SLOT].node) }?;
    // Its payload's name slot holds the name→index table.
    // SAFETY: the payload item's node is a live node of `module`.
    let payload_items =
        unsafe { array_items(module, marker_items[STRUCT_MARKER_PAYLOAD_SLOT].node) }?;
    let names_item = payload_items.get(STRUCT_MARKER_NAMES_SLOT)?;
    match module.node_value(names_item.node).and_then(|v| v.as_enum()) {
        Some(LowValue::Table(table)) => Some((kind_items[KIND_UNIVERSE_SLOT].node, shape, table)),
        _ => None,
    }
}

/// The struct's name→index table from a struct type value, or `None`.
///
/// # Invariant
/// The universe is supplied by the caller: a checker has one, a lowering does
/// not — [`field_names`] is this reader for that side.
pub fn struct_names_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> Option<AnyHandle<[TableItem]>>
where
    P::Value: ValueType,
{
    let (universe_slot, _, table) = struct_term_parts(module, ty)?;
    is_universe_any(module, universe, universe_slot).then_some(table)
}

// --- type references and field access ---------------------------------------

/// A type as the encoding holds it: the `[shape, kind]` term, or a node holding
/// one at its value slot.
///
/// # Invariant
/// Which one it is is stated, never guessed: a term and a holder are both
/// two-slot arrays, so only the caller's own knowledge separates them.
#[derive(Clone, Copy)]
pub enum TypeRef {
    /// The `[shape, kind]` expression itself — its shape at slot 0, its kind at
    /// slot 1.
    Term(AnyNodeId),
    /// A node whose value slot holds a `[shape, kind]` term, one level in.
    Slot(AnyNodeId),
}

impl TypeRef {
    /// The `[shape, kind]` term this names.
    pub fn term<P: Program>(self, module: &Module<P>) -> Option<AnyNodeId>
    where
        P::Value: AsEnum<LowValue>,
    {
        match self {
            TypeRef::Term(id) => Some(id),
            // SAFETY: `id` is a live node of `module`; nothing in this crate
            // calls `Module::drop_block`.
            TypeRef::Slot(id) => unsafe { array_items(module, id) }
                .and_then(|items| items.first())
                .map(|item| item.node),
        }
    }
}

/// The type an element **slot** names — its own `type` position.
pub fn slot_type<P: Program>(module: &Module<P>, slot: AnyNodeId) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    // SAFETY: `slot` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    unsafe { array_items(module, slot) }?
        .get(PAIR_TYPE_SLOT)
        .map(|item| item.node)
}

/// The **field-type list** of a value type — its term's shape: a struct's
/// fields and a tuple's elements share the slot.
pub fn field_list<P: Program>(module: &Module<P>, ty: TypeRef) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    shape_of(module, ty.term(module)?)
}

/// The type at field position `k` of a value type. See
/// `docs/notes/checker-encoding-instability.md`.
///
/// # Invariant
/// A compound type's shape element is an element **slot**, so a field's type is
/// its type position for every kind. See
/// `docs/notes/compound-element-slots.md`.
pub fn field_type<P: Program>(module: &Module<P>, ty: TypeRef, k: usize) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    let shape = field_list(module, ty)?;
    // SAFETY: `shape` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let element = unsafe { array_items(module, shape) }?.get(k)?.node;
    slot_type(module, element)
}

/// The **named fields** of a struct type, in field order, or `None`.
///
/// # Invariant
/// The universe is recognised by [`Module::is_self_referential`] — the
/// `[Type, ↺]` cycle — because the callers that need this (a lowering, which
/// has no universe handle to pass) are below the checker. The field count is
/// the shape's own length, so a name whose index maps outside the field list
/// is dropped rather than growing it.
pub fn field_names<P: Program>(module: &Module<P>, ty: TypeRef) -> Option<Vec<Option<&'static str>>>
where
    P::Value: ValueType,
{
    let (universe_slot, shape, table) = struct_term_parts(module, ty.term(module)?)?;
    if !module.is_self_referential(universe_slot) {
        return None;
    }
    // SAFETY: `shape` is a live node of `module`.
    let field_count = unsafe { array_items(module, shape) }?.len();
    let mut names: Vec<Option<&'static str>> = vec![None; field_count];
    // SAFETY: `table` is the payload of the value read from the live node
    // named by the marker payload's name slot.
    for item in unsafe { table.items() } {
        if let Some((name, index)) = module.name_table_entry(item)
            && index < field_count
        {
            names[index] = Some(name);
        }
    }
    Some(names)
}

/// The **named fields** and field-type list of the struct type a **slot** names.
///
/// # Invariant
/// The slot's convention stays here: the term is the slot itself, a node that
/// carries it at its value slot (an apply's answer), or either one as a class
/// member answers through its representative. A caller above `lowlevel` never
/// chooses among those spellings.
pub fn struct_fields<P: Program>(
    module: &Module<P>,
    slot: AnyNodeId,
) -> Option<(Vec<Option<&'static str>>, AnyNodeId)>
where
    P::Value: ValueType,
{
    for slot in type_slot_candidates(module, slot) {
        for ty in [TypeRef::Term(slot), TypeRef::Slot(slot)] {
            if let Some(names) = field_names(module, ty)
                && let Some(shape) = field_list(module, ty)
            {
                return Some((names, shape));
            }
        }
    }
    None
}

/// The types of a field-type list's entries: an element is a slot, so a field's
/// type is its type position.
pub fn field_types<P: Program>(module: &Module<P>, shape: AnyNodeId) -> Option<Vec<AnyNodeId>>
where
    P::Value: AsEnum<LowValue>,
{
    // SAFETY: `shape` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { array_items(module, shape) }?;
    Some(
        items
            .iter()
            .map(|item| slot_type(module, item.node).unwrap_or(item.node))
            .collect(),
    )
}

/// The type node at `path` inside the struct type a slot names — a leaf's own
/// type, resolved one field at a time.
pub fn field_type_at<P: Program>(
    module: &Module<P>,
    slot: AnyNodeId,
    path: &[usize],
) -> Option<AnyNodeId>
where
    P::Value: ValueType,
{
    let mut at = slot;
    for &index in path {
        let (_, shape) = struct_fields(module, at)?;
        at = field_types(module, shape)?.get(index).copied()?;
    }
    Some(at)
}

/// The spellings a type **slot** may wear: itself, and — for a class member — its
/// representative first.
fn type_slot_candidates<P: Program>(module: &Module<P>, slot: AnyNodeId) -> Vec<AnyNodeId>
where
    P::Value: ValueType,
{
    let mut candidates = vec![slot];
    if let AnyNodeId::Dynamic(node) = slot {
        let representative = module.class_root(node);
        if representative != node {
            candidates.push(AnyNodeId::Dynamic(representative));
        }
    }
    candidates
}

/// The **domain** a kernel function's parameter states, read from its type.
///
/// # Invariant
/// A pure read: nothing is seeded and no inference runs. `None` is "the type does
/// not state a domain" (a polymorphic template), which the caller refuses.
pub fn kernel_parameter_domain<P: Program>(
    module: &Module<P>,
    function: lichen_lowlevel::AnyFunctionId,
) -> Option<LowShape>
where
    P::Value: ValueType,
{
    let lichen_lowlevel::AnyFunctionId::Dynamic(function) = function else {
        return None;
    };
    let pair = module.functions.get(function)?.parameter;
    let type_slot = module.pair_type_item(pair)?;
    // A type pair's value position is the shape and its type position the kind
    // (`docs/notes/compound-element-slots.md`).
    low_type_of(module, type_slot).or_else(|| low_type_of_slot(module, type_slot))
}

/// Prepare a template's low types for a lowering: seed each term's type cell and
/// run the fixed point over the body.
///
/// # Invariant
/// The encoding and the class channel stay here; a lowering reads values'
/// shapes through [`value_low_shape`] and never seeds or infers itself.
pub fn prepare_template_low_types<P: Program>(
    module: &mut Module<P>,
    function: lichen_lowlevel::FunctionId,
) where
    P::Value: ValueType,
{
    let mut seeds: Vec<(lichen_lowlevel::NodeId, lichen_lowlevel::LowShape)> = Vec::new();
    for node in module.functions[function].nodes.clone() {
        let Some(LowValue::Array(array)) = module
            .node_value(AnyNodeId::Dynamic(node))
            .and_then(|value| value.as_enum())
        else {
            continue;
        };
        // Only a pair carries a layout; width cannot tell it from a data tuple
        // (`docs/notes/array-attribute-view.md`).
        if array.layout() == 0 {
            continue;
        }
        // SAFETY: `array` is the payload of a value of this module.
        let items = unsafe { array.items() };
        // A term is **full width** (`docs/notes/attributes.md`).
        let width = PAIR_ATTR_BASE + module.attribute_layout(array.layout()).len();
        if items.len() != width {
            continue;
        }
        let Some(shape) = low_type_of_slot(module, items[PAIR_TYPE_SLOT].node) else {
            continue;
        };
        let Some(value) = items[PAIR_VALUE_SLOT].node.dynamic() else {
            continue;
        };
        // The wildcard states nothing, so it has no shape to seed.
        if matches!(
            module
                .node_value(AnyNodeId::Dynamic(value))
                .and_then(|value| value.as_enum()),
            Some(LowValue::Any)
        ) {
            continue;
        }
        seeds.push((value, shape));
    }
    // The inference runs first, so a seed is only for what it left shapeless
    // (`docs/notes/compound-element-slots.md`).
    module.infer_template_low_types(function);
    for (value, shape) in seeds {
        // An open pair is an undecided type: it states nothing to seed.
        let states_nothing = match &shape {
            lichen_lowlevel::LowShape::Tuple(items) => items.iter().all(Option::is_none),
            _ => false,
        };
        if !states_nothing && module.class_low_type(value).is_none() {
            module.seed_class_low_type(value, shape);
        }
    }
}

/// The shape of the value at `node`, as the graph recorded it for its class.
pub fn value_low_shape<P: Program>(module: &Module<P>, node: AnyNodeId) -> Option<LowShape>
where
    P::Value: ValueType,
{
    node.dynamic()
        .and_then(|node| module.low_type_of_node(node))
}

/// A function's **parameter pair** — the `[value, type]` term a lowering reads.
pub fn function_parameter_pair<P: Program>(
    module: &Module<P>,
    function: lichen_lowlevel::AnyFunctionId,
) -> Option<lichen_lowlevel::NodeId> {
    let lichen_lowlevel::AnyFunctionId::Dynamic(function) = function else {
        return None;
    };
    Some(module.functions.get(function)?.parameter)
}

/// A pair's **value half**.
pub fn pair_value<P: Program>(
    module: &Module<P>,
    pair: lichen_lowlevel::NodeId,
) -> Option<lichen_lowlevel::NodeId> {
    module.pair_value_half(pair)
}

/// A pair's **type half**.
pub fn pair_type<P: Program>(
    module: &Module<P>,
    pair: lichen_lowlevel::NodeId,
) -> Option<AnyNodeId> {
    module.pair_type_item(pair)
}

/// Whether two references name **one value**: the encoding's identity is the
/// equality class, and that read stays here.
pub fn is_same_value<P: Program>(module: &Module<P>, a: AnyNodeId, b: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    match (a.dynamic(), b.dynamic()) {
        (Some(a), Some(b)) => module.class_root(a) == module.class_root(b),
        _ => false,
    }
}

/// Where `name` sits in a name→index table — the fold a named read performs.
///
/// # Invariant
/// The stored position is returned as it stands: the table is the authority
/// here, and a caller holding a field list bounds it against that.
pub fn name_table_index<P: Program>(
    module: &Module<P>,
    table: AnyHandle<[TableItem]>,
    name: &str,
) -> Option<usize>
where
    P::Value: AsEnum<LowValue>,
{
    module.name_table_index(table, name)
}

// --- low types -----------------------------------------------------------------

/// The **low type** a kinded type expression denotes. See
/// `docs/notes/lowlevel-low-types.md`.
///
/// # Invariant
/// The decode is by **kind marker** and answers `None` rather than falling back to
/// a scalar: a type the vocabulary has no shape for states *nothing*, and a silent
/// fallback would let a `jit` compile a domain it invented. A position inside a
/// shape that nothing states is `None` too; the shape type has no `Unknown`.
pub fn low_type_of<P: Program>(module: &Module<P>, type_value: AnyNodeId) -> Option<LowShape>
where
    P::Value: ValueType,
{
    // A type constant's own **value** is its marker node, and both spellings
    // denote one class.
    if let Some(value) = module.node_value(type_value) {
        if value == P::Value::int_marker() {
            return Some(LowShape::USize);
        }
        if value == P::Value::float_marker() {
            return Some(LowShape::Float);
        }
        if value == P::Value::string_marker() || value == P::Value::type_marker() {
            // A string is not a machine scalar, and a type is not a value at all.
            return None;
        }
    }
    // Anything that is not a `[shape, kind]` type expression is unreadable.

    // SAFETY: `type_value` is a live node of `module`; nothing in this crate
    // calls `Module::drop_block`.
    let kinded = (unsafe { array_items(module, type_value) })?;
    if kinded.len() < 2 {
        return None;
    }
    // A **function value's own type**, `f : f`: its halves are its own
    // signature cells, read by `function_shape`.
    if let Some((domain, codomain)) = module.function_type_signature(type_value) {
        return Some(function_shape(module, domain, codomain));
    }
    let shape = kinded[TYPE_SHAPE_SLOT].node;
    // An atomic type: its shape slot holds the marker itself.
    let shape_value = module.node_value(shape)?;
    if shape_value == P::Value::int_marker() {
        return Some(LowShape::USize);
    }
    if shape_value == P::Value::float_marker() {
        // A float is a **decided** member of the vocabulary, so a declared
        // `[float, K]` states it.
        return Some(LowShape::Float);
    }
    if shape_value == P::Value::string_marker() || shape_value == P::Value::type_marker() {
        return None;
    }
    // A compound type: the marker is the kind's.

    // SAFETY: `kinded[TYPE_KIND_SLOT].node` is a live node of `module`; nothing
    // in this crate calls `Module::drop_block`.
    let kind = (unsafe { array_items(module, kinded[TYPE_KIND_SLOT].node) })?;
    if kind.len() < 2 {
        return None;
    }
    let marker = module.node_value(kind[KIND_MARKER_SLOT].node)?;
    if marker == P::Value::tuple_type_marker()
        || is_struct_marker_any(module, kind[KIND_MARKER_SLOT].node)
    {
        // A structure's shape is its positional field list: read as a tuple
        // (`docs/notes/lowlevel-low-types.md`).

        // SAFETY: `shape` is a live node of `module`; nothing in this crate
        // calls `Module::drop_block`.
        let elements = (unsafe { array_items(module, shape) })?;
        return Some(LowShape::Tuple(
            elements
                .iter()
                .map(|element| {
                    slot_type(module, element.node).and_then(|ty| low_type_of_slot(module, ty))
                })
                .collect(),
        ));
    }
    if marker == P::Value::array_type_marker() {
        // An undecided element makes the whole shape undecided — a shape nobody
        // states is not a shape of `None`.
        return match low_type_of_array(module, shape) {
            Some((Some(element), length)) => Some(LowShape::Array(Box::new(Some(element)), length)),
            Some((None, length)) => Some(LowShape::Array(Box::new(None), length)),
            None => None,
        };
    }
    if marker == P::Value::table_type_marker() {
        // A table type's shape is `[key slot, value slot]`.

        // SAFETY: `shape` is a live node of `module`; nothing in this crate
        // calls `Module::drop_block`.
        let halves = (unsafe { array_items(module, shape) })?;
        let half =
            |node: AnyNodeId| slot_type(module, node).and_then(|ty| low_type_of_slot(module, ty));
        return Some(LowShape::Table(
            Box::new(half(halves[TABLE_TYPE_KEY_SLOT].node)),
            Box::new(half(halves[TABLE_TYPE_VALUE_SLOT].node)),
        ));
    }
    if marker == P::Value::function_type_marker() {
        // A function type's shape is the `[domain, codomain]` pair.

        // SAFETY: `shape` is a live node of `module`; nothing in this crate
        // calls `Module::drop_block`.
        let halves = (unsafe { array_items(module, shape) })?;
        return Some(function_shape(
            module,
            halves[FUNCTION_TYPE_DOMAIN_SLOT].node,
            halves[FUNCTION_TYPE_CODOMAIN_SLOT].node,
        ));
    }
    // A kind this decoder does not know.
    None
}

/// A **function shape** built from its two halves — the one spelling of "read
/// the two positions".
///
/// # Invariant
/// Each half is read through [`low_type_of_slot`]: a half may be a type value or
/// a term's type cell, and that reader is the authority for telling them apart.
fn function_shape<P: Program>(
    module: &Module<P>,
    domain: AnyNodeId,
    codomain: AnyNodeId,
) -> LowShape
where
    P::Value: ValueType,
{
    LowShape::Function(
        Box::new(low_type_of_slot(module, domain)),
        Box::new(low_type_of_slot(module, codomain)),
    )
}

/// Whether `node` holds a **kind marker** — the type position of a type pair.
fn is_a_kind_marker<P: Program>(module: &Module<P>, node: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `node` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(kind) = (unsafe { array_items(module, node) }) else {
        return false;
    };
    let Some(marker) = kind.first().and_then(|item| module.node_value(item.node)) else {
        return false;
    };
    marker == P::Value::tuple_type_marker()
        || marker == P::Value::array_type_marker()
        || marker == P::Value::table_type_marker()
        || marker == P::Value::function_type_marker()
        || is_struct_marker_any(module, kind[0].node)
}

/// The low type an expression's **type slot** names: a backend's seed for a
/// template's parameter domain.
///
/// # Invariant
/// The two are told apart by **whether the decode succeeds**, not by a
/// structural guess: a term pair and a type value have the same two-slot
/// silhouette. What separates them is the terminal marker — the `Type` marker,
/// the one kind [`low_type_of`] refuses — so a pair never decodes directly.
pub fn low_type_of_slot<P: Program>(module: &Module<P>, slot: AnyNodeId) -> Option<LowShape>
where
    P::Value: ValueType,
{
    // A type pair's kind position holds a kind marker
    // (`docs/notes/compound-element-slots.md`).
    if let AnyNodeId::Dynamic(node) = slot
        && module
            .pair_type_item(node)
            .is_some_and(|kind| is_a_kind_marker(module, kind))
        && let Some(shape) = low_type_of(module, slot)
    {
        return Some(shape);
    }
    // A pair holds its type value at the value position; a type value is the
    // `[shape, kind]` array itself.
    let mut slot = slot;
    for _ in 0..8 {
        let Some(LowValue::Array(array)) =
            module.node_value(slot).and_then(|value| value.as_enum())
        else {
            break;
        };
        if array.layout() == 0 {
            break;
        }
        // SAFETY: `array` is a live payload of a node of `module`.
        let items = unsafe { array.items() };
        let Some(value) = items.first().map(|item| item.node) else {
            break;
        };
        slot = value;
    }
    low_type_of(module, slot)
}

/// [`low_type_of`]'s array arm; `None` when the length is undecided.
fn low_type_of_array<P: Program>(
    module: &Module<P>,
    shape: AnyNodeId,
) -> Option<(Option<LowShape>, usize)>
where
    P::Value: ValueType,
{
    // SAFETY: `shape` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let parts = unsafe { array_items(module, shape) }?;
    if parts.len() != 2 {
        return None;
    }
    // An array's element position is a **slot**, so the element type is its type
    // position.
    let element = slot_type(module, parts[ARRAY_TYPE_ELEMENT_SLOT].node)
        .and_then(|ty| low_type_of_slot(module, ty));
    let LowValue::USize(length) = module
        .node_value(parts[ARRAY_TYPE_LENGTH_SLOT].node)?
        .as_enum()?
    else {
        return None;
    };
    Some((element, length))
}

// --- diagnostic descent ---------------------------------------------------------

/// The full-parse walk: append a [`LocStep`] per unify `step`, tagging each
/// node as a pair slot or a shape position.
///
/// # Invariant
/// Both sides of a unify are structurally parallel, so the tags are identical
/// whichever side is tracked; `b` is just the one the location is anchored to.
pub(crate) fn tag_descent<P: Program>(
    module: &Module<P>,
    mut path: Vec<LocStep>,
    start: NodeId,
    steps: &[UnifyStep],
) -> Vec<LocStep> {
    let mut cur = start;
    let mut in_shape = false;
    for step in steps {
        let tag = if in_shape {
            in_shape = false;
            LocStep::Elem(step.index)
        } else if step.index == PAIR_VALUE_SLOT {
            if slot0_is_shape(module, cur) {
                in_shape = true;
                LocStep::Shape
            } else {
                LocStep::Value
            }
        } else if step.index == PAIR_TYPE_SLOT {
            LocStep::Type
        } else {
            LocStep::Attr(attr_index(step.index))
        };
        path.push(tag);
        cur = step.b;
    }
    path
}

/// Whether `node`'s element 0 is a list — a shape, not an expression's pair.
///
/// # Invariant
/// A diagnostic rendering hint, never a check: the heuristic misfires on a pair
/// whose *value* is itself an array, tagging that `Value` descent as `Shape`.
fn slot0_is_shape<P: Program>(module: &Module<P>, node: NodeId) -> bool {
    // SAFETY: `node` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { module.array_items(node) }) else {
        return false;
    };
    if items.is_empty() {
        return false;
    }
    match items[0].node {
        // SAFETY: as above — `child` is a live node of `module`.
        AnyNodeId::Dynamic(child) => unsafe { module.array_items(child) }.is_some(),
        // A static element is a leaf (a package export); it is never a
        // tuple/array/struct shape we descend into.
        AnyNodeId::Static(_) => false,
    }
}
