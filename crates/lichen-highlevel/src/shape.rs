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
//! The 8 kind markers (`Int`, `String`, `Type`, `Function`, `Tuple`,
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
    AnyHandle, AnyNodeId, ArrayItem, Deferral, LowValue, Module, NodeId, PendingSides, Program,
    StaticNodeId, TableItem, UnifyStep,
};
use lichen_utils::extend::AsEnum;

use crate::ir::LocStep;
use crate::program::ValueType;

// --- the kind-marker registry ---------------------------------------------------
//
// THE one list of the 8 kind markers.  Adding or removing a marker means
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
// presentation can never renumber the format).
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
pub fn array_items<P: Program>(module: &Module<P>, id: AnyNodeId) -> Option<&'static [ArrayItem]>
where
    P::Value: AsEnum<LowValue>,
{
    let value = module.node_value(id)?;
    let LowValue::Array(array) = value.as_enum()? else {
        return None;
    };
    Some(array.items())
}

/// The shape slot of a kinded type expression `[shape, kind]`, for a dynamic
/// node or a static ref alike.  `None` when `ty` is not a 2-element array
/// (an unbound cell, a bare marker — anything that is not a type-expression
/// pair).
pub fn shape_of<P: Program>(module: &Module<P>, ty: AnyNodeId) -> Option<AnyNodeId>
where
    P::Value: AsEnum<LowValue>,
{
    let items = array_items(module, ty)?;
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
    let items = array_items(module, ty)?;
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
    let items = array_items(module, kind)?;
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
    let Some(items) = array_items(module, AnyNodeId::Static(sref)) else {
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

/// Whether `node`'s class holds a **type value**: a kinded type expression
/// `[shape, [marker, universe]]` — an arrow, tuple, array, struct, table, or
/// atomic type.  This is the highlevel half of what the lowlevel used to
/// know: "does this class hold a type" is meaningless without the encoding,
/// so it lives here, in the module that owns the encoding.
///
/// It is what makes [`defer_pending`] sound: a field/positional read's own
/// *type* is such a pair, so unifying a pending read against a type value is
/// a type round-trip rather than a value comparison.  A scalar — an `Int`
/// *value* as opposed to its type — is not a type value, and unifying a read
/// against one is left to fail.
pub fn class_holds_type<P: Program>(module: &mut Module<P>, node: NodeId) -> bool
where
    P::Value: AsEnum<LowValue>,
{
    let rep = module.equality_representative(node);
    let Some(kind) = kind_of(module, AnyNodeId::Dynamic(rep)) else {
        return false;
    };
    let Some(kind_items) = array_items(module, kind) else {
        return false;
    };
    // The kind's second slot is the universe — the self-referential cycle
    // every type chain bottoms out at (`K = [Type, ↺]`), recognised by its
    // cycle shape; see [`Module::is_self_referential`].
    module.is_self_referential(kind_items[KIND_UNIVERSE_SLOT].node)
}

/// The highlevel's [`Program::defer_pending`] policy: merge a pending
/// field/positional **read** with a class that **holds a type**, and nothing
/// else.
///
/// The merge is sound because neither side can be compared yet: the read
/// resolves to its field's actual type once the container binds, and a
/// genuine mismatch then surfaces at apply time against the real container
/// (the computation, when it runs, must reconcile with the value the other
/// side carried).  The deferral stays deliberately narrow — only an
/// *unresolvable* `Index` qualifies, never a resolved read nor arithmetic
/// nor a dependent-type branch — so an unresolvable real computation still
/// records an error.  Every other case returns `None` and falls through to
/// the lowlevel's generic rules.
pub fn defer_pending<P: Program>(module: &mut Module<P>, sides: &PendingSides) -> Option<Deferral>
where
    P::Value: AsEnum<LowValue>,
{
    let read_against_type = (sides.a.pending
        && sides.a.pending_index_read
        && class_holds_type(module, sides.b.representative))
        || (sides.b.pending
            && sides.b.pending_index_read
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
    let Some(items) = array_items(module, kind) else {
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
    let Some(items) = array_items(module, ty) else {
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
    let Some(items) = array_items(module, ty) else {
        return false;
    };
    if items.len() != 2 {
        return false;
    }
    let Some(kind_items) = array_items(module, items[TYPE_KIND_SLOT].node) else {
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
    array_items(module, marker).is_some_and(|items| items.len() == STRUCT_MARKER_LEN)
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
    let Some(items) = array_items(module, ty) else {
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
    let Some(items) = array_items(module, ty) else {
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

/// The struct's name→index table (the `struct<.a T, …>` names) from a
/// struct type value, or `None` when it is an anonymous struct (no names)
/// or not a struct type at all.  The table is reached through the kind:
/// `[shape, [marker, K]]` → the marker's [`STRUCT_MARKER_NAMES_SLOT`].
pub fn struct_names_any<P: Program>(
    module: &mut Module<P>,
    universe: NodeId,
    ty: AnyNodeId,
) -> Option<AnyHandle<[TableItem]>>
where
    P::Value: ValueType,
{
    let items = array_items(module, ty)?;
    if items.len() != 2 {
        return None;
    }
    let kind_items = array_items(module, items[TYPE_KIND_SLOT].node)?;
    if kind_items.len() != 2
        || !is_universe_any(module, universe, kind_items[KIND_UNIVERSE_SLOT].node)
    {
        return None;
    }
    // The struct marker `[id, names]`; its second field is the name table.
    let marker_items = array_items(module, kind_items[KIND_MARKER_SLOT].node)?;
    let Some(names_item) = marker_items.get(STRUCT_MARKER_NAMES_SLOT) else {
        return None;
    };
    match module.node_value(names_item.node).and_then(|v| v.as_enum()) {
        Some(LowValue::Table(table)) => Some(table),
        _ => None,
    }
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
    let Some(items) = module.array_items(node) else {
        return false;
    };
    if items.is_empty() {
        return false;
    }
    match items[0].node {
        AnyNodeId::Dynamic(child) => module.array_items(child).is_some(),
        // A static element is a leaf (a package export); it is never a
        // tuple/array/struct shape we descend into.
        AnyNodeId::Static(_) => false,
    }
}
