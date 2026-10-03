//! The single authority for the highlevel's pair/type encoding.
//!
//! lichen's type representation is an untyped node graph with positional
//! conventions: every expression compiles to a `[value, type, attrs…]` pair,
//! every kinded type expression is a `[shape, [marker, universe]]` pattern,
//! and every type spine bottoms out at the self-referential universe
//! `K = [Type, ↺]`.  This module owns those conventions as typed accessors
//! and predicates over [`NodeId`]/[`AnyNodeId`], so a rule reads *meaning*
//! (`shape_of`, `kind_of`, `is_function_type`) instead of re-deriving it
//! from raw array offsets.
//!
//! The 9 kind markers (`Int`, `Float`, `String`, `Type`, `Function`, `Tuple`,
//! `Array`, `Struct`, `Table`) are defined once in the
//! [`for_each_kind_marker`] registry list; the `TypeValue` variants, the
//! `ValueType` marker methods, the `Ctx` node accessors, the checker's
//! installed marker nodes ([`crate::checker::Markers`]), and the `TypeValue`
//! artifact codec are all macro-derived from it, so adding or removing a
//! marker touches that one list.
//!
//! Two known weaknesses are named here, not fixed (Phase 1 is a pure
//! refactor; Phase 4 may replace them):
//!
//! - [`is_struct_marker_any`] recognizes a struct marker structurally —
//!   "the marker is a 2-element array ⇒ struct" — because no other kind's
//!   marker is an array.  That is a guess about an open encoding, not a
//!   check of a `TypeStruct` tag.
//! - [`slot0_is_shape`]'s "element 0 is an array ⇒ shape" heuristic misfires
//!   on a pair whose *value* is an array (a tuple value is an array too),
//!   so a diagnostic path through such a pair may tag a `Value` descent as
//!   `Shape`.  It is a diagnostic rendering hint only; the unify itself is
//!   unaffected.

use lichen_lowlevel::{
    AnyHandle, AnyNodeId, ArrayItem, Deferral, LowShape, LowValue, Module, NodeId, PendingSides,
    Program, StaticNodeId, TableItem, UnifyStep,
};
use lichen_utils::extend::AsEnum;

use crate::ir::LocStep;
use crate::program::ValueType;

// --- the kind-marker registry ---------------------------------------------------
//
// THE one list of the 9 kind markers.  Adding or removing a marker means
// editing this list alone: the `TypeValue` variants, the `ValueType` marker
// methods (default bodies), the `Ctx` node accessors, the checker's
// installed marker fields (and its `Ctx::value_node` dispatch and the
// `Build` record), and the `TypeValue` artifact codec (both sides) are all
// macro-derived from it.
//
// Each entry: the `TypeValue` variant with its doc, then `{ codec tag,
// display name, ValueType method, Ctx accessor }` — `TypeInt { 0, "int",
// int_marker, int_marker_node }` declares the `TypeValue::TypeInt` variant,
// its persisted artifact tag, the `ValueType::int_marker()` constructor,
// and the `Ctx::int_marker_node()` accessor (the checker's installed
// `int_marker` field behind it).
//
// The codec tag is the compatibility contract with already-persisted
// artifacts: an existing entry's tag must NEVER change, and a new marker
// takes the next unused tag (the tags are deliberately NOT the list
// positions — `TypeString` is `7` — so reordering this list for
// presentation can never renumber the format).  The tag space is the
// `TypeValue` codec's, not this list's: `TypeValue::TypeId` (not a kind
// marker) holds tag `8` there, so a new kind marker starts at `9`.
//
// A consumer macro receives the whole list as its input; an optional
// `[ args… ]` group is forwarded verbatim ahead of it, so a consumer that
// needs call-site context (`self`, a block id) gets it passed in — macro
// hygiene does not let the consumer see the call site's identifiers.
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
            /// The kind marker of struct type expressions — the shape is
            /// `[TypeId(n), fields_types_array]`: the nominal id bundled with
            /// the positional field-type list.
            TypeStruct { 5, "TypeStruct", type_struct_marker, type_struct_marker_node }
            /// The kind marker of table type expressions — the shape is
            /// `[key type, value type]`.
            TypeTable { 6, "TypeTable", table_type_marker, table_type_marker_node }
        }
    };
}
pub(crate) use for_each_kind_marker;

// --- pair layout ------------------------------------------------------------
//
// An expression's compiled term is a `[value, type, attrs…]` array: element 0
// the value, element 1 the type, and one slot per schema-tail attribute
// starting at element 2.  The slot arithmetic below is the ONE spelling of
// that layout — every `2 + tail index` computation goes through it.

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

// --- kinded-type layout -------------------------------------------------------
//
// A kinded type expression is `[shape, kind]` with `kind = [marker, universe]`.
// The slots are positionally identical to a pair's (`0`/`1`), but they mean
// something different, so they get their own names — an expression pair and a
// type expression are distinguished by *role*, never by structure.

/// Element 0 of a kinded type expression `[shape, kind]` — the shape (a
/// tuple's element-type list, an array's `[element type, length]`, a struct's
/// positional field-type list).
pub const TYPE_SHAPE_SLOT: usize = 0;
/// Element 1 of a kinded type expression `[shape, kind]` — the kind
/// `[marker, universe]`.
pub const TYPE_KIND_SLOT: usize = 1;
/// Element 0 of a kind `[marker, universe]` — the kind marker.
pub const KIND_MARKER_SLOT: usize = 0;
/// Element 1 of a kind `[marker, universe]` — the universe the kind closes on.
pub const KIND_UNIVERSE_SLOT: usize = 1;

// --- struct marker layout -----------------------------------------------------
//
// A struct kind's marker is the two-field value `[TypeId, names]` — the
// nominal id plus the optional name→index table — sitting in the kind's
// marker slot like any other kind's marker.  The name table is reachable by
// two paths, depending on whether the read starts from a struct *type* or a
// struct *kind*; both are spelled once here, as the constant offset paths
// the checker's lazy `Index` chains walk.

/// Element 0 of a struct marker `[TypeId, names]` — the nominal type id.
pub const STRUCT_MARKER_ID_SLOT: usize = 0;
/// Element 1 of a struct marker `[TypeId, names]` — the optional name→index
/// table (`LowValue::Void` for an anonymous positional struct).
pub const STRUCT_MARKER_NAMES_SLOT: usize = 1;
/// The struct marker's field count: exactly `[id, names]`.  This is also the
/// arity [`is_struct_marker_any`] guesses on — see its documented weakness.
pub const STRUCT_MARKER_LEN: usize = 2;

// --- shape-half layout -----------------------------------------------------------
//
// A compound type's **shape** (element 0 of `[shape, kind]`) is itself a pair
// or a list, and what its two positions *mean* depends on the kind.  They get
// their own names for the same reason the kinded-type slots do: an array's
// element position and a function's domain position are both "0", but they
// mean different things, and a reader that reused one spelling for both would
// be reading a convention rather than the layout.

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

/// The lazy index path from a struct **type** `[shape, kind]` to its name
/// table — kind at `[1]`, then the marker at `[0]`, then the names at `[1]`.
/// The `a.x` read walks it (`container_ty[1][0][1]`).
pub const STRUCT_TYPE_NAMES_PATH: [usize; 3] =
    [TYPE_KIND_SLOT, KIND_MARKER_SLOT, STRUCT_MARKER_NAMES_SLOT];

/// The lazy index path from a TypeStruct **kind** `[marker, K]` to its name
/// table — the marker at `[0]`, then the names at `[1]`.  The `X::a` raw
/// read walks it (`container_ty[0][1]`).
pub const STRUCT_KIND_NAMES_PATH: [usize; 2] = [KIND_MARKER_SLOT, STRUCT_MARKER_NAMES_SLOT];

/// The array items behind either a dynamic node or a static ref — the raw
/// read every accessor and predicate in this module is built on.  `None`
/// when `id` is unbound or not an array.
///
/// # Safety
///
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
    // SAFETY: `array` is the payload of `id`, a node of `module`, and nothing
    // in this crate calls `Module::drop_block`, so the home block outlives
    // this read.
    Some(unsafe { array.items() })
}

/// The shape slot of a kinded type expression `[shape, kind]`, for a dynamic
/// node or a static ref alike.  `None` when `ty` is not a 2-element array
/// (an unbound cell, a bare marker — anything that is not a type-expression
/// pair).
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

/// The kind slot `[marker, universe]` of a kinded type expression
/// `[shape, kind]`, for a dynamic node or a static ref alike.  `None` under
/// the same condition as [`shape_of`].
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

/// The marker slot of a kind `[marker, universe]`, for a dynamic node or a
/// static ref alike.  `None` when `kind` is not a 2-element array.
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

/// Whether a static ref names the canonical universe.  A frozen universe is
/// a 2-item self-referential array (`[Type, itself]`), so it is recognized
/// by content instead of equality classes.
pub fn is_static_universe<P: Program>(module: &Module<P>, sref: StaticNodeId) -> bool
where
    P::Value: ValueType,
{
    // SAFETY: `sref`'s payload lives in a static module's arena, pinned by the
    // registry for as long as the module stays registered.
    let Some(items) = (unsafe { array_items(module, AnyNodeId::Static(sref)) }) else {
        return false;
    };
    items.len() == 2
        && module.node_value(items[PAIR_VALUE_SLOT].node) == Some(P::Value::type_marker())
        && matches!(items[PAIR_TYPE_SLOT].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
}

/// Whether `id` is the canonical universe `K = [Type, ↺]`: a dynamic node by
/// equality-class comparison against the module's canonical universe node
/// (`universe` — the one prebuilt composite, shared because cloning the
/// self-referential universe breaks unification), a static ref by content
/// (see [`is_static_universe`]).
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

// --- unification policy ---------------------------------------------------------

/// Whether `node`'s class holds a **type value**: a type-level value in the
/// pair/type encoding — the universe `K = [Type, ↺]`, an atomic type or kind
/// `[marker, K]`, a struct marker `[TypeId, names]`, a bare kind-marker atom,
/// or a kinded type expression `[shape, [marker, K]]`.  This is the
/// highlevel half of what the lowlevel used to know: "does this class hold a
/// type" is meaningless without the encoding, so it lives here, in the
/// module that owns the encoding.
///
/// It is what makes [`defer_pending`] sound: a field/positional read's own
/// *type* is such a value, so unifying a pending read against one is a type
/// round-trip rather than a value comparison.  A scalar — an `Int` *value*
/// as opposed to its type — is not a type value, and unifying a read against
/// one is left to fail.
///
/// The recognition is by **tag**, not by silhouette: atoms are tested against
/// the kind-marker registry ([`ValueType::is_kind_marker`]) and `TypeId`, and
/// the only structural fact used is the universe's self-referential cycle
/// ([`Module::is_self_referential`]) — an honest graph property, not an arity
/// guess.  A marker slot that is still unbound is accepted (a type whose kind
/// is not decided yet is still a type being built); any other undecided or
/// off-shape position is not a type value.
pub fn class_holds_type<P: Program>(module: &mut Module<P>, node: NodeId) -> bool
where
    P::Value: ValueType,
{
    let rep = module.equality_representative(node);
    // A bare merge can leave the class's decided value on a member other
    // than the representative, so read the committed carrier, not one slot.
    let Some(carrier) = module.class_committed_node(rep) else {
        return false;
    };
    node_holds_type(module, AnyNodeId::Dynamic(carrier))
}

/// The value-level test behind [`class_holds_type`], over a dynamic node or
/// a static ref alike.  See that function for the recognised forms.
fn node_holds_type<P: Program>(module: &mut Module<P>, node: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(value) = module.node_value(node) else {
        return false;
    };
    // A bare kind-marker atom or a nominal id is a type-level value.
    if value.is_kind_marker() || value.type_id().is_some() {
        return true;
    }
    // SAFETY: `node` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, node) }) else {
        return false;
    };
    if items.len() != 2 {
        return false;
    }
    // The universe itself (`K = [Type, ↺]`).
    if module.is_self_referential(node) {
        return true;
    }
    let first = items[0].node;
    let second = items[1].node;
    // An atomic type or a kind: `[marker, K]`.
    if node_is_marker(module, first) && module.is_self_referential(second) {
        return true;
    }
    // A struct marker `[TypeId, names]` — the nominal id is its tag.  The id
    // cell may still be unbound while the marker is being built, so the
    // honest signal is the pair's *shape*: an id slot (id or unbound) and a
    // names slot (a name table, `Void` for an anonymous struct, or unbound).
    let first_is_id = match module.node_value(first) {
        None => true,
        Some(first_value) => first_value.type_id().is_some(),
    };
    if first_is_id && node_is_names_table(module, second) {
        return true;
    }
    // A kinded type expression `[shape, [marker, K]]` — the shape is not
    // inspected: a type whose field types are still being built is a type.
    // SAFETY: `second` is a live node of `module`; nothing in this crate
    // calls `Module::drop_block`.
    let Some(kind_items) = (unsafe { array_items(module, second) }) else {
        return false;
    };
    kind_items.len() == 2
        && node_is_marker_open(module, kind_items[KIND_MARKER_SLOT].node)
        && module.is_self_referential(kind_items[KIND_UNIVERSE_SLOT].node)
}

/// Whether `node` is a **kind marker**: a registry marker atom, a nominal
/// `TypeId`, or a struct marker `[TypeId, names]` (the one marker that is an
/// array — recognised by its id-and-names shape, closing the
/// `is_struct_marker_any` arity guess at this site).
fn node_is_marker<P: Program>(module: &mut Module<P>, node: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(value) = module.node_value(node) else {
        return false;
    };
    if value.is_kind_marker() || value.type_id().is_some() {
        return true;
    }
    // SAFETY: `node` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    let Some(items) = (unsafe { array_items(module, node) }) else {
        return false;
    };
    if items.len() != STRUCT_MARKER_LEN {
        return false;
    }
    let id_is_id = match module.node_value(items[STRUCT_MARKER_ID_SLOT].node) {
        None => true,
        Some(id) => id.type_id().is_some(),
    };
    id_is_id && node_is_names_table(module, items[STRUCT_MARKER_NAMES_SLOT].node)
}

/// [`node_is_marker`], additionally accepting an unbound cell: a kind whose
/// marker is not decided yet is a kind being built, not evidence against.
fn node_is_marker_open<P: Program>(module: &mut Module<P>, node: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    module.node_value(node).is_none() || node_is_marker(module, node)
}

/// Whether `node` reads as a struct marker's **names slot**: a name→index
/// table, `Void` (an anonymous positional struct), or a still-unbound cell.
/// A `TypeId` slot alone cannot tag a struct marker honestly (it is unbound
/// while the marker is being built), so the names slot carries the signal.
fn node_is_names_table<P: Program>(module: &mut Module<P>, node: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(value) = module.node_value(node) else {
        return true;
    };
    matches!(value.as_enum(), Some(LowValue::Table(_) | LowValue::Void))
}

/// The highlevel's [`Program::defer_pending`] policy: merge a pending
/// **type-level computation** — a field/positional **read** or a lazy
/// **call** (a type function applied to an undecided argument) — with a
/// class that **holds a type**, and nothing else.
///
/// The merge is sound because neither side can be compared yet: the
/// computation resolves to its actual type once what it depends on binds,
/// and a genuine mismatch then surfaces against the resolved value (the
/// lowlevel commits the type value onto the class — `Module::unify`'s pin —
/// and reconciles it with the computation's outcome when that runs).  The
/// deferral stays deliberately narrow — only an unresolvable `Index` or a
/// lazy `Apply` qualifies, never a resolved read nor arithmetic nor a
/// dependent-type branch — so an unresolvable real computation still records
/// an error.  Every other case returns `None` and falls through to the
/// lowlevel's generic rules.
pub fn defer_pending<P: Program>(module: &mut Module<P>, sides: &PendingSides) -> Option<Deferral>
where
    P::Value: ValueType,
{
    let read_against_type = (sides.a.pending
        && (sides.a.pending_index_read || sides.a.pending_apply)
        && class_holds_type(module, sides.b.representative))
        || (sides.b.pending
            && (sides.b.pending_index_read || sides.b.pending_apply)
            && class_holds_type(module, sides.a.representative));
    read_against_type.then_some(Deferral::Merge)
}

// --- shape predicates ---------------------------------------------------------
//
// One implementation over `AnyNodeId` per predicate, plus a thin `NodeId`
// wrapper; the dynamic and static cases share the implementation through
// [`array_items`] and [`is_universe_any`].

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

/// Whether `ty` is a concrete function type expression:
/// `[shape, [FunctionType, K]]`.  The checker's function-ness guard skips
/// these — only concretely *non*-function types are caught statically.
pub fn is_function_type_any<P: Program>(
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

/// [`is_function_type_any`] over a dynamic node.
pub fn is_function_type<P: Program>(module: &mut Module<P>, universe: NodeId, ty: NodeId) -> bool
where
    P::Value: ValueType,
{
    is_function_type_any(module, universe, AnyNodeId::Dynamic(ty))
}

/// The two halves of a concrete function type expression
/// `[[domain, codomain], [FunctionType, K]]` — the reader symmetric with
/// the encoding every arrow build site produces.  `None` when `ty` is not a
/// concrete function type: an unbound cell, a type of another kind, or a
/// function type whose kind does not close on the universe.
///
/// The **shape is returned unwrapped** — the `[domain, codomain]` node
/// itself, not a futures pair — because every caller either reads its two
/// elements or inserts it into the checker's `arrows` set, which keys on that
/// node's identity (see [`is_function_type_any`] for the recognition
/// contract this mirrors).
pub fn function_type_parts_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> Option<AnyNodeId>
where
    P::Value: ValueType,
{
    if !is_function_type_any(module, universe, ty) {
        return None;
    }
    shape_of(module, ty)
}

/// [`function_type_parts_any`] over a dynamic node — the `[domain,
/// codomain]` shape node of `ty`, or `None` when `ty` is not a concrete
/// function type.
pub fn function_type_parts<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: NodeId,
) -> Option<AnyNodeId>
where
    P::Value: ValueType,
{
    function_type_parts_any(module, universe, AnyNodeId::Dynamic(ty))
}

/// Whether `ty` is a struct type: `[shape, [TypeStruct{id, names}, K]]`.
/// The kind's marker slot holds the two-field `TypeStruct` value (the
/// nominal id + the optional name table), so the kind is a standard
/// `[marker, K]` pair whose marker is a 2-element array.
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

/// Whether a value is a struct marker: the two-field `TypeStruct{id, names}`
/// value, encoded as a 2-element array `[id, names]`.  No other kind's
/// marker is an array (the function/tuple/array/table markers are plain
/// type-constant values), so a 2-element array marker names a struct.
///
/// **Known weakness** (see the module docs): this is a structural guess
/// about an open encoding — any 2-element array in a marker slot passes,
/// with no `TypeStruct` tag checked.  Phase 1 names it; Phase 4 may replace
/// it with an honest tag check.
pub fn is_struct_marker_any<P: Program>(module: &Module<P>, marker: AnyNodeId) -> bool
where
    P::Value: AsEnum<LowValue>,
{
    // SAFETY: `marker` is a live node of `module`; nothing in this crate calls
    // `Module::drop_block`.
    unsafe { array_items(module, marker) }.is_some_and(|items| items.len() == STRUCT_MARKER_LEN)
}

/// Whether `ty` is a TypeStruct **kind** — `[TypeStruct{id, names}, K]` —
/// the `[marker, universe]` form a raw named read `X::a` requires.  This is
/// the container type's *own* shape (a struct type value's `ty`), not the
/// `[shape, kind]` pair of a struct instance's type (which `.a` reads,
/// [`is_struct_type_any`]): the name→index table lies directly at
/// `ty[0][1]`.
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

/// Whether `ty` is a concrete positional type expression — a tuple type
/// (`[shape, [TypeTuple, K]]`) or a struct type (`[shape, [id,
/// [TypeStruct, K]]]`, whose shape is the positional field-type list).
/// The checker's field-read guard (`a(k)`) skips these; an array reads with
/// `a[i]` (its type is pinned, so misuse fails the pin unify), a table
/// with `t{k}`, and only concretely *non*-positional types are caught
/// statically.
pub fn is_positional_type_any<P: Program>(
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
    kind_marker_is_any(
        module,
        universe,
        items[TYPE_KIND_SLOT].node,
        P::Value::tuple_type_marker(),
    ) || is_struct_type_any(module, universe, ty)
}

/// [`is_positional_type_any`] over a dynamic node.
pub fn is_positional_type<P: Program>(module: &mut Module<P>, universe: NodeId, ty: NodeId) -> bool
where
    P::Value: ValueType,
{
    is_positional_type_any(module, universe, AnyNodeId::Dynamic(ty))
}

/// The pieces of a struct **type term** `[shape, kind]` that every reader of it
/// needs: the kind's universe slot — the node a caller checks to be sure the
/// term really is a kinded type — the term's field-type list, and the marker's
/// name table.
///
/// The universe *gate* is deliberately the caller's: the two readers below
/// recognise the universe differently (one by class equality against a
/// caller-supplied node, one by its self-referential cycle), and that
/// difference is the whole reason both exist.  Walking the encoding lives here
/// so the two cannot drift.
///
/// `None` when the node is not a two-slot term whose kind is a two-slot pair
/// with a table at the marker's [`STRUCT_MARKER_NAMES_SLOT`].
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
    // The struct marker `[id, names]`; its second field is the name table.
    // SAFETY: `kind_items[KIND_MARKER_SLOT].node` is a live node of `module`.
    let marker_items = unsafe { array_items(module, kind_items[KIND_MARKER_SLOT].node) }?;
    let names_item = marker_items.get(STRUCT_MARKER_NAMES_SLOT)?;
    match module.node_value(names_item.node).and_then(|v| v.as_enum()) {
        Some(LowValue::Table(table)) => Some((kind_items[KIND_UNIVERSE_SLOT].node, shape, table)),
        _ => None,
    }
}

/// The struct's name→index table (the `struct<.a T, …>` names) from a
/// struct type value, or `None` when it is an anonymous struct (no names)
/// or not a struct type at all.  The table is reached through the kind:
/// `[shape, [marker, K]]` → the marker's [`STRUCT_MARKER_NAMES_SLOT`].
///
/// **The universe is supplied by the caller.**  A checker has it; a lowering
/// does not, and [`struct_fields_by_shape`] is this reader for that side.
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

/// The fields of a struct **type term** `[shape, kind]`, in field order, plus
/// the term's field-type list node — read **without a universe handle**.
///
/// # Why this exists beside [`struct_names_any`]
///
/// `compile_parallel_fragment` runs on a [`Module`] and has no universe node:
/// the universe is a `Ctx` fact, and the lowering is below the checker.  A
/// kernel's parameter struct is a named struct type term, so the lowering needs
/// this reader and cannot call the one above.
///
/// # The gate is the cycle, not a second structural guess
///
/// The universe slot is recognised by [`Module::is_self_referential`] — the
/// `[Type, ↺]` cycle.  That is the same test the renderer
/// (`is_universe`) and `class_holds_type` already use, so this reader adds no
/// new guess about the encoding; it chooses the one the modules that have no
/// universe handle already had to use.
///
/// `None` when `ty` is not a named struct type term.  A `None` entry is a
/// positional field; a `Some(name)` entry is a `.name T` field.
pub fn struct_fields_by_shape<P: Program>(
    module: &mut Module<P>,
    ty: AnyNodeId,
) -> Option<(Vec<Option<&'static str>>, AnyNodeId)>
where
    P::Value: ValueType,
{
    let (universe_slot, shape, table) = struct_term_parts(module, ty)?;
    if !module.is_self_referential(universe_slot) {
        return None;
    }
    // The field count is the shape's own length: the names are sized to the
    // term, not to the table, so a name whose index maps outside the field list
    // is dropped rather than growing it.
    // SAFETY: `shape` is a live node of `module`.
    let field_count = unsafe { array_items(module, shape) }?.len();
    let mut names: Vec<Option<&'static str>> = vec![None; field_count];
    // SAFETY: `table` is the payload of the value read from the live node
    // named by the marker's name slot.
    for item in unsafe { table.items() } {
        let name = module
            .node_value(item.key)
            .and_then(|v| v.as_enum())
            .and_then(|v| match v {
                LowValue::Str(s) => Some(s),
                _ => None,
            });
        let index = module
            .node_value(item.value)
            .and_then(|v| v.as_enum())
            .and_then(|v| match v {
                LowValue::USize(n) => Some(n),
                _ => None,
            });
        if let (Some(name), Some(index)) = (name, index)
            && index < field_count
        {
            names[index] = Some(name);
        }
    }
    Some((names, shape))
}

// --- low types -----------------------------------------------------------------

/// The **low type** a kinded type expression denotes — the highlevel half of
/// the low-type layer, and the only place the type encoding is read to produce
/// one (see `docs/notes/lowlevel-low-types.md`).
///
/// This is the *seed* of the mechanism: a template's parameter position is the
/// one thing the value graph can never decide, because a template is never
/// evaluated and an apply binds the clones instead — so its domain is read
/// here, out of the type slot, and handed to the lowlevel as a
/// [`LowShape`].  Everything else about a body is decided by the value graph.
///
/// The decode is by **kind marker**, and it says [`LowShape::Unknown`]
/// explicitly rather than falling back to a scalar.  That difference is the
/// whole point of the function: a type the low type vocabulary has no shape
/// for — a `string`, a nominal struct, an inference cell that has not bound
/// yet — states *nothing*, and a reader must treat nothing as undecided.  A
/// silent scalar fallback would let a `jit` compile a domain it invented.
///
/// A struct is the interesting refusal: its fields are positional, so the
/// shape reads like a tuple, but a nominal struct is not a tuple value and the
/// low type vocabulary has no nominal shape.  Answering `Tuple(..)` would drop
/// exactly the identity that makes it a struct.
///
/// The marker is read from the slot that carries it, and the two kinds of type
/// carry it differently — that asymmetry is the encoding, not an accident:
/// an **atomic** type's shape *is* its marker (`int` is literally `[int, K]`),
/// while a **compound** type's shape is a list or a pair and its marker lives
/// in the kind.  Reading the kind's marker for both — the obvious mistake, and
/// the one an earlier structural decoder made — silently classifies every
/// scalar as an unrecognised kind.
///
/// The argument is a **type value** — the `[shape, kind]` expression itself.
/// A caller holding a term that *names* a type (an expression's type slot)
/// wants [`low_type_of_slot`], which resolves the indirection.
pub fn low_type_of<P: Program>(module: &Module<P>, type_value: AnyNodeId) -> LowShape
where
    P::Value: ValueType,
{
    // A type expression is `[shape, kind]`; anything else is not a type this
    // decoder can read.
    // SAFETY: `type_value` is a live node of `module`; nothing in this crate
    // calls `Module::drop_block`.
    let Some(kinded) = (unsafe { array_items(module, type_value) }) else {
        return LowShape::Unknown;
    };
    if kinded.len() != 2 {
        return LowShape::Unknown;
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
        // A float is a **decided** member of the low type vocabulary, so a
        // declared `[float, K]` states it.  Answering `Unknown` here would make
        // a declared float indistinguishable from an unbound cell, which is a
        // different claim.  Whether a kernel can lower it is not this decoder's
        // question (`lichen_compute::kernel_domain`,
        // `docs/notes/floating-point.md` §3.8).
        return LowShape::Float;
    }
    if shape_value == P::Value::string_marker() || shape_value == P::Value::type_marker() {
        // A string is not a machine scalar, and a type is not a value at all.
        return LowShape::Unknown;
    }
    // A compound type: the marker is the kind's, and the shape is a list or a
    // pair whose two positions mean different things per kind.
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
        // An array type's shape is `[element type, length]`, and the length is
        // a value slot — an undecided length makes the whole shape undecided,
        // because a length nobody knows is not a length of zero.
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
        // A function type's shape is the `[domain, codomain]` pair — the same
        // node `function_type_parts` returns.
        // SAFETY: `shape` is a live node of `module`; nothing in this crate
        // calls `Module::drop_block`.
        let Some(halves) = (unsafe { array_items(module, shape) }) else {
            return LowShape::Unknown;
        };
        return LowShape::Function(
            Box::new(low_type_of(module, halves[FUNCTION_TYPE_DOMAIN_SLOT].node)),
            Box::new(low_type_of(
                module,
                halves[FUNCTION_TYPE_CODOMAIN_SLOT].node,
            )),
        );
    }
    // A struct, or a kind this decoder does not know.
    LowShape::Unknown
}

/// The low type an expression's **type slot** names — the seed a backend
/// takes for a template's parameter domain.
///
/// The slot is not always a type value, and the indirection is the checker's:
/// an *annotated* parameter's type cell is unified with the annotation
/// expression's own `[value, type]` term ([`crate::checker::lambda`]), so the
/// slot holds that pair, while an *inferred* one is bound straight to a type
/// value by the body's own unifications.  Both name the same type — the pair's
/// value slot — and resolving that here is the authority's job precisely
/// because a backend that had to know would be reading the layout again.
///
/// The two are told apart by **whether the decode succeeds**, not by a
/// structural guess, and the guess would not be sound: a term pair and a type
/// value have the same two-slot silhouette.  What separates them is the
/// terminal marker: the type slot of a term names the *type of an expression*,
/// which for a type expression is the `Type` marker — and `Type` is the one
/// kind marker [`low_type_of`] refuses, because a type is not a value shape.
/// So a pair never decodes directly, and a bare type value always decodes on
/// the first try.
///
/// An undecidable answer is [`LowShape::Unknown`], never a fallback: a
/// polymorphic parameter has no domain at this boundary, and a backend that
/// invented one would compile a kernel for a type nobody wrote.
pub fn low_type_of_slot<P: Program>(module: &Module<P>, slot: AnyNodeId) -> LowShape
where
    P::Value: ValueType,
{
    let direct = low_type_of(module, slot);
    if direct.is_known() {
        return direct;
    }
    // The pair indirection: the term's own value slot, tried once.  A value
    // slot that does not decode either is not a type this decoder can read.
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

/// [`low_type_of`]'s array arm: an array type's `[element type, length]`
/// shape.  `None` when either half is undecided, so the caller answers
/// [`LowShape::Unknown`] instead of a length it guessed.
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

/// The full-parse walk: append a [`LocStep`] for each unify `step`,
/// classifying the current node as an expression's `[value, type]` pair
/// (→ `Value`/`Type`/`Attr`) or a tuple/array/struct shape (→ `Shape` then
/// `Elem`).
///
/// `start` is the unify's `b`-side top operand (`root_b`); the walk tracks the
/// `step.b` child as it descends.  Both sides of a unify are structurally
/// parallel (unification only descends where both are arrays), so the shape
/// tags are identical whichever side is tracked — the `b` side is just the one
/// the source-blind location is anchored to.
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

/// Whether `node`'s element 0 is a list — a tuple/array/struct structure (a
/// "shape") rather than an expression's `[value, type]` pair.
///
/// **Known limitation** (see the module docs): "element 0 is an array ⇒
/// shape" is a heuristic — it misfires on a pair whose *value* is itself an
/// array (a tuple value), tagging that `Value` descent as `Shape`.  Kept
/// as-is in Phase 1: it is a diagnostic rendering hint, never a check.
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
