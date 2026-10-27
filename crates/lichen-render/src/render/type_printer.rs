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
                LowValue::Function(fid) => self.function_spelling(fid),
                LowValue::None | LowValue::Error => "none".to_string(),
                // The wildcard's own spelling, as the source writes it.
                LowValue::Any => "@any".to_string(),
            };
        }
        self.type_constant(&value)
            .unwrap_or_else(|| "?".to_string())
    }

    /// Render a node-less mismatch value; arrays return `None` for node fallback.
    pub fn diagnostic_value(&mut self, value: P::Value) -> Option<String> {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => Some(n.to_string()),
                LowValue::Float(value) => Some(float_literal(value)),
                LowValue::Str(s) => Some(format!("\"{s}\"")),
                LowValue::Array(_) => None,
                LowValue::Table(_) => Some("Table".to_string()),
                LowValue::Function(fid) => Some(self.function_spelling(fid)),
                LowValue::None | LowValue::Error => Some("none".to_string()),
                LowValue::Any => Some("@any".to_string()),
            };
        }
        self.type_constant(&value)
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
            // The `Tuple` template: the layout a tuple value's type is.
            Some("Tuple".to_string())
        } else if value == &P::Value::array_type_marker() {
            // The `Array` template: the layout an array value's type is.
            Some("Array".to_string())
        } else if value == &P::Value::type_struct_marker() {
            // The `Struct` template: the layout a struct value's type is.
            Some("Struct".to_string())
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
        // The **struct template**: the self-referential `[StructType, ↺]` a
        // struct value's type is.
        if elements.len() >= 2 && self.is_struct_template(elements[0].node) {
            return "Struct".to_string();
        }
        // `[head, K]` — an atomic type: the kind slot is the universe, so
        // render the head (`int`, `Type`, `Tuple`, …).
        if elements.len() >= 2 && self.is_universe_any(elements[1].node) {
            return self.any_node(elements[0].node);
        }
        // An expr carrying a **function** in its value slot reads as that
        // function's signature.
        if elements.len() >= 2
            && let Some(fv) = self.module.node_value(elements[0].node)
            && let Some(LowValue::Function(fid)) = fv.as_enum()
            && let Some((dom, cod)) = self.function_signature(fid)
        {
            return format!("{dom} => {cod}");
        }
        // `[shape, [marker, K]]` — a compound type: the kind's marker decides
        // how the shape reads.
        if elements.len() >= 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = unsafe { kind.items() }
            && kind.len() >= 2
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
                        && s.len() >= 2
                    {
                        return format!(
                            "{} => {}",
                            self.any_node(s[0].node),
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
                        return format!("set{{{}}}", self.slot_any(s[0].node));
                    }
                }
                Some(m) if m == P::Value::table_type_marker() => {
                    // shape = [key slot, value slot] — render `table{K, V}`.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() >= 2
                    {
                        return format!(
                            "table{{{}, {}}}",
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
        if elements.len() >= 2 && self.is_arrow(node) {
            return format!(
                "{} => {}",
                self.any_node(elements[0].node),
                self.any_node(elements[1].node)
            );
        }
        // Fallback: dump the raw elements, an expr's two meaningful slots only
        // (`docs/notes/array-attribute-view.md`).
        let width = logical_width(self.module, node, elements.len());
        let parts: Vec<String> = elements
            .iter()
            .take(width)
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

    /// A function value's own spelling: `domain => codomain`.
    ///
    /// # Invariant
    ///
    /// `Function` is the fallback for a signature the template does not state —
    /// a hand-built function, or a frozen one with no readable cells. Printing
    /// `?a => ?b` then would claim a shape nobody wrote.
    pub(crate) fn function_spelling(&mut self, fid: AnyFunctionId) -> String {
        match self.function_signature(fid) {
            Some((domain, codomain)) => format!("{domain} => {codomain}"),
            None => "Function".to_string(),
        }
    }

    /// The `domain -> codomain` spelling of a signature, from the template's
    /// parameter and return type cells.
    ///
    /// # Invariant
    ///
    /// The cells read undecided for a polymorphic function (`?a -> ?a`) and
    /// bound for a monomorphic one.  `None` for a static function-type or
    /// non-expr entry points.
    pub(crate) fn function_signature(&mut self, fid: AnyFunctionId) -> Option<(String, String)> {
        match fid {
            AnyFunctionId::Dynamic(function) => {
                let function = &self.module.functions[function];
                let param_ty = self.expr_type_any(function.parameter)?;
                // The stored cell is the codomain without forcing the return expr; a
                // hand-built function may leave it unset.
                let return_ty = if self.module.nodes.contains_key(function.return_type) {
                    AnyNodeId::Dynamic(function.return_type)
                } else {
                    self.expr_type_any(function.r#return)?
                };
                Some((self.any_node(param_ty), self.any_node(return_ty)))
            }
            AnyFunctionId::Static(sref) => {
                let (param_ty, return_ty) = self.module.static_function_signature(sref)?;
                Some((self.any_node(param_ty), self.any_node(return_ty)))
            }
        }
    }

    /// Element 1 (the type slot) of a `[value, type, attrs…]` expr; `None` for a
    /// short expr.
    ///
    /// # Invariant
    /// A static slot stays a static ref: a frozen type (`Int` from the prelude) is
    /// what a `Check` build's expr often holds, and `any_node` renders it.
    fn expr_type_any(&self, expr: NodeId) -> Option<AnyNodeId> {
        // SAFETY: `expr` is a live node of the module being rendered; this
        // printer releases no block.
        let items = unsafe { self.module.array_items(expr) }?;
        items.get(1).map(|item| item.node)
    }

    /// Render any node — dynamic or static — as a type.
    pub fn any_node(&mut self, id: AnyNodeId) -> String {
        match id {
            AnyNodeId::Dynamic(node) => self.node(node),
            AnyNodeId::Static(sref) => self.static_node(sref),
        }
    }

    /// Whether a node holds the **struct marker** — the shared `Struct`
    /// template's own value.
    ///
    /// # Invariant
    ///
    /// The template is recognised by its **value**: the marker at the value slot
    /// is the struct one, and only the template carries it.
    fn is_struct_template(&self, node: AnyNodeId) -> bool {
        self.module.node_value(node) == Some(P::Value::type_struct_marker())
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
    /// was written. A position holding the attribute's **default** reads as
    /// unspelled too ([`AttrExt::holds_default`]).
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
            let ext = attrs(marker);
            if ext.holds_default(self.module, node) {
                continue;
            }
            if let Some(spelling) = ext.render(self.module, node, attrs) {
                out.push(spelling);
            }
        }
        out
    }

    /// An element slot's value position, when it holds a **pin** rather than the
    /// wildcard.
    ///
    /// # Invariant
    /// The wildcard is **one shared node**, so an apply's clone mints a fresh
    /// undecided cell in its place — a slot read through a call therefore shows a
    /// hole, not `@any`. A hole is no pin either, so the position is suppressed
    /// the same way, and only a written `v : T` prints.
    fn slot_value_any(&self, slot: AnyNodeId) -> Option<AnyNodeId> {
        let LowValue::Array(array) = self.module.node_value(slot)?.as_enum()? else {
            return None;
        };
        // SAFETY: `array` is the payload of the value read from the live node
        // `slot`.
        let value = unsafe { array.items() }.first()?.node;
        if self.module.node_value(value).is_none() {
            return None;
        }
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
                // A frozen function value's type is the shared marker too.
                self.static_elements(sref, items, visiting)
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
        // The **struct template**: the self-referential `[StructType, ↺]` a
        // struct value's type is.
        if elements.len() >= 2 && self.is_struct_template(elements[0].node) {
            return "Struct".to_string();
        }
        // `[head, K]` — an atomic type: the kind slot is the universe, so
        // render the head (`int`, `Type`, `Tuple`, …).
        if elements.len() >= 2 && self.is_static_universe(elements[1].node) {
            return self.static_any(elements[0].node, visiting);
        }
        // An expr carrying a **function** in its value slot reads as that
        // function's signature.
        if elements.len() >= 2
            && let Some(fv) = self.module.node_value(elements[0].node)
            && let Some(LowValue::Function(fid)) = fv.as_enum()
            && let Some((dom, cod)) = self.function_signature(fid)
        {
            return format!("{dom} => {cod}");
        }
        // `[shape, [marker, K]]` — a compound type: the kind's marker decides
        // how the shape reads.
        if elements.len() >= 2
            && let Some(kind) = self.module.node_value(elements[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = unsafe { kind.items() }
            && kind.len() >= 2
            && self.is_static_universe(kind[1].node)
        {
            match self.module.node_value(kind[0].node) {
                Some(m) if m == P::Value::function_type_marker() => {
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() >= 2
                    {
                        return format!(
                            "{} => {}",
                            self.static_any(s[0].node, visiting),
                            self.static_any(s[1].node, visiting)
                        );
                    }
                }
                Some(m) if m == P::Value::set_type_marker() => {
                    // A set has no length, so its one position is the element
                    // slot.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() == 1
                    {
                        return format!(
                            "set{{{}}}",
                            match self.slot_type_any(s[0].node) {
                                Some(ty) => self.static_any(ty, visiting),
                                None => self.static_any(s[0].node, visiting),
                            }
                        );
                    }
                }
                Some(m) if m == P::Value::table_type_marker() => {
                    // shape = [key slot, value slot] — render `table{K, V}`.
                    if let Some(shape) = self.module.node_value(elements[0].node)
                        && let Some(LowValue::Array(shape)) = shape.as_enum()
                        // SAFETY: `shape` is the payload of a value read from
                        // the live node `elements[0]`.
                        && let s = unsafe { shape.items() }
                        && s.len() >= 2
                    {
                        return format!(
                            "table{{{}, {}}}",
                            match self.slot_type_any(s[0].node) {
                                Some(ty) => self.static_any(ty, visiting),
                                None => self.static_any(s[0].node, visiting),
                            },
                            match self.slot_type_any(s[1].node) {
                                Some(ty) => self.static_any(ty, visiting),
                                None => self.static_any(s[1].node, visiting),
                            }
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

    /// Whether a **static** ref's value is the universe `[Type, ↺]`, whose head
    /// is the `Type` marker, not a containing kind.
    ///
    /// # Invariant
    ///
    /// The tail is the universe too, but need not be this very node: a kind read
    /// from a frozen module is a replica whose items are refs into the writing
    /// module, so its tail names that module's canonical self-loop.  Reading
    /// through the ref lets a `[shape, [Type, ↺]]` expr that crossed a module
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
            if items.len() < 2
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
}

/// The number of *value* slots an array's payload states: an attribute region is
/// not part of the value's structure.
fn logical_width<P: HighProgram>(module: &Module<P>, node: NodeId, len: usize) -> usize
where
    P::Value: ValueType,
{
    match module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|value| value.as_enum())
    {
        Some(LowValue::Array(array)) if array.layout() != 0 => shape::EXPR_ATTR_BASE.min(len),
        _ => len,
    }
}
