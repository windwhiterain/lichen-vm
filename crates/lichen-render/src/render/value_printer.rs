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

    /// A printer that renders an extension vocabulary's own variants through
    /// `render_ext` — see [`TypePrinter::new_with_ext`].
    pub fn new_with_ext(
        module: &'a Module<P>,
        render_ext: Option<RenderExt<'a, P::Value>>,
    ) -> Self {
        ValuePrinter {
            module,
            printer: TypePrinter::new_with_ext(module, render_ext),
            path: AncestorNodes::new(),
            tpath: AncestorNodes::new(),
        }
    }

    /// Render the runtime value `value`, whose type is `ty`.
    pub fn print(&mut self, value: P::Value, ty: NodeId) -> String {
        self.value(value, ty)
    }

    /// Render a value against its type: the type chain decides how the value
    /// reads.  When the type chain is opaque, fall back to the raw layout,
    /// which marks a list of cells `raw[…]`.
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
        // A struct type itself: the value's type is the struct kind
        // `[id, [TypeStruct, K], names]` (not a `[shape, [marker, K]]` pair),
        // and the value is the field-type list — render
        // `struct<T1, ..., Tn>` (or `struct<.a T1, ...>` when named).
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
            && let kind = unsafe { kind.items() }
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
        // SAFETY: `values`/`shape` are payloads of the value being printed and
        // of the shape node, both in the module being rendered.
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

    /// Whether an `AnyNodeId` names a struct kind `[id, [TypeStruct, K]]`.
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
        // SAFETY: `kind` is the payload of the value read from the live node
        // `kind_node`.
        let marker = self
            .module
            .node_value(unsafe { kind.items() }.first()?.node)?;
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
    /// (`[TypeInt, K]` → `Int`), a list of cells as `raw[…]` (the mark that
    /// says the type chain did not read it), functions `Function`, and the
    /// type constants by their spellings `Int` / `Type`.
    fn raw(&mut self, value: P::Value) -> String {
        self.raw_any(value)
    }

    /// [`Self::raw`] for a value whose array items may be static refs.
    fn raw_any(&mut self, value: P::Value) -> String {
        if let Some(structural) = value.as_enum() {
            return match structural {
                LowValue::USize(n) => n.to_string(),
                LowValue::Float(value) => float_literal(value),
                LowValue::Str(s) => format!("\"{s}\""),
                LowValue::Function(_) => "Function".to_string(),
                LowValue::Table(_) => "Table".to_string(),
                LowValue::None => "none".to_string(),
                LowValue::Void => "none".to_string(),
                LowValue::Parameterized => "parameterized".to_string(),
                LowValue::Array(array) => {
                    // SAFETY: `array` is the payload of `value`, a value of the
                    // module being rendered.
                    let elements = unsafe { array.items() };
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
                    format!("raw[{}]", out.join(", "))
                }
            };
        }
        self.printer
            .type_constant(&value)
            .unwrap_or_else(|| "?".to_string())
    }
}
