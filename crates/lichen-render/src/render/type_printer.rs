//! [`TypePrinter`]'s methods.  The struct itself is in the parent module,
//! which is where its state is documented.

use super::*;
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
            static_names: HashMap::new(),
            next: 0,
            path: AncestorNodes::new(),
            render_ext,
            show_struct_id: false,
        }
    }

    /// Turn the nominal-id suffix on — the diagnostic printer needs it so two
    /// structs with the same field shape stay distinguishable.
    pub fn show_struct_ids(&mut self) {
        self.show_struct_id = true;
    }

    /// The module this printer renders from — for a renderer that needs to walk
    /// the module alongside a rendered type (e.g. a [`DiagKind::NamedField`]
    /// diagnostic's did-you-mean clause, which enumerates the struct's fields).
    pub fn module(&self) -> &'a Module<P> {
        self.module
    }

    /// Render a type node; an unbound cell renders as its class name.  A
    /// computed nothing ([`LowValue::Void`]) is a concrete value and renders
    /// as `none` — it is never a fresh class variable.
    pub fn node(&mut self, node: NodeId) -> String {
        if self.path.contains(node) {
            return "…".to_string();
        }
        // A value-less node is an unbound cell: the lazy marker is the
        // honest stand-in so it routes to the class name.
        let value = self
            .module
            .node_value(AnyNodeId::Dynamic(node))
            .unwrap_or_else(|| P::Value::from(LowValue::Parameterized));
        if matches!(value.as_enum(), Some(LowValue::Parameterized)) {
            return self.class_name(node);
        }
        self.path.insert(node);
        let out = self.value(node, value);
        self.path.remove(node);
        out
    }

    /// The stable name of an unbound cell's class: `?a`, `?b`, … — cells in
    /// the same class share a name.  A node the walk cannot place has no class
    /// to name, so it renders as the unknown `?`.
    pub fn class_name(&mut self, node: NodeId) -> String {
        let Some(rep) = representative(self.module, node) else {
            return "?".to_string();
        };
        if let Some(name) = self.names.get(&rep) {
            return name.clone();
        }
        let name = letter_name(self.next);
        self.next += 1;
        self.names.insert(rep, name.clone());
        name
    }

    /// The stable name of an unbound **static** cell: `?a`, `?b`, … — a frozen
    /// module's type variable.  Keyed by the cell's **equality class**, exactly
    /// as [`Self::class_name`] is: the artifact keeps a class whole, so two refs
    /// in one class are one variable, and naming them by ref alone would print
    /// an imported polymorphic `?a -> ?a` as `?a -> ?b`.
    pub fn static_class_name(&mut self, sref: lichen_lowlevel::StaticNodeId) -> String {
        let representative = self.module.static_equality_representative(sref);
        if let Some(name) = self.static_names.get(&representative) {
            return name.clone();
        }
        let name = letter_name(self.next);
        self.next += 1;
        self.static_names.insert(representative, name.clone());
        name
    }

    /// Render a type value, descending into arrays.
    fn value(&mut self, node: NodeId, value: P::Value) -> String {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => n.to_string(),
                // A float in a type expression is a structural value, as a
                // `USize` array length is: it prints its own digits, never the
                // marker's name (`type_constant` spells that).
                LowValue::Float(value) => float_literal(value),
                LowValue::Str(s) => format!("\"{s}\""),
                // SAFETY: `array` is the payload of `node`, a live node of the
                // module being rendered; this printer releases no block.
                LowValue::Array(array) => self.elements(node, unsafe { array.items() }),
                LowValue::Table(_) => "Table".to_string(),
                LowValue::Function(_) => "Function".to_string(),
                LowValue::None | LowValue::Void => "none".to_string(),
                LowValue::Parameterized => {
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
        } else if value == &P::Value::float_marker() {
            // The marker itself, not a float value: source syntax spells the
            // type constant `Float` (the lexer's keyword), not the registry's
            // doc label `float`.  A float *value* is the other site, and prints
            // its digits through `float_literal`.
            Some("Float".to_string())
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
        } else if value == &P::Value::set_type_marker() {
            Some("SetType".to_string())
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
            // SAFETY: `kind` is the payload of a value read from the live node
            // `elements[1]`; this printer releases no block.  The note covers
            // the three `items()` calls in this arm.
            && self.kind_is_struct_any(unsafe { kind.items() })
        {
            let fields = self.fields_any(elements[0].node);
            let names = struct_field_names(self.module, unsafe { kind.items() }, fields.len());
            let fields = struct_fields_with_names(&fields, &names);
            let id = struct_kind_id(self.module, unsafe { kind.items() });
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
            && let kind = unsafe { kind.items() }
            && kind.len() == 2
            && self.is_universe_any(kind[1].node)
        {
            match self.module.node_value(kind[0].node) {
                Some(m) if m == P::Value::function_type_marker() => {
                    // shape = [in, out] — render `in -> out`.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
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
                    // shape = [element type, length] — render `array<T, len>`.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() == 2
                    {
                        return format!(
                            "array<{}, {}>",
                            self.any_node(s[0].node),
                            self.any_node(s[1].node)
                        );
                    }
                }
                Some(m) if m == P::Value::set_type_marker() => {
                    // shape = the element type alone — render `set<T>`.  A set
                    // has no length, which is exactly what separates it from
                    // `array<T, n>`.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() == 1
                    {
                        return format!("set<{}>", self.any_node(s[0].node));
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
        // Fallback: render the raw elements, marked as raw.
        let parts: Vec<String> = elements
            .iter()
            .map(|item| self.any_node(item.node))
            .collect();
        format!("raw[{}]", parts.join(", "))
    }

    /// Is `node`'s class a checker-registered arrow shape?  A class the walk
    /// cannot place is not a registered arrow.
    fn is_arrow(&self, node: NodeId) -> bool {
        let Some(rep) = representative(self.module, node) else {
            return false;
        };
        self.arrows.is_some_and(|arrows| {
            disjoint::members(&self.module.nodes, rep).any(|m| arrows.contains(&m))
        })
    }

    /// Read-only variant of [`Self::fields`] for a static or dynamic shape.
    fn fields_any(&mut self, shape: AnyNodeId) -> Vec<String> {
        if let Some(LowValue::Array(array)) =
            self.module.node_value(shape).and_then(|v| v.as_enum())
        {
            // SAFETY: `array` is the payload of the value read from the live
            // node `shape`.
            unsafe { array.items() }
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
        if value.is_none_or(|v| matches!(v.as_enum(), Some(LowValue::Parameterized))) {
            visiting.remove(&sref);
            return self.static_class_name(sref);
        }
        let value = value.unwrap();
        let out = match value.as_enum() {
            Some(LowValue::USize(n)) => n.to_string(),
            // As in `value`: the float's digits, not the marker's name.
            Some(LowValue::Float(value)) => float_literal(value),
            Some(LowValue::Str(s)) => format!("\"{s}\""),
            Some(LowValue::Parameterized) => self.static_class_name(sref),
            // A computed nothing is a concrete value, never a class letter.
            Some(LowValue::None | LowValue::Void) => "none".to_string(),
            Some(LowValue::Function(_)) => "Function".to_string(),
            Some(LowValue::Table(_)) => "Table".to_string(),
            // SAFETY: `array` is a static payload read through `sref`, whose
            // registered module pins the arena.
            Some(LowValue::Array(array)) => {
                self.static_elements(sref, unsafe { array.items() }, visiting)
            }
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
            // SAFETY: `kind` is the payload of a value read from the live node
            // `elements[1]`; the note covers the three `items()` calls in this
            // arm.
            && self.kind_is_struct_any(unsafe { kind.items() })
        {
            let fields = self.static_fields(elements[0].node, visiting);
            let names = struct_field_names(self.module, unsafe { kind.items() }, fields.len());
            let fields = struct_fields_with_names(&fields, &names);
            let id = struct_kind_id(self.module, unsafe { kind.items() });
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
            && let kind = unsafe { kind.items() }
            && kind.len() == 2
            && self.is_static_universe(kind[1].node)
        {
            match self.module.node_value(kind[0].node) {
                Some(m) if m == P::Value::function_type_marker() => {
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
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
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() == 2
                    {
                        return format!(
                            "array<{}, {}>",
                            self.static_any(s[0].node, visiting),
                            self.static_any(s[1].node, visiting)
                        );
                    }
                }
                _ => {}
            }
        }
        // Fallback: render the raw static elements, marked as raw.
        let parts: Vec<String> = elements
            .iter()
            .map(|item| self.static_any(item.node, visiting))
            .collect();
        format!("raw[{}]", parts.join(", "))
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
            // SAFETY: `array` is the payload of the value read from the live
            // node `shape`.
            unsafe { array.items() }
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
            // SAFETY: `array` is the payload of the value read from the live
            // node `id`.
            let items = unsafe { array.items() };
            return items.len() == 2
                && self.module.node_value(items[0].node) == Some(P::Value::type_marker())
                && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index);
        }
        false
    }

    pub(super) fn is_universe_any(&self, id: AnyNodeId) -> bool {
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
