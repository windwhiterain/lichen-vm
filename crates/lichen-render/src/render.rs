//! The pretty printer core: the type printer, the value printer, and the
//! attribute and struct-field renderers.

use std::collections::{HashMap, HashSet};

use lichen_highlevel::attr::{AttrExt, AttrSet};
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
/// (`Int`), a **layout template** a tuple `(1, 2)`, an array `[1, 2, 3]`, a
/// struct its field tuple, and a kind a compound type — the set, the table, the
/// function type.  An opaque chain falls back to the raw layout, marked
/// `raw[…]`, `raw 6` / `raw Int`.  A dump never spells itself like a read.
pub fn print_value<P: HighProgram + 'static>(
    module: &Module<P>,
    value: P::Value,
    ty: NodeId,
) -> String
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
pub fn print_type<P: HighProgram + 'static>(module: &Module<P>, root: NodeId) -> String
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
/// `tail` and `expr` are the schema-tail/expr expr [`render_attributes`] reads,
/// and the label is looked for the same way: one spelling per *present*
/// attribute, first answer wins.  `None` means the expression prints as before.
pub fn value_label<P: HighProgram>(
    module: &Module<P>,
    expr: NodeId,
    tail: &[P::Attr],
    attr_ext: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
) -> Option<String>
where
    P::Value: ValueType,
{
    if tail.is_empty() {
        return None;
    }
    let value = module.node_value(AnyNodeId::Dynamic(expr))?;
    let value = value.as_enum()?;
    for marker in tail {
        let Some(AnyNodeId::Dynamic(slot)) =
            module.array_attribute(&value, marker.order_index() as u32)
        else {
            continue;
        };
        if let Some(label) = attr_ext(marker).label(module, slot) {
            // A labelled value reads as `?name`: the name is the attribute's.
            return Some(format!("?{label}"));
        }
    }
    None
}

/// Render the attributes an expression carries, from its schema tail and runtime
/// expr, space-separated.
///
/// # Invariant
///
/// Empty when the expression carries no attribute, and otherwise one spelling
/// per *present* attribute: an un-annotated expression renders as before, and
/// only attributes that are genuinely there appear. A position holding the
/// attribute's **default** is one an absent occurrence reads, so it was never
/// spelled and does not appear either ([`AttrExt::holds_default`]).
pub fn render_attributes<P: HighProgram>(
    module: &Module<P>,
    expr: NodeId,
    tail: &[P::Attr],
    attr_ext: &dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>,
) -> String
where
    P::Value: ValueType,
{
    if tail.is_empty() {
        return String::new();
    }
    let Some(value) = module.node_value(AnyNodeId::Dynamic(expr)) else {
        return String::new();
    };
    let Some(value) = value.as_enum() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for marker in tail {
        let Some(AnyNodeId::Dynamic(slot)) =
            module.array_attribute(&value, marker.order_index() as u32)
        else {
            continue;
        };
        let ext = attr_ext(marker);
        if ext.holds_default(module, slot) {
            continue;
        }
        if let Some(spelling) = ext.render(module, slot, attr_ext) {
            parts.push(spelling);
        }
    }
    parts.join(" ")
}

/// The shared pretty type printer, stateful across calls so one instance names
/// classes consistently within a diagnostic.
pub struct TypePrinter<'a, P: HighProgram + 'static>
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
    /// The attribute registry, the same callback [`render_attributes`] reads;
    /// `None` spells no element attribute.
    attrs: Option<Attrs<'a, P>>,
}

/// The **attribute registry** a printer spells element attributes through.
pub type Attrs<'a, P> = &'a dyn Fn(&<P as HighProgram>::Attr) -> &'static dyn AttrExt<P>;

/// The pretty value printer: renders a runtime value against its type chain.
pub struct ValuePrinter<'a, P: HighProgram + 'static>
where
    P::Value: ValueType,
{
    module: &'a Module<P>,
    printer: TypePrinter<'a, P>,
    /// Value nodes on the current recursion; a cycle renders as `…`.
    path: AncestorNodes<NodeId>,
    /// Type nodes on the current recursion; a cycle renders as `…`.
    tpath: AncestorNodes<AnyNodeId>,
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
/// as an `Int`.  A non-finite float prints as its own name — `NaN`, `inf`,
/// `-inf` — and a name is not a literal the reader accepts, so the reader
/// refuses it as it already refused `NaN`.  See docs/notes/floating-point.md.
fn float_literal(value: f32) -> String {
    if !value.is_finite() {
        // A non-finite float prints under `Debug`'s own class name: `NaN`, `inf`, `-inf`.
        return format!("{value:?}");
    }
    let mut text = value.to_string();
    if !text.contains('.') {
        text.push_str(".0");
    }
    text
}

/// Whether a type node is the shared **`Struct` template`.
///
/// # Invariant
///
/// The template is recognised by its **value**: the marker at the value slot is
/// the struct one, and only the template carries it.  It is the
/// self-referential `[StructType, ↺]` every struct value's type is.
fn is_struct_template<P: HighProgram>(module: &Module<P>, ty: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(items) = shape::value_items(module, ty) else {
        return false;
    };
    items.len() >= 2
        && module.node_value(items[shape::EXPR_VALUE_SLOT].node)
            == Some(P::Value::type_struct_marker())
}

/// The per-field names of a struct **value**, read from its own name table.
///
/// # Invariant
///
/// The result is sized to the field count: `None` for an unnamed field, `Some`
/// for a named one; a name whose index falls outside the field list is
/// dropped.  The table is the value's own, so the names travel with the value
/// and a clone inherits them.
fn struct_value_names<P: HighProgram>(
    module: &Module<P>,
    items: &[ArrayItem],
) -> Vec<Option<&'static str>>
where
    P::Value: ValueType,
{
    let field_count = items.len().saturating_sub(shape::STRUCT_VALUE_FIELD_BASE);
    let mut out = vec![None; field_count];
    let Some(names_item) = items.get(shape::STRUCT_VALUE_NAMES_SLOT) else {
        return out;
    };
    let Some(LowValue::Table(table)) = module.node_value(names_item.node).and_then(|v| v.as_enum())
    else {
        // An anonymous struct has no table: every field is unnamed.
        return out;
    };
    // SAFETY: `table` is the payload of the value read from the live node
    // `names_item` of the module being rendered.
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

/// The per-field names of a struct **value**, in field order.
///
/// # Invariant
///
/// The names come from the value's own name table — the layout authority — and
/// never from a hardcoded shape; `None` when the node holds no struct value.
pub fn struct_value_named_fields<P: HighProgram>(
    module: &Module<P>,
    value: AnyNodeId,
) -> Option<Vec<Option<&'static str>>>
where
    P::Value: ValueType,
{
    let items = shape::value_items(module, value)?;
    // The nominal id at the second slot is the struct's own: no other layout
    // carries one.
    let id = module.node_value(items.get(shape::STRUCT_VALUE_ID_SLOT)?.node)?;
    id.type_id()?;
    Some(struct_value_names(module, items))
}

/// Render a struct value's named fields (`name = value, …`), the names read
/// from the value's own name table.
///
/// # Invariant
///
/// The renderer walks the **value**, so the names come from the value's name
/// table — the layout authority — and never from a hardcoded shape; `None`
/// when the value and type do not form a struct value.
pub fn render_struct_fields_named<P: HighProgram + 'static>(
    module: &Module<P>,
    value_node: AnyNodeId,
    ty_node: AnyNodeId,
) -> Option<String>
where
    P::Value: ValueType,
{
    // The type is the shared `Struct` template: the layout is the value's, and
    // the template says which layout this is.
    if !is_struct_template(module, ty_node) {
        return None;
    }
    let items = shape::value_items(module, value_node)?;
    if items.len() < shape::STRUCT_VALUE_FIELD_BASE {
        return None;
    }
    let names = struct_value_names(module, items);
    let mut vp = ValuePrinter::new(module);
    let mut fields = Vec::with_capacity(items.len() - shape::STRUCT_VALUE_FIELD_BASE);
    for (index, field) in items[shape::STRUCT_VALUE_FIELD_BASE..].iter().enumerate() {
        // A field is a complete expression: its value and its type are its own.
        let rendered = vp.element_expr(field.node);
        match names.get(index).copied().flatten() {
            Some(name) => fields.push(format!("{name} = {rendered}")),
            None => fields.push(rendered),
        }
    }
    Some(fields.join(", "))
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
    if items.len() < 2 || module.node_value(items[0].node) != Some(P::Value::type_marker()) {
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
                    items.len() >= 2
                        && module.node_value(items[0].node) == Some(P::Value::type_marker())
                        && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
                }
                _ => false,
            }),
    }
}
