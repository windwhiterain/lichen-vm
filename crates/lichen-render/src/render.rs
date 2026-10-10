//! The pretty printer core: the type printer, the value printer, and the
//! attribute and struct-field renderers.

use std::collections::{HashMap, HashSet};

use lichen_highlevel::attr::AttrExt;
use lichen_highlevel::program::{HighProgram, ValueType};
use lichen_highlevel::shape;
use lichen_lowlevel::ancestors::AncestorNodes;
use lichen_lowlevel::{AnyFunctionId, AnyNodeId, ArrayItem, LowValue, Module, NodeId};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

mod type_printer;
mod value_printer;
// A rendering hook for extension vocabularies: an extension value to its
// spelling, or `None` when not the extension's.

type RenderExt<'a, V> = &'a dyn Fn(&V) -> Option<String>;

/// Render a runtime value as the program's output, read against its type.
///
/// # Invariant
///
/// The type chain decides how the value reads: the universe an atomic constant
/// (`Int`), a kind a compound type, a tuple `(1, Int)`, an array `[1, 2, 3]`, a
/// struct its field tuple.  An opaque chain — an undecided cell, an extension
/// type — falls back to the raw layout, and every reading of it is marked:
/// `raw[…]`, `raw 6` / `raw Int`.  A dump never spells itself like a read.
pub fn print_value<P: HighProgram>(module: &Module<P>, value: P::Value, ty: NodeId) -> String
where
    P::Value: ValueType,
{
    ValuePrinter::new(module).print(value, ty)
}

/// Render a type expression in the language's own type syntax.
///
/// # Invariant
///
/// Undecided cells get stable `?a`, `?b`, … names, shared within a unification
/// class, so the type shows which parts are linked; cycles are cut at `…`; a node
/// the walk cannot read as a form renders as its marked raw layout.
pub fn print_type<P: HighProgram>(module: &Module<P>, root: NodeId) -> String
where
    P::Value: ValueType,
{
    TypePrinter::new(module).node(root)
}

/// The **label** an expression reads as when an attribute names it: how a value
/// with no spelling of its own is printed.
///
/// # Invariant
///
/// `tail` and `pair` are the schema-tail/pair pair [`render_attributes`] reads,
/// and the label is looked for the same way: one spelling per *present*
/// attribute, first answer wins.  `None` means the expression prints as before.
pub fn value_label<P: HighProgram>(
    module: &Module<P>,
    pair: NodeId,
    tail: &[P::Attr],
    attr_ext: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
) -> Option<String>
where
    P::Value: ValueType,
{
    if tail.is_empty() {
        return None;
    }
    let values = module
        .node_value(AnyNodeId::Dynamic(pair))
        .and_then(|v| v.as_enum())
        .and_then(|v| match v {
            // SAFETY: `a` is a value read from a live node of the module being
            // rendered; this printer releases no block.
            LowValue::Array(a) => Some(unsafe { a.items() }.to_vec()),
            _ => None,
        })?;
    for (i, marker) in tail.iter().enumerate() {
        let slot = values
            .get(shape::attr_slot(i))
            .and_then(|item| match item.node {
                AnyNodeId::Dynamic(n) => Some(n),
                AnyNodeId::Static(_) => None,
            });
        if let Some(slot) = slot
            && let Some(label) = attr_ext(marker).label(module, slot)
        {
            // A labelled value reads as `?name`: the name is the attribute's.
            return Some(format!("?{label}"));
        }
    }
    None
}

/// Render the attributes an expression carries, from its schema tail and runtime
/// pair, space-separated.
///
/// # Invariant
///
/// Empty when the expression carries no attribute, and otherwise one spelling
/// per *present* attribute: an un-annotated expression renders as before, and
/// only attributes that are genuinely there appear.
pub fn render_attributes<P: HighProgram>(
    module: &Module<P>,
    pair: NodeId,
    tail: &[P::Attr],
    attr_ext: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
) -> String
where
    P::Value: ValueType,
{
    if tail.is_empty() {
        return String::new();
    }
    let values = module
        .node_value(AnyNodeId::Dynamic(pair))
        .and_then(|v| v.as_enum())
        .and_then(|v| match v {
            // SAFETY: `a` is a value read from a live node of the module being
            // rendered; this printer releases no block.
            LowValue::Array(a) => Some(unsafe { a.items() }.to_vec()),
            _ => None,
        });
    let Some(values) = values else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (i, marker) in tail.iter().enumerate() {
        let slot = values
            .get(shape::attr_slot(i))
            .and_then(|item| match item.node {
                AnyNodeId::Dynamic(n) => Some(n),
                AnyNodeId::Static(_) => None,
            });
        if let Some(slot) = slot
            && let Some(spelling) = attr_ext(marker).render(module, slot, attr_ext)
        {
            parts.push(spelling);
        }
    }
    parts.join(" ")
}

/// The shared pretty type printer, stateful across calls so one instance names
/// classes consistently within a diagnostic.
pub struct TypePrinter<'a, P: HighProgram>
where
    P::Value: ValueType,
{
    module: &'a Module<P>,
    /// The checker's arrow-shape registry: a bare `[in, out]` shape renders as an
    /// arrow only when registered, else raw.
    arrows: Option<&'a HashSet<NodeId>>,
    /// Class names within one type: representative → `?a`, `?b`, ….
    names: HashMap<NodeId, String>,
    /// Names for undecided *static* cells, keyed by the absolute static ref so one
    /// signature cell keeps one name.
    static_names: HashMap<lichen_lowlevel::StaticNodeId, String>,
    next: usize,
    /// Array nodes on the current recursion; a cycle renders as `…`.
    path: AncestorNodes<NodeId>,
    /// The extension's own value variants; `None` leaves them `?`.
    render_ext: Option<RenderExt<'a, P::Value>>,
    /// Render a struct type's nominal id as `struct<…>#n`, on for the diagnostic
    /// printer only.
    show_struct_id: bool,
}

/// The pretty value printer: renders a runtime value against its type chain.
pub struct ValuePrinter<'a, P: HighProgram>
where
    P::Value: ValueType,
{
    module: &'a Module<P>,
    printer: TypePrinter<'a, P>,
    /// Value nodes on the current recursion; a cycle renders as `…`.
    path: AncestorNodes<NodeId>,
    /// Type nodes on the current recursion; a cycle renders as `…`.
    tpath: AncestorNodes<NodeId>,
    /// Cells the current raw dump has entered; one met again renders `…`, which
    /// stops a self-referential kind unrolling.
    ///
    /// # Invariant
    ///
    /// Static refs count too: a frozen kind carries its own self-loop.
    raw_path: AncestorNodes<AnyNodeId>,
}

/// The class representative of `node`, via a read-only `parent` walk (the
/// printers never mutate the module).
///
/// # Invariant
///
/// `None` when the walk cannot answer — a node the table does not hold, or a
/// chain longer than the table (a revisit) — so the caller renders its own "no
/// answer" instead of panicking or looping.
fn representative<P: HighProgram>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P::Value: ValueType,
{
    let mut n = node;
    // A parent chain visits each node at most once, so it is never longer than
    // the node table.
    for _ in 0..=module.nodes.len() {
        module.nodes.get(n)?;
        match module.node_equality(n).parent() {
            Some(parent) => n = parent,
            None => return Some(n),
        }
    }
    None
}

/// `0 → "?a"`, `1 → "?b"`, …, `26 → "?a1"`, `27 → "?b1"`, …
fn letter_name(i: usize) -> String {
    let letter = (b'a' + (i % 26) as u8) as char;
    let round = i / 26;
    if round == 0 {
        format!("?{letter}")
    } else {
        format!("?{letter}{round}")
    }
}

/// The source spelling of a float value, the one place a float becomes text.
///
/// # Invariant
///
/// The digits are `f32`'s own positional `Display`, never `{:?}`, whose exponent
/// the lexer has no literal for; a `.` is forced, since the bare digits read back
/// as an `Int`; an infinity is a magnitude past the round-to-infinity threshold,
/// the only form the lexer reads back, and `NaN` keeps Rust's spelling.  See
/// docs/notes/floating-point.md.
fn float_literal(value: f32) -> String {
    if value.is_nan() {
        return value.to_string();
    }
    if value.is_infinite() {
        // `2 * f32::MAX` in `f64` is exact and past the threshold, and `f64`'s
        // own `Display` spells that magnitude positionally.
        let magnitude = f64::from(f32::MAX) * 2.0;
        let sign = if value.is_sign_negative() { "-" } else { "" };
        return format!("{sign}{magnitude}.0");
    }
    let mut text = value.to_string();
    if !text.contains('.') {
        text.push_str(".0");
    }
    text
}

/// Whether `node` is itself a struct kind, rather than a struct type term whose
/// kind slot is such a node.
fn is_struct_kind<P: HighProgram>(module: &Module<P>, node: NodeId) -> bool
where
    P::Value: ValueType,
{
    module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|v| v.as_enum())
        .is_some_and(|v| match v {
            // SAFETY: `kind` is the payload of the value read from the live
            // node `node`.
            LowValue::Array(kind) => kind_is_struct(module, unsafe { kind.items() }),
            _ => false,
        })
}

/// Whether `node`'s class is the canonical universe `K = [Type, ↺]`.
///
/// # Invariant
///
/// The head is the `Type` marker and the length is two, checked before the tail:
/// a kind `[marker, K]` whose `K` is the frozen universe has the same
/// self-referential silhouette, so a tail-only test would read every such kind
/// as the universe.  The tail is a class comparison, so it covers the canonical
/// node and a cell carrying the replicated value.
fn is_universe<P: HighProgram>(module: &Module<P>, node: NodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(rep) = representative(module, node) else {
        return false;
    };
    let Some(LowValue::Array(array)) = module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|value| value.as_enum())
    else {
        return false;
    };
    // SAFETY: `array` is the payload of the value read from the live node
    // `node`.
    let items = unsafe { array.items() };
    if items.len() != 2 || module.node_value(items[0].node) != Some(P::Value::type_marker()) {
        return false;
    }
    match items[1].node {
        AnyNodeId::Dynamic(item) => representative(module, item) == Some(rep),
        AnyNodeId::Static(_) => is_universe_any(module, items[1].node),
    }
}

/// The read-only, static-aware universe test shared by the printer helpers.
fn is_universe_any<P: HighProgram>(module: &Module<P>, id: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    match id {
        AnyNodeId::Dynamic(node) => is_universe(module, node),
        AnyNodeId::Static(sref) => module
            .node_value(id)
            .and_then(|v| v.as_enum())
            .is_some_and(|v| match v {
                LowValue::Array(array) => {
                    // SAFETY: `array` is a static payload read through `sref`,
                    // whose registered module pins the arena.
                    let items = unsafe { array.items() };
                    items.len() == 2
                        && module.node_value(items[0].node) == Some(P::Value::type_marker())
                        && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
                }
                _ => false,
            }),
    }
}

/// Whether a value is a struct marker, the pair `[payload, TypeStruct]`.
///
/// # Invariant
///
/// The test is the tag: the marker is a two-element `[value, type]` pair whose
/// type slot holds the `TypeStruct` atom; anything else is not a struct marker.
fn marker_is_struct<P: HighProgram>(module: &Module<P>, marker: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(LowValue::Array(m)) = module.node_value(marker).and_then(|v| v.as_enum()) else {
        return false;
    };
    // SAFETY: `m` is the payload of the value read from the live node `marker`.
    let items = unsafe { m.items() };
    items.len() == 2
        && module.node_value(items[shape::STRUCT_MARKER_TAG_SLOT].node)
            == Some(P::Value::type_struct_marker())
}

/// Whether `kind_items` describe a struct kind: a standard `[marker, K]` pair
/// whose marker carries the `TypeStruct` tag.
fn kind_is_struct<P: HighProgram>(module: &Module<P>, kind_items: &[ArrayItem]) -> bool
where
    P::Value: ValueType,
{
    kind_items.len() == 2
        && is_universe_any(module, kind_items[1].node)
        && marker_is_struct(module, kind_items[0].node)
}

/// The per-field names of a struct type, read from the marker pair's payload.
///
/// # Invariant
///
/// The result is sized to `field_count`: `None` for an unnamed field, `Some` for
/// a `.name Ty` one; a name whose index falls outside the field list is dropped.
fn struct_field_names<P: HighProgram>(
    module: &Module<P>,
    kind_items: &[ArrayItem],
    field_count: usize,
) -> Vec<Option<&'static str>>
where
    P::Value: ValueType,
{
    let mut out = vec![None; field_count];
    let marker_items = module
        .node_value(kind_items[0].node)
        .and_then(|v| v.as_enum())
        .and_then(|v| match v {
            // SAFETY: `m` is the payload of a value read from a live node of
            // the module being rendered.
            LowValue::Array(m) => Some(unsafe { m.items() }),
            _ => None,
        });
    let Some(marker_items) = marker_items else {
        return out;
    };
    // The marker pair's value slot is the payload `[TypeId, names,
    // names_in_order]`.
    let Some(payload_item) = marker_items.get(shape::STRUCT_MARKER_PAYLOAD_SLOT) else {
        return out;
    };
    let Some(LowValue::Array(payload)) = module
        .node_value(payload_item.node)
        .and_then(|v| v.as_enum())
    else {
        return out;
    };
    // SAFETY: `payload` is the payload of a value read from a live node of the
    // module being rendered.
    let Some(names_item) = unsafe { payload.items() }.get(shape::STRUCT_MARKER_NAMES_SLOT) else {
        return out;
    };
    let Some(LowValue::Table(table)) = module.node_value(names_item.node).and_then(|v| v.as_enum())
    else {
        return out;
    };
    // SAFETY: `table` is the payload of the value read from the live node
    // `names_item`.
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
            out[index] = Some(name);
        }
    }
    out
}

/// The named-field list of a struct type term, read from its kind marker pair.
///
/// # Invariant
///
/// `None` when `node` is not a concrete struct type (an undecided cell, a tuple,
/// an array, a function); otherwise sized to the shape with `None` entries
/// positional and `Some` entries `.name Ty`.
pub fn struct_type_named_fields<P: HighProgram>(
    module: &Module<P>,
    node: NodeId,
) -> Option<Vec<Option<&'static str>>>
where
    P::Value: ValueType,
{
    // The type term `[shape, kind]`.
    let ty = module.node_value(AnyNodeId::Dynamic(node))?;
    let LowValue::Array(ty_arr) = ty.as_enum()? else {
        return None;
    };
    // SAFETY: `ty_arr`, `shape` and `kind` come from live nodes of `module`,
    // whose `items()` calls this note covers.
    let tys = unsafe { ty_arr.items() };
    if tys.len() != 2 {
        return None;
    }
    // The shape (the positional field-type list) gives the field count.
    let LowValue::Array(shape) = module.node_value(tys[0].node)?.as_enum()? else {
        return None;
    };
    let field_count = unsafe { shape.items() }.len();
    // The kind `[marker, K]`: only a struct kind carries a name table.
    let LowValue::Array(kind) = module.node_value(tys[1].node)?.as_enum()? else {
        return None;
    };
    let kind_items = unsafe { kind.items() };
    if !kind_is_struct(module, kind_items) {
        return None;
    }
    Some(struct_field_names(module, kind_items, field_count))
}

/// The nominal id of a struct type, read from its kind's marker payload.
fn struct_kind_id<P: HighProgram>(module: &Module<P>, kind_items: &[ArrayItem]) -> Option<usize>
where
    P::Value: ValueType,
{
    let marker_items = module
        .node_value(kind_items[0].node)
        .and_then(|v| v.as_enum())
        .and_then(|v| match v {
            // SAFETY: `m` is the payload of a value read from a live node of
            // `module`.
            LowValue::Array(m) => Some(unsafe { m.items() }),
            _ => None,
        })?;
    let payload_item = marker_items.get(shape::STRUCT_MARKER_PAYLOAD_SLOT)?;
    // SAFETY: `payload` is the payload of a value read from a live node of
    // `module`.
    let LowValue::Array(payload) = module.node_value(payload_item.node)?.as_enum()? else {
        return None;
    };
    let id_item = unsafe { payload.items() }.get(shape::STRUCT_MARKER_ID_SLOT)?;
    module.node_value(id_item.node).and_then(|v| v.type_id())
}

/// Render a struct field list with per-field names (`.name T` for a named
/// field, `T` for an unnamed one).
fn struct_fields_with_names(fields: &[String], names: &[Option<&'static str>]) -> Vec<String> {
    fields
        .iter()
        .enumerate()
        .map(|(i, ty)| match names.get(i).copied().flatten() {
            // The canonical spelling is the `.name type` prefix marker.
            Some(name) => format!(".{name} {ty}"),
            None => ty.clone(),
        })
        .collect()
}

/// Render a struct instance's named fields (`name = value, …`), the names read
/// from the struct type's name table.
///
/// # Invariant
///
/// The renderer walks the *type chain*, so the names come from the type, never a
/// hardcoded shape; `None` when the value and type do not form a struct instance.
pub fn render_struct_fields_named<P: HighProgram>(
    module: &Module<P>,
    value_node: AnyNodeId,
    ty_node: AnyNodeId,
) -> Option<String>
where
    P::Value: ValueType,
{
    // The struct instance value: a field-tuple array.
    let value = module.node_value(value_node)?.as_enum()?;
    let LowValue::Array(value_arr) = value else {
        return None;
    };
    // SAFETY: each slice below is a value read from a live node of `module`,
    // whose `items()` calls this note covers.
    let field_values = unsafe { value_arr.items() };

    // The struct type: `[shape, kind]` where kind is `[marker, K]`.
    let ty = module.node_value(ty_node)?.as_enum()?;
    let LowValue::Array(ty_arr) = ty else {
        return None;
    };
    let tys = unsafe { ty_arr.items() };
    if tys.len() != 2 {
        return None;
    }
    let kind = module.node_value(tys[1].node)?.as_enum()?;
    let LowValue::Array(kind_items) = kind else {
        return None;
    };
    if !kind_is_struct(module, unsafe { kind_items.items() }) {
        return None;
    }
    // The shape is the positional field-type list.
    let shape = module.node_value(tys[0].node)?.as_enum()?;
    let LowValue::Array(shape_items) = shape else {
        return None;
    };
    let field_types = unsafe { shape_items.items() };
    let names = struct_field_names(module, unsafe { kind_items.items() }, field_values.len());

    let mut vp = ValuePrinter::new(module);
    let mut fields = Vec::with_capacity(field_values.len());
    for (i, value_item) in field_values.iter().enumerate() {
        let rendered = match field_types.get(i) {
            Some(ty) => vp.element_any(value_item.node, ty.node),
            None => "?".to_string(),
        };
        match names.get(i).copied().flatten() {
            Some(name) => fields.push(format!("{name} = {rendered}")),
            None => fields.push(rendered),
        }
    }
    Some(fields.join(", "))
}
