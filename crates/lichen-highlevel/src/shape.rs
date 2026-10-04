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
//! Struct-ness is recognized by **tag**, never by silhouette: a struct marker
//! is an ordinary `[payload, type]` pair whose [`STRUCT_MARKER_TAG_SLOT`] holds
//! the `TypeStruct` atom ([`ValueType::type_struct_marker`]), so a pair whose
//! type slot is not that atom is not a struct ([`is_struct_marker_any`]).
//!
//! One known weakness is named here, not fixed (Phase 1 is a pure refactor;
//! Phase 4 may replace it):
//!
//! - [`slot0_is_shape`]'s "element 0 is an array ⇒ shape" heuristic misfires
//!   on a pair whose *value* is an array (a tuple value is an array too, and so
//!   is a struct marker's payload — `[payload, TypeStruct]`), so a diagnostic
//!   path through such a pair may tag a `Value` descent as
//!   `Shape`.  It is a diagnostic rendering hint only; the unify itself is
//!   unaffected.

use lichen_lowlevel::{
    AnyFunctionId, AnyHandle, AnyNodeId, ArrayItem, FunctionTypeUnify, LowShape, LowValue, Module,
    NodeId, Program, StaticNodeId, TableItem, UnifyStep,
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
            /// The kind marker of set type expressions — the shape is the
            /// **element type alone**: a set has no length, so `set{a}` and
            /// `set{a, b}` share one type, which is what separates the set kind
            /// from `array<T, n>`.  This marker took tag `10`; the
            /// class-domain *value* tag that briefly held it was removed before
            /// release (the set's value is its members — no tag is needed).
            TypeSet { 10, "SetType", set_type_marker, set_type_marker_node }
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
// A struct kind's marker is an ordinary `[value, type]` pair whose **value** is
// the struct's payload and whose **type** is the `TypeStruct` atom:
//
//     marker  = [ payload, TypeStruct ]
//     payload = [ TypeId, names, names_in_order ]
//
// so a struct marker is a normal type value — the one whose type read is the
// `TypeStruct` atom.  That tag is what makes "is this a struct marker?" a check
// of a type constant rather than a guess about the arity of an open encoding.
// The payload carries the nominal id, the optional name→index table, and the
// same names again in definition order.  The name table is reachable by two
// paths, depending on whether the read starts from a struct *type* or a struct
// *kind*; both are spelled once here, as the constant offset paths the
// checker's lazy `Index` chains walk.
//
// The last payload field exists for the one reader a name→index table cannot
// serve: a named instantiation through an **unresolved** callee reorders its
// arguments when the struct type resolves, and that reorder needs the field's
// *name* at each definition position ([`STRUCT_MARKER_NAMES_ORDER_SLOT`]) — the
// table's inverse, which a lazy `Index` cannot derive.

/// Element 0 of a struct marker `[payload, TypeStruct]` — the marker's *value*:
/// the payload `[TypeId, names, names_in_order]`.  The marker is an ordinary
/// `[value, type]` pair, so this is positionally [`PAIR_VALUE_SLOT`] with the
/// marker's own meaning.
pub const STRUCT_MARKER_PAYLOAD_SLOT: usize = 0;
/// Element 1 of a struct marker `[payload, TypeStruct]` — the marker's *type*:
/// the `TypeStruct` atom ([`ValueType::type_struct_marker`]), the honest signal
/// that the pair is a struct marker.  An ordinary pair whose type slot is not
/// that atom is not a struct marker.
pub const STRUCT_MARKER_TAG_SLOT: usize = 1;

/// Element 0 of a struct marker's payload `[TypeId, names, names_in_order]` —
/// the nominal type id.
pub const STRUCT_MARKER_ID_SLOT: usize = 0;
/// Element 1 of a struct marker's payload — the optional name→index table
/// (`LowValue::Void` for an anonymous positional struct).
pub const STRUCT_MARKER_NAMES_SLOT: usize = 1;
/// Element 2 of a struct marker's payload — the field names in definition
/// order, one array element per definition position (a `Str` for a named field,
/// `Void` for a positional one; the whole field is `Void` when no field is
/// named).  This is the inverse of the [`STRUCT_MARKER_NAMES_SLOT`] table: a
/// position's *name*, which the deferred named instantiation's reorder reads
/// lazily (see [`STRUCT_KIND_NAMES_ORDER_PATH`]).
pub const STRUCT_MARKER_NAMES_ORDER_SLOT: usize = 2;

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
/// table — kind at `[1]`, then the marker at `[0]`, then the marker's payload
/// at `[0]`, then the names at `[1]` of the payload.  The `a.x` read walks it
/// (`container_ty[1][0][0][1]`).
pub const STRUCT_TYPE_NAMES_PATH: [usize; 4] = [
    TYPE_KIND_SLOT,
    KIND_MARKER_SLOT,
    STRUCT_MARKER_PAYLOAD_SLOT,
    STRUCT_MARKER_NAMES_SLOT,
];

/// The lazy index path from a TypeStruct **kind** `[marker, K]` to its name
/// table — the marker at `[0]`, then the marker's payload at `[0]`, then the
/// names at `[1]` of the payload.  The `X::a` raw read walks it
/// (`container_ty[0][0][1]`).
pub const STRUCT_KIND_NAMES_PATH: [usize; 3] = [
    KIND_MARKER_SLOT,
    STRUCT_MARKER_PAYLOAD_SLOT,
    STRUCT_MARKER_NAMES_SLOT,
];

/// The lazy index path from a TypeStruct **kind** `[marker, K]` to its
/// definition-order names — the marker at `[0]`, then the marker's payload at
/// `[0]`, then the names at `[2]` of the payload.  The
/// deferred named instantiation reads through it rather than through the
/// [`STRUCT_TYPE_NAMES_PATH`] spelling: a callee's `[shape, kind]` term is a
/// pair, and reading *its* slot 1 would pull the shape half into the read's
/// operand chain — the forced pass walks every element of an operation's
/// operand array, not only the selected one, so the shape half (which holds the
/// deferred reorder's own field-type probe) would be forced mid-read and the
/// read would meet itself.  The kind node is the same node, read directly.
pub const STRUCT_KIND_NAMES_ORDER_PATH: [usize; 3] = [
    KIND_MARKER_SLOT,
    STRUCT_MARKER_PAYLOAD_SLOT,
    STRUCT_MARKER_NAMES_ORDER_SLOT,
];

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

/// Whether `ty` is a concrete **arrow-term** function type:
/// `[shape, [FunctionType, K]]` — the spelling a written `A -> B` lowers to.
/// Distinct from a function-type *node* `[Function(fid), ↺]` (a function's
/// own type, `f : f`), which has no `[dom, cod]` shape: its signature lives
/// in the function template. Use [`is_function_type_any`] to recognise both.
fn is_arrow_type_any<P: Program>(module: &mut Module<P>, universe: NodeId, ty: AnyNodeId) -> bool
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

/// Whether `ty`'s class holds a **function-type node** `[Function(fid), ↺]`
/// — a function's own type (`f : f`). The lowlevel recognition lives in
/// [`Module::is_function_type_node`]; this is the `AnyNodeId` wrapper that
/// admits a static ref (a frozen module's function-types are dynamic clones
/// after materialisation, so a raw static ref is not one).
fn is_function_type_node_any<P: Program>(module: &mut Module<P>, ty: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    match ty {
        AnyNodeId::Dynamic(node) => module.is_function_type_node(node),
        AnyNodeId::Static(_) => false,
    }
}

/// Whether `ty` is a concrete function type — either an **arrow term**
/// `[shape, [FunctionType, K]]` (a written `A -> B`) or a **function-type
/// node** `[Function(fid), ↺]` (a function's own type, `f : f`). The
/// checker's function-ness guard skips these — only concretely *non*-function
/// types are caught statically.
pub fn is_function_type_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> bool
where
    P::Value: ValueType,
{
    is_arrow_type_any(module, universe, ty) || is_function_type_node_any(module, ty)
}

/// [`is_function_type_any`] over a dynamic node.
pub fn is_function_type<P: Program>(module: &mut Module<P>, universe: NodeId, ty: NodeId) -> bool
where
    P::Value: ValueType,
{
    is_function_type_any(module, universe, AnyNodeId::Dynamic(ty))
}

/// The two halves of a concrete **arrow-term** function type
/// `[[domain, codomain], [FunctionType, K]]` — the reader symmetric with the
/// encoding every arrow build site produces.  `None` when `ty` is not an
/// arrow term: an unbound cell, a type of another kind, an arrow whose kind
/// does not close on the universe, or a **function-type node** (whose
/// signature lives in the function template, not in a shape — read it via
/// [`function_type_signature`]).
///
/// The **shape is returned unwrapped** — the `[domain, codomain]` node
/// itself, not a futures pair — because every caller either reads its two
/// elements or inserts it into the checker's `arrows` set, which keys on that
/// node's identity (see [`is_arrow_type_any`] for the recognition contract
/// this mirrors).
pub fn function_type_parts_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> Option<AnyNodeId>
where
    P::Value: ValueType,
{
    if !is_arrow_type_any(module, universe, ty) {
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

// --- function-type nodes (a function's own type, `f : f`) ---------------------
//
// A function-type node is the self-referential `[Function(fid), ↺]` — a
// function's own type, the `f : f` representation (see
// `docs/notes/function-type-as-function.md`). Its signature (domain/codomain,
// and in Phase 2 the attribute slots) lives in the function *template*
// (`Function::parameter` / `Function::r#return`), reached through `fid`, not
// in a `[dom, cod]` shape. Unifying a function-type therefore clones the
// signature (so the template's shared cells are never bound) rather than doing
// a positional array unify — the mechanism `unify_function_type` below owns.

/// The `AnyFunctionId` a function-type node `[Function(fid), ↺]` carries, or
/// `None` when `ty` is not a function-type node. Reads the class's committed
/// carrier (a bare merge may leave the value on a member other than the
/// representative) and its slot 0.
fn function_type_function<P: Program>(
    module: &mut Module<P>,
    ty: AnyNodeId,
) -> Option<AnyFunctionId>
where
    P::Value: ValueType,
{
    let AnyNodeId::Dynamic(node) = ty else {
        return None;
    };
    let rep = module.equality_representative(node);
    let carrier = module.class_committed_node(rep)?;
    // SAFETY: `carrier` is a live node of `module`; nothing here drops a block.
    let items = unsafe { array_items(module, AnyNodeId::Dynamic(carrier)) }?;
    if items.len() != 2 {
        return None;
    }
    module
        .node_value(items[0].node)
        .and_then(|v| match v.as_enum()? {
            LowValue::Function(fid) => Some(fid),
            _ => None,
        })
}

/// The `[domain, codomain]` pair a type carries for the clone-on-unify, as
/// two dynamic nodes — the signature the function-type is unified against.
///
/// - a **function-type node** `[Function(fid), ↺]`: clone the function
///   template's signature and read the clone's parameter and return *type*
///   cells (fresh cells, so the template's shared cells stay unbound);
/// - an **arrow term** `[[dom, cod], [FunctionType, K]]`: read its `[dom,
///   cod]` shape directly (the counterpart is a concrete type value, not a
///   shared template, so no clone is needed);
/// - anything else: `None` — unifying a function-type against a non-function
///   type is a conflict the caller records.
///
/// The universe is recognised by its self-referential cycle
/// ([`Module::is_self_referential`]) rather than by a caller-supplied handle,
/// because this runs inside the lowlevel's unify policy hook, which has no
/// checker universe to pass — the same reason [`field_names`] reads the cycle.
fn signature_pair<P: Program>(module: &mut Module<P>, ty: AnyNodeId) -> Option<(NodeId, NodeId)>
where
    P::Value: ValueType,
{
    // A function-type node: clone its signature — the clone returns the
    // parameter and return *type cells* directly as (domain, codomain).
    if let AnyNodeId::Dynamic(node) = ty
        && module.is_function_type_node(node)
    {
        let fid = function_type_function(module, ty)?;
        return clone_signature_dynamic(module, fid);
    }
    // An arrow term `[[dom, cod], [FunctionType, K]]` recognised without a
    // universe handle (K by its self-cycle): read its shape's two halves.
    // SAFETY: `ty` is a live node of `module`; nothing here drops a block.
    let Some(items) = (unsafe { array_items(module, ty) }) else {
        return None;
    };
    if items.len() != 2 {
        return None;
    }
    // The kind `[marker, K]`.
    // SAFETY: the kind node is a live node of `module`.
    let Some(kind_items) = (unsafe { array_items(module, items[TYPE_KIND_SLOT].node) }) else {
        return None;
    };
    if kind_items.len() != 2 {
        return None;
    }
    let is_function_kind = module
        .node_value(kind_items[KIND_MARKER_SLOT].node)
        .is_some_and(|v| v == P::Value::function_type_marker())
        && module.is_self_referential(kind_items[KIND_UNIVERSE_SLOT].node);
    if !is_function_kind {
        return None;
    }
    let shape = items[TYPE_SHAPE_SLOT].node;
    // SAFETY: `shape` is a live node of `module`.
    let Some(halves) = (unsafe { array_items(module, shape) }) else {
        return None;
    };
    if halves.len() != 2 {
        return None;
    }
    let dom = dynamic_of(halves[FUNCTION_TYPE_DOMAIN_SLOT].node)?;
    let cod = dynamic_of(halves[FUNCTION_TYPE_CODOMAIN_SLOT].node)?;
    Some((dom, cod))
}

/// [`Module::clone_signature`] over an [`AnyFunctionId`].  A **dynamic**
/// function-type clones the template's signature (so the template's shared
/// cells stay unbound); a **static** one (a frozen module's) has an immutable
/// signature, so its parameter and return type cells are materialized as fresh
/// dynamic leaves and reconciled as a check.
fn clone_signature_dynamic<P: Program>(
    module: &mut Module<P>,
    fid: AnyFunctionId,
) -> Option<(NodeId, NodeId)>
where
    P::Value: ValueType,
{
    match fid {
        AnyFunctionId::Dynamic(function) => module.clone_signature(function),
        AnyFunctionId::Static(sref) => module.materialize_static_signature(sref),
    }
}

/// The dynamic [`NodeId`] of an [`AnyNodeId`], or `None` for a static ref.
fn dynamic_of(id: AnyNodeId) -> Option<NodeId> {
    match id {
        AnyNodeId::Dynamic(n) => Some(n),
        AnyNodeId::Static(_) => None,
    }
}

/// The highlevel's [`Program::unify_function_type`] policy: when a
/// function-type node `[Function(fid), ↺]` (a function's own type, `f : f`)
/// is unified against another type, **clone the function's signature and unify
/// the clone's fresh cells against the counterpart's `[dom, cod]`** — never
/// binding the template's shared cells, so the function stays let-polymorphic
/// and the type chain stays `f : f : f …`.
///
/// Both sides are reduced to a `[dom, cod]` pair by [`signature_pair`]: a
/// function-type node contributes a *cloned* signature, an arrow term
/// contributes its shape directly. **Only when both sides are signatures**
/// (both a function-type node or an arrow term) does the clone-on-unify fire —
/// unifying the two domains and codomains through fresh clone cells. When
/// either side is *not* a signature (the function's own value-equal pair, a
/// scalar, a tuple type), the hook defers (`NotFunctionType`) and the
/// lowlevel's positional unify resolves it: a value-equal pair merges
/// (the function's pair and its type node are `f : f`, one class), and a
/// genuine mismatch (a function-type against `Int`) conflicts. On success the
/// two sides are resolved **without merging classes** — the function-type
/// node stays a distinct, polymorphic class; only the per-site clone's cells
/// were bound.
///
/// See [`function-type-as-function`](../notes/function-type-as-function.md).
pub fn unify_function_type<P: Program>(
    module: &mut Module<P>,
    a: NodeId,
    b: NodeId,
) -> FunctionTypeUnify
where
    P::Value: ValueType,
{
    let sa = signature_pair(module, AnyNodeId::Dynamic(a));
    let sb = signature_pair(module, AnyNodeId::Dynamic(b));
    match (sa, sb) {
        (Some((dom_a, cod_a)), Some((dom_b, cod_b))) => {
            let pre = module.unify_errors.len();
            module.unify(dom_a, dom_b);
            module.unify(cod_a, cod_b);
            if module.unify_errors.len() > pre {
                FunctionTypeUnify::Conflict
            } else {
                FunctionTypeUnify::Handled
            }
        }
        // Exactly one side is a signature. The other is either the function's
        // own value-equal pair (which the positional unify merges — `f : f`,
        // one class), a scalar / tuple type (which it conflicts on), or a
        // *self-referential* type — the universe `Type` or a recursive struct
        // — which the positional unify's "two self-referential arrays merge"
        // rule would wrongly merge with the function-type node. That rule is
        // the one case the clone-on-unify must own: a function's type is not
        // `Type` (nor a struct type), so it is a conflict, not a merge.
        (Some(_), None) => {
            if is_self_referential_type(module, b) {
                FunctionTypeUnify::Conflict
            } else {
                FunctionTypeUnify::NotFunctionType
            }
        }
        (None, Some(_)) => {
            if is_self_referential_type(module, a) {
                FunctionTypeUnify::Conflict
            } else {
                FunctionTypeUnify::NotFunctionType
            }
        }
        (None, None) => FunctionTypeUnify::NotFunctionType,
    }
}

/// Whether `node`'s class holds a **self-referential type** — a 2-element
/// self-cycle, read through the class's committed carrier (a bare merge may
/// leave the value on a member other than the representative). The universe
/// `[Type, ↺]` and a recursive struct's type expression are the instances;
/// a function-type node `[Function(fid), ↺]` is one too, but the callers of
/// this helper have already excluded it (its signature paired). Used by
/// [`unify_function_type`] to refuse merging a function-type node with a
/// self-referential non-function type (`(\x. x) : Type` must fail).
fn is_self_referential_type<P: Program>(module: &mut Module<P>, node: NodeId) -> bool
where
    P::Value: ValueType,
{
    let rep = module.equality_representative(node);
    let Some(carrier) = module.class_committed_node(rep) else {
        return false;
    };
    module.is_self_referential(AnyNodeId::Dynamic(carrier))
}

/// Whether `ty` is a struct type:
/// `[shape, [[payload, TypeStruct], K]]` with `payload = [TypeId, names,
/// names_in_order]`.  The kind's marker slot holds the ordinary
/// `[value, type]` marker pair whose type slot is the `TypeStruct` atom, so the
/// kind is a standard `[marker, K]` pair whose marker is recognised by that tag
/// ([`is_struct_marker_any`]).
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

/// Whether a value is a struct marker: the pair `[payload, TypeStruct]` whose
/// payload is `[TypeId, names, names_in_order]`.
///
/// The test is the **tag**: the marker must be an ordinary `[value, type]` pair
/// (a two-element array) whose [`STRUCT_MARKER_TAG_SLOT`] holds the `TypeStruct`
/// atom ([`ValueType::type_struct_marker`]).  A pair whose type slot is anything
/// else — including a marker-shaped payload under another type — is not a
/// struct marker.  The payload is not inspected: its cells may still be unbound
/// while the marker is being built (a read's pin writes them as fresh cells).
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

/// Whether `ty` is a TypeStruct **kind** — `[[payload, TypeStruct], K]` — the
/// `[marker, universe]` form a raw named read `X::a`
/// requires.  This is the container type's *own* shape (a struct type value's
/// `ty`), not the `[shape, kind]` pair of a struct instance's type (which `.a`
/// reads, [`is_struct_type_any`]): the name→index table lies directly at
/// `ty[0][0][1]`, and the definition-order names at `ty[0][0][2]`.
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
/// whose marker is a `[payload, TypeStruct]` pair carrying a table at the
/// payload's [`STRUCT_MARKER_NAMES_SLOT`].
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

/// The struct's name→index table (the `struct<.a T, …>` names) from a
/// struct type value, or `None` when it is an anonymous struct (no names)
/// or not a struct type at all.  The table is reached through the kind:
/// `[shape, [marker, K]]` → the marker's payload at
/// [`STRUCT_MARKER_NAMES_SLOT`].
///
/// **The universe is supplied by the caller.**  A checker has it; a lowering
/// does not, and [`field_names`] is this reader for that side.
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

/// A type as the encoding holds it: the `[shape, kind]` **term** itself, or a
/// node that *holds* one at its value slot — an expression's type cell, which
/// for an annotated parameter is the annotation's own `[value, type]` pair
/// ([`low_type_of_slot`] resolves that same indirection for a low type).
///
/// **Which one it is, is stated — never guessed.**  A term and a holder are both
/// two-slot arrays, so no structural test separates them (the heuristic
/// [`slot0_is_shape`] documents), while the side that holds one always knows: the
/// checker walks a *type cell*, the lowering a *type term*.  A caller that
/// genuinely does not know asks twice, once per variant, and lets the decode
/// answer (`struct_fields_of_slot` in `lichen-compute`).
///
/// The unwrap this type exists to stop being written by hand is `Index(ty, 0)`:
/// that is a holder's value slot *or* a term's shape, and only the caller's own
/// knowledge says which of the two it just read.
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

/// The **field-type list** of the value type a term names: a struct's field
/// list, a tuple's element list — the term's shape (`[shape, kind]` slot 0).
/// Both kinds carry the list in the same slot, so one reader serves both; what
/// separates them is the kind marker, and each read form pins or checks the one
/// it accepts ([`crate::checker`]'s field reads).
///
/// `None` when the node is not a two-slot term.
pub fn field_list<P: Program>(module: &Module<P>, ty: TypeRef) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    shape_of(module, ty.term(module)?)
}

/// The type at field position `k` of a value type — the node its field list
/// holds there, which is that field's own type term (a fresh cell for a field
/// nothing has decided).
///
/// This is the **resolved** form of the `Index(shape, k)` a reader would
/// otherwise build: `Index(ty, 0)` is the shape and `[k]` its entry, so this is
/// the node that `Index` would evaluate to, read without building an operation
/// node.  The difference is observable wherever a *cell* is read rather than
/// evaluated — [`low_type_of_slot`] cannot see through an unevaluated `Index`,
/// so a class question asked of a field read would answer nothing.
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

/// The **named fields** of a struct type, in field order — `None` for a
/// positional field.  `None` for the whole answer when the type is not a named
/// struct: an anonymous struct (whose marker payload's name slot holds no
/// table),
/// another kind, or a kind that does not close on the universe.
///
/// The universe is recognised by [`Module::is_self_referential`] — the
/// `[Type, ↺]` cycle — because the callers that need this (a lowering, which has
/// no universe handle to pass) are below the checker.  That is the test the
/// renderer and [`class_holds_type`] already use, not a new guess.
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
    // The field count is the shape's own length: the names are sized to the
    // term, not to the table, so a name whose index maps outside the field list
    // is dropped rather than growing it.
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

/// Where `name` sits in a struct type's name→index table — the fold a named read
/// performs, shared by the two sides that hold the table by different routes:
/// the checker reaches it through its canonical universe
/// ([`struct_names_any`]), a lowering through the self-cycle ([`field_names`]).
/// The **gate** stays each caller's, which is the whole reason the two readers
/// exist; the fold over the entries is what they must not each re-spell.
///
/// The stored position is returned as it stands, without a field-count check:
/// the table is the authority here, and a caller that has a field list bounds it
/// against that (as [`field_names`] does).
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

/// One name-table entry decoded as its `(name, position)` pair.  `None` for an
/// entry that is not a `Str` key beside a `USize` value — an entry still being
/// built, or a table this encoding did not write.
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
    // A type constant's own **value** is its marker node: `Int` *is* the
    // `TypeValue::TypeInt` leaf, and the type value `[int, K]` is the array whose
    // shape slot holds that leaf.  Both spellings denote one class, so a marker
    // seen on its own answers what the array answers — and that case is not
    // exotic: the members of a *source* set of type values (`Num = set{Int,
    // Float}`) are markers, so a decoder that could only read arrays classified
    // no source-written class domain at all (`docs/notes/operator-polymorphism.md`
    // §3).
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
    // Otherwise a type expression is `[shape, kind]`; anything else is not a type
    // this decoder can read.
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
