//! [`ValuePrinter`]'s methods.  The struct itself is in the parent module,
//! which is where its state is documented.

use super::*;

/// The compound layouts a type can name.
///
/// # Invariant
///
/// The tuple, the array and the struct are **layout templates**: a value of one
/// of them has the shared template as its type, and the template's marker names
/// the layout.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Layout {
    /// The shared `Tuple` template.
    Tuple,
    /// The shared `Array` template.
    Array,
    /// The shared `Struct` template.
    Struct,
}

impl<'a, P: HighProgram + 'static> ValuePrinter<'a, P>
where
    P::Value: ValueType,
{
    pub fn new(module: &'a Module<P>) -> Self {
        Self::new_with_ext(module, None)
    }

    /// Spell element slots' attributes through `attrs`; see
    /// [`TypePrinter::show_attributes`].
    pub fn show_attributes(&mut self, attrs: Attrs<'a, P>) {
        self.printer.show_attributes(attrs);
    }

    /// A printer that renders an extension vocabulary's variants through
    /// `render_ext`; see [`TypePrinter::new_with_ext`].
    pub fn new_with_ext(
        module: &'a Module<P>,
        render_ext: Option<RenderExt<'a, P::Value>>,
    ) -> Self {
        ValuePrinter {
            module,
            printer: TypePrinter::new_with_ext(module, render_ext),
            path: AncestorNodes::new(),
            tpath: AncestorNodes::new(),
            raw_path: AncestorNodes::new(),
        }
    }

    /// Render the runtime value `value`, whose type is `ty`.
    pub fn print(&mut self, value: P::Value, ty: NodeId) -> String {
        self.value(value, ty)
    }

    /// The type under the class names [`Self::print`] already assigned.
    pub fn type_node(&mut self, node: NodeId) -> String {
        self.printer.node(node)
    }

    /// Render `value`, whose type is `ty`: the type chain decides how it reads.
    ///
    /// # Invariant
    ///
    /// When the type chain is opaque the raw layout is the fallback, marking
    /// every reading it dumps (`raw[…]`, `raw Int`).
    fn value(&mut self, value: P::Value, ty: NodeId) -> String {
        self.value_any(value, AnyNodeId::Dynamic(ty))
    }

    /// [`Self::value`] for a type node that may be a frozen module's.
    ///
    /// # Invariant
    ///
    /// A static type is read through the module it belongs to, exactly as a dynamic one is:
    /// only the *node* is frozen, so the same cascade decides, and the value's cells are
    /// static refs this path meets in turn.
    fn value_any(&mut self, value: P::Value, ty: AnyNodeId) -> String {
        if self.printer.is_universe_any(ty) {
            return self.atomic(value);
        }
        // The `Struct` template constant: the value is the marker, and the
        // constant reads as its name.
        if value == P::Value::type_struct_marker() {
            return self
                .printer
                .type_constant(&value)
                .unwrap_or_else(|| "?".to_string());
        }
        // The layout templates: the type names the layout, and the value
        // carries the shape.
        match self.layout_any(ty) {
            Some(Layout::Tuple) => return self.tuple_value(value),
            Some(Layout::Array) => return self.array_value(value),
            Some(Layout::Struct) => return self.struct_value(value),
            None => {}
        }
        let Some(type_value) = self.module.node_value(ty) else {
            // One hole, not a disagreement: the cell's class name is the value's
            // own spelling and the type print reuses it.
            return match ty {
                AnyNodeId::Dynamic(node) => self.printer.class_name(node),
                AnyNodeId::Static(sref) => self.printer.static_class_name(sref),
            };
        };
        let Some(LowValue::Array(ty_array)) = type_value.as_enum() else {
            return self.raw(value);
        };
        // SAFETY: `ty_array` is the payload of the value read from the live
        // node `ty`.
        let tys = unsafe { ty_array.items() };
        // A kind `[marker, K]`: the value is a compound type — render its
        // shape in type syntax.
        if tys.len() >= 2
            && self.printer.is_universe_any(tys[1].node)
            && let Some(out) = self.compound_type(value, tys[0].node)
        {
            return out;
        }
        // A term of a set or table type `[shape, [marker, K]]`: the value's
        // elements read against the shape.
        if tys.len() >= 2
            && let Some(kind) = self.module.node_value(tys[1].node)
            && let Some(LowValue::Array(kind)) = kind.as_enum()
            && let kind = unsafe { kind.items() }
            && kind.len() >= 2
            && self.printer.is_universe_any(kind[1].node)
            && let Some(marker) = self.module.node_value(kind[0].node)
            && let Some(out) = self.instance(value, tys[0].node, marker)
        {
            return out;
        }
        // A leaf class: the type is two cells.
        if let Some(spelling) = self.leaf_class(value, ty, tys) {
            return spelling;
        }
        self.raw(value)
    }

    /// The layout a type node names, read from its **marker**.
    ///
    /// # Invariant
    ///
    /// The template's value slot holds the marker that names the layout, and
    /// only the template carries it; a type that names no layout is a scalar or
    /// one of the kinds that still carry a shape.
    fn layout_any(&self, ty: AnyNodeId) -> Option<Layout> {
        let value = self.module.node_value(ty)?;
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return None;
        };
        // SAFETY: `array` is the payload of the value read from the live node
        // `ty`.
        let items = unsafe { array.items() };
        let marker = self.module.node_value(items.first()?.node)?;
        if marker == P::Value::tuple_type_marker() {
            Some(Layout::Tuple)
        } else if marker == P::Value::array_type_marker() {
            Some(Layout::Array)
        } else if marker == P::Value::type_struct_marker() {
            Some(Layout::Struct)
        } else {
            None
        }
    }

    /// A **tuple value**: the elements are the value's own items.
    ///
    /// # Invariant
    ///
    /// A tuple reads `(v1, ..., vn)` with a single element keeping its trailing
    /// comma, the source spelling of a one-tuple.
    fn tuple_value(&mut self, value: P::Value) -> String {
        let Some(items) = self.compound_items(value) else {
            return self.raw(value);
        };
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(self.element_expr(item.node));
        }
        self.parens(&out)
    }

    /// An **array value**: the element template holds the first slot, the
    /// elements the rest.
    ///
    /// # Invariant
    ///
    /// The template is internal — it names the layout and is never user data —
    /// so an array reads `[v1, ..., vn]` over its elements alone.
    fn array_value(&mut self, value: P::Value) -> String {
        let Some(items) = self.compound_items(value) else {
            return self.raw(value);
        };
        let elements = items
            .get(shape::ARRAY_VALUE_ELEMENT_BASE..)
            .unwrap_or_default();
        let mut out = Vec::with_capacity(elements.len());
        for item in elements {
            out.push(self.element_expr(item.node));
        }
        format!("[{}]", out.join(", "))
    }

    /// A **struct value**: the name table and the nominal id hold the two
    /// header slots, the fields the rest.
    ///
    /// # Invariant
    ///
    /// The header is internal — it names the struct and is never user data — so
    /// a struct reads as its field tuple.
    fn struct_value(&mut self, value: P::Value) -> String {
        let Some(items) = self.compound_items(value) else {
            return self.raw(value);
        };
        let fields = items
            .get(shape::STRUCT_VALUE_FIELD_BASE..)
            .unwrap_or_default();
        let mut out = Vec::with_capacity(fields.len());
        for item in fields {
            out.push(self.element_expr(item.node));
        }
        self.parens(&out)
    }

    /// The items of a compound value.
    fn compound_items(&self, value: P::Value) -> Option<&'static [ArrayItem]> {
        let Some(LowValue::Array(array)) = value.as_enum() else {
            return None;
        };
        // SAFETY: `array` is the payload of a value of the module being rendered.
        Some(unsafe { array.items() })
    }

    /// A leaf class named by `ty`, read as the cells `tys`; `None` otherwise.
    ///
    /// # Invariant
    ///
    /// A leaf class is two cells whose **class slot** names what it holds: a
    /// function type is a `Function` value and closes on itself
    /// (`[Function(fid), ↺]`), every other class a type constant whose kind slot
    /// is the self-looping universe (`[Int, ↺]`, `[string, ↺]`, …).
    fn leaf_class(&mut self, value: P::Value, ty: AnyNodeId, tys: &[ArrayItem]) -> Option<String> {
        if tys.len() < 2 {
            return None;
        }
        let class = self.module.node_value(tys[0].node)?;
        // The class slot distinguishes a function type from a type constant.
        let leaf = if matches!(class.as_enum(), Some(LowValue::Function(_))) {
            self.printer.slot1_is_self(tys[1].node, ty)
        } else {
            self.printer.is_universe_any(tys[1].node)
        };
        if !leaf {
            return None;
        }
        self.spelling(value)
    }

    /// An atomic type constant: a type expression's value whose type is the
    /// universe.
    ///
    /// # Invariant
    ///
    /// A value this vocabulary has no name for reads `?`: the chain *did* read
    /// it as a type constant, so the raw mark would claim the opposite.  A
    /// structural value typed by the universe is no type constant at all, and
    /// falls back to the raw layout.
    fn atomic(&mut self, value: P::Value) -> String {
        if matches!(value.as_enum(), Some(LowValue::Any)) {
            return "@any".to_string();
        }
        match self.printer.type_constant(&value) {
            Some(spelling) => spelling,
            None if value.as_enum().is_some() => self.raw(value),
            None => "?".to_string(),
        }
    }

    /// A compound type value: the value is the shape, and the marker decides how
    /// it reads.
    ///
    /// # Invariant
    ///
    /// The shape is `[in, out]` for a function type, the element **slot** alone
    /// for a set type, and the key and value **slots** for a table type.
    /// `None` when the value or marker does not fit.
    fn compound_type(&mut self, value: P::Value, marker_node: AnyNodeId) -> Option<String> {
        let marker = self.module.node_value(marker_node)?;
        let Some(LowValue::Array(shape)) = value.as_enum() else {
            return None;
        };
        // SAFETY: `shape` is the payload of the value being printed, which
        // belongs to the module being rendered.
        let shape = unsafe { shape.items() };
        if marker == P::Value::function_type_marker() {
            if shape.len() == 2 {
                return Some(format!(
                    "{} => {}",
                    self.printer.any_node(shape[0].node),
                    self.printer.any_node(shape[1].node)
                ));
            }
        } else if marker == P::Value::set_type_marker() && shape.len() == 1 {
            // A set type value: the shape is the element **slot** alone.
            return Some(format!("set{{{}}}", self.printer.slot_any(shape[0].node)));
        } else if marker == P::Value::table_type_marker() && shape.len() == 2 {
            // A table type value: the shape is the key and value **slots**.
            return Some(format!(
                "table{{{}, {}}}",
                self.printer.slot_any(shape[0].node),
                self.printer.slot_any(shape[1].node)
            ));
        }
        // The tuple and the array have no type value: their type is the shared
        // template, and the value carries the layout.
        None
    }

    /// A table instance: the handle's entries, `table(k ==> v, …)`.
    ///
    /// # Invariant
    /// The shape is `[key slot, value slot]`, so an entry's key and value read
    /// against their slots' type positions; a table with no entry reads
    /// `table()`, and an undecided payload is not a table at all.
    fn table_instance(&mut self, value: P::Value, shape: &[ArrayItem]) -> Option<String> {
        let Some(LowValue::Table(table)) = value.as_enum() else {
            return None;
        };
        if shape.len() != 2 {
            return None;
        }
        let key_ty = self
            .printer
            .slot_type_any(shape[0].node)
            .unwrap_or(shape[0].node);
        let value_ty = self
            .printer
            .slot_type_any(shape[1].node)
            .unwrap_or(shape[1].node);
        // SAFETY: `table` is the payload of the value being printed, whose
        // entries live in the module being rendered.
        let items = unsafe { table.items() };
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let key = self.element_any(item.key, key_ty);
            let entry = self.element_any(item.value, value_ty);
            entries.push(format!("{key} ==> {entry}"));
        }
        Some(format!("table({})", entries.join(", ")))
    }

    /// A term of a set or table type: the elements read against the shape.
    ///
    /// # Invariant
    ///
    /// A set reads `set(v1, ..., vn)` and a table its entries; `None` when the
    /// value or the shape does not fit.
    fn instance(
        &mut self,
        value: P::Value,
        shape_node: AnyNodeId,
        marker: P::Value,
    ) -> Option<String> {
        let shape = self.module.node_value(shape_node).and_then(|v| v.as_enum());
        let Some(LowValue::Array(shape)) = shape else {
            return None;
        };
        // SAFETY: `shape` is the payload of the shape node, in the module being
        // rendered.
        let shape = unsafe { shape.items() };
        // A table's value is a **handle**, not an element list, so it renders
        // from its entries before the list decoding below.
        if marker == P::Value::table_type_marker() {
            return self.table_instance(value, shape);
        }
        let Some(LowValue::Array(values)) = value.as_enum() else {
            return None;
        };
        // SAFETY: `values` is the payload of the value being printed, which
        // belongs to the module being rendered.
        let values = unsafe { values.items() };
        if marker == P::Value::set_type_marker() {
            // The shape is the element **slot** alone (a set has no length), so
            // every member reads against its type position.
            if shape.len() != 1 {
                return None;
            }
            let element_ty = self
                .printer
                .slot_type_any(shape[0].node)
                .unwrap_or(shape[0].node);
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                let (value, ty) = self.element_slot(v.node, element_ty);
                out.push(self.element_any(value, ty));
            }
            return Some(format!("set({})", out.join(", ")));
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
    ///
    /// # Invariant
    /// A frozen value reads against the type chain like a live one: its cells are static
    /// refs, and each is met here in turn, so a reused cell prints as its form rather than as
    /// a raw dump. The cycle guards cover both halves: a dynamic value node by id, a type node
    /// by its absolute ref.
    /// A compound's element: the value and type its slot states.
    fn element_slot(&self, element: AnyNodeId, shape_ty: AnyNodeId) -> (AnyNodeId, AnyNodeId) {
        let Some(LowValue::Array(array)) =
            self.module.node_value(element).and_then(|v| v.as_enum())
        else {
            return (element, shape_ty);
        };
        if array.layout() == 0 {
            return (element, shape_ty);
        }
        // SAFETY: `array` is the payload of a value of the module being rendered.
        let items = unsafe { array.items() };
        let value = items.first().map(|item| item.node).unwrap_or(element);
        // **An undecided type half says nothing**: the container's element type is
        // the class the array declares.
        let ty = items
            .get(shape::EXPR_TYPE_SLOT)
            .map(|item| item.node)
            .filter(|node| self.module.node_value(*node).is_some())
            .unwrap_or(shape_ty);
        (value, ty)
    }

    /// An element **expr**: its own value and type.
    ///
    /// # Invariant
    ///
    /// An element is a complete expression: the read's value is the element's
    /// own value slot and its type the element's own type slot, so an element's
    /// type and attributes are read from the element.
    pub(super) fn element_expr(&mut self, expr: AnyNodeId) -> String {
        let (value, ty) = self.element_expr_slots(expr);
        self.render_element(value, ty)
    }

    /// An element expr's own value and type, read from its two slots.
    ///
    /// # Invariant
    ///
    /// The element states its own type, so an undecided type half says nothing
    /// and the element reads as the class the printer names.  A **binding
    /// reference** is the one other shape: a cycle-cut skeleton holds the
    /// binding's own value and type cells, and the element reads through them.
    fn element_expr_slots(&self, expr: AnyNodeId) -> (AnyNodeId, AnyNodeId) {
        let Some(LowValue::Array(array)) = self.module.node_value(expr).and_then(|v| v.as_enum())
        else {
            return (expr, expr);
        };
        if array.layout() == 0 {
            // A skeleton: the binding's own `[value, type]` cells.
            // SAFETY: the payload of a value of the module being rendered.
            let items = unsafe { array.items() };
            if items.len() == 2 {
                return (items[0].node, items[1].node);
            }
            return (expr, expr);
        }
        // SAFETY: `array` is the payload of a value of the module being rendered.
        let items = unsafe { array.items() };
        let value = items.first().map(|item| item.node).unwrap_or(expr);
        let ty = items
            .get(shape::EXPR_TYPE_SLOT)
            .map(|item| item.node)
            .unwrap_or(expr);
        (value, ty)
    }

    pub(super) fn element_any(&mut self, id: AnyNodeId, ty: AnyNodeId) -> String {
        // A compound's element is a slot: print its value against its own type.
        let (id, ty) = self.element_slot(id, ty);
        self.render_element(id, ty)
    }

    /// The guarded reading of an element: its value against its own type.
    ///
    /// # Invariant
    ///
    /// A frozen value reads against the type chain like a live one: its cells
    /// are static refs, and each is met here in turn, so a reused cell prints
    /// as its form rather than as a raw dump.  The cycle guards cover both
    /// halves: a dynamic value node by id, a type node by its absolute ref.
    fn render_element(&mut self, id: AnyNodeId, ty: AnyNodeId) -> String {
        // The value guard holds the value's **class**, the equality the
        // encoding gives it; a layout template holds no type path.
        let recurs = self.layout_any(ty).is_none();
        if recurs && self.tpath.contains(ty) {
            return "…".to_string();
        }
        let pinned = match id {
            AnyNodeId::Dynamic(node) => {
                // A node the walk cannot place stands for itself.
                let class = representative(self.module, node).unwrap_or(node);
                if self.path.contains(class) {
                    return "…".to_string();
                }
                self.path.insert(class);
                Some(class)
            }
            AnyNodeId::Static(_) => None,
        };
        if recurs {
            self.tpath.insert(ty);
        }
        let value = self
            .module
            .node_value(id)
            .unwrap_or_else(|| P::Value::from(LowValue::None));
        let out = self.value_any(value, ty);
        if recurs {
            self.tpath.remove(ty);
        }
        if let Some(class) = pinned {
            self.path.remove(class);
        }
        out
    }

    /// The raw value layout: the fallback for a value the type chain named no
    /// class for.
    ///
    /// # Invariant
    ///
    /// Every reading is marked `raw`: a cell list as `raw[…]`, an atomic as
    /// `raw x`.  The mark is unconditional, because a dump spelled like a read
    /// one would be indistinguishable from it though the two sit behind
    /// different structures.  See docs/notes/raw-rendering-mark.md.
    fn raw(&mut self, value: P::Value) -> String {
        self.raw_any(value)
    }

    /// [`Self::raw`] for a value whose array items may be static refs.
    fn raw_any(&mut self, value: P::Value) -> String {
        if let Some(LowValue::Array(array)) = value.as_enum() {
            // SAFETY: `array` is the payload of `value`, a value of the module
            // being rendered.
            return self.raw_cells(unsafe { array.items() });
        }
        // The array, the one value with no spelling, is handled above, so `?`
        // here means unnameable.
        let atomic = self.spelling(value).unwrap_or_else(|| "?".to_string());
        format!("raw {atomic}")
    }

    /// The raw reading of a list's cells: `raw[…]`, each cell dumped through
    /// [`Self::raw_any`] in its turn.
    fn raw_cells(&mut self, elements: &[ArrayItem]) -> String {
        let mut out = Vec::with_capacity(elements.len());
        for item in elements {
            // A cell already on the dump path is a cycle and reads as `…`.
            if self.raw_path.contains(item.node) {
                out.push("…".to_string());
                continue;
            }
            let child = self
                .module
                .node_value(item.node)
                .unwrap_or_else(|| P::Value::from(LowValue::None));
            self.raw_path.insert(item.node);
            out.push(self.raw_any(child));
            self.raw_path.remove(item.node);
        }
        format!("raw[{}]", out.join(", "))
    }

    /// The value's own spelling, or `None` for an array (its cells read).
    ///
    /// # Invariant
    ///
    /// This is the **standard** reading wherever the type chain named a class
    /// for the value ([`Self::leaf_class`]), and the marked content of a dump
    /// where it did not.
    fn spelling(&mut self, value: P::Value) -> Option<String> {
        match value.as_enum() {
            Some(LowValue::USize(n)) => Some(n.to_string()),
            Some(LowValue::Float(value)) => Some(float_literal(value)),
            Some(LowValue::Str(s)) => Some(format!("\"{s}\"")),
            Some(LowValue::Function(fid)) => Some(self.printer.function_spelling(fid)),
            Some(LowValue::Table(_)) => Some("Table".to_string()),
            Some(LowValue::None) | Some(LowValue::Error) => Some("none".to_string()),
            Some(LowValue::Any) => Some("@any".to_string()),
            Some(LowValue::Array(_)) => None,
            None => Some(
                self.printer
                    .type_constant(&value)
                    .unwrap_or_else(|| "?".to_string()),
            ),
        }
    }
}
