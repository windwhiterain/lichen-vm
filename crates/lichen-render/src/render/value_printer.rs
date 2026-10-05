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
            raw_path: AncestorNodes::new(),
        }
    }

    /// Render the runtime value `value`, whose type is `ty`.
    pub fn print(&mut self, value: P::Value, ty: NodeId) -> String {
        self.value(value, ty)
    }

    /// Render a value against its type: the type chain decides how the value
    /// reads.  When the type chain is opaque, fall back to the raw layout,
    /// which marks every reading it dumps (`raw[…]`, `raw Int`).
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
        // `[[payload, TypeStruct], K]` (not a `[shape, [marker, K]]` pair),
        // and the value is the field-type list — render
        // `struct<.a T1, ..., .n Tn>` (a struct field always carries a name).
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
        // field-type list (the shape); its kind is `[[payload, TypeStruct], K]`
        // (a standard `[marker, K]` pair whose marker carries the tag), so it is
        // detected beside the `[marker, K]` kinds.
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
        // A type that names a **leaf class**: `[class, K]` whose kind slot is
        // the self-looping universe, or a function-type node
        // `[Function(fid), ↺]` whose kind slot is the node's own self-cycle.
        // The value such a type holds is a scalar or a function, and reads by
        // its own spelling (`1`, `"s"`, `Function`) — a standard format, so
        // it is **not** what the tail of this cascade dumps.  A value the
        // class does not hold (a list, say) is a mismatch and falls through.
        if let Some(spelling) = self.leaf_class(value, ty, tys) {
            return spelling;
        }
        self.raw(value)
    }

    /// [`Self::spelling`] when the type `ty` — read as the cells `tys` —
    /// names a leaf class, `None` for any other type and for a value the
    /// class does not hold.  A leaf class is a two-cell type node whose
    /// **class slot** names what it holds: a function type is named by a
    /// `Function` value and closes on itself (`[Function(fid), ↺]`), every
    /// other class by a type constant whose kind slot is the self-looping
    /// universe (`[Int, ↺]`, `[string, ↺]`, …).
    fn leaf_class(&self, value: P::Value, ty: NodeId, tys: &[ArrayItem]) -> Option<String> {
        if tys.len() != 2 {
            return None;
        }
        let class = self.module.node_value(tys[0].node)?;
        // The **class slot** says which kind of leaf this is: a function type
        // is named by a `Function` value and its kind slot is the node's own
        // self-cycle; every other class is a type constant whose kind slot is
        // the universe — read through the ref, so a class frozen in another
        // module counts too.
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

    /// An atomic type constant: the value of a type expression whose type is
    /// the universe.  A value this vocabulary has no name for is `?`: the
    /// chain *did* read it as a type constant, so the mark would claim the
    /// opposite.  A structural value typed by the universe (the universe node
    /// itself) is no type constant at all, and falls back to the raw layout.
    fn atomic(&mut self, value: P::Value) -> String {
        match self.printer.type_constant(&value) {
            Some(spelling) => spelling,
            None if value.as_enum().is_some() => self.raw(value),
            None => "?".to_string(),
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
        } else if marker == P::Value::set_type_marker() && shape.len() == 1 {
            // A set type value: the shape is the element type alone.
            return Some(format!("set<{}>", self.printer.any_node(shape[0].node)));
        }
        // A struct type never reaches `compound_type` — its kind is a standard
        // `[marker, K]` pair whose marker is the `[payload, TypeStruct]` pair,
        // and the struct branch in `value` / `elements` handles it
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
        // A struct marker is the pair `[payload, TypeStruct]`, whose *type* slot
        // holds the `TypeStruct` atom.  Checking the tag (never the array's
        // shape) is what keeps another kind's array-like value from being read
        // as a struct.
        if self.marker_is_struct(marker) {
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

    /// Whether a marker *value* is a struct marker: the pair
    /// `[payload, TypeStruct]`, whose type slot is the `TypeStruct` atom.
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

    /// The struct marker value (`[payload, TypeStruct]`) from
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
        self.marker_is_struct(marker).then_some(marker)
    }

    /// The raw value layout — the fallback for a value the type chain named
    /// no class for.  **Every reading here is marked `raw`**: a list of cells
    /// as `raw[…]` (the mark fuses with the list's own brackets), an atomic
    /// as `raw x` — `raw 6`, `raw Int`.  The mark is unconditional because no
    /// reading here is a form the chain explained, and a dump spelled exactly
    /// like a read one (`6`, `Int`) would be indistinguishable from that read
    /// though the two sit behind entirely different structures — see
    /// [raw-rendering-mark](../docs/notes/raw-rendering-mark.md).
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
        // An atomic reads by its own spelling; the array is the one value
        // [`Self::spelling`] has no answer for, and it is handled above, so
        // `?` here is the printer's own "this vocabulary cannot name it".
        let atomic = self.spelling(value).unwrap_or_else(|| "?".to_string());
        format!("raw {atomic}")
    }

    /// The raw reading of a list's cells: `raw[…]`, each cell dumped through
    /// [`Self::raw_any`] in its turn.
    fn raw_cells(&mut self, elements: &[ArrayItem]) -> String {
        let mut out = Vec::with_capacity(elements.len());
        for item in elements {
            // A cell this dump has already entered is a cycle, and reads as
            // `…` — the type printer's own spelling.  It is what keeps a
            // self-referential kind (`[Type, ↺]`) from unrolling forever.
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

    /// The value's own spelling: a scalar's digits, a string's quotes, a
    /// function's or a table's name, a type constant's spelling.  This is the
    /// **standard** reading wherever the type chain named a class for the
    /// value ([`Self::leaf_class`]), and the content of a dump — marked — where
    /// it did not.  `None` for an array, the one value with no single-token
    /// spelling: its cells are the reading instead.
    fn spelling(&self, value: P::Value) -> Option<String> {
        match value.as_enum() {
            Some(LowValue::USize(n)) => Some(n.to_string()),
            Some(LowValue::Float(value)) => Some(float_literal(value)),
            Some(LowValue::Str(s)) => Some(format!("\"{s}\"")),
            Some(LowValue::Function(_)) => Some("Function".to_string()),
            Some(LowValue::Table(_)) => Some("Table".to_string()),
            Some(LowValue::None) | Some(LowValue::Void) => Some("none".to_string()),
            Some(LowValue::Parameterized) => Some("parameterized".to_string()),
            Some(LowValue::Array(_)) => None,
            None => Some(
                self.printer
                    .type_constant(&value)
                    .unwrap_or_else(|| "?".to_string()),
            ),
        }
    }
}
