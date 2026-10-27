//! The single authority for the expr/type encoding. See
//! `docs/notes/checker-encoding-instability.md`.
//!
//! # Invariant
//! Every expression compiles to a `[value, type, attrs…]` expr and every type
//! spine bottoms out at the self-referential universe `K = [Type, ↺]`.
//!
//! # Invariant
//! **The compound values carry their own shape.** A tuple's value is its
//! elements' exprs, an array's value the element template and the elements, and
//! a struct's value the name table, the nominal id and the fields; the
//! template types (`Tuple`, `Array`, `Struct`) name a layout and hold none. The
//! offsets below are the one place those layouts are written down.
//!
//! # Invariant
//! `LowShape` and `ScalarClass` are not unified: a shape is a structure (Tuple,
//! Array, Function, Table, Unknown) and a class is one bit of meaning (integer or
//! float), so `low_type_of`'s `Unknown` for a struct is correct and its gap is an
//! accessor's. The encoding lives here, above `lichen-lowlevel`, never inside it.

use lichen_lowlevel::{
    AnyFunctionId, AnyHandle, AnyNodeId, ArrayItem, LowShape, LowValue, Module, NodeId, Program,
    StaticNodeId, TableItem, UnifyStep,
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
            /// The `int` type constant — `USize` literals expr with `[int, K]`.
            TypeInt { 0, "int", int_marker, int_marker_node }
            /// The `float` type constant — `Float` literals expr with
            /// `[float, K]`.
            TypeFloat { 9, "float", float_marker, float_marker_node }
            /// The `string` type constant — the builtin immutable string
            /// value; `Str` literals expr with `[string, K]`.
            TypeString { 7, "string", string_marker, string_marker_node }
            /// The `Type` constant — the canonical universe node itself
            /// (`Type : Type`).
            TypeType { 1, "Type", type_marker, type_marker_node }
            /// The kind marker of function type expressions — the expr's
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
            /// The kind marker of a **native buffer** type: the shape is
            /// `[element type, length]`.
            TypeBuffer { 11, "$buf", buffer_marker, buffer_marker_node }
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

// --- expr layout: `[value, type, attrs…]`, one spelling of the slots ---------

/// Element 0 of an expression's `[value, type, attrs…]` expr — the value.
pub const EXPR_VALUE_SLOT: usize = 0;
/// Element 1 of an expression's `[value, type, attrs…]` expr — the type.
pub const EXPR_TYPE_SLOT: usize = 1;
/// The first attribute slot: the schema tail's attribute `i` sits at
/// `EXPR_ATTR_BASE + i` of the expr.
pub const EXPR_ATTR_BASE: usize = 2;

/// The expr slot of the schema tail's `index`-th attribute.
pub const fn attr_slot(index: usize) -> usize {
    EXPR_ATTR_BASE + index
}

/// The schema-tail index of expr attribute `slot` — the inverse of
/// [`attr_slot`].
pub const fn attr_index(slot: usize) -> usize {
    slot - EXPR_ATTR_BASE
}

/// The **element-slot width** of a compound type: the value, the type, and one
/// slot per attribute.
///
/// # Invariant
/// Every element of every compound kind has this width, so a written type's
/// slots and a value's unify however few positions either one spells. See
/// `docs/notes/compound-element-slots.md`.
pub fn slot_width<P: HighProgram>() -> usize {
    EXPR_ATTR_BASE + P::Attr::ORDER.len()
}

// --- kinded-type layout: `[shape, kind]` ---

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

// --- value layout: the compound values carry their own shape ---

/// A **tuple value**'s first element: the elements are its own items.
pub const TUPLE_VALUE_ELEMENT_BASE: usize = 0;
/// An **array value**'s element template — the slot `0` of `[template, …]`.
///
/// # Invariant
/// The result's value is `[T's element template, the elements…]`, so a read
/// of element `i` is the value's slot `i + 1`. The first slot is the element
/// template, so it carries the template marker: the slot holds the layout and
/// not a user element, and a key comparison reads its presence, never its
/// value.
pub const ARRAY_VALUE_TEMPLATE_SLOT: usize = 0;
/// An **array value**'s first element: the template holds slot `0`.
pub const ARRAY_VALUE_ELEMENT_BASE: usize = 1;
/// A **struct value**'s name table — the name→field-index table.
pub const STRUCT_VALUE_NAMES_SLOT: usize = 0;
/// A **struct value**'s nominal id — the written occurrence's identity.
pub const STRUCT_VALUE_ID_SLOT: usize = 1;
/// A **struct value**'s first field: the name table and the nominal id hold
/// the two slots before it.
pub const STRUCT_VALUE_FIELD_BASE: usize = 2;

// --- shape-half layout: each kind names its own shape positions --------------

/// Element 0 of a **buffer type**'s `[element type, length]` shape.
pub const BUFFER_TYPE_ELEMENT_SLOT: usize = 0;
/// Element 1 of a buffer type's shape — a `USize` value slot.
pub const BUFFER_TYPE_LENGTH_SLOT: usize = 1;
/// Element 0 of a **table type**'s `[key type, value type]` shape.
pub const TABLE_TYPE_KEY_SLOT: usize = 0;
/// Element 1 of a table type's shape.
pub const TABLE_TYPE_VALUE_SLOT: usize = 1;
/// Element 0 of a **function type**'s `[domain, codomain]` shape.
pub const FUNCTION_TYPE_DOMAIN_SLOT: usize = 0;
/// Element 1 of a function type's shape.
pub const FUNCTION_TYPE_CODOMAIN_SLOT: usize = 1;

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
    let value = node_value(module, id)?;
    let LowValue::Array(array) = value.as_enum()? else {
        return None;
    };
    // SAFETY: as the `# Safety` contract states, `id`'s block outlives the read.
    Some(unsafe { array.items() })
}

/// The **items** of a compound value — the payload a value holds.
///
/// # Invariant
/// A compound value's shape lives in its value and nowhere else, so this is the
/// one reader of it: the offsets above say which item is which, and a caller
/// never writes its own `+1` or `+2`.
pub fn value_items<P: Program>(module: &Module<P>, value: AnyNodeId) -> Option<&'static [ArrayItem]>
where
    P::Value: AsEnum<LowValue>,
{
    // SAFETY: `value` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    unsafe { array_items(module, value) }
}

/// The item of a compound value at `index`, or `None` when the value holds no
/// such slot.
pub fn value_item<P: Program>(
    module: &Module<P>,
    value: AnyNodeId,
    index: usize,
) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    Some(value_items(module, value)?.get(index)?.node)
}

/// How many items a compound value holds.
pub fn value_len<P: Program>(module: &Module<P>, value: AnyNodeId) -> Option<usize>
where
    P::Value: AsEnum<LowValue>,
{
    Some(value_items(module, value)?.len())
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
        && node_value(module, items[EXPR_VALUE_SLOT].node) == Some(P::Value::type_marker())
        && matches!(items[EXPR_TYPE_SLOT].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
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
        && node_value(module, items[KIND_MARKER_SLOT].node) == Some(marker)
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

/// Whether `ty` is the **`Function` type** — the shared `[FunctionType, Type]`.
///
/// # Invariant
/// The marker is read by its **value** and its type slot is the universe, so
/// the type names no function: the function's identity lives in its value.
pub fn is_function_type_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    template_is_any(module, ty, P::Value::function_type_marker(), false)
        && universe_is_any(module, universe, ty)
}

/// Whether `ty` is the **struct template** — the shared self-referential
/// `[StructType, ↺]` expression.
///
/// # Invariant
/// The template is recognised by its **value** (the struct marker) and by its
/// type slot being itself: the struct layout closes on itself, so no second
/// node carries it.
pub fn is_struct_type_any<P: Program>(
    module: &mut Module<P>,
    _universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    template_is_any(module, ty, P::Value::type_struct_marker(), true)
}

/// Whether `ty` is the **tuple template** — the shared `[TupleType, Type]`
/// expression.
pub fn is_tuple_type_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    template_is_any(module, ty, P::Value::tuple_type_marker(), false)
        && universe_is_any(module, universe, ty)
}

/// Whether `ty` is the **array template** — the shared `[ArrayType, Type]`
/// expression.
pub fn is_array_type_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    template_is_any(module, ty, P::Value::array_type_marker(), false)
        && universe_is_any(module, universe, ty)
}

/// Whether `ty` holds the marker `marker` at its value slot.
fn template_is_any<P: Program>(
    module: &Module<P>,
    ty: AnyNodeId,
    marker: P::Value,
    self_referential: bool,
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
    if node_value(module, items[EXPR_VALUE_SLOT].node) != Some(marker) {
        return false;
    }
    if !self_referential {
        return true;
    }
    // The template's type slot is in its class, or aliases the same frozen expr.
    if module.same_class(items[EXPR_TYPE_SLOT].node, ty) {
        return true;
    }
    match (
        node_value(module, items[EXPR_TYPE_SLOT].node),
        node_value(module, ty),
    ) {
        (Some(slot), Some(template)) => slot == template,
        _ => false,
    }
}

/// Whether `ty`'s type slot is the universe.
fn universe_is_any<P: Program>(module: &mut Module<P>, universe: NodeId, ty: AnyNodeId) -> bool
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
    is_universe_any(module, universe, items[EXPR_TYPE_SLOT].node)
}

/// Whether `ty` is a **native buffer type** `[shape, [marker, K]]`, its kind
/// carrying the buffer marker.
///
/// # Invariant
/// The shape is `[element type, length]`, the shape a buffer carries, so the
/// shape's two positions are the buffer's own.
pub fn is_buffer_type_any<P: Program>(
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
            P::Value::buffer_marker(),
        )
}

/// A native buffer's **element type and length**: its shape's two positions.
pub fn buffer_shape<P: Program>(module: &Module<P>, ty: TypeRef) -> Option<(AnyNodeId, AnyNodeId)>
where
    P::Value: AsEnum<LowValue>,
{
    let shape = field_list(module, ty)?;
    // SAFETY: `shape` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let items = unsafe { array_items(module, shape) }?;
    Some((
        items.get(BUFFER_TYPE_ELEMENT_SLOT)?.node,
        items.get(BUFFER_TYPE_LENGTH_SLOT)?.node,
    ))
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
        .get(EXPR_TYPE_SLOT)
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
    let expr = module.functions.get(function)?.parameter;
    let type_slot = module.expr_type_item(expr)?;
    // A type expr's value position is the shape and its type position the kind
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
        // Only an expr carries a layout; width cannot tell it from a data tuple
        // (`docs/notes/array-attribute-view.md`).
        if array.layout() == 0 {
            continue;
        }
        // SAFETY: `array` is the payload of a value of this module.
        let items = unsafe { array.items() };
        // A term is **full width** (`docs/notes/attributes.md`).
        let width = EXPR_ATTR_BASE + module.attribute_layout(array.layout()).len();
        if items.len() != width {
            continue;
        }
        let Some(shape) = low_type_of_slot(module, items[EXPR_TYPE_SLOT].node) else {
            continue;
        };
        // A function's detailed halves come from the function the expr names ✓.
        let shape = match shape {
            LowShape::Function(domain, codomain) if domain.is_none() && codomain.is_none() => {
                match module
                    .function_apply_function(node)
                    .and_then(|fid| function_shape(module, fid))
                {
                    Some(detailed) => detailed,
                    None => LowShape::Function(domain, codomain),
                }
            }
            shape => shape,
        };
        let Some(value) = items[EXPR_VALUE_SLOT].node.dynamic() else {
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
        // An open expr is an undecided type: it states nothing to seed.
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

/// A function's **parameter expr** — the `[value, type]` term a lowering reads.
pub fn function_parameter_expr<P: Program>(
    module: &Module<P>,
    function: lichen_lowlevel::AnyFunctionId,
) -> Option<lichen_lowlevel::NodeId> {
    let lichen_lowlevel::AnyFunctionId::Dynamic(function) = function else {
        return None;
    };
    Some(module.functions.get(function)?.parameter)
}

/// An expr's **value half**.
pub fn expr_value<P: Program>(
    module: &Module<P>,
    expr: lichen_lowlevel::NodeId,
) -> Option<lichen_lowlevel::NodeId> {
    module.expr_value_half(expr)
}

/// An expr's **type half**.
pub fn expr_type<P: Program>(
    module: &Module<P>,
    expr: lichen_lowlevel::NodeId,
) -> Option<AnyNodeId> {
    module.expr_type_item(expr)
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
    if let Some(value) = node_value(module, type_value) {
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
    // The **`Function` type**: it names a function and carries no
    // signature, so its halves are unknown here.
    if node_value(module, kinded[TYPE_SHAPE_SLOT].node) == Some(P::Value::function_type_marker()) {
        return Some(LowShape::Function(Box::new(None), Box::new(None)));
    }
    let shape = kinded[TYPE_SHAPE_SLOT].node;
    // An atomic type: its shape slot holds the marker itself.
    let shape_value = node_value(module, shape)?;
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
    let marker = node_value(module, kind[KIND_MARKER_SLOT].node)?;
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
    // A kind this decoder does not know.
    None
}

/// A **function's** low shape, read from the function itself.
///
/// # Invariant
/// The halves come from the function's own cells (`Module::function_signature`),
/// never from a type: a function's type is the shared marker and carries no
/// signature. A function with no readable signature has **unknown** halves.
pub fn function_shape<P: Program>(module: &Module<P>, fid: AnyFunctionId) -> Option<LowShape>
where
    P::Value: ValueType,
{
    let (domain, codomain) = module.function_signature(fid)?;
    Some(LowShape::Function(
        Box::new(low_type_of_slot(module, domain)),
        Box::new(low_type_of_slot(module, codomain)),
    ))
}

/// Whether `node` holds a **kind marker** — the type position of a type expr.
fn is_a_kind_marker<P: Program>(module: &Module<P>, node: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `node` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(kind) = (unsafe { array_items(module, node) }) else {
        return false;
    };
    let Some(marker) = kind.first().and_then(|item| node_value(module, item.node)) else {
        return false;
    };
    marker == P::Value::tuple_type_marker()
        || marker == P::Value::array_type_marker()
        || marker == P::Value::table_type_marker()
        || marker == P::Value::function_type_marker()
}

/// The low type an expression's **type slot** names: a backend's seed for a
/// template's parameter domain.
///
/// # Invariant
/// The two are told apart by **whether the decode succeeds**, not by a
/// structural guess: a term expr and a type value have the same two-slot
/// silhouette. What separates them is the terminal marker — the `Type` marker,
/// the one kind [`low_type_of`] refuses — so an expr never decodes directly.
pub fn low_type_of_slot<P: Program>(module: &Module<P>, slot: AnyNodeId) -> Option<LowShape>
where
    P::Value: ValueType,
{
    // A type expr's kind position holds a kind marker
    // (`docs/notes/compound-element-slots.md`).
    if let AnyNodeId::Dynamic(node) = slot
        && module
            .expr_type_item(node)
            .is_some_and(|kind| is_a_kind_marker(module, kind))
        && let Some(shape) = low_type_of(module, slot)
    {
        return Some(shape);
    }
    // An expr holds its type value at the value position; a type value is the
    // `[shape, kind]` array itself.
    let mut slot = slot;
    for _ in 0..8 {
        let Some(LowValue::Array(array)) =
            node_value(module, slot).and_then(|value| value.as_enum())
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

// --- diagnostic descent ---------------------------------------------------------

/// The full-parse walk: append a [`LocStep`] per unify `step`, tagging each
/// node as an expr slot or a shape position.
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
        } else if step.index == EXPR_VALUE_SLOT {
            if slot0_is_shape(module, cur) {
                in_shape = true;
                LocStep::Shape
            } else {
                LocStep::Value
            }
        } else if step.index == EXPR_TYPE_SLOT {
            LocStep::Type
        } else {
            LocStep::Attr(attr_index(step.index))
        };
        path.push(tag);
        cur = step.b;
    }
    path
}

/// Whether `node`'s element 0 is a list — a shape, not an expression's expr.
///
/// # Invariant
/// A diagnostic rendering hint, never a check: the heuristic misfires on an expr
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

/// The value a dynamic node or a frozen ref answers: a static ref is read in its own
/// module.
///
/// # Invariant
/// A static node is read **directly**, exactly as a dynamic one: a reader never copies
/// it into a dynamic node first. A copy would give each frozen position a fresh class,
/// so one value would read as several (`docs/notes/kernel-parameter-class.md`).
pub(crate) fn node_value<P: Program>(module: &Module<P>, id: AnyNodeId) -> Option<P::Value> {
    match id {
        AnyNodeId::Dynamic(_) => module.node_value(id),
        AnyNodeId::Static(sref) => module.static_read(sref),
    }
}
