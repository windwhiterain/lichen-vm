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

use crate::ir::LocStep;
use crate::program::ValueType;

// --- the kind-marker registry ---
//
// THE one list of the 9 kind markers; every other definition is macro-derived.

// Each entry: `{ variant doc, codec tag, display name, ValueType method, Ctx
// accessor }`.

// The codec tag is the compatibility contract with persisted artifacts: an
// existing tag must NEVER change.

// A new marker takes the next unused tag, deliberately NOT the next position.

// `TypeValue::TypeId` holds tag `8` there, so a new kind marker starts at
// `9`. See `docs/notes/code-audit.md` P1-36.

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
            /// `set{a}` and `set{a, b}` share one type.
            TypeSet { 10, "SetType", set_type_marker, set_type_marker_node }
        }
    };
}
pub(crate) use for_each_kind_marker;

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

/// The lazy index path from a TypeStruct **kind** to its name table; the
/// `X::a` read walks it (`container_ty[0][0][1]`).
pub const STRUCT_KIND_NAMES_PATH: [usize; 3] = [
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
    if items.len() == 2 {
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
    if items.len() == 2 {
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
    if items.len() == 2 {
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
    items.len() == 2
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
    items.len() == 2
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
    items.len() == 2
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
    if items.len() != 2 {
        return false;
    }
    // SAFETY: `items[TYPE_KIND_SLOT].node` is a live node of `module`; nothing
    // in this crate calls `Module::drop_block`.
    let Some(kind_items) = (unsafe { array_items(module, items[TYPE_KIND_SLOT].node) }) else {
        return false;
    };
    kind_items.len() == 2
        && is_universe_any(module, universe, kind_items[KIND_UNIVERSE_SLOT].node)
        && is_struct_marker_any(module, kind_items[KIND_MARKER_SLOT].node)
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
    items.len() == 2
        && module.node_value(items[STRUCT_MARKER_TAG_SLOT].node)
            == Some(P::Value::type_struct_marker())
}

/// Whether `ty` is a TypeStruct **kind** rather than a struct *type* — the
/// form a raw named read `X::a` requires.
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
    items.len() == 2
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
    if items.len() != 2 {
        return None;
    }
    let shape = items[TYPE_SHAPE_SLOT].node;
    // SAFETY: `items[TYPE_KIND_SLOT].node` is a live node of `module`.
    let kind_items = unsafe { array_items(module, items[TYPE_KIND_SLOT].node) }?;
    if kind_items.len() != 2 {
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
pub fn field_type<P: Program>(module: &Module<P>, ty: TypeRef, k: usize) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    let shape = field_list(module, ty)?;
    // SAFETY: `shape` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    unsafe { array_items(module, shape) }?
        .get(k)
        .map(|item| item.node)
}

/// The **named fields** of a struct type, in field order, or `None`.
    ///
    /// # Invariant
    /// The universe is recognised by [`Module::is_self_referential`] — the
    /// `[Type, ↺]` cycle — because this reader's callers are below the checker and
    /// have no universe handle to pass. The field count is the shape's own length,
    /// so a name whose index maps outside the field list is dropped rather than
    /// growing it.
pub fn field_names<P: Program>(
    module: &mut Module<P>,
    ty: TypeRef,
) -> Option<Vec<Option<&'static str>>>
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
        if let Some((name, index)) = name_table_entry(module, item)
            && index < field_count
        {
            names[index] = Some(name);
        }
    }
    Some(names)
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
    // SAFETY: `table` is the payload of the value read from a live node of
    // `module`.
    unsafe { table.items() }
        .iter()
        .find_map(|item| match name_table_entry(module, item) {
            Some((entry, index)) if entry == name => Some(index),
            _ => None,
        })
}

/// One name-table entry as its `(name, position)` pair, or `None` for an entry
/// still being built.
fn name_table_entry<P: Program>(
    module: &Module<P>,
    item: &TableItem,
) -> Option<(&'static str, usize)>
where
    P::Value: AsEnum<LowValue>,
{
    let name = match module.node_value(item.key)?.as_enum()? {
        LowValue::Str(name) => name,
        _ => return None,
    };
    match module.node_value(item.value)?.as_enum()? {
        LowValue::USize(index) => Some((name, index)),
        _ => None,
    }
}

// --- low types -----------------------------------------------------------------

/// The **low type** a kinded type expression denotes. See
/// `docs/notes/lowlevel-low-types.md`.
    ///
    /// # Invariant
    /// The decode is by **kind marker** and says [`LowShape::Unknown`] explicitly
    /// rather than falling back to a scalar: a type the vocabulary has no shape for
    /// states *nothing*, and a silent fallback would let a `jit` compile a domain it
    /// invented.
pub fn low_type_of<P: Program>(module: &Module<P>, type_value: AnyNodeId) -> LowShape
where
    P::Value: ValueType,
{
    // A type constant's own **value** is its marker node, and both spellings
    // denote one class.
    if let Some(value) = module.node_value(type_value) {
        if value == P::Value::int_marker() {
            return LowShape::USize;
        }
        if value == P::Value::float_marker() {
            return LowShape::Float;
        }
        if value == P::Value::string_marker() || value == P::Value::type_marker() {
            // The same refusals as the array spelling below: a string is not a
            // machine scalar, and a type is not a value at all.
            return LowShape::Unknown;
        }
    }
    // Anything that is not a `[shape, kind]` type expression is unreadable.

    // SAFETY: `type_value` is a live node of `module`; nothing in this crate
    // calls `Module::drop_block`.
    let Some(kinded) = (unsafe { array_items(module, type_value) }) else {
        return LowShape::Unknown;
    };
    if kinded.len() != 2 {
        return LowShape::Unknown;
    }
    // A **function value's own type**, `f : f`: its halves are its own
    // signature cells, read by `function_shape`.
    if let Some((domain, codomain)) = module.function_type_signature(type_value) {
        return function_shape(module, domain, codomain);
    }
    let shape = kinded[TYPE_SHAPE_SLOT].node;
    // An atomic type: its shape slot holds the marker itself.
    let Some(shape_value) = module.node_value(shape) else {
        return LowShape::Unknown;
    };
    if shape_value == P::Value::int_marker() {
        return LowShape::USize;
    }
    if shape_value == P::Value::float_marker() {
        // A float is a **decided** member of the vocabulary, so a declared
        // `[float, K]` states it.
        return LowShape::Float;
    }
    if shape_value == P::Value::string_marker() || shape_value == P::Value::type_marker() {
        // A string is not a machine scalar, and a type is not a value at all.
        return LowShape::Unknown;
    }
    // A compound type: the marker is the kind's.

    // SAFETY: `kinded[TYPE_KIND_SLOT].node` is a live node of `module`; nothing
    // in this crate calls `Module::drop_block`.
    let Some(kind) = (unsafe { array_items(module, kinded[TYPE_KIND_SLOT].node) }) else {
        return LowShape::Unknown;
    };
    if kind.len() != 2 {
        return LowShape::Unknown;
    }
    let Some(marker) = module.node_value(kind[KIND_MARKER_SLOT].node) else {
        return LowShape::Unknown;
    };
    if marker == P::Value::tuple_type_marker() {
        // A tuple type's shape *is* its element-type list.

        // SAFETY: `shape` is a live node of `module`; nothing in this crate
        // calls `Module::drop_block`.
        let Some(elements) = (unsafe { array_items(module, shape) }) else {
            return LowShape::Unknown;
        };
        return LowShape::Tuple(
            elements
                .iter()
                .map(|element| low_type_of(module, element.node))
                .collect(),
        );
    }
    if marker == P::Value::array_type_marker() {
        // An undecided length makes the whole shape undecided — a length
        // nobody knows is not a length of zero.
        return match low_type_of_array(module, shape) {
            Some((element, length)) => LowShape::Array(Box::new(element), length),
            None => LowShape::Unknown,
        };
    }
    if marker == P::Value::table_type_marker() {
        // A table type's shape is `[key type, value type]`.

        // SAFETY: `shape` is a live node of `module`; nothing in this crate
        // calls `Module::drop_block`.
        let Some(halves) = (unsafe { array_items(module, shape) }) else {
            return LowShape::Unknown;
        };
        return LowShape::Table(
            Box::new(low_type_of(module, halves[TABLE_TYPE_KEY_SLOT].node)),
            Box::new(low_type_of(module, halves[TABLE_TYPE_VALUE_SLOT].node)),
        );
    }
    if marker == P::Value::function_type_marker() {
        // A function type's shape is the `[domain, codomain]` pair.

        // SAFETY: `shape` is a live node of `module`; nothing in this crate
        // calls `Module::drop_block`.
        let Some(halves) = (unsafe { array_items(module, shape) }) else {
            return LowShape::Unknown;
        };
        return function_shape(
            module,
            halves[FUNCTION_TYPE_DOMAIN_SLOT].node,
            halves[FUNCTION_TYPE_CODOMAIN_SLOT].node,
        );
    }
    // A struct, or a kind this decoder does not know.
    LowShape::Unknown
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

/// The low type an expression's **type slot** names: a backend's seed for a
/// template's parameter domain.
    ///
    /// # Invariant
    /// The two are told apart by **whether the decode succeeds**, not by a
    /// structural guess: a term pair and a type value have the same two-slot
    /// silhouette. What separates them is the terminal marker — the `Type` marker,
    /// the one kind [`low_type_of`] refuses — so a pair never decodes directly.
pub fn low_type_of_slot<P: Program>(module: &Module<P>, slot: AnyNodeId) -> LowShape
where
    P::Value: ValueType,
{
    let direct = low_type_of(module, slot);
    if direct.is_known() {
        return direct;
    }
    // The pair indirection: the term's own value slot, tried once.

    // SAFETY: `slot` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, slot) }) else {
        return LowShape::Unknown;
    };
    let Some(value) = items.first() else {
        return LowShape::Unknown;
    };
    low_type_of(module, value.node)
}

/// [`low_type_of`]'s array arm; `None` when either half is undecided.
fn low_type_of_array<P: Program>(module: &Module<P>, shape: AnyNodeId) -> Option<(LowShape, usize)>
where
    P::Value: ValueType,
{
    // SAFETY: `shape` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let parts = unsafe { array_items(module, shape) }?;
    if parts.len() != 2 {
        return None;
    }
    let element = low_type_of(module, parts[ARRAY_TYPE_ELEMENT_SLOT].node);
    if !element.is_known() {
        return None;
    }
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
