//! The program-generic pretty printer core: the type printer, the value
//! printer, and the attribute-list / struct-field renderers.  All generic over
//! `P: HighProgram`, so a host and a plugin that spells its own attribute slot
//! (e.g. `lichen-doc`'s `? name = "…"`) reuse the same machinery.
//!
//! Everything here reads the top of the recursive-pair encoding and spells it
//! in a concrete language's syntax.  Nothing names a concrete host program —
//! a host (e.g. `lichen-language`) composes `ValuePrinter`/`TypePrinter` with
//! its own value vocabulary and the free printers below.

use std::collections::{HashMap, HashSet};

use lichen_highlevel::attr::AttrExt;
use lichen_highlevel::program::{HighProgram, ValueType};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module, NodeId};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;
// A rendering hook for extension vocabularies: an extension value → its
// spelling, or `None` when the value is not the extension’s to name.  The
// alias exists because the bare closure type trips clippy’s `type_complexity`.

type RenderExt<'a, V> = &'a dyn Fn(&V) -> Option<String>;

/// Render a runtime value as the program's output, *read against its type*.
///
/// The type chain decides how the value reads: a value whose type is the
/// universe is an atomic type constant (`Int` / `Type`), a value whose type
/// is a kind is a compound type (`struct<Int, Type>`, `Int -> Int`,
/// `<Int, Type>`, `Int<3>`), a value whose type is a tuple type reads as a
/// tuple `(1, Int)`, an array type as an array `[1, 2, 3]`, and a struct
/// type as its field tuple.  When the type chain is opaque (an unbound cell,
/// an extension type), the value falls back to its raw layout — the old
/// `[head, K]` reading of a recursive pair.
pub fn print_value<P: HighProgram>(module: &Module<P>, value: P::Value, ty: NodeId) -> String
where
    P::Value: ValueType,
{
    ValuePrinter::new(module).print(value, ty)
}

/// Render a type expression (the recursive-pair encoding again) in the
/// language's own type syntax: `Int`, `Type`, `T1 -> T2`, `<T1, ..., Tn>`,
/// `T<len>`, `struct<T1, ...>`.  Unbound cells get stable `?a`, `?b`, …
/// names — cells in the same unification class share a name — so the type
/// shows which parts are linked.  Cycles are cut at `…`.
pub fn print_type<P: HighProgram>(module: &Module<P>, root: NodeId) -> String
where
    P::Value: ValueType,
{
    TypePrinter::new(module).node(root)
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
            LowValue::Array(a) => Some(a.items().to_vec()),
            _ => None,
        });
    let Some(values) = values else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (i, marker) in tail.iter().enumerate() {
        let slot = values.get(2 + i).and_then(|item| match item.node {
            AnyNodeId::Dynamic(n) => Some(n),
            AnyNodeId::Static(_) => None,
        });
        if let Some(slot) = slot
            && let Some(spelling) = attr_ext(marker).render(module, slot)
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
    /// The CLI output path has no registry and leaves such shapes raw.
    arrows: Option<&'a HashSet<NodeId>>,
    /// Stable class names: representative → `?a`, `?b`, …, within one type
    /// (or one diagnostic report).
    names: HashMap<NodeId, String>,
    next: usize,
    /// Array nodes on the current recursion; a cycle renders as `…`.
    path: Vec<NodeId>,
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

impl<'a, P: HighProgram> TypePrinter<'a, P>
where
    P::Value: ValueType,
{
    pub fn new(module: &'a Module<P>) -> Self {
        Self::new_with(module, None, None)
    }

    /// A printer that also knows the checker's arrow shapes, so bare
    /// `[in, out]` shapes render as `in -> out`.
    pub fn new_with_arrows(module: &'a Module<P>, arrows: Option<&'a HashSet<NodeId>>) -> Self {
        Self::new_with(module, arrows, None)
    }

    /// A printer that renders an extension vocabulary's own variants through
    /// `render_ext` — a hook the base renderer cannot know, returning the
    /// variant's spelling (or `None` for a value it does not recognize).
    pub fn new_with_ext(
        module: &'a Module<P>,
        render_ext: Option<RenderExt<'a, P::Value>>,
    ) -> Self {
        Self::new_with(module, None, render_ext)
    }

    fn new_with(
        module: &'a Module<P>,
        arrows: Option<&'a HashSet<NodeId>>,
        render_ext: Option<RenderExt<'a, P::Value>>,
    ) -> Self {
        TypePrinter {
            module,
            arrows,
            names: HashMap::new(),
            next: 0,
            path: Vec::new(),
            render_ext,
            show_struct_id: false,
        }
    }

    /// Turn the nominal-id suffix on — the diagnostic printer needs it so two
    /// structs with the same field shape stay distinguishable.
    pub fn show_struct_ids(&mut self) {
        self.show_struct_id = true;
    }

    /// Render a type node; an unbound cell renders as its class name.
    pub fn node(&mut self, node: NodeId) -> String {
        if self.path.contains(&node) {
            return "…".to_string();
        }
        let value = self
            .module
            .node_value(AnyNodeId::Dynamic(node))
            .unwrap_or_else(|| P::Value::from(LowValue::None));
        if matches!(
            value.as_enum(),
            Some(LowValue::None | LowValue::Parameterized)
        ) {
            return self.class_name(node);
        }
        self.path.push(node);
        let out = self.value(node, value);
        self.path.pop();
        out
    }

    /// The stable name of an unbound cell's class: `?a`, `?b`, … — cells in
    /// the same class share a name.
    pub fn class_name(&mut self, node: NodeId) -> String {
        let rep = representative(self.module, node);
        if let Some(name) = self.names.get(&rep) {
            return name.clone();
        }
        let name = letter_name(self.next);
        self.next += 1;
        self.names.insert(rep, name.clone());
        name
    }

    /// Render a type value, descending into arrays.
    fn value(&mut self, node: NodeId, value: P::Value) -> String {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => n.to_string(),
                LowValue::Str(s) => format!("\"{s}\""),
                LowValue::Array(array) => self.elements(node, array.items()),
                LowValue::Table(_) => "Table".to_string(),
                LowValue::Function(_) => "Function".to_string(),
                LowValue::None | LowValue::Parameterized => {
                    unreachable!("handled by node()")
                }
            };
        }
        self.type_constant(&value)
            .unwrap_or_else(|| "?".to_string())
    }

    /// The spelling of a type constant — or of an extension's own variant:
    /// the lowlevel structural values return `None`, they are rendered by
    /// [`Self::value`]'s structural branch.
    pub(crate) fn type_constant(&self, value: &P::Value) -> Option<String> {
        if value == &P::Value::int_marker() {
            Some("Int".to_string())
        } else if value == &P::Value::string_marker() {
            Some("string".to_string())
        } else if value == &P::Value::type_marker() {
            Some("Type".to_string())
        } else if value == &P::Value::function_type_marker() {
            Some("TypeFunction".to_string())
        } else if value == &P::Value::tuple_type_marker() {
            Some("TypeTuple".to_string())
        } else if value == &P::Value::array_type_marker() {
            Some("TypeArray".to_string())
        } else if value == &P::Value::type_struct_marker() {
            Some("TypeStruct".to_string())
        } else if let Some(n) = value.type_id() {
            Some(format!("TypeId({n})"))
        } else if let Some(render_ext) = self.render_ext {
            render_ext(value)
        } else {
            None
        }
    }

    fn elements(&mut self, node: NodeId, elements: &[ArrayItem]) -> String {
        // A bare struct kind `[TypeStruct{id, names}, K]` (a struct type
        // pair's type slot): render its tag `TypeStruct`.  Detected before the
        // `[head, K]` atomic branch, since its marker is a 2-element array
        // (not a plain type constant).
        if is_struct_kind(self.module, node) {
            return "TypeStruct".to_string();
        }
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && self.is_universe_any(elements[1].node) {
            return self.any_node(elements[0].node);
        }
        // A struct type: `[shape, [TypeStruct{id, names}, K]]` — the kind is a
        // standard `[marker, K]` pair whose marker is the two-field struct
        // value `[id, names]`.  The id renders as `#n` so two structs with the
        // same field shape stay distinguishable (their nominal types differ).
        if elements.len() == 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && self.kind_is_struct_any(kind.items())
        {
            let fields = self.fields_any(elements[0].node);
            let names = struct_field_names(self.module, kind.items(), fields.len());
            let fields = struct_fields_with_names(&fields, &names);
            let id = struct_kind_id(self.module, kind.items());
            return match (self.show_struct_id, id) {
                (true, Some(n)) => format!("struct<{}>#{n}", fields.join(", ")),
                _ => format!("struct<{}>", fields.join(", ")),
            };
        }
        // `[shape, [marker, K]]` — a compound type: the kind's marker decides
        // how the shape reads.
        if elements.len() == 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = kind.items()
            && kind.len() == 2
            && self.is_universe_any(kind[1].node)
        {
            match self.module.node_value(kind[0].node) {
                Some(m) if m == P::Value::function_type_marker() || m.is_function_kind() => {
                    // shape = [in, out] — render `in -> out`.  An extension
                    // marker that mirrors a function kind (e.g. the compute
                    // plugin's `TypeKernel`) reads the same way.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        && let s = shape.items()
                        && s.len() == 2
                    {
                        return format!(
                            "{} -> {}",
                            self.any_node(s[0].node),
                            self.any_node(s[1].node)
                        );
                    }
                }
                Some(m) if m == P::Value::tuple_type_marker() => {
                    // shape = the field-type list — render `<T1, ..., Tn>`.
                    let fields = self.fields_any(elements[0].node);
                    return format!("<{}>", fields.join(", "));
                }
                Some(m) if m == P::Value::array_type_marker() => {
                    // shape = [element type, length] — render `T<len>`.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        && let s = shape.items()
                        && s.len() == 2
                    {
                        return format!(
                            "{}<{}>",
                            self.any_node(s[0].node),
                            self.any_node(s[1].node)
                        );
                    }
                }
                _ => {}
            }
        }
        // A bare `[in, out]` shape with no kind wrapper: an arrow only when
        // the checker registered the shape (the diagnostic path); otherwise
        // it falls through to the raw pair.
        if elements.len() == 2 && self.is_arrow(node) {
            return format!(
                "{} -> {}",
                self.any_node(elements[0].node),
                self.any_node(elements[1].node)
            );
        }
        // Fallback: render the raw elements.
        let parts: Vec<String> = elements
            .iter()
            .map(|item| self.any_node(item.node))
            .collect();
        format!("[{}]", parts.join(", "))
    }

    /// Is `node`'s class a checker-registered arrow shape?
    fn is_arrow(&self, node: NodeId) -> bool {
        self.arrows.is_some_and(|arrows| {
            disjoint::members(&self.module.nodes, representative(self.module, node))
                .any(|m| arrows.contains(&m))
        })
    }

    /// Read-only variant of [`Self::fields`] for a static or dynamic shape.
    fn fields_any(&mut self, shape: AnyNodeId) -> Vec<String> {
        if let Some(LowValue::Array(array)) =
            self.module.node_value(shape).and_then(|v| v.as_enum())
        {
            array
                .items()
                .iter()
                .map(|item| self.any_node(item.node))
                .collect()
        } else {
            vec![self.any_node(shape)]
        }
    }

    /// Render any node — dynamic or static — as a type.
    pub fn any_node(&mut self, id: AnyNodeId) -> String {
        match id {
            AnyNodeId::Dynamic(node) => self.node(node),
            AnyNodeId::Static(sref) => self.static_node(sref),
        }
    }

    fn static_node(&mut self, sref: lichen_lowlevel::StaticNodeId) -> String {
        let mut visiting = HashSet::new();
        self.static_inner(sref, &mut visiting)
    }

    fn static_inner(
        &mut self,
        sref: lichen_lowlevel::StaticNodeId,
        visiting: &mut HashSet<lichen_lowlevel::StaticNodeId>,
    ) -> String {
        if !visiting.insert(sref) {
            return "…".to_string();
        }
        let value = self.module.node_value(AnyNodeId::Static(sref));
        if value
            .is_none_or(|v| matches!(v.as_enum(), Some(LowValue::None | LowValue::Parameterized)))
        {
            visiting.remove(&sref);
            return "?".to_string();
        }
        let value = value.unwrap();
        let out = match value.as_enum() {
            Some(LowValue::USize(n)) => n.to_string(),
            Some(LowValue::Str(s)) => format!("\"{s}\""),
            Some(LowValue::None | LowValue::Parameterized) => "?".to_string(),
            Some(LowValue::Function(_)) => "Function".to_string(),
            Some(LowValue::Table(_)) => "Table".to_string(),
            Some(LowValue::Array(array)) => self.static_elements(sref, array.items(), visiting),
            None => self
                .type_constant(&value)
                .unwrap_or_else(|| "?".to_string()),
        };
        visiting.remove(&sref);
        out
    }

    fn static_elements(
        &mut self,
        _sref: lichen_lowlevel::StaticNodeId,
        elements: &[ArrayItem],
        visiting: &mut HashSet<lichen_lowlevel::StaticNodeId>,
    ) -> String {
        // A bare struct kind `[TypeStruct{id, names}, K]`: render its tag.
        if kind_is_struct(self.module, elements) {
            return "TypeStruct".to_string();
        }
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && self.is_static_universe(elements[1].node) {
            return self.static_any(elements[0].node, visiting);
        }
        // A struct type: `[shape, [TypeStruct{id, names}, K]]` — the kind is a
        // standard `[marker, K]` pair whose marker is the two-field struct
        // value `[id, names]`; a name table rides at the marker's slot 1.
        if elements.len() == 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && self.kind_is_struct_any(kind.items())
        {
            let fields = self.static_fields(elements[0].node, visiting);
            let names = struct_field_names(self.module, kind.items(), fields.len());
            let fields = struct_fields_with_names(&fields, &names);
            let id = struct_kind_id(self.module, kind.items());
            return match (self.show_struct_id, id) {
                (true, Some(n)) => format!("struct<{}>#{n}", fields.join(", ")),
                _ => format!("struct<{}>", fields.join(", ")),
            };
        }
        // `[shape, [marker, K]]` — a compound type: the kind's marker decides
        // how the shape reads.
        if elements.len() == 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = kind.items()
            && kind.len() == 2
            && self.is_static_universe(kind[1].node)
        {
            match self.module.node_value(kind[0].node) {
                Some(m) if m == P::Value::function_type_marker() || m.is_function_kind() => {
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        && let s = shape.items()
                        && s.len() == 2
                    {
                        return format!(
                            "{} -> {}",
                            self.static_any(s[0].node, visiting),
                            self.static_any(s[1].node, visiting)
                        );
                    }
                }
                Some(m) if m == P::Value::tuple_type_marker() => {
                    let fields = self.static_fields(elements[0].node, visiting);
                    return format!("<{}>", fields.join(", "));
                }
                Some(m) if m == P::Value::array_type_marker() => {
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        && let s = shape.items()
                        && s.len() == 2
                    {
                        return format!(
                            "{}<{}>",
                            self.static_any(s[0].node, visiting),
                            self.static_any(s[1].node, visiting)
                        );
                    }
                }
                _ => {}
            }
        }
        // Fallback: render the raw static elements.
        let parts: Vec<String> = elements
            .iter()
            .map(|item| self.static_any(item.node, visiting))
            .collect();
        format!("[{}]", parts.join(", "))
    }

    fn static_any(
        &mut self,
        id: AnyNodeId,
        visiting: &mut HashSet<lichen_lowlevel::StaticNodeId>,
    ) -> String {
        match id {
            AnyNodeId::Dynamic(node) => self.node(node),
            AnyNodeId::Static(sref) => self.static_inner(sref, visiting),
        }
    }

    fn static_fields(
        &mut self,
        shape: AnyNodeId,
        visiting: &mut HashSet<lichen_lowlevel::StaticNodeId>,
    ) -> Vec<String> {
        if let Some(LowValue::Array(array)) =
            self.module.node_value(shape).and_then(|v| v.as_enum())
        {
            array
                .items()
                .iter()
                .map(|item| self.static_any(item.node, visiting))
                .collect()
        } else {
            vec![self.static_any(shape, visiting)]
        }
    }

    fn is_static_universe(&self, id: AnyNodeId) -> bool {
        let AnyNodeId::Static(sref) = id else {
            return false;
        };
        if let Some(value) = self.module.node_value(id)
            && let Some(LowValue::Array(array)) = value.as_enum()
        {
            let items = array.items();
            return items.len() == 2
                && self.module.node_value(items[0].node) == Some(P::Value::type_marker())
                && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index);
        }
        false
    }

    fn is_universe_any(&self, id: AnyNodeId) -> bool {
        match id {
            AnyNodeId::Dynamic(node) => is_universe(self.module, node),
            AnyNodeId::Static(_) => self.is_static_universe(id),
        }
    }

    fn kind_is_struct_any(&self, kind_items: &[ArrayItem]) -> bool {
        kind_items.len() == 2
            && self.is_universe_any(kind_items[1].node)
            && marker_is_struct(self.module, kind_items[0].node)
    }
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
    path: Vec<NodeId>,
    /// Type nodes on the current recursion; a cycle renders as `…`.
    tpath: Vec<NodeId>,
}

impl<'a, P: HighProgram> ValuePrinter<'a, P>
where
    P::Value: ValueType,
{
    pub fn new(module: &'a Module<P>) -> Self {
        Self::new_with_ext(module, None)
    }

    /// A printer that renders an extension vocabulary's own variants through
    /// `render_ext` — see [`TypePrinter::new_with_ext`].
    pub fn new_with_ext(
        module: &'a Module<P>,
        render_ext: Option<RenderExt<'a, P::Value>>,
    ) -> Self {
        ValuePrinter {
            module,
            printer: TypePrinter::new_with_ext(module, render_ext),
            path: Vec::new(),
            tpath: Vec::new(),
        }
    }

    /// Render the runtime value `value`, whose type is `ty`.
    pub fn print(&mut self, value: P::Value, ty: NodeId) -> String {
        self.value(value, ty)
    }

    /// Render a value against its type: the type chain decides how the value
    /// reads.  When the type chain is opaque, fall back to the raw layout.
    fn value(&mut self, value: P::Value, ty: NodeId) -> String {
        // The universe as the type: the value is an atomic type constant —
        // `Int`, `Type`, or an extension's own type constant.
        if self.printer.is_universe_any(AnyNodeId::Dynamic(ty)) {
            return self.atomic(value);
        }
        let Some(LowValue::Array(ty_array)) = self
            .module
            .node_value(AnyNodeId::Dynamic(ty))
            .and_then(|v| v.as_enum())
        else {
            return self.raw(value);
        };
        let tys = ty_array.items();
        // A struct type itself: the value's type is the struct kind
        // `[id, [TypeStruct, K], names]` (not a `[shape, [marker, K]]` pair),
        // and the value is the field-type list — render
        // `struct<T1, ..., Tn>` (or `struct<a :: T1, ...>` when named).
        if is_struct_kind(self.module, ty)
            && let Some(LowValue::Array(shape)) = value.as_enum()
        {
            let fields: Vec<String> = shape
                .items()
                .iter()
                .map(|item| self.printer.any_node(item.node))
                .collect();
            let names = struct_field_names(self.module, tys, fields.len());
            let fields = struct_fields_with_names(&fields, &names);
            return format!("struct<{}>", fields.join(", "));
        }
        // A kind `[marker, K]`: the value is a compound type — render its
        // shape in type syntax.
        if tys.len() == 2
            && self.printer.is_universe_any(tys[1].node)
            && let Some(out) = self.compound_type(value, tys[0].node)
        {
            return out;
        }
        // A struct instance: the value reads against the struct's
        // field-type list (the shape); its kind is `[TypeStruct{id, names}, K]`
        // (a standard `[marker, K]` pair), so it is detected beside the
        // `[marker, K]` kinds.
        if tys.len() == 2
            && self.is_struct_kind_any(tys[1].node)
            && let Some(marker) = self.struct_marker_value(tys[1].node)
            && let Some(out) = self.instance(value, tys[0].node, marker)
        {
            return out;
        }
        // A term of a tuple/array/struct type `[shape, [marker, K]]`: the
        // value's elements read against the shape.
        if tys.len() == 2
            && let Some(kind) = self.module.node_value(tys[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = kind.items()
            && kind.len() == 2
            && self.printer.is_universe_any(kind[1].node)
            && let Some(marker) = self.module.node_value(kind[0].node)
            && let Some(out) = self.instance(value, tys[0].node, marker)
        {
            return out;
        }
        self.raw(value)
    }

    /// An atomic type constant: the value of a type expression whose type is
    /// the universe.  A structural value typed by the universe (the universe
    /// node itself) falls back to the raw layout.
    fn atomic(&mut self, value: P::Value) -> String {
        if let Some(spelling) = self.printer.type_constant(&value) {
            spelling
        } else {
            self.raw(value)
        }
    }

    /// A compound type value: the value is the shape `[in, out]` /
    /// element list / `[element, length]` / `[TypeId, fields]`, and the
    /// marker decides how the shape reads.  `None` when the value or the
    /// marker does not fit a compound type.
    fn compound_type(&mut self, value: P::Value, marker_node: AnyNodeId) -> Option<String> {
        let marker = self.module.node_value(marker_node)?;
        let Some(LowValue::Array(shape)) = value.as_enum() else {
            return None;
        };
        let shape = shape.items();
        if marker == P::Value::function_type_marker() || marker.is_function_kind() {
            if shape.len() == 2 {
                return Some(format!(
                    "{} -> {}",
                    self.printer.any_node(shape[0].node),
                    self.printer.any_node(shape[1].node)
                ));
            }
        } else if marker == P::Value::tuple_type_marker() {
            let fields: Vec<String> = shape
                .iter()
                .map(|item| self.printer.any_node(item.node))
                .collect();
            return Some(format!("<{}>", fields.join(", ")));
        } else if marker == P::Value::array_type_marker() && shape.len() == 2 {
            return Some(format!(
                "{}<{}>",
                self.printer.any_node(shape[0].node),
                self.printer.any_node(shape[1].node)
            ));
        }
        // A struct type never reaches `compound_type` — its kind is a standard
        // `[marker, K]` pair whose marker is the two-field `TypeStruct`
        // value, and the struct branch in `value` / `elements` handles it
        // before this falls through.
        None
    }

    /// A term of a tuple, array, or struct type: the value's elements read
    /// against the shape — a tuple reads `(v1, ..., vn)` (a single element
    /// `(v1,)`), an array `[v1, ..., vn]`, a struct instance its field tuple
    /// `(v1, ..., vn)`.  `None` when the value or the shape does not fit.
    fn instance(
        &mut self,
        value: P::Value,
        shape_node: AnyNodeId,
        marker: P::Value,
    ) -> Option<String> {
        let Some(LowValue::Array(values)) = value.as_enum() else {
            return None;
        };
        let values = values.items();
        let shape = self.module.node_value(shape_node).and_then(|v| v.as_enum());
        let Some(LowValue::Array(shape)) = shape else {
            return None;
        };
        let shape = shape.items();
        if marker == P::Value::tuple_type_marker() {
            if shape.len() != values.len() {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for (i, v) in values.iter().enumerate() {
                out.push(self.element_any(v.node, shape[i].node));
            }
            return Some(self.parens(&out));
        }
        if marker == P::Value::array_type_marker() {
            if shape.len() != 2 {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                out.push(self.element_any(v.node, shape[0].node));
            }
            return Some(format!("[{}]", out.join(", ")));
        }
        // A struct marker is the two-field `TypeStruct{id, names}` value, a
        // 2-element array.  No other kind's marker is an array, so an array
        // marker names a struct.
        if marker
            .as_enum()
            .is_some_and(|m| matches!(m, LowValue::Array(_)))
        {
            // The shape is the positional field-type list (the nominal id
            // lives in the struct marker), so the element types are the fields.
            let fields = shape;
            if fields.len() != values.len() {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for (i, v) in values.iter().enumerate() {
                out.push(self.element_any(v.node, fields[i].node));
            }
            return Some(self.parens(&out));
        }
        None
    }

    /// `(v1, ..., vn)` — a single element keeps its trailing comma, the
    /// source spelling of a one-tuple.
    fn parens(&self, elements: &[String]) -> String {
        let body = elements.join(", ");
        let body = if elements.len() == 1 {
            format!("{body},")
        } else {
            body
        };
        format!("({body})")
    }

    /// [`Self::element`] for static or dynamic value/type refs.
    fn element_any(&mut self, id: AnyNodeId, ty: AnyNodeId) -> String {
        match (id, ty) {
            (AnyNodeId::Dynamic(id), AnyNodeId::Dynamic(ty)) => {
                if self.path.contains(&id) || self.tpath.contains(&ty) {
                    return "…".to_string();
                }
                self.path.push(id);
                self.tpath.push(ty);
                let value = self
                    .module
                    .node_value(AnyNodeId::Dynamic(id))
                    .unwrap_or_else(|| P::Value::from(LowValue::None));
                let out = self.value(value, ty);
                self.tpath.pop();
                self.path.pop();
                out
            }
            _ => {
                let value = self
                    .module
                    .node_value(id)
                    .unwrap_or_else(|| P::Value::from(LowValue::None));
                self.raw_any(value)
            }
        }
    }

    /// Whether an `AnyNodeId` names a struct kind `[id, [TypeStruct, K]]`.
    fn is_struct_kind_any(&self, id: AnyNodeId) -> bool {
        self.module
            .node_value(id)
            .and_then(|v| v.as_enum())
            .is_some_and(|v| match v {
                LowValue::Array(kind) => kind_is_struct(self.module, kind.items()),
                _ => false,
            })
    }

    /// The struct marker value (`TypeStruct{id, names}` = `[id, names]`) from
    /// a struct type's kind node (`[marker, K]`), or `None` when the kind is
    /// not a struct kind.  Used to render a struct instance whose value reads
    /// against the field-type shape.
    fn struct_marker_value(&self, kind_node: AnyNodeId) -> Option<P::Value> {
        let Some(LowValue::Array(kind)) =
            self.module.node_value(kind_node).and_then(|v| v.as_enum())
        else {
            return None;
        };
        let marker = self.module.node_value(kind.items()[0].node)?;
        if marker
            .as_enum()
            .is_some_and(|m| matches!(m, LowValue::Array(_)))
        {
            Some(marker)
        } else {
            None
        }
    }

    /// The raw value layout — the fallback when the type chain cannot guide
    /// the reading: a type pair `[head, [Type, ↺]]` renders as its head
    /// (`[TypeInt, K]` → `Int`), arrays `[ ]`, functions `Function`, and the
    /// type constants by their spellings `Int` / `Type`.
    fn raw(&mut self, value: P::Value) -> String {
        self.raw_any(value)
    }

    /// [`Self::raw`] for a value whose array items may be static refs.
    fn raw_any(&mut self, value: P::Value) -> String {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => n.to_string(),
                LowValue::Str(s) => format!("\"{s}\""),
                LowValue::Function(_) => "Function".to_string(),
                LowValue::Table(_) => "Table".to_string(),
                LowValue::None => "none".to_string(),
                LowValue::Parameterized => "parameterized".to_string(),
                LowValue::Array(array) => {
                    let elements = array.items();
                    // A type pair `[head, K]`: the kind slot is the
                    // self-looping universe, so render just the head (and cut
                    // the cycle).
                    if elements.len() == 2 && self.printer.is_universe_any(elements[1].node) {
                        let head = self
                            .module
                            .node_value(elements[0].node)
                            .unwrap_or_else(|| P::Value::from(LowValue::None));
                        return self.raw_any(head);
                    }
                    let mut out = Vec::new();
                    for item in elements {
                        let value = self
                            .module
                            .node_value(item.node)
                            .unwrap_or_else(|| P::Value::from(LowValue::None));
                        let text = self.raw_any(value);
                        out.push(text);
                    }
                    format!("[{}]", out.join(", "))
                }
            };
        }
        self.printer
            .type_constant(&value)
            .unwrap_or_else(|| "?".to_string())
    }
}

/// The class representative of `node`, via a read-only `parent` walk (the
/// printers never mutate the module).
fn representative<P: HighProgram>(module: &Module<P>, node: NodeId) -> NodeId
where
    P::Value: ValueType,
{
    let mut n = node;
    while let Some(parent) = module.nodes[n].equality.parent {
        n = parent;
    }
    n
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

/// The canonical universe `K = [Type, ↺]` — a node whose value is an array
/// that contains a member of its own unification class.  A plain
/// self-referential member (`contains(&node)`) is the canonical node itself;
/// a cell unified into the universe class carries the replicated value, whose
/// self-referential member is the canonical node — the class check covers
/// both.
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
            LowValue::Array(kind) => kind_is_struct(module, kind.items()),
            _ => false,
        })
}

fn is_universe<P: HighProgram>(module: &Module<P>, node: NodeId) -> bool
where
    P::Value: ValueType,
{
    let rep = representative(module, node);
    matches!(module.node_value(AnyNodeId::Dynamic(node)), Some(value)
    if matches!(value.as_enum(), Some(LowValue::Array(array))
        if array.items().iter().any(|item| match item.node {
            AnyNodeId::Dynamic(item) => representative(module, item) == rep,
            AnyNodeId::Static(_) => is_universe_any(module, item.node),
        })))
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
                    let items = array.items();
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
            LowValue::Array(m) => m.items().len() == 2,
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
/// field, `Some(name)` for a `name :: Ty` field.  Sized to `field_count`; a
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
            LowValue::Array(m) => Some(m.items()),
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
    for item in table.items() {
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
        if let (Some(name), Some(index)) = (name, index) {
            if index < field_count {
                out[index] = Some(name);
            }
        }
    }
    out
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
            LowValue::Array(m) => Some(m.items()),
            _ => None,
        })?;
    let id_item = marker_items.get(0)?;
    module.node_value(id_item.node).and_then(|v| v.type_id())
}

/// Render a struct field list with per-field names (`name :: T` for a named
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
    let field_values = value_arr.items();

    // The struct type: `[shape, kind]` where kind is `[marker, K]`.
    let ty = module.node_value(ty_node)?.as_enum()?;
    let LowValue::Array(ty_arr) = ty else {
        return None;
    };
    let tys = ty_arr.items();
    if tys.len() != 2 {
        return None;
    }
    let kind = module.node_value(tys[1].node)?.as_enum()?;
    let LowValue::Array(kind_items) = kind else {
        return None;
    };
    if !kind_is_struct(module, kind_items.items()) {
        return None;
    }
    // The shape is the positional field-type list.
    let shape = module.node_value(tys[0].node)?.as_enum()?;
    let LowValue::Array(shape_items) = shape else {
        return None;
    };
    let field_types = shape_items.items();
    let names = struct_field_names(module, kind_items.items(), field_values.len());

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
