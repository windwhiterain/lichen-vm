//! Arrays: the array **template**, the array **instance**, and the element
//! read with its bounds.

use std::collections::HashSet;

use lichen_lowlevel::{AnyNodeId, ArrayItem, LowOperator, LowValue, NodeId};

use lichen_utils::extend::AsEnum;

use crate::attr::AttrSet;
use crate::diagnostic::DiagKind;
use crate::ir::{BinOp, ExprId, ExprKind};
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;
use crate::diagnostic::AssertSpelling;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// An array-element read `a[i]`. See `docs/notes/type-access.md`.
    ///
    /// # Invariant
    /// The container's type is the shared `Array` template, so a non-array
    /// fails here. The element is at `i + 1`: the array's first slot holds the
    /// element template, and the elements follow it. The length is the value's
    /// own length less one, which is the element count.
    pub(super) fn check_index(&mut self, e: ExprId, array: ExprId, index: ExprId) -> NodeId {
        self.check_expr(array);
        self.check_expr(index);
        let array_ty = self.state[array].ty.unwrap();
        // A written annotation may be unevaluated.
        // Force it before deciding whether the read can be checked.
        self.module.evaluate_node_deep(array_ty, None);
        if self.type_is_concrete(array_ty) {
            self.check_unify(
                array_ty,
                self.array_template(),
                self.loc(array, 1),
                DiagKind::Guard,
            );
        } else if !self.is_loop_state(array) {
            return self.refuse_undecided_container(e, array, array_ty, "array read");
        }
        let array_value = self.value_of(array);
        let index_value = self.value_of(index);
        self.node_edges.insert(index_value, self.loc(index, 0));
        // The element sits one slot past the template.
        let element_index = self.plus(index_value);
        self.array_subscripts.insert(element_index, index_value);
        let position = self.constant_position(index).map(|at| at + 1);
        let (element, value_node, ty_node) =
            self.value_slot_read(array_value, element_index, position);
        // The bounds assert: the element count is the value's own length less
        // one.
        let length = self.array_length_of(array_value);
        let beyond_ops = self.array_node(self.current_block, &[length, index_value]);
        let beyond = self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::from(BinOp::Leq)),
            Some(beyond_ops),
        );
        let in_range_ops = self.array_node(self.current_block, &[beyond, self.zero()]);
        let in_range = self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::from(BinOp::Eq)),
            Some(in_range_ops),
        );
        self.register_assert(in_range, self.loc(e, 0), false, AssertSpelling::Condition);
        let expr = self.element_expr(value_node, ty_node, element);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        // The element's attributes are the read's: the slot is the field's own.
        self.state[e].attr = self.stated_element_slot(element);
        expr
    }

    /// A table read `t <== k`: the lowlevel `TableGet` reads the entry whose
    /// stored key is deep-content-equal to `k`.
    ///
    /// # Invariant
    /// The container's type must be decided at the read; it is then pinned to a
    /// fresh table type, so a non-table fails here. The read's type is the
    /// pinned shape's value-type cell.
    pub(super) fn check_table_find(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let container_ty = self.state[container].ty.unwrap();
        self.module.evaluate_node_deep(container_ty, None);
        if !self.type_is_concrete(container_ty) && !self.is_loop_state(container) {
            return self.refuse_undecided_container(e, container, container_ty, "table read");
        }
        let key_cell = self.fresh_cell();
        let value_cell = self.fresh_cell();
        // The two positions are slots, so the pin's shape is `[key, value]`.
        let key_slot = self.element_slot(None, Some(key_cell), None);
        let value_slot = self.element_slot(None, Some(value_cell), None);
        let shape = self.array_node(self.current_block, &[key_slot, value_slot]);
        let kind = self.kind_expr(self.current_block, self.markers.table_type_marker);
        let table_ty = self.slot_expr(self.current_block, shape, kind);
        self.check_unify(
            container_ty,
            table_ty,
            self.loc(container, 1),
            DiagKind::Guard,
        );
        let container_value = self.value_of(container);
        let key_value = self.value_of(key);
        self.node_edges.insert(key_value, self.loc(key, 0));
        let ops = self.array_node(self.current_block, &[container_value, key_value]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::TableGet),
            Some(ops),
        );
        let expr = self.expr_of(value_node, value_cell);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(value_cell);
        expr
    }

    /// An array **template** `[t, n]` — the element template and the length.
    ///
    /// # Invariant
    /// The value is computed, not built: the operator builds
    /// `[template, hole × length]` when the length is evaluable and leaves the
    /// value lazy when it is not. The holes are the positions an instance
    /// fills, and each constructor application has its own.
    pub(super) fn check_array_template(
        &mut self,
        e: ExprId,
        template: ExprId,
        length: ExprId,
    ) -> NodeId {
        self.check_expr(template);
        self.check_expr(length);
        let template_term = self.state[template]
            .term
            .expect("the element template is compiled");
        let length_value = self.value_of(length);
        let value = match self.constant_length(length) {
            Some(count) => {
                let mut items = Vec::with_capacity(count + 1);
                items.push(ArrayItem::template(AnyNodeId::Dynamic(template_term)));
                for _ in 0..count {
                    items.push(ArrayItem::new(AnyNodeId::Dynamic(self.placeholder_term())));
                }
                self.array_value_node(self.current_block, items, 0)
            }
            None => {
                let operands = self.array_node(self.current_block, &[template_term, length_value]);
                self.op_node(
                    self.current_block,
                    P::Operator::from(TypeOperator::ArrayTemplate),
                    Some(operands),
                )
            }
        };
        let ty_node = self.array_template();
        let expr = self.expr_of(value, ty_node);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        expr
    }

    /// A fresh **placeholder expression** — the hole an array template's
    /// element position is.
    ///
    /// # Invariant
    /// It is the operator's own construction, so a template built here and one
    /// built at run time have the same shape.
    fn placeholder_term(&mut self) -> NodeId {
        let width = P::Attr::ORDER.len();
        let layout = self.full_width_layout();
        crate::program::placeholder_expr(&mut self.module, self.current_block, width, layout)
    }

    /// The count a length expression states, when it states one.
    fn constant_length(&self, length: ExprId) -> Option<usize> {
        let value = self.state[length].val?;
        match self
            .module
            .node_value(AnyNodeId::Dynamic(value))
            .and_then(|held| held.as_enum())
        {
            Some(LowValue::USize(n)) => Some(n),
            _ => None,
        }
    }

    /// An array **instance** `T[e1, …, en]`.
    ///
    /// # Invariant
    /// The constructor is applied once. Every element unifies with the same
    /// returned element requirement, and the returned length is checked
    /// against the instance's count. No second clone is made.
    pub(super) fn check_array_inst(
        &mut self,
        e: ExprId,
        template: ExprId,
        elements: crate::ir::ChildRange,
        depths: crate::ir::ChildRange,
    ) -> NodeId {
        self.check_expr(template);
        let template_ty = self.state[template].ty.unwrap();
        self.module.evaluate_node_deep(template_ty, None);
        if self.type_is_concrete(template_ty) {
            self.check_unify(
                template_ty,
                self.array_template(),
                self.loc(template, 1),
                DiagKind::Guard,
            );
        } else if !self.is_loop_state(template) {
            return self.refuse_undecided_container(e, template, template_ty, "array instance");
        }
        let element_ids: Vec<ExprId> =
            self.ir.children[elements.start as usize..elements.end as usize].to_vec();
        let element_depths: Vec<usize> =
            self.ir.depths[depths.start as usize..depths.end as usize].to_vec();
        // The constructor application supplies the array value and its shape.
        let template_value = self.array_constructor_value(template);
        self.module
            .evaluate_node(AnyNodeId::Dynamic(template_value), None);
        let (requirement, template_count) = match self.template_shape(template_value) {
            Some(shape) => shape,
            None => {
                // Only a length an enclosing lambda parameter decides may be
                // deferred; any other undecided length is refused.
                if !self.length_depends_on_parameter(template) {
                    // An undecided clone states no shape, and no parameter
                    // decides it: an unchecked requirement is worse than a
                    // refusal.
                    self.record_guard(
                        template_ty,
                        template_ty,
                        self.loc(template, 0),
                        DiagKind::Guard,
                        Some("array instance"),
                    );
                    return self.refused_expr(e);
                }
                self.deferred_shape(template, template_value)
            }
        };
        let mut values = Vec::with_capacity(element_ids.len());
        let mut marked = Vec::with_capacity(element_ids.len());
        for (index, &el) in element_ids.iter().enumerate() {
            self.check_expr(el);
            let depth = element_depths.get(index).copied().unwrap_or(0);
            let element = self.state[el].term.expect("an element is compiled");
            let (element, is_marked) = match depth {
                0 => (element, false),
                // The **bare** mark: the element's own value, held lazy, and the
                // element's own type as the position's type.
                usize::MAX => {
                    let value = self.value_of(el);
                    (self.marked_element(el, value, element), true)
                }
                // The **depth** form: the first `depth` levels of the element's
                // type spine are masked.
                depth => {
                    let value = self.wrap_shallow(el, depth);
                    (self.marked_element(el, value, element), true)
                }
            };
            self.check_array_element(el, element, requirement);
            values.push(element);
            marked.push(is_marked);
        }
        // The count is the template's own length: a directed unify, so a
        // mismatch is a diagnostic, not a bounds failure.
        let count = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(values.len()))),
        );
        // The length is read first: a read of the parameter's cell joins
        // the reader with it, so the unify below pins the cell.
        self.compute_operands(count, template_count);
        self.check_unify(
            count,
            template_count,
            self.loc(e, 0),
            DiagKind::ArrayElement,
        );
        // The result's value: the constructor's returned element template, then
        // the elements.
        let first = requirement;
        let mut items = vec![first];
        items.extend(values);
        // A marked element is a masked item: the deep pass skips the position,
        // so its value stays lazy until a read asks for it.
        let mut mask = vec![false];
        mask.extend(marked);
        let value = self.array_instance_node(self.current_block, &items, &mask);
        let ty_node = self.array_template();
        let expr = self.expr_of(value, ty_node);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        expr
    }

    /// A marked element's expr: its own value slot, and a type cell that
    /// closes on the element's own type.
    ///
    /// # Invariant
    /// The type cell is fresh and undecided, so the position states no type of
    /// its own until the closure decides it. The value slot is the element's
    /// own value, so a read of the position reads the element and not a
    /// wrapper around it.
    fn marked_element(&mut self, el: ExprId, value: NodeId, element: NodeId) -> NodeId {
        let ty = self.fresh_cell();
        let inner = self.state[el].ty.expect("an element is compiled");
        // A call's own type application is what decides the element's type;
        // an element that is not a call has nothing to apply.
        let type_apply = self.call_applies.get(&el).copied();
        self.shallow_types.push((type_apply, ty, inner));
        self.element_expr(value, ty, element)
    }

    /// A set `set(a, b, …)` typed by the **set kind**, not by `array{T, n}`.
    ///
    /// # Invariant
    /// The shape is the element type alone, so `set(a)` and `set(a, b)` are one
    /// type; the separate kind keeps a set from flowing into an `array{T, n}`
    /// and makes `s[i]` a refusal. See [`crate::set`].
    pub(super) fn check_set_term(&mut self, e: ExprId) -> NodeId {
        let members = self.range_children(e);
        let mut vals = Vec::new();
        let element_ty = self.fresh_cell();
        for &member in &members {
            self.check_expr(member);
            // An element is a complete slot: the term is the value expr.
            vals.push(self.state[member].term.expect("a member is compiled"));
            // Found = the member's type, expected = the shared cell: a set is
            // homogeneous, exactly like an array.
            self.check_unify(
                self.state[member].ty.unwrap(),
                element_ty,
                self.loc(member, 1),
                DiagKind::ArrayElement,
            );
        }
        let value = self.array_node(self.current_block, &vals);
        // A set's element position is a **slot**, as an array's is.
        let element = self.element_slot(None, Some(element_ty), None);
        let shape = self.array_node(self.current_block, &[element]);
        let kind = self.kind_expr(self.current_block, self.markers.set_type_marker);
        let ty_node = self.slot_expr(self.current_block, shape, kind);
        let expr = self.expr_of(value, ty_node);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        expr
    }

    /// A constant table literal `table(k1 ==> v1, …)`, typed by
    /// `[[key type, value type], [TypeTable, Type]]`.
    ///
    /// # Invariant
    /// Entries are checked against one shared key-type cell and one shared
    /// value-type cell. The value is built eagerly by [`Module::build_table`],
    /// which drops an entry whose key is not concrete and records
    /// [`EvalError::TableKeyUndecided`] instead.
    pub(super) fn check_table_term(&mut self, e: ExprId) -> NodeId {
        let entries = self.range_children(e);
        let key_ty = self.fresh_cell();
        let value_ty = self.fresh_cell();
        let mut exprs = Vec::with_capacity(entries.len() / 2);
        for chunk in entries.chunks(2) {
            let (key, value) = (chunk[0], chunk[1]);
            self.check_expr(key);
            self.check_expr(value);
            let key_node = self.value_of(key);
            self.node_edges.insert(key_node, self.loc(key, 0));
            exprs.push((
                AnyNodeId::Dynamic(key_node),
                AnyNodeId::Dynamic(self.value_of(value)),
            ));
            self.check_unify(
                self.state[key].ty.unwrap(),
                key_ty,
                self.loc(key, 1),
                DiagKind::TableKey,
            );
            self.check_unify(
                self.state[value].ty.unwrap(),
                value_ty,
                self.loc(value, 1),
                DiagKind::TableValue,
            );
        }
        let table = self.module.build_table(&exprs, self.current_block);
        let value = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Table(table))),
        );
        // A table's two positions are **slots**, as a tuple's elements are.
        let key = self.element_slot(None, Some(key_ty), None);
        let value_ty = self.element_slot(None, Some(value_ty), None);
        let shape = self.array_node(self.current_block, &[key, value_ty]);
        let kind = self.kind_expr(self.current_block, self.markers.table_type_marker);
        let ty_node = self.slot_expr(self.current_block, shape, kind);
        let expr = self.expr_of(value, ty_node);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        expr
    }

    /// Resolve pending shallow element types before a dependent read or the
    /// definition pass.
    ///
    /// # Invariant
    /// An already decided inner type is used directly; the type application is evaluated
    /// only while the inner type is open. An element that is not a call has no application,
    /// and its own type is decided already.
    ///
    /// An entry that is still unresolved stays pending for the later pass: a
    /// position whose type is open keeps its own undecided cell.
    pub(super) fn resolve_shallow_types(&mut self) {
        let shallow_types = std::mem::take(&mut self.shallow_types);
        for (type_apply, tail, inner) in shallow_types {
            if self.module.node_value(AnyNodeId::Dynamic(inner)).is_none()
                && let Some(type_apply) = type_apply
            {
                self.module.evaluate_node_deep(type_apply, None);
            }
            self.close_shallow_tail(tail, inner);
            if self.module.node_value(AnyNodeId::Dynamic(tail)).is_none() {
                self.shallow_types.push((type_apply, tail, inner));
            }
        }
    }

    /// Close an open `@shallow` tail on the type its application decided.
    /// See `docs/notes/index-read-unify.md` §7.
    ///
    /// # Invariant
    /// Only an **undecided** tail position is closed, and only when the
    /// element's own type is decided: a tail that already names a type states
    /// it, and an undecided type has nothing to close.
    pub(super) fn close_shallow_tail(&mut self, tail: NodeId, inner: NodeId) {
        self.module.evaluate_node_deep(inner, None);
        if self.module.node_value(AnyNodeId::Dynamic(inner)).is_none() {
            return;
        }
        if self.module.node_value(AnyNodeId::Dynamic(tail)).is_none() {
            self.module.unify(tail, inner);
        }
    }

    /// Wrap `e`'s checked term so the value slot at each of the first `depth`
    /// levels of its type spine stays shallow.
    ///
    /// # Invariant
    /// Each level is a fresh expr carrying the `shallow=[true, false]` mask, so
    /// a shared subexpression is never itself marked. The descent follows slot
    /// 1 only while the spine is a concrete expr at check time; an undecided
    /// slot ends it, and the wraps above the stop still apply.
    ///
    /// The descent stops at a spine node already walked: a spine that
    /// repeats itself is a cycle, not a deeper level.
    fn wrap_shallow(&mut self, e: ExprId, depth: usize) -> NodeId {
        // Level 1 is the element's own expr; a call's is built from the
        // extracted value and type, the apply expr being virtual.

        // Each deeper level is the previous level's type slot.
        let mut levels: Vec<(NodeId, NodeId)> = Vec::new();
        let mut current = self.state[e].term.unwrap();
        if self.module.node_operation(current).is_some() {
            // A call's expr is the apply node, so the wrapper's first item is the
            // expr a read dereferences.
            levels.push((current, self.state[e].ty.unwrap()));
        } else {
            let mut walked: HashSet<NodeId> = HashSet::new();
            while levels.len() < depth && walked.insert(current) {
                // SAFETY: `current` is a live node of this module (a checked
                // expression's term).
                let Some(items) = (unsafe { self.module.array_items(current) }) else {
                    break;
                };
                if items.len() < 2 {
                    break;
                }
                let slot0 = self.module.as_dynamic(items[0].node, self.current_block);
                let slot1 = self.module.as_dynamic(items[1].node, self.current_block);
                // SAFETY: `slot1` was just materialized into the current
                // block, whose arena is alive.
                let descend =
                    unsafe { self.module.array_items(slot1) }.is_some_and(|next| next.len() >= 2);
                levels.push((slot0, slot1));
                if !descend {
                    break;
                }
                current = slot1;
            }
        }
        // Rebuild from the innermost level out, each a fresh masked slot.
        let mut wrapped = None;
        for &(slot0, slot1) in levels.iter().rev() {
            let next = wrapped.unwrap_or(slot1);
            wrapped = Some(self.array_node_masked_slot(
                self.current_block,
                &[slot0, next],
                &[true, false],
            ));
        }
        wrapped.unwrap_or(self.state[e].term.unwrap())
    }

    /// The set type `set{E}`: the shape is the element **slot** alone.
    ///
    /// # Invariant
    /// [`Self::check_set_term`]'s members are checked against exactly this slot;
    /// one element position serves every member, as an array's does, so a pin
    /// stated here is enforced on each member.
    pub(super) fn check_set_type(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let element = elements[0];
        let slot = self.element_slot_of(element);
        let shape = self.array_node(self.current_block, &[slot]);
        let kind = self.kind_expr(self.current_block, self.markers.set_type_marker);
        let expr = self.slot_expr(self.current_block, shape, kind);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(shape);
        self.state[e].ty = Some(kind);
        expr
    }

    /// The table type `table{K, V}`: the key and value **slots**.
    ///
    /// # Invariant
    /// The two positions are the same slots the value form builds, so a table
    /// type and a table value are one type however each is spelled.
    pub(super) fn check_table_type(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let key = self.element_slot_of(elements[0]);
        let value = self.element_slot_of(elements[1]);
        let shape = self.array_node(self.current_block, &[key, value]);
        let kind = self.kind_expr(self.current_block, self.markers.table_type_marker);
        let expr = self.slot_expr(self.current_block, shape, kind);
        self.state[e].term = Some(expr);
        self.state[e].val = Some(shape);
        self.state[e].ty = Some(kind);
        expr
    }

    /// The constructor result used for shape checks, or a direct template's value.
    fn array_constructor_value(&mut self, template: ExprId) -> NodeId {
        let applied = self
            .call_applies
            .get(&template)
            .copied()
            .and_then(|type_apply| self.module.applied_exprs.get(&type_apply).copied())
            .and_then(|applied| self.module.expr_value_half(applied));
        if let Some(value) = applied
            && (shape::value_items(&self.module, AnyNodeId::Dynamic(value)).is_some()
                || self.module.node_operation(value).is_some())
        {
            return value;
        }
        self.value_of(template)
    }

    /// The **element count** of an array value: its length less one.
    ///
    /// # Invariant
    /// The array's first slot holds the element template, so the elements are
    /// the rest.
    fn array_length_of(&mut self, array_value: NodeId) -> NodeId {
        let operands = self.array_node(self.current_block, &[array_value]);
        self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::ArrayLength),
            Some(operands),
        )
    }

    /// The template's **shape**: the element template and the element count.
    ///
    /// # Invariant
    /// Both are read from the constructor result's **value**, never from the
    /// syntax. An unevaluated result states no shape.
    fn template_shape(&mut self, root: NodeId) -> Option<(NodeId, NodeId)> {
        let requirement = self.element_template(root)?;
        // The array's first slot holds the element template, so the elements
        // are the rest.
        let length = shape::value_len(&self.module, AnyNodeId::Dynamic(root))?;
        // The count is fresh: the shared `0` and `1` are constants.
        let count = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(length.saturating_sub(1)))),
        );
        Some((requirement, count))
    }

    /// The constructor result's **element template**: its value's first slot.
    fn element_template(&self, root: NodeId) -> Option<NodeId> {
        shape::value_item(
            &self.module,
            AnyNodeId::Dynamic(root),
            shape::ARRAY_VALUE_TEMPLATE_SLOT,
        )
        .and_then(|node| node.dynamic())
    }

    /// Whether a slot is `Any` or an empty clone whose origin reaches `Any`.
    fn wildcard_slot(&self, slot: AnyNodeId) -> bool {
        if matches!(
            self.module
                .node_value(slot)
                .and_then(|value| value.as_enum()),
            Some(LowValue::Any)
        ) {
            return true;
        }
        if self.module.node_value(slot).is_some() {
            return false;
        }
        let mut current = slot
            .dynamic()
            .and_then(|node| self.module.node_origin(node));
        for _ in 0..8 {
            let Some(node) = current else {
                return false;
            };
            if matches!(
                self.module
                    .node_value(AnyNodeId::Dynamic(node))
                    .and_then(|value| value.as_enum()),
                Some(LowValue::Any)
            ) {
                return true;
            }
            current = self.module.node_origin(node);
        }
        false
    }

    /// Whether an element requirement is a wildcard, including an empty clone of one.
    ///
    /// # Invariant
    /// Empty alone is not wildcard: the node's origin must reach `Any`, so an ordinary
    /// undecided or computed pin still checks the whole expression.
    fn wildcard_requirement(&self, requirement: NodeId) -> bool {
        let Some(slot) = shape::value_item(&self.module, AnyNodeId::Dynamic(requirement), 0) else {
            return false;
        };
        self.wildcard_slot(slot)
    }

    /// Check one array element without merging a wildcard's expression root.
    fn check_array_element(&mut self, el: ExprId, element: NodeId, requirement: NodeId) {
        let wildcard = self.wildcard_requirement(requirement);
        if !wildcard {
            self.check_unify(
                element,
                requirement,
                self.loc(el, 1),
                DiagKind::ArrayElement,
            );
            return;
        }
        let Some(actual) = shape::value_items(&self.module, AnyNodeId::Dynamic(element)) else {
            return;
        };
        let Some(expected) = shape::value_items(&self.module, AnyNodeId::Dynamic(requirement))
        else {
            return;
        };
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).skip(1).enumerate() {
            if let (Some(actual), Some(expected)) = (actual.node.dynamic(), expected.node.dynamic())
            {
                // A cloned wildcard attribute states no value requirement; an ordinary open
                // attribute still runs its refinement check.
                if index > 0 && self.wildcard_slot(AnyNodeId::Dynamic(expected)) {
                    continue;
                }
                // Two open slots state no constraint yet; a decided element still pins the shared type.
                if self
                    .module
                    .node_value(AnyNodeId::Dynamic(expected))
                    .is_none()
                    && self.module.node_operation(expected).is_none()
                    && self.module.node_value(AnyNodeId::Dynamic(actual)).is_none()
                    && self.module.node_operation(actual).is_none()
                {
                    continue;
                }
                self.check_unify(actual, expected, self.loc(el, 1), DiagKind::ArrayElement);
            }
        }
    }

    /// The template's **deferred shape**: its element template and length.
    ///
    /// # Invariant
    /// The count is the named length — the node the parameter's cell is read
    /// through, so a unify with it pins the cell. If the result names no
    /// length, its own length read is used.
    fn deferred_shape(&mut self, template: ExprId, root: NodeId) -> (NodeId, NodeId) {
        if let Some((element, length)) = self.named_shape(template, root) {
            return (element, length);
        }
        (self.index_of(root, self.zero()), self.array_length_of(root))
    }

    /// The element template and length the array template's operator names.
    ///
    /// # Invariant
    /// The operand's two positions are the element template and length. Only
    /// an array template has this operand.
    fn named_shape(&self, template: ExprId, root: NodeId) -> Option<(NodeId, NodeId)> {
        let operation = match self.call_applies.get(&template).copied() {
            Some(type_apply) => {
                let applied = self.module.applied_exprs.get(&type_apply).copied()?;
                let value = self.module.expr_value_half(applied)?;
                self.module.node_operation(value)?
            }
            None if matches!(self.ir[template].kind, ExprKind::Array { .. }) => {
                self.module.node_operation(root)?
            }
            None => return None,
        };
        let operand = operation.operand?;
        let items = shape::value_items(&self.module, AnyNodeId::Dynamic(operand))?;
        if items.len() < 2 {
            return None;
        }
        Some((items[0].node.dynamic()?, items[1].node.dynamic()?))
    }

    /// The value one more than `value`: the `+ 1` an array read's subscript is.
    fn plus(&mut self, value: NodeId) -> NodeId {
        let operands = self.array_node(self.current_block, &[value, self.one()]);
        self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Add),
            Some(operands),
        )
    }
}
