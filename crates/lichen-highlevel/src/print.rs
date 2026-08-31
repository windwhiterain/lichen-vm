//! The shared pretty printer for highlevel types.
//!
//! This module owns the single `TypePrinter` used by:
//! - the language crate's value/output rendering, and
//! - the highlevel diagnostic messages.
//!
//! Previously the highlevel crate had its own raw type printer and the
//! language crate had a second pretty printer.  Consolidating here keeps the
//! two consumers on one implementation so type syntax, cycle-cutting, and
//! class-name stability cannot drift apart.

use std::collections::{HashMap, HashSet};

use crate::program::{HighProgram, HighProgramValue, ValueType};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module, NodeId};
use lichen_utils::disjoint;

/// A rendering hook for extension vocabularies: an extension value → its
/// spelling, or `None` when the value is not the extension's to name.
pub type RenderExt<'a, V> = &'a dyn Fn(&V) -> Option<String>;

/// The shared pretty type printer: stateful across calls, so one instance
/// renders a whole diagnostic (or report) with consistent `?a`/`?b` class
/// names.  Generic over the value vocabulary, and extension values can be
/// rendered through the [`RenderExt`] hook.
pub struct TypePrinter<'a, V: ValueType = HighProgramValue> {
    module: &'a Module<HighProgram<V>>,
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
    render_ext: Option<RenderExt<'a, V>>,
    /// Render a struct type's nominal id as `struct<…>#n` — on for the
    /// diagnostic printer (two structs with the same field shape are
    /// distinguishable), off for the value/type output path (a single value's
    /// type needs no id noise).
    show_struct_id: bool,
}

impl<'a, V: ValueType> TypePrinter<'a, V> {
    pub fn new(module: &'a Module<HighProgram<V>>) -> Self {
        Self::new_with(module, None, None)
    }

    /// A printer that also knows the checker's arrow shapes, so bare
    /// `[in, out]` shapes render as `in -> out`.
    pub fn new_with_arrows(
        module: &'a Module<HighProgram<V>>,
        arrows: Option<&'a HashSet<NodeId>>,
    ) -> Self {
        Self::new_with(module, arrows, None)
    }

    /// A printer that renders an extension vocabulary's own variants through
    /// `render_ext` — a hook the base renderer cannot know, returning the
    /// variant's spelling (or `None` for a value it does not recognize).
    pub fn new_with_ext(
        module: &'a Module<HighProgram<V>>,
        render_ext: Option<RenderExt<'a, V>>,
    ) -> Self {
        Self::new_with(module, None, render_ext)
    }

    fn new_with(
        module: &'a Module<HighProgram<V>>,
        arrows: Option<&'a HashSet<NodeId>>,
        render_ext: Option<RenderExt<'a, V>>,
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
        let value = self.module.nodes[node]
            .value
            .unwrap_or_else(|| V::from(LowValue::None));
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
    fn value(&mut self, node: NodeId, value: V) -> String {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => n.to_string(),
                LowValue::Array(array) => self.elements(node, array.items()),
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
    pub fn type_constant(&self, value: &V) -> Option<String> {
        if value == &V::int_marker() {
            Some("Int".to_string())
        } else if value == &V::type_marker() {
            Some("Type".to_string())
        } else if value == &V::function_type_marker() {
            Some("TypeFunction".to_string())
        } else if value == &V::tuple_type_marker() {
            Some("TypeTuple".to_string())
        } else if value == &V::array_type_marker() {
            Some("TypeArray".to_string())
        } else if value == &V::type_struct_marker() {
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
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && self.is_universe_any(elements[1].node) {
            return self.any_node(elements[0].node);
        }
        // A bare struct kind `[id, [TypeStruct, K]]` (a struct type pair's
        // type slot): render its tag `TypeStruct`.
        if is_struct_kind(self.module, node)
            && let Some(LowValue::Array(kind)) =
                self.module.nodes[node].value.and_then(|v| v.as_enum())
            && let Some(LowValue::Array(inner)) = self
                .module
                .node_value(kind.items()[1].node)
                .and_then(|v| v.as_enum())
        {
            return self.any_node(inner.items()[0].node);
        }
        // A struct type: `[shape, [id, [TypeStruct, K]]]` — the nominal id
        // heads the kind and the `TypeStruct` tag is the inner
        // `[TypeStruct, K]` layer, so it is detected beside the `[marker, K]`
        // compound kinds.  The id renders as `#n` so two structs with the
        // same field shape stay distinguishable (their nominal types differ).
        if elements.len() == 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && self.kind_is_struct_any(kind.items())
        {
            let fields = self.fields_any(elements[0].node);
            let id = self
                .module
                .node_value(kind.items()[0].node)
                .and_then(|v| v.type_id());
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
                Some(m) if m == V::function_type_marker() => {
                    // shape = [in, out] — render `in -> out`.
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
                Some(m) if m == V::tuple_type_marker() => {
                    // shape = the field-type list — render `<T1, ..., Tn>`.
                    let fields = self.fields_any(elements[0].node);
                    return format!("<{}>", fields.join(", "));
                }
                Some(m) if m == V::array_type_marker() => {
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
            Some(LowValue::None | LowValue::Parameterized) => "?".to_string(),
            Some(LowValue::Function(_)) => "Function".to_string(),
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
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && self.is_static_universe(elements[1].node) {
            return self.static_any(elements[0].node, visiting);
        }
        // A struct type: `[shape, [id, [TypeStruct, K]]]` — the nominal id
        // heads the kind and the `TypeStruct` tag is the inner
        // `[TypeStruct, K]` layer.
        if elements.len() == 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && self.kind_is_struct_any(kind.items())
        {
            let fields = self.static_fields(elements[0].node, visiting);
            let id = self
                .module
                .node_value(kind.items()[0].node)
                .and_then(|v| v.type_id());
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
                Some(m) if m == V::function_type_marker() => {
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
                Some(m) if m == V::tuple_type_marker() => {
                    let fields = self.static_fields(elements[0].node, visiting);
                    return format!("<{}>", fields.join(", "));
                }
                Some(m) if m == V::array_type_marker() => {
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
                && self.module.node_value(items[0].node) == Some(V::type_marker())
                && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index);
        }
        false
    }

    pub fn is_universe_any(&self, id: AnyNodeId) -> bool {
        match id {
            AnyNodeId::Dynamic(node) => is_universe(self.module, node),
            AnyNodeId::Static(_) => self.is_static_universe(id),
        }
    }

    fn kind_is_struct_any(&self, kind_items: &[ArrayItem]) -> bool {
        kind_items.len() == 2
            && self
                .module
                .node_value(kind_items[1].node)
                .and_then(|v| v.as_enum())
                .is_some_and(|v| match v {
                    LowValue::Array(inner) => {
                        let items = inner.items();
                        items.len() == 2
                            && self.module.node_value(items[0].node)
                                == Some(V::type_struct_marker())
                            && self.is_universe_any(items[1].node)
                    }
                    _ => false,
                })
    }
}
/// The class representative of `node`, via a read-only `parent` walk (the
/// printers never mutate the module).
fn representative<V: ValueType>(module: &Module<HighProgram<V>>, node: NodeId) -> NodeId {
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
pub fn is_struct_kind<V: ValueType>(module: &Module<HighProgram<V>>, node: NodeId) -> bool {
    module
        .nodes
        .get(node)
        .and_then(|n| n.value)
        .and_then(|v| v.as_enum())
        .is_some_and(|v| match v {
            LowValue::Array(kind) => kind_is_struct(module, kind.items()),
            _ => false,
        })
}

fn is_universe<V: ValueType>(module: &Module<HighProgram<V>>, node: NodeId) -> bool {
    let rep = representative(module, node);
    matches!(module.nodes[node].value, Some(value)
    if matches!(value.as_enum(), Some(LowValue::Array(array))
        if array.items().iter().any(|item| match item.node {
            AnyNodeId::Dynamic(item) => representative(module, item) == rep,
            AnyNodeId::Static(_) => is_universe_any(module, item.node),
        })))
}

/// The read-only, static-aware universe test shared by the free printer
/// helpers: a static universe is the self-referential `[Type, itself]`.
fn is_universe_any<V: ValueType>(module: &Module<HighProgram<V>>, id: AnyNodeId) -> bool {
    match id {
        AnyNodeId::Dynamic(node) => is_universe(module, node),
        AnyNodeId::Static(sref) => module
            .node_value(id)
            .and_then(|v| v.as_enum())
            .is_some_and(|v| match v {
                LowValue::Array(array) => {
                    let items = array.items();
                    items.len() == 2
                        && module.node_value(items[0].node) == Some(V::type_marker())
                        && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
                }
                _ => false,
            }),
    }
}

/// Whether `kind_items` (the element items of a kind value) describe a struct
/// kind: `[id, [TypeStruct, K]]`.  The nominal id heads the kind and the
/// `TypeStruct` tag is the inner `[TypeStruct, K]` layer, so it cannot be
/// detected the way the `[marker, K]` kinds are.
pub fn kind_is_struct<V: ValueType>(module: &Module<HighProgram<V>>, kind_items: &[ArrayItem]) -> bool {
    kind_items.len() == 2
        && module
            .node_value(kind_items[1].node)
            .and_then(|v| v.as_enum())
            .is_some_and(|v| match v {
                LowValue::Array(inner) => {
                    let items = inner.items();
                    items.len() == 2
                        && module.node_value(items[0].node) == Some(V::type_struct_marker())
                        && is_universe_any(module, items[1].node)
                }
                _ => false,
            })
}

