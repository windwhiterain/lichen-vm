//! [`ValuePrinter`]'s methods.  The struct itself is in the parent module,
//! which is where its state is documented.

use super::*;
impl<'a, P: HighProgram> ValuePrinter<'a, P>
where
    P::Value: ValueType,
{
    pub fn new(module: &'a Module<P>) -> Self {
        Self::new_with_ext(module, None)
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

    /// Render `value`, whose type is `ty`: the type chain decides how it reads.
    ///
    /// # Invariant
    ///
    /// When the type chain is opaque the raw layout is the fallback, marking
    /// every reading it dumps (`raw[…]`, `raw Int`).
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
        // SAFETY: `ty_array` is the payload of the value read from the live
        // node `ty`.
        let tys = unsafe { ty_array.items() };
        // A struct kind: the type node is `[[payload, TypeStruct], K]`, and the
        // value is the field-type list.
        if is_struct_kind(self.module, ty)
            && let Some(LowValue::Array(shape)) = value.as_enum()
        {
            // SAFETY: `shape` is the payload of the value being printed,
            // which belongs to the module being rendered.
            let fields: Vec<String> = unsafe { shape.items() }
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
        // A struct instance: the value reads against the kind's field-type
        // shape, whose kind slot is the struct marker.
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
            && let kind = unsafe { kind.items() }
            && kind.len() == 2
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

    /// A leaf class named by `ty`, read as the cells `tys`; `None` otherwise.
    ///
    /// # Invariant
    ///
    /// A leaf class is two cells whose **class slot** names what it holds: a
    /// function type is a `Function` value and closes on itself
    /// (`[Function(fid), ↺]`), every other class a type constant whose kind slot
    /// is the self-looping universe (`[Int, ↺]`, `[string, ↺]`, …).
    fn leaf_class(&self, value: P::Value, ty: NodeId, tys: &[ArrayItem]) -> Option<String> {
        if tys.len() != 2 {
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
    /// The shape is `[in, out]`, an element list, `[element, length]`, or
    /// `[TypeId, fields]`.  `None` when the value or marker does not fit.
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
                "array<{}, {}>",
                self.printer.any_node(shape[0].node),
                self.printer.any_node(shape[1].node)
            ));
        } else if marker == P::Value::set_type_marker() && shape.len() == 1 {
            // A set type value: the shape is the element type alone.
            return Some(format!("set<{}>", self.printer.any_node(shape[0].node)));
        }
        // A struct type never reaches here: its kind is the struct marker pair,
        // handled earlier.
        None
    }

    /// A term of a tuple, array, or struct type: the elements read against the
    /// shape.
    ///
    /// # Invariant
    ///
    /// A tuple reads `(v1, ..., vn)` with a single element keeping its trailing
    /// comma, an array `[v1, ..., vn]`, a struct instance its field tuple.
    /// `None` when the value or the shape does not fit.
    fn instance(
        &mut self,
        value: P::Value,
        shape_node: AnyNodeId,
        marker: P::Value,
    ) -> Option<String> {
        let Some(LowValue::Array(values)) = value.as_enum() else {
            return None;
        };
        // SAFETY: `values`/`shape` are payloads of the value and shape node,
        // both in the module being rendered.
        let values = unsafe { values.items() };
        let shape = self.module.node_value(shape_node).and_then(|v| v.as_enum());
        let Some(LowValue::Array(shape)) = shape else {
            return None;
        };
        let shape = unsafe { shape.items() };
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
        if marker == P::Value::set_type_marker() {
            // The shape is the element type *alone* (a set has no length), so
            // every member reads against `shape[0]`.
            if shape.len() != 1 {
                return None;
            }
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                out.push(self.element_any(v.node, shape[0].node));
            }
            return Some(format!("set{{{}}}", out.join(", ")));
        }
        // `[payload, TypeStruct]`: checking the tag, not the shape, keeps
        // another kind's value from reading as a struct.
        if self.marker_is_struct(marker) {
            // The shape is the positional field-type list.
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
    pub(super) fn element_any(&mut self, id: AnyNodeId, ty: AnyNodeId) -> String {
        match (id, ty) {
            (AnyNodeId::Dynamic(id), AnyNodeId::Dynamic(ty)) => {
                if self.path.contains(id) || self.tpath.contains(ty) {
                    return "…".to_string();
                }
                self.path.insert(id);
                self.tpath.insert(ty);
                let value = self
                    .module
                    .node_value(AnyNodeId::Dynamic(id))
                    .unwrap_or_else(|| P::Value::from(LowValue::None));
                let out = self.value(value, ty);
                self.tpath.remove(ty);
                self.path.remove(id);
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

    /// Whether an `AnyNodeId` names a struct kind `[[payload, TypeStruct], K]`.
    fn is_struct_kind_any(&self, id: AnyNodeId) -> bool {
        self.module
            .node_value(id)
            .and_then(|v| v.as_enum())
            .is_some_and(|v| match v {
                // SAFETY: `kind` is the payload of the value read from the
                // live node `id`.
                LowValue::Array(kind) => kind_is_struct(self.module, unsafe { kind.items() }),
                _ => false,
            })
    }

    /// Whether a marker value is a struct marker: `[payload, TypeStruct]`.
    fn marker_is_struct(&self, marker: P::Value) -> bool {
        let Some(LowValue::Array(m)) = marker.as_enum() else {
            return false;
        };
        // SAFETY: `m` is the payload of a value read from the module being
        // rendered.
        let items = unsafe { m.items() };
        items.len() == 2
            && self
                .module
                .node_value(items[shape::STRUCT_MARKER_TAG_SLOT].node)
                == Some(P::Value::type_struct_marker())
    }

    /// The struct marker value from a struct type's kind node, or `None` when
    /// the kind is not a struct kind.
    fn struct_marker_value(&self, kind_node: AnyNodeId) -> Option<P::Value> {
        let Some(LowValue::Array(kind)) =
            self.module.node_value(kind_node).and_then(|v| v.as_enum())
        else {
            return None;
        };
        // SAFETY: `kind` is the payload of the value read from the live node
        // `kind_node`.
        let marker = self
            .module
            .node_value(unsafe { kind.items() }.first()?.node)?;
        self.marker_is_struct(marker).then_some(marker)
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
    fn spelling(&self, value: P::Value) -> Option<String> {
        match value.as_enum() {
            Some(LowValue::USize(n)) => Some(n.to_string()),
            Some(LowValue::Float(value)) => Some(float_literal(value)),
            Some(LowValue::Str(s)) => Some(format!("\"{s}\"")),
            Some(LowValue::Function(_)) => Some("Function".to_string()),
            Some(LowValue::Table(_)) => Some("Table".to_string()),
            Some(LowValue::None) | Some(LowValue::Error) => Some("none".to_string()),
            Some(LowValue::Array(_)) => None,
            None => Some(
                self.printer
                    .type_constant(&value)
                    .unwrap_or_else(|| "?".to_string()),
            ),
        }
    }
}
