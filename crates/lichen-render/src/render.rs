//! The program-generic pretty printer core: the type printer, the value
//! printer, and the attribute-list / struct-field renderers.  All generic over
//! `P: HighProgram`, so a host and a plugin that contributes an attribute
//! (e.g. `lichen-doc`'s `? name = "…"`) reuse the same machinery.
//!
//! Everything here reads the top of the recursive-pair encoding and spells it
//! in a concrete language's syntax.  Nothing names a concrete host program —
//! a host (e.g. `lichen-language`) composes `ValuePrinter`/`TypePrinter` with
//! its own value vocabulary and the free printers below.
//!
//! The two printers' methods live in the sibling modules `type_printer` and
//! `value_printer`; their state and the shared helpers stay here.

use std::collections::{HashMap, HashSet};

use lichen_highlevel::attr::AttrExt;
use lichen_highlevel::program::{HighProgram, ValueType};
use lichen_highlevel::shape;
use lichen_lowlevel::ancestors::AncestorNodes;
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module, NodeId};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

mod type_printer;
mod value_printer;
// A rendering hook for extension vocabularies: an extension value → its
// spelling, or `None` when the value is not the extension’s to name.  The
// alias exists because the bare closure type trips clippy’s `type_complexity`.

type RenderExt<'a, V> = &'a dyn Fn(&V) -> Option<String>;

/// Render a runtime value as the program's output, *read against its type*.
///
/// The type chain decides how the value reads: a value whose type is the
/// universe is an atomic type constant (`Int` / `Type`), a value whose type
/// is a kind is a compound type (`struct<Int, Type>`, `Int -> Int`,
/// `<Int, Type>`, `array<Int, 3>`), a value whose type is a tuple type reads as a
/// tuple `(1, Int)`, an array type as an array `[1, 2, 3]`, and a struct
/// type as its field tuple.  When the type chain is opaque (an unbound cell,
/// an extension type), the value falls back to its raw layout — the old
/// `[head, K]` reading of a recursive pair, with a list of cells spelled
/// `raw[…]` so a raw reading never reads as a form the chain explained.
pub fn print_value<P: HighProgram>(module: &Module<P>, value: P::Value, ty: NodeId) -> String
where
    P::Value: ValueType,
{
    ValuePrinter::new(module).print(value, ty)
}

/// Render a type expression (the recursive-pair encoding again) in the
/// language's own type syntax: `Int`, `Type`, `T1 -> T2`, `<T1, ..., Tn>`,
/// `array<T, len>`, `struct<T1, ...>`.  Unbound cells get stable `?a`, `?b`, …
/// names — cells in the same unification class share a name — so the type
/// shows which parts are linked.  Cycles are cut at `…`; a node the walk
/// cannot read as a form renders as its raw layout, marked `raw[…]`.
pub fn print_type<P: HighProgram>(module: &Module<P>, root: NodeId) -> String
where
    P::Value: ValueType,
{
    TypePrinter::new(module).node(root)
}

/// The **label** an expression reads as, when one of the attributes it carries
/// names it — the general answer to "how is a value printed" for a value with no
/// spelling of its own (a function, which is what a refinement's slot holds).
///
/// `tail` and `pair` are the same schema-tail/pair pair [`render_attributes`]
/// reads, and the label is looked for the same way: one spelling per *present*
/// attribute, first answer wins.  `None` means the expression prints as it
/// always did.
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
            // SAFETY: `a` is the payload of a value read from a live node of
            // the module being rendered; this printer releases no block.
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
            // A labelled value *reads* as `?name`: the name is the attribute's
            // ([`AttrExt::label`]), the doc sigil is this reading's.
            return Some(format!("?{label}"));
        }
    }
    None
}

/// Render the attributes an expression actually carries, from its schema tail
/// (the expression's attribute *set*) and its runtime pair.  Returns empty when
/// the expression carries no attribute; otherwise one spelling per *present*
/// attribute, space-separated — so an un-annotated expression renders exactly
/// as it always did, and only the attributes that are genuinely there appear.
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
            // SAFETY: `a` is the payload of a value read from a live node of
            // the module being rendered; this printer releases no block.
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

/// The shared pretty type printer: stateful across calls, so one instance
/// renders a whole diagnostic (or report) with consistent `?a`/`?b` class
/// names.  Generic over the value vocabulary: the lowlevel structural values
/// render through [`AsEnum`], the type constants through [`ValueType`], and
/// an extension's own variants through the render hook.
pub struct TypePrinter<'a, P: HighProgram>
where
    P::Value: ValueType,
{
    module: &'a Module<P>,
    /// The checker's arrow-shape registry, when rendering diagnostics: a
    /// class whose representative is a bare `[in, out]` shape (no kind
    /// wrapper) renders as an arrow only if a member is registered here.
    /// The CLI output path has no registry and leaves such shapes raw, which
    /// the `raw[…]` mark makes visible.
    arrows: Option<&'a HashSet<NodeId>>,
    /// Stable class names: representative → `?a`, `?b`, …, within one type
    /// (or one diagnostic report).
    names: HashMap<NodeId, String>,
    /// Stable names for unbound *static* cells — a frozen module's type
    /// variables.  Keyed by the absolute static ref, so the same cell (e.g. a
    /// kernel's shared `d`/`c` signature cells) keeps one name across the
    /// whole type, exactly as a dynamic class shares one.  Distinct from
    /// `names` because a static ref has no union-find class to join.
    static_names: HashMap<lichen_lowlevel::StaticNodeId, String>,
    next: usize,
    /// Array nodes on the current recursion; a cycle renders as `…`.
    path: AncestorNodes<NodeId>,
    /// The extension's own value variants — how a variant the base
    /// vocabulary does not know renders.  `None` (the base vocabulary) or a
    /// hook returning `None` for a value leaves it `?`.
    render_ext: Option<RenderExt<'a, P::Value>>,
    /// Render a struct type's nominal id as `struct<…>#n` — on for the
    /// diagnostic printer (two structs with the same field shape are
    /// distinguishable), off for the value/type output path (a single value's
    /// type needs no id noise).
    show_struct_id: bool,
}

/// The pretty value printer: renders a runtime value against its type chain,
/// so a value reads like the code that produced it.  Generic over the value
/// vocabulary, like [`TypePrinter`]; an extension's own variants render
/// through the same render hook.
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
}

/// The class representative of `node`, via a read-only `parent` walk (the
/// printers never mutate the module).  `None` when the walk cannot answer: a
/// node the module's table does not hold, or a `parent` chain longer than the
/// table (a revisit — corrupt equality state).  Either way the caller renders
/// its own "no answer" instead of panicking or looping.
fn representative<P: HighProgram>(module: &Module<P>, node: NodeId) -> Option<NodeId>
where
    P::Value: ValueType,
{
    let mut n = node;
    // A parent chain visits each node at most once, so it cannot be longer
    // than the node table.
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

/// The source spelling of a float value — the ONE place a float becomes text
/// (`docs/notes/floating-point.md` §3.5).  Both printers reach it: a float is a
/// structural value wherever it appears, so a type expression prints its digits
/// exactly as a value does.
///
/// # The invariant: the spelling reads back as the same `f32`
///
/// The digits are `f32`'s own `Display`: the shortest decimal that parses back
/// bit-identical, and positional — `{:?}` switches to an exponent (`1e38`) that
/// the lexer's `[0-9]+\.[0-9]+` float literal has no syntax for.  A `.` is
/// forced, because the same digits without one (`1`) read back as an `Int`: a
/// different `LowValue`, and so a different type.
///
/// An infinity is spelled as a magnitude past the round-to-infinity threshold
/// `(2 - 2^-24) * 2^127` — the only form the lexer reads back as an infinity,
/// since `inf` is a name rather than a literal.  `NaN` has no spelling in the
/// literal syntax at all and keeps Rust's own, so a reader refuses it instead
/// of acquiring a different float.
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

/// Whether `node` is itself a struct kind `[id, [TypeStruct, K]]` (as opposed
/// to a struct type term `[shape, kind]`, whose kind slot is such a node).
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

/// Whether `node`'s class is the canonical universe `K = [Type, ↺]` — the
/// 2-element array whose **head is the `Type` marker** and whose tail is a
/// member of its own unification class.  The tail test is a class comparison,
/// so it covers both the canonical node itself and a cell that carries the
/// replicated value; a class the walk cannot place is not the universe.
///
/// The head is checked first and the length pinned: a **kind** `[marker, K]`
/// has the same self-referential silhouette whenever its `K` is the frozen
/// (static) universe — a type value computed inside a frozen module and read
/// in the importing one — so a tail-only test reads every such kind as the
/// universe (see `docs/notes/universe-containment.md`).
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

/// The read-only, static-aware universe test shared by the free printer
/// helpers: a static universe is the self-referential `[Type, itself]`.
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

/// Whether a value is a struct marker — the two-field `TypeStruct{id, names}`
/// value, encoded as a 2-element array `[id, names]`.  No other kind's marker
/// is an array, so a 2-element array marker names a struct.
fn marker_is_struct<P: HighProgram>(module: &Module<P>, marker: AnyNodeId) -> bool
where
    P::Value: ValueType,
{
    module
        .node_value(marker)
        .and_then(|v| v.as_enum())
        .is_some_and(|v| match v {
            // SAFETY: `m` is the payload of the value read from the live node
            // `marker`.
            LowValue::Array(m) => unsafe { m.items() }.len() == 2,
            _ => false,
        })
}

/// Whether `kind_items` (the element items of a kind value) describe a struct
/// kind: `[TypeStruct{id, names}, K]`.  The kind is a standard `[marker, K]`
/// pair whose marker is the two-field `TypeStruct` value.
fn kind_is_struct<P: HighProgram>(module: &Module<P>, kind_items: &[ArrayItem]) -> bool
where
    P::Value: ValueType,
{
    kind_items.len() == 2
        && is_universe_any(module, kind_items[1].node)
        && marker_is_struct(module, kind_items[0].node)
}

/// The per-field names of a struct type, read from its marker `[id, names]`
/// (the marker sits at the kind's slot 0): `None` for an unnamed (positional)
/// field, `Some(name)` for a `.name Ty` field.  Sized to `field_count`; a
/// name whose index maps outside the field list is dropped (defensive).
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
    let Some(names_item) = marker_items.get(1) else {
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

/// The named-field list of a struct **type term** (`[shape, kind]`), read from
/// the type's kind marker `[id, names]`.  `None` when `node` is not a concrete
/// struct type (an unbound cell, a tuple, an array, a function).  A `None`
/// entry is a positional (unnamed) field; a `Some(name)` entry is a
/// `.name Ty` field.
///
/// This is the read-only counterpart to the checker's `struct_names_any`, for a
/// renderer that only has the module (e.g. the did-you-mean clause on a
/// [`DiagKind::NamedField`] diagnostic, which needs the struct's field names).
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
    // SAFETY: `ty_arr`/`shape`/`kind` are payloads of values read from live
    // nodes of `module`; the note covers this function's `items()` calls.
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

/// The nominal id of a struct type, read from its kind's marker `[id, names]`
/// (the marker sits at the kind's slot 0, the id at the marker's slot 0).
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
    let id_item = marker_items.first()?;
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

/// Render a struct-instance value's **named fields** (`name = value, …`) —
/// the doc label's spelling — reading the *names* from the struct type's name
/// table and each value against its field type.  `None` when the value/type
/// does not form a struct instance.
///
/// `value_node` is the struct value (a field-tuple array) and `ty_node` its
/// struct type (a `[shape, [TypeStruct{id, names}, K]]` pair).  The renderer
/// walks the *type chain*, so the names come from the type, never a hardcoded
/// shape.
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
    // SAFETY: every slice below is the payload of a value read from a live
    // node of `module`; the note covers this function's `items()` calls.
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
