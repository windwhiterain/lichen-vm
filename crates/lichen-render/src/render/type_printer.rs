//! [`TypePrinter`]'s methods.  The struct itself is in the parent module,
//! which is where its state is documented.

use super::*;
impl<'a, P: HighProgram + 'static> TypePrinter<'a, P>
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

    /// A printer whose extension vocabulary renders through `render_ext`, the
    /// hook returning a variant's spelling or `None`.
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
            attrs: None,
            show_struct_id: false,
        }
    }

    /// Spell each element slot's attributes through the registry `attrs`;
    /// without it a slot spells its type alone.
    ///
    /// # Invariant
    /// An attribute spells itself, so an unspelled position answers `None`.
    pub fn show_attributes(&mut self, attrs: Attrs<'a, P>) {
        self.attrs = Some(attrs);
    }

    /// Turn the nominal-id suffix on, so two structs of one field shape stay
    /// distinguishable.
    pub fn show_struct_ids(&mut self) {
        self.show_struct_id = true;
    }

    /// The module this printer renders from, for a renderer that must walk it
    /// alongside a rendered type.
    pub fn module(&self) -> &'a Module<P> {
        self.module
    }

    /// Render a type node; an undecided cell renders as its class name.
    ///
    /// # Invariant
    ///
    /// An empty value ([`LowValue::Error`]) is concrete and renders `none`,
    /// never a fresh class variable.
    pub fn node(&mut self, node: NodeId) -> String {
        if self.path.contains(node) {
            return "…".to_string();
        }
        // A value-less node is an undecided cell: it renders as its class name.
        let Some(value) = self.module.node_value(AnyNodeId::Dynamic(node)) else {
            return self.class_name(node);
        };
        self.path.insert(node);
        let out = self.value(node, value);
        self.path.remove(node);
        out
    }

    /// The stable name of an undecided cell's class: cells in one class share a
    /// name; an unplaceable node renders `?`.
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

    /// The stable name of an undecided **static** cell, keyed by the cell's
    /// equality class as [`Self::class_name`] is.
    ///
    /// # Invariant
    ///
    /// The artifact keeps a class whole, so two refs in one class are one
    /// variable; naming by ref alone would print an imported polymorphic
    /// `?a -> ?a` as `?a -> ?b`.
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
                // A float in a type expression is structural, like a `USize`
                // array length: it prints its digits, not the marker's name.
                LowValue::Float(value) => float_literal(value),
                LowValue::Str(s) => format!("\"{s}\""),
                // SAFETY: `array` is the payload of `node`, a live node of the
                // module being rendered; this printer releases no block.
                LowValue::Array(array) => self.elements(node, unsafe { array.items() }),
                LowValue::Table(_) => "Table".to_string(),
                LowValue::Function(_) => "Function".to_string(),
                LowValue::None | LowValue::Error => "none".to_string(),
                // The wildcard's own spelling, as the source writes it.
                LowValue::Any => "@any".to_string(),
            };
        }
        self.type_constant(&value)
            .unwrap_or_else(|| "?".to_string())
    }

    /// The spelling of a type constant, or of an extension's own variant.
    pub(crate) fn type_constant(&self, value: &P::Value) -> Option<String> {
        if value == &P::Value::int_marker() {
            Some("Int".to_string())
        } else if value == &P::Value::float_marker() {
            // The marker, not a float value: the constant is spelled `Float`,
            // not a registry label; a float value prints its digits.
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
        // A bare struct kind `[[payload, TypeStruct], K]`: render its tag, since
        // its marker is a pair.
        if is_struct_kind(self.module, node) {
            return "TypeStruct".to_string();
        }
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && self.is_universe_any(elements[1].node) {
            return self.any_node(elements[0].node);
        }
        // A function-type node `[Function(fid), ↺]`: its signature lives in the
        // template, not a `[dom, cod]` shape.
        if elements.len() == 2
            && self.slot1_is_self(elements[1].node, node)
            && let Some(fv) = self.module.node_value(elements[0].node)
            && let Some(LowValue::Function(fid)) = fv.as_enum()
            && let Some((dom, cod)) = self.function_signature(fid)
        {
            return format!("{dom} -> {cod}");
        }
        // A struct type: the kind is the standard marker pair, whose id renders
        // `#n` to distinguish same-shaped structs.
        if elements.len() == 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            // SAFETY: `kind` is a value read from the live node `elements[1]`;
            // this arm's other `items()` calls share the note.
            && self.kind_is_struct_any(unsafe { kind.items() })
        {
            let fields = self.slot_fields_any(elements[0].node);
            let names = struct_field_names(self.module, unsafe { kind.items() }, fields.len());
            let fields = struct_fields_with_names(&fields, &names);
            let id = struct_kind_id(self.module, unsafe { kind.items() });
            return match (self.show_struct_id, id) {
                (true, Some(n)) => format!("struct{{{}}}#{n}", fields.join(", ")),
                _ => format!("struct{{{}}}", fields.join(", ")),
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
                    // An open field list — an undecided shape — shows its
                    // placeholder with an ellipsis, so `tuple{?a}` is not a tuple.
                    let open = !matches!(
                        self.module
                            .node_value(elements[0].node)
                            .and_then(|value| value.as_enum()),
                        Some(LowValue::Array(_))
                    );
                    let fields = self.slot_fields_any(elements[0].node);
                    return if open {
                        format!("tuple{{{}, …}}", fields.join(", "))
                    } else {
                        format!("tuple{{{}}}", fields.join(", "))
                    };
                }
                Some(m) if m == P::Value::array_type_marker() => {
                    // shape = [element slot, length] — render `array{T, len}`,
                    // the element as source so a pin is stated.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() == 2
                    {
                        return format!(
                            "array{{{}, {}}}",
                            self.slot_any(s[0].node),
                            self.any_node(s[1].node)
                        );
                    }
                }
                Some(m) if m == P::Value::set_type_marker() => {
                    // A set has no length, which is what separates it from
                    // `array{T, n}`.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() == 1
                    {
                        return format!("set<{}>", self.slot_any(s[0].node));
                    }
                }
                Some(m) if m == P::Value::table_type_marker() => {
                    // shape = [key slot, value slot] — render `table<K, V>`.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() == 2
                    {
                        return format!(
                            "table<{}, {}>",
                            self.slot_any(s[0].node),
                            self.slot_any(s[1].node)
                        );
                    }
                }
                _ => {}
            }
        }
        // A bare `[in, out]` shape is an arrow only when the checker registered
        // the shape; otherwise it reads raw.
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

    /// Whether element 1 **is** the node's own self-cycle, marking a function
    /// type `[Function(fid), ↺]`.
    ///
    /// # Invariant
    ///
    /// Two forms count: a **dynamic** self-cycle, compared by class, so a cell
    /// bound to the function-type counts; and a **static** self-ref, because a
    /// materialized static function-type carries the frozen node's value, so its
    /// slot 1 names that node's cycle, not the copy.
    pub(super) fn slot1_is_self(&self, slot1: AnyNodeId, node: NodeId) -> bool {
        match slot1 {
            AnyNodeId::Dynamic(slot1) => {
                representative(self.module, slot1) == representative(self.module, node)
            }
            AnyNodeId::Static(sref) => self.static_function_type_function(sref).is_some(),
        }
    }

    /// The function a **static** node's value is the type of: a
    /// `[Function(fid), t]` whose slot 1 is itself a function type.
    ///
    /// # Invariant
    ///
    /// Slot 1 is about the node `t` names, never about the node printed: a
    /// materialized member of a function type's class carries the frozen type's
    /// value, so its slot 1 names the frozen node that *is* the cycle.  See
    /// docs/notes/class-channel.md.
    fn static_function_type_function(
        &self,
        sref: lichen_lowlevel::StaticNodeId,
    ) -> Option<AnyFunctionId> {
        let value = self.module.static_read(sref)?;
        let LowValue::Array(array) = value.as_enum()? else {
            return None;
        };
        // SAFETY: `array` is the value read through `sref`, whose registered
        // module pins the arena.
        let items = unsafe { array.items() };
        let [head, tail] = items else {
            return None;
        };
        let head = self.module.node_value(head.node)?;
        let LowValue::Function(fid) = head.as_enum()? else {
            return None;
        };
        let AnyNodeId::Static(tail) = tail.node else {
            return None;
        };
        matches!(
            self.module.static_read(tail).and_then(|value| value.as_enum()),
            Some(LowValue::Array(items))
                if unsafe { items.items() }.get(1).is_some_and(|item| item.node == AnyNodeId::Static(tail))
        )
        .then_some(fid)
    }

    /// The `domain -> codomain` spelling of a signature, from the template's
    /// parameter and return type cells.
    ///
    /// # Invariant
    ///
    /// The cells read undecided for a polymorphic function (`?a -> ?a`) and
    /// bound for a monomorphic one.  `None` for a static function-type or
    /// non-pair entry points.
    fn function_signature(&mut self, fid: AnyFunctionId) -> Option<(String, String)> {
        match fid {
            AnyFunctionId::Dynamic(function) => {
                let function = &self.module.functions[function];
                let param_ty = self.pair_type_slot(function.parameter)?;
                let return_ty = self.pair_type_slot(function.r#return)?;
                Some((self.node(param_ty), self.node(return_ty)))
            }
            AnyFunctionId::Static(sref) => {
                let (param_ty, return_ty) = self.module.static_function_signature(sref)?;
                Some((self.any_node(param_ty), self.any_node(return_ty)))
            }
        }
    }

    /// Element 1 (the type slot) of a `[value, type, attrs…]` pair; `None` for a
    /// short pair or a static slot.
    fn pair_type_slot(&self, pair: NodeId) -> Option<NodeId> {
        // SAFETY: `pair` is a live node of the module being rendered; this
        // printer releases no block.
        let items = unsafe { self.module.array_items(pair) }?;
        match items.get(1)?.node {
            AnyNodeId::Dynamic(n) => Some(n),
            AnyNodeId::Static(_) => None,
        }
    }

    /// Render any node — dynamic or static — as a type.
    pub fn any_node(&mut self, id: AnyNodeId) -> String {
        match id {
            AnyNodeId::Dynamic(node) => self.node(node),
            AnyNodeId::Static(sref) => self.static_node(sref),
        }
    }

    /// [`Self::fields_any`] over an element-**slot** list: each element read as
    /// source, a pin stated and the wildcard not.
    fn slot_fields_any(&mut self, shape: AnyNodeId) -> Vec<String> {
        if let Some(LowValue::Array(array)) =
            self.module.node_value(shape).and_then(|v| v.as_enum())
        {
            // SAFETY: `array` is the payload of the value read from the live
            // node `shape`.
            unsafe { array.items() }
                .iter()
                .map(|item| self.slot_any(item.node))
                .collect()
        } else {
            vec![self.any_node(shape)]
        }
    }

    /// An element slot read as source: a pin stated, the wildcard not.
    ///
    /// # Invariant
    /// An element that spells no value holds the wildcard, which constrains
    /// nothing, so source states no value either; anything else is a pin and
    /// states it. See `docs/notes/compound-element-slots.md`.
    pub(crate) fn slot_any(&mut self, slot: AnyNodeId) -> String {
        let Some(ty) = self.slot_type_any(slot) else {
            return self.any_node(slot);
        };
        let ty = self.any_node(ty);
        let mut out = match self.slot_value_any(slot) {
            Some(value) => format!("{} : {ty}", self.any_node(value)),
            None => ty,
        };
        for spelling in self.slot_attributes(slot) {
            out.push(' ');
            out.push_str(&spelling);
        }
        out
    }

    /// An element slot's attribute **spellings**, in the canonical order.
    ///
    /// # Invariant
    /// Position `i` of the slot holds `P::Attr::ORDER`'s `i`-th extension's own
    /// value, and [`AttrExt::render`] answers `None` for a position the source
    /// left unspelled — the wildcard included — so an element reads back as it
    /// was written.
    fn slot_attributes(&mut self, slot: AnyNodeId) -> Vec<String> {
        let Some(attrs) = self.attrs else {
            return Vec::new();
        };
        let Some(LowValue::Array(items)) = self.module.node_value(slot).and_then(|v| v.as_enum())
        else {
            return Vec::new();
        };
        // SAFETY: `items` is the payload of the value read from the live node
        // `slot`.
        let items = unsafe { items.items() };
        let mut out = Vec::new();
        for marker in P::Attr::ORDER {
            let Some(item) = items.get(shape::attr_slot(marker.order_index())) else {
                continue;
            };
            let AnyNodeId::Dynamic(node) = item.node else {
                continue;
            };
            if let Some(spelling) = attrs(marker).render(self.module, node, attrs) {
                out.push(spelling);
            }
        }
        out
    }

    /// An element slot's value position, when it holds a **pin** rather than the
    /// wildcard.
    fn slot_value_any(&self, slot: AnyNodeId) -> Option<AnyNodeId> {
        let LowValue::Array(array) = self.module.node_value(slot)?.as_enum()? else {
            return None;
        };
        // SAFETY: `array` is the payload of the value read from the live node
        // `slot`.
        let value = unsafe { array.items() }.first()?.node;
        match self
            .module
            .node_value(value)
            .and_then(|value| value.as_enum())
        {
            Some(LowValue::Any) => None,
            _ => Some(value),
        }
    }

    /// An element slot's type position, when it has one.
    pub(crate) fn slot_type_any(&self, slot: AnyNodeId) -> Option<AnyNodeId> {
        let LowValue::Array(array) = self.module.node_value(slot)?.as_enum()? else {
            return None;
        };
        // SAFETY: `array` is the payload of the value read from the live node
        // `slot`.
        unsafe { array.items() }.get(1).map(|item| item.node)
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
        let Some(value) = self.module.node_value(AnyNodeId::Static(sref)) else {
            visiting.remove(&sref);
            return self.static_class_name(sref);
        };
        let out = match value.as_enum() {
            Some(LowValue::USize(n)) => n.to_string(),
            // As in `value`: the float's digits, not the marker's name.
            Some(LowValue::Float(value)) => float_literal(value),
            Some(LowValue::Str(s)) => format!("\"{s}\""),
            // An empty value is a concrete value, never a class letter.
            Some(LowValue::None | LowValue::Error) => "none".to_string(),
            Some(LowValue::Any) => "@any".to_string(),
            Some(LowValue::Function(_)) => "Function".to_string(),
            Some(LowValue::Table(_)) => "Table".to_string(),
            // SAFETY: `array` is a static payload read through `sref`, whose
            // registered module pins the arena.
            Some(LowValue::Array(array)) => {
                let items = unsafe { array.items() };
                // A static function-type node, or a member carrying one, prints
                // the template's signature rather than the raw pair.
                if let Some(fid) = self.static_function_type_function(sref)
                    && let Some((dom, cod)) = self.function_signature(fid)
                {
                    format!("{dom} -> {cod}")
                } else {
                    self.static_elements(sref, items, visiting)
                }
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
        // A bare struct kind `[[payload, TypeStruct], K]`: render its tag.
        if kind_is_struct(self.module, elements) {
            return "TypeStruct".to_string();
        }
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && self.is_static_universe(elements[1].node) {
            return self.static_any(elements[0].node, visiting);
        }
        // A struct type: the kind is the standard marker pair, and a name table
        // rides in the payload's names slot.
        if elements.len() == 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            // SAFETY: `kind` is a value read from the live node `elements[1]`;
            // this arm's other `items()` calls share the note.
            && self.kind_is_struct_any(unsafe { kind.items() })
        {
            let fields = self.static_slot_fields(elements[0].node, visiting);
            let names = struct_field_names(self.module, unsafe { kind.items() }, fields.len());
            let fields = struct_fields_with_names(&fields, &names);
            let id = struct_kind_id(self.module, unsafe { kind.items() });
            return match (self.show_struct_id, id) {
                (true, Some(n)) => format!("struct{{{}}}#{n}", fields.join(", ")),
                _ => format!("struct{{{}}}", fields.join(", ")),
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
                    return format!("tuple{{{}}}", fields.join(", "));
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
                            "array{{{}, {}}}",
                            match self.slot_type_any(s[0].node) {
                                Some(ty) => self.static_any(ty, visiting),
                                None => self.static_any(s[0].node, visiting),
                            },
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

    /// [`Self::static_fields`] over an element-**slot** list, each read as source.
    ///
    /// # Invariant
    /// Its own spelling, because a static read threads the visiting set a cycle
    /// needs.
    fn static_slot_fields(
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
                .map(|item| match self.slot_type_any(item.node) {
                    Some(ty) => self.static_any(ty, visiting),
                    None => self.static_any(item.node, visiting),
                })
                .collect()
        } else {
            vec![self.static_any(shape, visiting)]
        }
    }

    /// Whether a **static** ref's value is the universe `[Type, ↺]`, whose head
    /// is the `Type` marker, not a containing kind.
    ///
    /// # Invariant
    ///
    /// The tail is the universe too, but need not be this very node: a kind read
    /// from a frozen module is a replica whose items are refs into the writing
    /// module, so its tail names that module's canonical self-loop.  Reading
    /// through the ref lets a `[shape, [Type, ↺]]` pair that crossed a module
    /// boundary render as its head rather than the raw mark.
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
            if items.len() != 2
                || self.module.node_value(items[0].node) != Some(P::Value::type_marker())
            {
                return false;
            }
            return match items[1].node {
                AnyNodeId::Static(tail)
                    if tail.module == sref.module && tail.index == sref.index =>
                {
                    true
                }
                tail => self.is_universe_any(tail),
            };
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
