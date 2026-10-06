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
    /// empty value ([`LowValue::Error`]) is a concrete value and renders
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
                LowValue::None | LowValue::Error => "none".to_string(),
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
        // A bare struct kind `[[payload, TypeStruct], K]` (a struct type
        // pair's type slot): render its tag `TypeStruct`.  Detected before the
        // `[head, K]` atomic branch, since its marker is a pair
        // (not a plain type constant).
        if is_struct_kind(self.module, node) {
            return "TypeStruct".to_string();
        }
        // `[head, K]` — an atomic type: the kind slot is the self-looping
        // universe, so render the head (`int`, `Type`, …).
        if elements.len() == 2 && self.is_universe_any(elements[1].node) {
            return self.any_node(elements[0].node);
        }
        // A function-type node `[Function(fid), ↺]` — a function's own type
        // (`f : f`): slot 0 holds a `Function` value and slot 1 is the node
        // itself (the self-cycle, like the universe `[Type, ↺]`). Its
        // signature lives in the function template (`parameter` / `r#return`),
        // not in a `[dom, cod]` shape, so render `domain -> codomain` from the
        // template's parameter and return type cells. A static function-type
        // (a frozen module's) is left to the raw fallback — its signature
        // reads through the static module, not yet wired here.
        if elements.len() == 2
            && self.slot1_is_self(elements[1].node, node)
            && let Some(fv) = self.module.node_value(elements[0].node)
            && let Some(LowValue::Function(fid)) = fv.as_enum()
            && let Some((dom, cod)) = self.function_signature(fid)
        {
            return format!("{dom} -> {cod}");
        }
        // A struct type: `[shape, [[payload, TypeStruct], K]]` — the kind is a
        // standard `[marker, K]` pair whose marker is the `[payload, TypeStruct]`
        // pair.  The id renders as `#n` so two structs with the
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
                    // shape = the field-type list — render `<T1, ..., Tn>`.  A
                    // shape the graph has not decided (a cell — the pin a
                    // positional read states on an undecided container) is an
                    // **open** field list: the arity is unknown, so its one
                    // placeholder is shown with an ellipsis rather than
                    // mistaken for the whole tuple (`<?a>` reads as a
                    // one-element tuple).
                    let open = !matches!(
                        self.module
                            .node_value(elements[0].node)
                            .and_then(|value| value.as_enum()),
                        Some(LowValue::Array(_))
                    );
                    let fields = self.fields_any(elements[0].node);
                    return if open {
                        format!("<{}, …>", fields.join(", "))
                    } else {
                        format!("<{}>", fields.join(", "))
                    };
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

    /// Whether an element-1 node **is** the node's own self-cycle — the mark of
    /// a function-type `[Function(fid), ↺]`, as against a plain `[value, type]`
    /// pair.  Two forms count:
    ///
    /// - a **dynamic** self-cycle, compared by class so a cell bound to the
    ///   function-type also counts (it carries the same value but is a class
    ///   member, not the self-ref node itself);
    /// - a **static** self-ref: a materialized static function-type copies the
    ///   frozen node's value, so its slot 1 points at the frozen node's own
    ///   cycle rather than back at the copy.
    pub(super) fn slot1_is_self(&self, slot1: AnyNodeId, node: NodeId) -> bool {
        match slot1 {
            AnyNodeId::Dynamic(slot1) => {
                representative(self.module, slot1) == representative(self.module, node)
            }
            AnyNodeId::Static(sref) => matches!(
                self.module.static_read(sref).as_enum(),
                Some(LowValue::Array(array))
                    if unsafe { array.items() }.get(1)
                        .is_some_and(|item| item.node == AnyNodeId::Static(sref))
            ),
        }
    }

    /// The `domain -> codomain` spelling of a function-type node's signature,
    /// read from the function template's parameter and return *type* cells
    /// (`Function::parameter` and `Function::r#return` are the `[value, type]`
    /// pairs; slot 1 is the type). The template's cells are read directly —
    /// they are unbound for a polymorphic function (so `?a -> ?a`) and bound
    /// for a monomorphic one, which is exactly the signature to print. `None`
    /// for a static function-type (its template lives in a static module, not
    /// wired here yet) or a function whose entry points are not pairs.
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

    /// Element 1 (the type slot) of a `[value, type, attrs…]` pair, as a
    /// dynamic node. `None` when `pair` is not a 2+-element array or its type
    /// slot is a static ref.
    fn pair_type_slot(&self, pair: NodeId) -> Option<NodeId> {
        // SAFETY: `pair` is a live node of the module being rendered; this
        // printer releases no block.
        let items = unsafe { self.module.array_items(pair) }?;
        match items.get(1)?.node {
            AnyNodeId::Dynamic(n) => Some(n),
            AnyNodeId::Static(_) => None,
        }
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
            // An empty value is a concrete value, never a class letter.
            Some(LowValue::None | LowValue::Error) => "none".to_string(),
            Some(LowValue::Function(_)) => "Function".to_string(),
            Some(LowValue::Table(_)) => "Table".to_string(),
            // SAFETY: `array` is a static payload read through `sref`, whose
            // registered module pins the arena.
            Some(LowValue::Array(array)) => {
                let items = unsafe { array.items() };
                // A static **function-type node** `[Function(fid), ↺]`: slot 1
                // is the node's own static self-cycle, so print the template's
                // signature rather than the raw pair.  The dynamic case is in
                // `elements`.
                if items.len() == 2
                    && items[1].node == AnyNodeId::Static(sref)
                    && let Some(fv) = self.module.node_value(items[0].node)
                    && let Some(LowValue::Function(fid)) = fv.as_enum()
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
        // A struct type: `[shape, [[payload, TypeStruct], K]]` — the kind is a
        // standard `[marker, K]` pair whose marker is the `[payload, TypeStruct]`
        // pair; a name table rides in the payload's names slot.
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

    /// Whether a **static** ref's value is the universe, `[Type, ↺]`.
    ///
    /// The head must be the `Type` marker: that is what separates *the* universe
    /// from a kind that merely *contains* it — an `array<…>`'s `[TypeArray, K]`
    /// has the same self-referential silhouette
    /// ([universe-containment](../docs/notes/universe-containment.md) §2).
    ///
    /// The tail must be the universe too, but it need not be *this very node*: a
    /// kind read out of a frozen module is a **replica** whose two items are refs
    /// into the module that wrote it, and the tail then names that module's
    /// canonical self-loop rather than the replica.  Reading through the ref is
    /// what lets a `[shape, [Type, ↺]]` pair that crossed a module boundary render
    /// as its head instead of falling back to the raw mark
    /// ([raw-rendering-mark](../docs/notes/raw-rendering-mark.md) §2).
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
