//! Structs: the struct value, the struct instance, and the two reads.

use lichen_lowlevel::{
    AnyFunctionId, AnyHandle, AnyNodeId, LowOperator, LowValue, NodeId, TableItem,
};

use lichen_utils::extend::AsEnum;

use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind};
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A positional slot read `a(k)` — a **tuple** element.
    ///
    /// # Invariant
    /// The container's type is the shared `Tuple` template, so a non-tuple
    /// fails here. The element is the container's own item at the key, and the
    /// read's type and attributes are the element's own.
    pub(super) fn check_field(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let container_ty = self.state[container].ty.unwrap();
        // A written annotation may be unevaluated.
        // Force it before deciding whether the read can be checked.
        self.module.evaluate_node_deep(container_ty, None);
        let container_value = self.value_of(container);
        // A lazy region's read states no type requirement: its expr is open.
        let lazy = self.lazy_region(container_value);
        if !lazy {
            if self.type_is_concrete(container_ty) {
                self.check_unify(
                    container_ty,
                    self.tuple_template(),
                    self.loc(container, 1),
                    DiagKind::Guard,
                );
            } else if !self.is_loop_state(container) {
                return self.refuse_undecided_container(e, container, container_ty, "tuple read");
            }
        }
        let key_value = self.value_of(key);
        self.node_edges.insert(key_value, self.loc(key, 0));
        self.read_site = Some(self.loc(e, 0));
        let position = self.constant_position(key);
        let (element, value_node, ty_node) =
            self.value_slot_read(container_value, key_value, position);
        let ty_node = if position.is_none() && !self.is_struct_layout(container_ty) {
            let branches = shape::value_items(&self.module, AnyNodeId::Dynamic(container_value));
            if let Some(items) = branches.filter(|items| items.len() == 2)
                && let Some(first) =
                    shape::value_item(&self.module, items[0].node, shape::EXPR_TYPE_SLOT)
                        .and_then(|node| node.dynamic())
                && let Some(second) =
                    shape::value_item(&self.module, items[1].node, shape::EXPR_TYPE_SLOT)
                        .and_then(|node| node.dynamic())
                && self.module.class_root(first) == self.module.class_root(second)
            {
                first
            } else {
                ty_node
            }
        } else {
            ty_node
        };
        // A lazy region's read states no attribute: its expr is the wildcard expr.
        let expr = if lazy {
            self.expr_of(value_node, ty_node)
        } else {
            self.element_expr(value_node, ty_node, element)
        };
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        // The element's attributes are the read's: the slot is the field's own.
        self.state[e].attr = self.stated_element_slot(element);
        expr
    }

    /// Whether a node holds a shallow mark's expr: two items, the first lazy.
    fn masked_expr(&self, node: NodeId) -> bool {
        let mut current = Some(node);
        while let Some(node) = current {
            if let Some(value) = self.module.node_value(AnyNodeId::Dynamic(node)) {
                let Some(value) = lichen_utils::extend::AsEnum::<LowValue>::as_enum(&value) else {
                    return false;
                };
                // SAFETY: the array is the value of a live node of this module.
                let LowValue::Array(array) = value else {
                    return false;
                };
                let items = unsafe { array.items() };
                return array.layout() != 0 && items.first().is_some_and(|item| item.shallow);
            }
            current = self.module.node_origin(node);
        }
        false
    }

    /// A lazy region's own expr: the read addresses the expr, not a slot's value.
    ///
    /// # Invariant
    /// The operands are the read's own, so both predate the read.
    fn lazy_region(&self, value: NodeId) -> bool {
        // The container's value *is* the region when it is the wrapper; a read's
        // value is the array that read resolves to.
        match self.array_of(value) {
            Some(array) => self.is_region(array) || self.masked_expr(array),
            None => self.is_region(value) || self.masked_expr(value),
        }
    }

    /// Whether a node is a wrapper this check built, through its origin chain.
    ///
    /// # Invariant
    /// A clone instantiates the template's wrapper, so the origin chain answers for it.
    fn is_region(&self, node: NodeId) -> bool {
        let mut current = Some(node);
        while let Some(node) = current {
            if self.shallow_regions.contains(&node) {
                return true;
            }
            current = self.module.node_origin(node);
        }
        false
    }

    /// The array a node's evaluated value *is*.
    ///
    /// # Invariant
    /// A read's own value is decided at evaluation, which a check may not force; its
    /// constant selection and a call's template expr are the same value without it.
    fn array_of(&self, node: NodeId) -> Option<NodeId> {
        if unsafe { self.module.array_items_of(AnyNodeId::Dynamic(node)) }.is_some() {
            return Some(node);
        }
        if let Some(selected) = self.selected_item(node)
            && unsafe { self.module.array_items_of(AnyNodeId::Dynamic(selected)) }.is_some()
        {
            return Some(selected);
        }
        // A call's value is the array its template's return expr holds.
        let function = self.module.callee_function(node)?;
        self.module
            .expr_value_half(self.module.functions[function].r#return)
    }

    /// The array item a constant read selects: `[v0, v1][k]` is `vk`.
    ///
    /// # Invariant
    /// Only a constant subscript is read through: a runtime index states nothing.
    fn selected_item(&self, node: NodeId) -> Option<NodeId> {
        let operation = self.module.node_operation(node)?;
        if !matches!(
            lichen_utils::extend::AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Index)
        ) {
            return None;
        }
        // SAFETY: a payload of a value of this module; a stale node answers `None`.
        let items = unsafe {
            self.module
                .array_items_of(AnyNodeId::Dynamic(operation.operand?))
        }?;
        let container = items
            .first()
            .map(|item| item.node)
            .and_then(|node| node.dynamic())?;
        let key = items.get(1)?.node.dynamic()?;
        let position = self.module.usize_value(key)?;
        // The container's array, through the node, a constant read, or a call's expr.
        let array = self.array_of(container)?;
        let values = unsafe { self.module.array_items_of(AnyNodeId::Dynamic(array)) }?;
        values
            .get(position as usize)
            .map(|item| item.node)
            .and_then(|node| node.dynamic())
    }

    /// A named field read `a.name`, resolved through the value's own name
    /// table.
    ///
    /// # Invariant
    /// The name table is the value's first slot and the field is at the name's
    /// position plus the two header slots, so the layout is read from the value
    /// and never from a type.  A missing field is reported with the container's
    /// **value** in `a` — where the names live — and its type in `b`.
    pub(super) fn check_named_field(
        &mut self,
        e: ExprId,
        container: ExprId,
        name: &'static str,
    ) -> NodeId {
        self.check_expr(container);
        let container_ty = self.state[container].ty.unwrap();
        self.module.evaluate_node_deep(container_ty, None);
        let concrete = self.type_is_concrete(container_ty);
        if !concrete && !self.is_loop_state(container) {
            return self.refuse_undecided_container(e, container, container_ty, "field read");
        }
        if concrete {
            self.check_unify(
                container_ty,
                self.struct_template(),
                self.loc(container, 1),
                DiagKind::Guard,
            );
        }
        let container_value = self.value_of(container);
        self.read_site = Some(self.loc(e, 0));
        // The field's position: the name table's own entry, when the value is
        // decided.
        let table = self.struct_name_table(container_value);
        let position = table.and_then(|table| self.module.name_table_index(table, name));
        if table.is_some() && position.is_none() {
            // A struct without this field: the name rides in the entry, so the
            // language layer can append a did-you-mean clause.
            self.record_guard(
                container_value,
                container_ty,
                self.loc(e, 0),
                DiagKind::NamedField,
                Some(name),
            );
        }
        // The field's subscript: a constant position when the value states it,
        // else the lazy `TableGet(names, name) + 2`.
        let (index, constant) = match position {
            Some(at) => (
                self.usize_node(at + shape::STRUCT_VALUE_FIELD_BASE),
                Some(at + shape::STRUCT_VALUE_FIELD_BASE),
            ),
            None => {
                let names = self.index_of(container_value, self.zero());
                let name_node = self.name_node(name);
                let key_ops = self.array_node(self.current_block, &[names, name_node]);
                let key = self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::TableGet),
                    Some(key_ops),
                );
                self.node_edges.insert(key, self.loc(e, 0));
                let header = self.usize_node(shape::STRUCT_VALUE_FIELD_BASE);
                let plus_ops = self.array_node(self.current_block, &[key, header]);
                let index = self.op_node(
                    self.current_block,
                    P::Operator::from(TypeOperator::Add),
                    Some(plus_ops),
                );
                (index, None)
            }
        };
        let (element, value_node, ty_node) = self.value_slot_read(container_value, index, constant);
        let expr = self.element_expr(value_node, ty_node, element);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        // The element's attributes are the read's: the slot is the field's own.
        self.state[e].attr = self.stated_element_slot(element);
        expr
    }

    /// The name table a struct value holds at its first slot.
    pub(super) fn struct_name_table(
        &self,
        container_value: NodeId,
    ) -> Option<AnyHandle<[TableItem]>> {
        let names = shape::value_item(
            &self.module,
            AnyNodeId::Dynamic(container_value),
            shape::STRUCT_VALUE_NAMES_SLOT,
        )?;
        let value = self.module.node_value(names)?.as_enum()?.clone();
        match value {
            LowValue::Table(table) => Some(table),
            _ => None,
        }
    }

    /// Whether a type cell is statically inspectable. Gates the
    /// field-read and function-ness guards.
    pub(super) fn type_is_concrete(&self, ty: NodeId) -> bool {
        self.module
            .node_value(AnyNodeId::Dynamic(self.module.class_root(ty)))
            .is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    None | Some(LowValue::USize(_)) | Some(LowValue::Array(_))
                )
            })
    }

    /// Whether a type node is the shared **struct layout**.
    ///
    /// # Invariant
    /// The layout is recognised by its **class**: a cell the shared template
    /// was unified into is that template's class, so a call result whose type
    /// cell carries the layout is the layout too. The template's own
    /// self-reference is not needed to name it.
    pub(super) fn is_struct_layout(&mut self, ty: NodeId) -> bool {
        if shape::is_struct_type_any(&mut self.module, self.type_expr, AnyNodeId::Dynamic(ty)) {
            return true;
        }
        let template = self.struct_template();
        self.module.equality_representative(ty) == self.module.equality_representative(template)
    }

    /// A struct **value** `{.a e1, .b e2}`.
    ///
    /// # Invariant
    /// The value is `[name table, nominal id, the fields' own exprs…]` and the
    /// type is the shared `Struct` template: the layout lives in the value, so
    /// a read takes a field's type and attributes from the field itself.
    pub(super) fn check_struct_value(
        &mut self,
        e: ExprId,
        fields: crate::ir::ChildRange,
        names: crate::ir::ChildRange,
    ) -> NodeId {
        let elements: Vec<ExprId> =
            self.ir.children[fields.start as usize..fields.end as usize].to_vec();
        let field_names: Vec<Option<&'static str>> =
            self.ir.struct_names[names.start as usize..names.end as usize].to_vec();
        let mut vals = Vec::with_capacity(elements.len());
        for &el in &elements {
            self.check_expr(el);
            vals.push(self.state[el].term.expect("a field is compiled"));
        }
        let (value, ty_node) = self.struct_value_node(e, &vals, &field_names);
        self.require_struct_field_names(&field_names, &elements, value);
        let expr = self.expr_of(value, ty_node);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        expr
    }

    /// Refuse a struct **value** with an unnamed field, at that field's own
    /// location (language-spec.md §Structs).
    ///
    /// # Invariant
    /// Every struct field carries a name: an instance reads by name, and the
    /// positional form is the *tuple* read. A block's fields need no check —
    /// they are its bindings.
    fn require_struct_field_names(
        &mut self,
        names: &[Option<&'static str>],
        fields: &[ExprId],
        struct_node: NodeId,
    ) {
        let Some(at) = names.iter().position(|name| name.is_none()) else {
            return;
        };
        // `fields` is index-aligned with `names`, so an unnamed field has the
        // expression that states it.
        let Some(&field) = fields.get(at) else {
            return;
        };
        self.record_guard(
            struct_node,
            struct_node,
            self.loc(field, 1),
            DiagKind::StructFieldName,
            None,
        );
    }

    /// A struct-returning block: an anonymous struct over the bindings,
    /// a fresh nominal type per occurrence.
    ///
    /// # Invariant
    /// It shares the struct's construction: the block's value is a struct value
    /// with the block's names, never a second value layout.
    pub(super) fn check_record(
        &mut self,
        e: ExprId,
        value: ExprId,
        field_names: &[Option<&'static str>],
    ) -> NodeId {
        self.check_expr(value);
        let elements = self.range_children(value);
        let mut vals = Vec::with_capacity(elements.len());
        let mut names = Vec::with_capacity(elements.len());
        for (&el, &name) in elements.iter().zip(field_names) {
            let Some(name) = name else {
                // A bare expression: no field. Checked with the block and
                // discarded, so it is not evaluated either.
                continue;
            };
            vals.push(self.state[el].term.expect("a field expression is compiled"));
            names.push(Some(name));
        }
        let (value_node, ty_node) = self.struct_value_node(e, &vals, &names);
        let expr = self.expr_of(value_node, ty_node);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        expr
    }

    /// The struct **value**'s layout: `[name table, nominal id, fields…]`.
    ///
    /// # Invariant
    /// The name table holds each name's **relative field position** (from
    /// zero), and the nominal id is the written occurrence's identity — so a
    /// clone inherits both, and only one name table is stored.
    fn struct_value_node(
        &mut self,
        occurrence: ExprId,
        fields: &[NodeId],
        names: &[Option<&'static str>],
    ) -> (NodeId, NodeId) {
        let id = self.fresh_nominal_id(occurrence);
        let name_table = self.build_struct_names(names);
        let mut items = vec![name_table, id];
        items.extend_from_slice(fields);
        let value = self.array_node(self.current_block, &items);
        (value, self.struct_template())
    }

    /// The placeholder in a callee position, including the apply inserted by
    /// the instantiation desugaring.
    fn placeholder_callee_expr(&self, template: ExprId) -> Option<ExprId> {
        match self.ir[template].kind {
            ExprKind::Placeholder => Some(template),
            ExprKind::Apply { function, .. }
                if matches!(self.ir[function].kind, ExprKind::Placeholder) =>
            {
                Some(function)
            }
            _ => None,
        }
    }

    /// The constructor function and its type, when `value` is an apply result.
    fn constructor_of(&mut self, value: NodeId) -> Option<(NodeId, NodeId)> {
        if !self.module.nodes.contains_key(value) {
            return None;
        }
        let apply = self
            .module
            .apply_result_owners
            .get(&value)
            .copied()
            .or_else(|| {
                let root = self.module.class_root(value);
                self.module
                    .apply_result_owners
                    .iter()
                    .find_map(|(candidate, apply)| {
                        (self.module.nodes.contains_key(*candidate)
                            && self.module.class_root(*candidate) == root)
                            .then_some(*apply)
                    })
            });
        let apply = match apply {
            Some(apply) => apply,
            None => self
                .module
                .apply_result_owners
                .iter()
                .find_map(|(candidate, apply)| {
                    self.module
                        .structural_eq(AnyNodeId::Dynamic(*candidate), AnyNodeId::Dynamic(value))
                        .then_some(*apply)
                })?,
        };
        let operation = self.module.node_operation(apply)?;
        if !matches!(
            lichen_utils::extend::AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Apply)
        ) {
            return None;
        }
        let items = self.module.operand_items(operation.operand?).ok()?;
        let callee = self
            .module
            .as_dynamic(items.first()?.node, self.current_block);
        // Element 3 is the callee's expr; its result type is the callee's.
        let expr = items.get(3)?.node.dynamic()?;
        let ty = self.result_type_of(expr)?;
        if !crate::shape::is_function_type_any(
            &mut self.module,
            self.type_expr,
            AnyNodeId::Dynamic(ty),
        ) {
            return None;
        }
        Some((callee, ty))
    }

    /// The **result type** an expr states, following a deferred `Unify` to the
    /// actual's own result type.
    ///
    /// # Invariant
    /// The **actual** is followed, never the template: a decided actual states
    /// its own type, so a decided non-function — or an `Error` — is never read
    /// as the template's.
    fn result_type_of(&mut self, expr: NodeId) -> Option<NodeId> {
        if let Some(ty) = self.module.expr_type_item(expr) {
            return ty.dynamic();
        }
        let operation = self.module.node_operation(expr)?;
        if !matches!(
            lichen_utils::extend::AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Unify)
        ) {
            return None;
        }
        let items = self.module.operand_items(operation.operand?).ok()?;
        let actual = items.first()?.node.dynamic()?;
        self.result_type_of(actual)
    }

    /// A placeholder callee takes the template the **context requires**.
    ///
    /// # Invariant
    /// Only a **placeholder** callee is resolved here: an undetermined callee
    /// (a parameter) stays refused (`docs/notes/function-normalization.md`
    /// §3.7).  An undecided template decides nothing, so the instantiation
    /// refuses itself.
    pub(super) fn pin_placeholder_callee(&mut self, function: ExprId, argument: ExprId) {
        let ExprKind::Instantiate { template, .. } = self.ir[argument].kind else {
            return;
        };
        let Some(placeholder) = self.placeholder_callee_expr(template) else {
            return;
        };
        let Some(requirement) = self.expected_argument_requirement(function) else {
            return;
        };
        // The requirement must be decided: an undecided one decides nothing.
        self.module.evaluate_node_deep(requirement, None);
        let Some(value) = self.module.class_value(requirement) else {
            return;
        };
        // A requirement is a term; anything else states no template to pin.
        if !matches!(
            lichen_utils::extend::AsEnum::<LowValue>::as_enum(&value),
            Some(LowValue::Array(_))
        ) {
            return;
        }
        // The requirement's two halves: the template and the type.
        let Some((expected_template, expected_type)) = self.requirement_halves(requirement) else {
            return;
        };
        // The placeholder's own pair is the callee: pinning it to the
        // requirement makes the instantiation that template.
        self.check_expr(placeholder);
        if let Some((constructor, constructor_ty)) = self.constructor_of(expected_template) {
            self.state[placeholder].val = Some(constructor);
            self.state[placeholder].ty = Some(constructor_ty);
            self.placeholder_constructors.insert(placeholder);
            return;
        }
        // Assign the requirement directly; a placeholder has no prior content.
        self.state[placeholder].val = Some(expected_template);
        self.state[placeholder].ty = Some(expected_type);
    }

    /// The requirement a call's parameter states: the template and the type.
    ///
    /// # Invariant
    /// The requirement is the **pair** the parameter states: the template in
    /// its **value slot**, the type in its **type slot**.  A parameter that
    /// states only a type keeps the requirement in its type slot, as the
    /// annotation's own term.
    fn expected_argument_requirement(&mut self, function: ExprId) -> Option<NodeId> {
        let function_expr = self.state[function].term?;
        let callee = self.module.function_apply_function(function_expr)?;
        let parameter = match callee {
            AnyFunctionId::Dynamic(fid) => self.module.functions[fid].parameter,
            AnyFunctionId::Static(sref) => {
                self.module
                    .materialize_static_signature_expr(sref, false, self.current_block)?
            }
        };
        // SAFETY: `parameter` is a live node of this module; nothing in this
        // crate calls `Module::drop_block`.
        let items = unsafe { self.module.array_items(parameter) }?;
        let value = items.get(shape::EXPR_VALUE_SLOT)?.node.dynamic()?;
        let ty = items.get(shape::EXPR_TYPE_SLOT)?.node.dynamic()?;
        // A template in the value slot makes the parameter's own term the
        // requirement.
        if self.module.class_value(value).is_some() {
            return Some(parameter);
        }
        // Otherwise the requirement is the term the type slot holds.
        Some(ty)
    }

    /// A requirement term's two halves: the template and the type.
    ///
    /// # Invariant
    /// The halves are the term's **value** and **type** slots, read through the
    /// class that holds them, so a cell answers with the value its class knows.
    fn requirement_halves(&mut self, requirement: NodeId) -> Option<(NodeId, NodeId)> {
        let root = self.module.equality_representative(requirement);
        let items = shape::value_items(&self.module, AnyNodeId::Dynamic(root))?;
        let template = items.get(shape::EXPR_VALUE_SLOT)?.node.dynamic()?;
        let ty = items.get(shape::EXPR_TYPE_SLOT)?.node.dynamic()?;
        Some((
            self.module.equality_representative(template),
            self.module.equality_representative(ty),
        ))
    }

    /// Record a template parameter's field sources in `<:`.
    ///
    /// # Invariant
    /// Only the desugared `actual <: template` shape with a parameter actual enters. Sources
    /// are constant projections of that parameter, written on the constructor's own template
    /// return fields; an instantiated return or an arbitrary unify creates no provenance.
    pub(super) fn record_template_field_sources(
        &mut self,
        expected: ExprId,
        actual: ExprId,
        actual_value: NodeId,
    ) {
        if !matches!(self.ir[actual].kind, ExprKind::Parameter) {
            return;
        }
        let ExprKind::Apply {
            function, argument, ..
        } = self.ir[expected].kind
        else {
            return;
        };
        if !matches!(self.ir[argument].kind, ExprKind::Placeholder) {
            return;
        }
        let Some(constructor) = self.state[function].term else {
            return;
        };
        let Some(AnyFunctionId::Dynamic(function)) =
            self.module.function_apply_function(constructor)
        else {
            return;
        };
        let returned = self.module.functions[function].r#return;
        let Some(value) = self.module.expr_value_half(returned) else {
            return;
        };
        let root = self.module.class_root(value);
        let Some(required) = shape::value_items(&self.module, AnyNodeId::Dynamic(root)) else {
            return;
        };
        for (position, required) in required[shape::STRUCT_VALUE_FIELD_BASE..]
            .iter()
            .enumerate()
        {
            let Some(required) = required.node.dynamic() else {
                continue;
            };
            let required = self.module.expr_value_half(required).unwrap_or(required);
            let selector = self.usize_node(shape::STRUCT_VALUE_FIELD_BASE + position);
            let operand = self.array_node(self.current_block, &[actual_value, selector]);
            let field = self.op_node(
                self.current_block,
                P::Operator::from(LowOperator::Index),
                Some(operand),
            );
            let zero = self.zero();
            let operand = self.array_node(self.current_block, &[field, zero]);
            let supplied = self.op_node(
                self.current_block,
                P::Operator::from(LowOperator::Index),
                Some(operand),
            );
            self.module.field_sources.insert(required, supplied);
        }
    }

    /// A struct instance `S{…}`: apply the constructor to `_`, then fill its fields.
    ///
    /// # Invariant
    /// The template is a **struct value** returned by the constructor, not a
    /// type. Its fields are the requirements, and the instance inherits its
    /// name table and nominal id. No second clone is made.
    pub(super) fn check_instantiate(
        &mut self,
        e: ExprId,
        template: ExprId,
        value: ExprId,
        arg_names: &[Option<&'static str>],
    ) -> NodeId {
        let placeholder = self.placeholder_callee_expr(template);
        let constructor = placeholder
            .map(|placeholder| self.placeholder_constructors.contains(&placeholder))
            .unwrap_or(false);
        if constructor || placeholder.is_none() {
            self.check_expr(template);
        } else if let Some(placeholder) = placeholder {
            self.check_expr(placeholder);
        }
        self.check_expr(value);
        let (template_value, template_ty) = if constructor {
            (self.value_of(template), self.state[template].ty.unwrap())
        } else {
            match placeholder {
                Some(placeholder) => (
                    self.value_of(placeholder),
                    self.state[placeholder].ty.unwrap(),
                ),
                None => {
                    let template_value = self.value_of(template);
                    (template_value, self.state[template].ty.unwrap())
                }
            }
        };
        self.module.evaluate_node_deep(template_value, None);
        self.module.evaluate_node_deep(template_ty, None);
        if !self.type_is_concrete(template_ty) {
            // The check is determined here, not at the apply: refuse the undetermined.
            self.record_guard(
                template_ty,
                template_ty,
                self.loc(template, 1),
                DiagKind::InstantiateCalleeUndetermined,
                None,
            );
            let value_node = self.value_of(value);
            let expr = self.expr_of(value_node, template_ty);
            self.state[e].term = Some(expr);
            self.state[e].val = Some(value_node);
            self.state[e].ty = Some(template_ty);
            return expr;
        }
        if !self.is_struct_layout(template_ty) {
            // A non-struct template is a diagnostic, never a runtime panic.
            self.record_guard(
                template_ty,
                template_ty,
                self.loc(template, 1),
                DiagKind::InstantiateCallee,
                None,
            );
            let value_node = self.value_of(value);
            let expr = self.expr_of(value_node, template_ty);
            self.state[e].term = Some(expr);
            self.state[e].val = Some(value_node);
            self.state[e].ty = Some(template_ty);
            return expr;
        }
        self.check_unify(
            template_ty,
            self.struct_template(),
            self.loc(template, 1),
            DiagKind::Guard,
        );
        // The arguments, in call order: the value is the tuple of arguments.
        let Some((elements, mut vals)) = self.instantiation_arguments(value, template_ty) else {
            return self.refused_expr(e);
        };
        // The constructor application supplies the fresh struct value; its
        // fields are the requirements the arguments fill.
        let Some(items) = shape::value_items(&self.module, AnyNodeId::Dynamic(template_value))
        else {
            self.record_guard(
                template_ty,
                template_ty,
                self.loc(template, 0),
                DiagKind::Guard,
                None,
            );
            return self.refused_expr(e);
        };
        if items.len() < shape::STRUCT_VALUE_FIELD_BASE {
            self.record_guard(
                template_ty,
                template_ty,
                self.loc(template, 0),
                DiagKind::Guard,
                None,
            );
            return self.refused_expr(e);
        }
        let names_node = items[shape::STRUCT_VALUE_NAMES_SLOT].node;
        let nominal_node = items[shape::STRUCT_VALUE_ID_SLOT].node;
        let fields: Vec<NodeId> = items[shape::STRUCT_VALUE_FIELD_BASE..]
            .iter()
            .map(|item| self.module.as_dynamic(item.node, self.current_block))
            .collect();
        let table = self.struct_name_table(template_value);
        let (ordered, valid) = self.reorder_arguments(
            e,
            &template_ty,
            template_value,
            &elements,
            arg_names,
            &fields,
            table,
        );
        if valid {
            // Each argument is matched against the clone's own field.
            for (position, argument) in ordered.iter().enumerate() {
                let Some(field) = fields.get(position) else {
                    continue;
                };
                let field = *field;
                let argument_expr = elements[*argument];
                let actual_value = self.value_of(argument_expr);
                let required_value = self.module.expr_value_half(field).unwrap_or(field);
                self.module
                    .field_sources
                    .insert(required_value, actual_value);
                if self.actual_value_is_decided(actual_value) {
                    // A field read unifies like any other argument: no deferred node.
                    self.check_unify(
                        vals[*argument],
                        field,
                        self.loc(argument_expr, 1),
                        DiagKind::Annotation,
                    );
                } else {
                    // Undecided values cannot mismatch; check only type and attributes.
                    let width = shape::value_len(&self.module, AnyNodeId::Dynamic(field))
                        .into_iter()
                        .chain(shape::value_len(
                            &self.module,
                            AnyNodeId::Dynamic(vals[*argument]),
                        ))
                        .max()
                        .unwrap_or(0);
                    for position in 1..width {
                        let Some(expected) =
                            shape::value_item(&self.module, AnyNodeId::Dynamic(field), position)
                        else {
                            continue;
                        };
                        let Some(actual) = shape::value_item(
                            &self.module,
                            AnyNodeId::Dynamic(vals[*argument]),
                            position,
                        ) else {
                            continue;
                        };
                        let expected = self.module.as_dynamic(expected, self.current_block);
                        let actual = self.module.as_dynamic(actual, self.current_block);
                        self.check_unify(
                            actual,
                            expected,
                            self.loc(argument_expr, position),
                            DiagKind::Annotation,
                        );
                    }
                }
            }
        }
        let mut value_items = vec![
            self.module.as_dynamic(names_node, self.current_block),
            self.module.as_dynamic(nominal_node, self.current_block),
        ];
        if valid {
            value_items.extend(ordered.iter().map(|&argument| vals[argument]));
        } else {
            value_items.extend(vals.iter().copied());
        }
        let value_node = self.array_node(self.current_block, &value_items);
        let ty_node = self.struct_template();
        let expr = self.expr_of(value_node, ty_node);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        expr
    }

    /// The argument expressions and their compiled terms, in call order.
    fn instantiation_arguments(
        &mut self,
        value: ExprId,
        template_ty: NodeId,
    ) -> Option<(Vec<ExprId>, Vec<NodeId>)> {
        let elements = match self.ir[value].kind {
            ExprKind::Tuple(_) => self.range_children(value),
            _ => {
                self.record_guard(
                    template_ty,
                    template_ty,
                    self.loc(value, 0),
                    DiagKind::Guard,
                    Some("struct instance"),
                );
                return None;
            }
        };
        let mut vals = Vec::with_capacity(elements.len());
        for &el in &elements {
            self.check_expr(el);
            vals.push(self.state[el].term.expect("an argument is compiled"));
        }
        Some((elements, vals))
    }

    /// The arguments in the definition's order, and whether the call matched
    /// the definition at all.
    ///
    /// # Invariant
    /// The pairing is the **definition's** order: the IR keeps `S(.b …, .a …)`
    /// in call order, so pairing by position would take the wrong field. A
    /// missing, duplicate, unknown or excess field is recorded here, and the
    /// caller then skips the field unify.
    ///
    /// A recorded failure names the clone's **value** — where the field names
    /// live — and the template's type beside it.
    fn reorder_arguments(
        &mut self,
        e: ExprId,
        template_ty: &NodeId,
        template_value: NodeId,
        elements: &[ExprId],
        arg_names: &[Option<&'static str>],
        fields: &[NodeId],
        table: Option<AnyHandle<[TableItem]>>,
    ) -> (Vec<usize>, bool) {
        let def_len = fields.len();
        let mut assign: Vec<Option<usize>> = Vec::new();
        let mut valid = true;
        let mut unknown_field = false;
        // Named arguments claim their name's positional index.
        for (i, name) in arg_names.iter().enumerate() {
            let Some(name) = name else {
                continue;
            };
            let position = table.and_then(|table| self.module.name_table_index(table, name));
            match position {
                Some(position) => {
                    if position >= assign.len() {
                        assign.resize(position + 1, None);
                    }
                    if assign[position].is_some() {
                        self.record_guard(
                            template_value,
                            *template_ty,
                            self.loc(elements[i], 0),
                            DiagKind::StructDuplicateField,
                            Some(name),
                        );
                        valid = false;
                    } else {
                        assign[position] = Some(i);
                    }
                }
                None => {
                    self.record_guard(
                        template_value,
                        *template_ty,
                        self.loc(elements[i], 0),
                        DiagKind::StructUnknownField,
                        Some(name),
                    );
                    valid = false;
                    unknown_field = true;
                }
            }
        }
        // Positional arguments fill the lowest-numbered unclaimed position.
        let mut next = 0usize;
        for (i, name) in arg_names.iter().enumerate() {
            if name.is_some() {
                continue;
            }
            while next < assign.len() && assign[next].is_some() {
                next += 1;
            }
            if next < assign.len() {
                assign[next] = Some(i);
                next += 1;
            } else if assign.len() >= def_len {
                // Every definition position is claimed; this is an excess.
                self.record_guard(
                    *template_ty,
                    *template_ty,
                    self.loc(elements[i], 0),
                    DiagKind::StructExcessField,
                    None,
                );
                valid = false;
            } else {
                assign.push(Some(i));
                next += 1;
            }
        }
        // Missing fields — only when the definition's field count is known.
        if !unknown_field {
            if assign.len() < def_len {
                assign.resize(def_len, None);
            }
            for (position, slot) in assign.iter().enumerate().take(def_len) {
                if slot.is_none() {
                    let name = table.and_then(|table| self.struct_field_name(table, position));
                    self.record_guard(
                        template_value,
                        *template_ty,
                        self.loc(e, 0),
                        DiagKind::StructMissingField,
                        name,
                    );
                    valid = false;
                }
            }
        }
        let ordered: Vec<usize> = assign
            .iter()
            .take(def_len)
            .filter_map(|slot| *slot)
            .collect();
        (ordered, valid)
    }

    /// The field name at a definition position, from the name table (`None` for
    /// an anonymous struct).
    fn struct_field_name(
        &self,
        table: AnyHandle<[TableItem]>,
        position: usize,
    ) -> Option<&'static str> {
        // SAFETY: `table` is read from a live node of this module; nothing in
        // this crate calls `Module::drop_block`.
        for item in unsafe { table.items() } {
            if self
                .module
                .node_value(item.value)
                .and_then(|v| v.as_enum())
                .is_some_and(|v| v == LowValue::USize(position))
                && let Some(LowValue::Str(name)) =
                    self.module.node_value(item.key).and_then(|v| v.as_enum())
            {
                return Some(name);
            }
        }
        None
    }

    /// The struct name→index table value.
    ///
    /// # Invariant
    /// A non-empty list of wholly unnamed fields builds the [`LowValue::Error`]
    /// marker — an anonymous struct has no table. An **empty** list is not that
    /// case: it has no unnamed field, so it builds a present, empty table.
    pub(super) fn build_struct_names(&mut self, names: &[Option<&'static str>]) -> NodeId {
        if !names.is_empty() && names.iter().all(|n| n.is_none()) {
            return self.alloc_node(
                self.current_block,
                None,
                Some(P::Value::from(LowValue::Error)),
            );
        }
        let mut entries = Vec::new();
        for (i, name) in names.iter().enumerate() {
            if let Some(name) = name {
                let key = self.name_node(name);
                let value = self.alloc_node(
                    self.current_block,
                    None,
                    Some(P::Value::from(LowValue::USize(i))),
                );
                entries.push((AnyNodeId::Dynamic(key), AnyNodeId::Dynamic(value)));
            }
        }
        let handle = self.module.build_table(&entries, self.current_block);
        self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Table(handle))),
        )
    }
}
