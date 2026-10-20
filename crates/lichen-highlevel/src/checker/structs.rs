//! Structs, fields, instantiation: the nominal id, the field reads, and
//! the instantiation checked against a field list.

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, NodeId};

use lichen_utils::extend::AsEnum;

use crate::attr::AttrSet;
use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind};
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;
use super::annotations::ElementWalk;

/// What a named instantiation decided.
struct NamedInstance {
    /// The field values, in the definition's order.
    value: NodeId,
    /// The arguments' types in that order, which the field-list unify compares.
    value_shape: NodeId,
    /// Whether the named arguments matched the definition; a mismatch was
    /// recorded, so the caller skips the field-list unify.
    valid: bool,
    /// `assign[position]` = the call-order argument index filling it.
    ///
    /// # Invariant
    /// Empty when nothing was assigned, so the reconciliation walks nothing:
    /// every early return here is one that recorded a mismatch or deferred the
    /// whole reorder.
    assign: Vec<Option<usize>>,
}

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A positional slot read `a(k)` — a **tuple** element. See
    /// `docs/notes/eval-before-unify.md` §2.2.
    ///
    /// # Invariant
    /// A decided container is refused outright; an undecided one is registered as
    /// a per-apply assert on its kind. A struct instance reads by name, so a
    /// struct type reaching this form is refused like any other non-tuple.
    pub(super) fn check_field(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let container_ty = self.state[container].ty.unwrap();
        // A written annotation may be unevaluated.
        // Force it before deciding whether the read can be checked.
        self.module.evaluate_node_deep(container_ty, None);
        let container_value = self.value_of(container);
        // A lazy region's read states no type requirement: its pair is open.
        let lazy = self.lazy_region(container_value);
        // The requirement names the container's **kind**, never its type.
        if lazy {
        } else if self.type_is_concrete(container_ty) {
            let kind = self.tuple_kind_requirement();
            let container_kind = self.lazy_index_path(container_ty, &[shape::TYPE_KIND_SLOT]);
            // The kind read runs first: an unevaluated `Index` holds no value, so
            // a unify writes into its class and asks nothing.
            self.module
                .evaluate_node(AnyNodeId::Dynamic(container_kind), None);
            self.check_unify(
                container_kind,
                kind,
                self.loc(container, 1),
                DiagKind::Guard,
            );
        } else if !self.is_loop_state(container) {
            return self.refuse_undecided_container(e, container, container_ty, "tuple read");
        }
        let key_value = self.value_of(key);
        self.node_edges.insert(key_value, self.loc(key, 0));
        self.read_site = Some(self.loc(e, 0));
        // The field's position, when the key is a literal: `slot_read` reads
        // the field's type out of the field list.
        let position = self.constant_position(key);
        let (element, value_node, ty_node) =
            self.slot_read(container_ty, container_value, key_value, position);
        // A lazy region's read states no attribute: its pair is the wildcard pair.
        let pair = if lazy {
            self.pair_of(value_node, ty_node)
        } else {
            self.element_pair(value_node, ty_node, element)
        };
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        // The element's attributes are the read's: the slot is the field's own.
        self.state[e].attr = self.stated_element_slot(element);
        pair
    }

    /// The first position of the array a node's class holds, through its origin.
    ///
    /// # Invariant
    /// A clone states no value at check time, so the walk follows
    /// [`Module::node_origin`] back to the template that holds the array.
    fn first_position(&self, node: NodeId) -> Option<NodeId> {
        let mut current = Some(node);
        while let Some(node) = current {
            // SAFETY: a payload of a value of this module; a stale node answers `None`.
            if let Some(items) = unsafe { self.module.array_items_of(AnyNodeId::Dynamic(node)) } {
                return items
                    .first()
                    .map(|item| item.node)
                    .and_then(|node| node.dynamic());
            }
            current = self.module.node_origin(node);
        }
        None
    }

    /// Whether a node holds a shallow mark's pair: two items, the first lazy.
    fn masked_pair(&self, node: NodeId) -> bool {
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

    /// A lazy region's own pair: the read addresses the pair, not a slot's value.
    ///
    /// # Invariant
    /// The operands are the read's own, so both predate the read.
    fn lazy_region(&self, value: NodeId) -> bool {
        let Some(operation) = self.module.node_operation(value) else {
            return self.masked_pair(value);
        };
        if !matches!(
            lichen_utils::extend::AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Index)
        ) {
            return self.masked_pair(value);
        }
        let Some(operand) = operation.operand else {
            return false;
        };
        // The operand array is `[slot, key]`; the slot's value position is the wrap.
        let slot = self.first_position(operand);
        let Some(slot) = slot else {
            return false;
        };
        let position = match self.first_position(slot) {
            Some(node) => Some(node),
            None => self.selected_item(slot),
        };
        // The selected node may be the pair that holds the wrapper, or the wrapper
        // itself: both readings are the same region.
        position.is_some_and(|node| {
            self.masked_pair(node)
                || self
                    .first_position(node)
                    .is_some_and(|first| self.masked_pair(first))
        })
    }

    /// The pair a call instantiates: the callee function's template return pair.
    ///
    /// # Invariant
    /// A call's own value is decided at evaluation, which a check may not force; the
    /// template's return pair is the pair the call instantiates.
    fn return_pair(&self, node: NodeId) -> Option<NodeId> {
        let function = self.module.callee_function(node)?;
        Some(self.module.functions[function].r#return)
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
        // The container's array is the node itself, the array a constant read of it
        // selects, or the pair a call instantiates.
        let array = match unsafe { self.module.array_items_of(AnyNodeId::Dynamic(container)) } {
            Some(_) => container,
            None => match self.selected_item(container) {
                Some(node) => node,
                // A call's pair is its function's return pair.
                None => self.return_pair(container)?,
            },
        };
        let values = unsafe { self.module.array_items_of(AnyNodeId::Dynamic(array)) }?;
        values
            .get(position as usize)
            .map(|item| item.node)
            .and_then(|node| node.dynamic())
    }

    /// The structural slot read shared by `a(k)` and `a.name`.
    /// See `docs/notes/eval-before-unify.md`.
    ///
    /// # Invariant
    /// A known `position` makes the read's type decided rather than lazy: it is the
    /// field's own node out of the field list, which a class question sees through
    /// where an unevaluated `Index` cannot. A frozen field type is materialized as a
    /// leaf, so it is decided the same way; an undecided container keeps the lazy
    /// form, resolved at the apply.
    pub(super) fn slot_read(
        &mut self,
        container_ty: NodeId,
        container_value: NodeId,
        key: NodeId,
        position: Option<usize>,
    ) -> (NodeId, NodeId, NodeId) {
        // A lazy region's pair is narrower than a slot: the element is its value.
        let lazy = self.lazy_region(container_value);
        let value_ops = self.array_node(self.current_block, &[container_value, key]);
        let element = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        self.stamp_read_node(element);
        if lazy {
            let ty_node = self.fresh_cell();
            return (element, element, ty_node);
        }
        // A compound's elements are complete slots: the value is the first position.
        let value_ops = self.array_node(self.current_block, &[element, self.zero()]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        self.stamp_read_node(value_node);
        // A field type is a node of this table: a frozen one is materialized as its own leaf.
        let resolved = position
            .and_then(|at| {
                shape::field_type(
                    &self.module,
                    shape::TypeRef::Term(AnyNodeId::Dynamic(container_ty)),
                    at,
                )
            })
            .map(|ty| self.module.as_dynamic(ty, self.current_block));
        let ty_node = match resolved {
            Some(ty_node) => ty_node,
            None => {
                let shape_ops = self.array_node(self.current_block, &[container_ty, self.zero()]);
                let shape = self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::Index),
                    Some(shape_ops),
                );
                let ty_ops = self.array_node(self.current_block, &[shape, key]);
                let element = self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::Index),
                    Some(ty_ops),
                );
                // An element is a slot, so the field's type is its type position,
                // one level further in.
                let slot_ops = self.array_node(self.current_block, &[element, self.one()]);
                self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::Index),
                    Some(slot_ops),
                )
            }
        };
        (element, value_node, ty_node)
    }

    /// A **named** type access `X::a` — see `docs/notes/type-access.md`.
    ///
    /// # Invariant
    /// The container's **type** must be a TypeStruct kind, stated as a unify:
    /// a decided container is refused where it stands, an undecided one is
    /// pinned. The read is the element's own pair, so its type is the field's
    /// type *value* — `struct{.a Int, .b string}::a` is `Int : Type`.
    pub(super) fn check_named_type_access(
        &mut self,
        e: ExprId,
        container: ExprId,
        name: &'static str,
    ) -> NodeId {
        self.check_expr(container);
        let container_ty = self.state[container].ty.unwrap();

        self.module.evaluate_node_deep(container_ty, None);
        if !self.type_is_concrete(container_ty) {
            return self.refuse_undecided_container(
                e,
                container,
                container_ty,
                "named type access",
            );
        }
        // The requirement, as a unify: the container's type must be a struct
        // kind — its `TypeStruct` tag, not a payload shape.
        let kind = self.struct_kind_requirement();
        // A decided container of another kind is refused and not read.
        if !self.check_unify(container_ty, kind, self.loc(container, 1), DiagKind::Guard) {
            return self.refused_pair(e);
        }
        // The chains root at the pair's type item: an Index reads its subscript first.
        self.read_site = Some(self.loc(e, 0));
        let ty_source = {
            let pair = self.state[container]
                .term
                .expect("the container must be compiled");
            let ops = self.array_node(self.current_block, &[pair, self.one()]);
            self.op_node(
                self.current_block,
                P::Operator::from(LowOperator::Index),
                Some(ops),
            )
        };
        let names_node = self.lazy_index_path(ty_source, &shape::STRUCT_KIND_NAMES_PATH);
        // A decided struct type without this component is refused like a named read.
        self.module
            .evaluate_node(AnyNodeId::Dynamic(names_node), None);
        let names = self
            .module
            .node_value(AnyNodeId::Dynamic(names_node))
            .and_then(|value| value.as_enum())
            .and_then(|value| match value {
                LowValue::Table(table) => Some(table),
                _ => None,
            });
        if let Some(table) = names {
            if shape::name_table_index(&self.module, table, name).is_none() {
                let container_value = self.value_of(container);
                self.record_guard(
                    container_value,
                    container_ty,
                    self.loc(e, 0),
                    DiagKind::NamedField,
                    Some(name),
                );
                return self.refused_pair(e);
            }
        }
        // key = TableGet(names, name) — the field's positional index.
        let name_node = self.name_node(name);
        let key_ops = self.array_node(self.current_block, &[names_node, name_node]);
        let key = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::TableGet),
            Some(key_ops),
        );
        self.node_edges.insert(key, self.loc(e, 0));
        // The element's own pair, as the positional type access builds it.
        let container_value = self.value_of(container);
        let (pair, element, ty_node) = self.element_read(container_value, key);
        // As there: the element is the target of both slot reads.
        self.node_edges.insert(element, self.loc(e, 1));
        self.state[e].term = Some(pair);
        self.state[e].val = None;
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// A named field read `a.name`, resolved through the container's name
    /// table.
    ///
    /// # Invariant
    /// An undecided container's index stays lazy and resolves when the call
    /// binds it; a non-struct container or an unknown field is the guard's
    /// error below, never a runtime panic.
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
        // The field's position, resolved once for the guard below and the
        // read's type.

        // The lookup is the **structural** reader, asked whether the cell is
        // decided: a type through a class is readable.
        let position = self.named_field_index_any(AnyNodeId::Dynamic(container_ty), name);
        // Decided containers are refused here; the undecided tier carries no requirement.
        if concrete {
            let kind = self.struct_kind_requirement();
            let container_kind = self.lazy_index_path(container_ty, &[shape::TYPE_KIND_SLOT]);
            self.module
                .evaluate_node(AnyNodeId::Dynamic(container_kind), None);
            self.check_unify(
                container_kind,
                kind,
                self.loc(container, 1),
                DiagKind::Guard,
            );
        }
        // The lazy name-table read keeps an undecided container's read
        // resolvable at the apply (eval-before-unify.md §2.4).
        if concrete
            && shape::is_struct_type_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(container_ty),
            )
            && position.is_none()
            // ... and its **name table is readable**. Missing from a readable
            // table is a genuine miss; undecided stays lazy.
            && shape::struct_names_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(container_ty),
            )
            .is_some()
        {
            // A struct without this field: the name rides in the entry, so the
            // language layer can append a did-you-mean clause.
            self.record_guard(
                container_ty,
                container_ty,
                self.loc(e, 0),
                DiagKind::NamedField,
                Some(name),
            );
        }
        // The field's **subscript**: a constant position when the container
        // states it, else the lazy `TableGet(names, name)`.

        // A constant is not an optimisation: a kernel body is walked by
        // operands, and a `TableGet` is no value to resolve.
        self.read_site = Some(self.loc(e, 0));
        // Index reads its subscript first: root the chains at the pair.
        let ty_source = {
            let pair = self.state[container]
                .term
                .expect("the container must be compiled");
            let ops = self.array_node(self.current_block, &[pair, self.one()]);
            self.op_node(
                self.current_block,
                P::Operator::from(LowOperator::Index),
                Some(ops),
            )
        };
        let key = match position {
            Some(at) => self.alloc_node(
                self.current_block,
                None,
                Some(P::Value::from(LowValue::USize(at))),
            ),
            None => {
                // The struct marker's name table, read through the container
                // type's kind.
                let names_node = self.lazy_index_path(ty_source, &shape::STRUCT_TYPE_NAMES_PATH);
                // key = TableGet(names, name) — the field index.
                let name_node = self.name_node(name);
                let key_ops = self.array_node(self.current_block, &[names_node, name_node]);
                self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::TableGet),
                    Some(key_ops),
                )
            }
        };
        self.node_edges.insert(key, self.loc(e, 0));
        let container_value = self.value_of(container);
        let ty_root = if position.is_some() {
            container_ty
        } else {
            ty_source
        };
        let (element, value_node, ty_node) =
            self.slot_read(ty_root, container_value, key, position);
        let pair = self.element_pair(value_node, ty_node, element);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        // The element's attributes are the read's: the slot is the field's own.
        self.state[e].attr = self.stated_element_slot(element);
        pair
    }

    /// The positional index of a named struct field; `None` for a non-struct,
    /// an anonymous struct, or an unknown name.
    ///
    /// # Invariant
    /// The **universe gate stays here** (`struct_names_any` compares against the
    /// checker's canonical universe node); the fold over the table's entries is
    /// [`shape::name_table_index`], shared with the lowering's reader so the two
    /// cannot decode an entry differently.
    pub(super) fn named_field_index_any(
        &mut self,
        ty: AnyNodeId,
        name: &'static str,
    ) -> Option<usize> {
        let table = shape::struct_names_any(&mut self.module, self.type_expr, ty)?;
        shape::name_table_index(&self.module, table, name)
    }

    /// The constant position a positional key states; `None` otherwise, where
    /// `slot_read` keeps its lazy form.
    fn constant_position(&self, key: ExprId) -> Option<usize> {
        let value = self.state[key].val?;
        match self
            .module
            .node_value(AnyNodeId::Dynamic(value))
            .and_then(|held| held.as_enum())
        {
            Some(LowValue::USize(n)) => Some(n),
            _ => None,
        }
    }

    /// Whether a type cell is statically inspectable. Gates the
    /// field-read and function-ness guards.
    pub(super) fn type_is_concrete(&self, ty: NodeId) -> bool {
        self.module
            .node_value(AnyNodeId::Dynamic(ty))
            .is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    None | Some(LowValue::USize(_)) | Some(LowValue::Array(_))
                )
            })
    }

    /// A struct-returning block: an anonymous struct over the bindings,
    /// a fresh nominal type per occurrence.
    ///
    /// # Invariant
    /// An expression statement is not a field. It is checked like any other and
    /// discarded, so `field_names` stays index-aligned with the value tuple; a
    /// block's fields are therefore always named (`docs/language-spec.md`
    /// §Blocks).
    pub(super) fn check_record(
        &mut self,
        e: ExprId,
        value: ExprId,
        field_names: &[Option<&'static str>],
    ) -> NodeId {
        self.check_expr(value);
        let elements = self.range_children(value);
        let mut vals = Vec::with_capacity(elements.len());
        let mut tys = Vec::with_capacity(elements.len());
        let mut names = Vec::with_capacity(elements.len());
        for (&el, &name) in elements.iter().zip(field_names) {
            let Some(name) = name else {
                // A bare expression: no field. Checked with the block and
                // discarded, so it is not evaluated either.
                continue;
            };
            vals.push(self.state[el].term.expect("a field expression is compiled"));
            let ty = self.state[el].ty.unwrap();
            let attrs = self.element_attr_slots(el);
            tys.push(self.element_slot(None, Some(ty), Some(attrs)));
            names.push(Some(name));
        }
        let id = self.fresh_nominal_id(e);
        let (_shape, _kind, struct_ty) = self.struct_type_type(id, &tys, &names);
        let value_node = self.array_node(self.current_block, &vals);
        let pair = self.pair_of(value_node, struct_ty);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(struct_ty);
        pair
    }

    /// A struct instantiation `C(f1, …, fn)`, recognised syntactically and
    /// validated type-directed.
    ///
    /// # Invariant
    /// The callee's **type** must be a TypeStruct kind: a concretely non-struct
    /// callee is a diagnostic, an undecided one is pinned so a non-struct
    /// actual fails the apply's argument unify, per call.
    pub(super) fn check_instantiate(
        &mut self,
        e: ExprId,
        type_expr: ExprId,
        value: ExprId,
        arg_names: &[Option<&'static str>],
    ) -> NodeId {
        self.check_expr(type_expr);
        self.check_expr(value);
        let type_pair = self.state[type_expr].term.unwrap();
        // An unevaluated callee has no readable pair; force its evaluation so
        // the checks below see the struct type.

        // An undecided-parameter callee stays lazy, and a non-terminating
        // one trips the VM's guard and stays lazy.

        // SAFETY: `type_pair` is a live node of this module; nothing in this
        // crate calls `Module::drop_block`.
        if unsafe { self.module.array_items(type_pair) }.is_none() && !self.force_failed {
            self.module.evaluate_node_deep(type_pair, None);
            self.force_failed = self.module.budget_exhausted.is_some();
        }
        let callee_ty = self.state[type_expr].ty.unwrap();
        let concrete = self.type_is_concrete(callee_ty);
        let any_named = arg_names.iter().any(|n| n.is_some());
        if concrete
            && !shape::is_type_struct_kind_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(callee_ty),
            )
        {
            // Non-struct (structs are nominal): record a diagnostic at the
            // callee's type slot, never a runtime panic.
            self.record_guard(
                type_pair,
                type_pair,
                self.loc(type_expr, 1),
                DiagKind::InstantiateCallee,
                None,
            );
            // Nominality: only a struct type instantiates. The instance still
            // gets a term, so the descent stays total.
            let value_node = self.value_of(value);
            let pair = self.pair_of(value_node, type_pair);
            self.state[e].term = Some(pair);
            self.state[e].val = Some(value_node);
            self.state[e].ty = Some(type_pair);
            return pair;
        }
        if !concrete {
            // The check is determined here, not at the apply: refuse the undetermined.
            self.record_guard(
                type_pair,
                type_pair,
                self.loc(type_expr, 1),
                DiagKind::InstantiateCalleeUndetermined,
                None,
            );
            let value_node = self.value_of(value);
            let pair = self.pair_of(value_node, type_pair);
            self.state[e].term = Some(pair);
            self.state[e].val = Some(value_node);
            self.state[e].ty = Some(type_pair);
            return pair;
        }
        // The shape *is* the positional field-type list; mid-recursion it is
        // undecided, so the check goes through a probe cell.

        // SAFETY: `type_pair` is a live node of this module; nothing in this
        // crate calls `Module::drop_block`.
        let field_list =
            match unsafe { self.module.array_items(type_pair) }.and_then(|items| items.first()) {
                Some(item) => {
                    let shape = self.module.as_dynamic(item.node, self.current_block);
                    // SAFETY: `shape` was just materialized into the current
                    // block, whose arena is alive.
                    match unsafe { self.module.array_items(shape) } {
                        Some(_) => shape,
                        // Mid-recursion: bind the shape cell to a probe, which
                        // the real shape unifies with when the descent ends.
                        _ => {
                            let fields_cell = self.fresh_cell();
                            self.module.unify(shape, fields_cell);
                            fields_cell
                        }
                    }
                }
                // Lazy shape read: `Index(type_pair, 0)`. The deferred unify merges
                // the classes, so the callee checks the real fields.
                _ => {
                    let ops = self.array_node(self.current_block, &[type_pair, self.zero()]);
                    self.op_node(
                        self.current_block,
                        P::Operator::from(LowOperator::Index),
                        Some(ops),
                    )
                }
            };
        let (value_node, value_shape, valid, argument_types) = if any_named {
            let named = self.named_instantiate(e, type_pair, value, arg_names);
            self.unify_named_instantiate_elements(type_expr, value, &named.assign);
            let elem_ids = self.compound_children(value).unwrap_or_default();
            let tys: Vec<NodeId> = elem_ids
                .iter()
                .map(|&a| self.state[a].ty.unwrap())
                .collect();
            let in_order = named
                .assign
                .iter()
                .map(|supplied| supplied.and_then(|index| tys.get(index).copied()))
                .collect();
            (named.value, named.value_shape, named.valid, in_order)
        } else {
            // A positional instantiation keeps the value's own shape: a tuple's
            // element-type list, or the type itself.
            let value_ty = self.state[value].ty.unwrap();
            // SAFETY: `value_ty` is a live node of this module; nothing in
            // this crate calls `Module::drop_block`.
            let value_shape = match unsafe { self.module.array_items(value_ty) } {
                Some(items) if items.len() >= 2 => {
                    // Materialize static refs so the shape can participate in
                    // dynamic array construction below.
                    let shape = self.module.as_dynamic(items[0].node, self.current_block);
                    let kind = self.module.as_dynamic(items[1].node, self.current_block);
                    if shape::kind_marker_is(
                        &mut self.module,
                        self.type_expr,
                        kind,
                        P::Value::tuple_type_marker(),
                    ) {
                        // A tuple's shape element is a slot, so the positional
                        // field list is each slot's type position.
                        self.slot_types_of(shape, arg_names.len())
                    } else {
                        value_ty
                    }
                }
                _ => value_ty,
            };
            // A positional call gets the structural checks once the type is decided.
            let mut structural = true;
            if concrete
                && shape::is_type_struct_kind_any(
                    &mut self.module,
                    self.type_expr,
                    AnyNodeId::Dynamic(callee_ty),
                )
                && shape::struct_names_any(
                    &mut self.module,
                    self.type_expr,
                    AnyNodeId::Dynamic(type_pair),
                )
                .is_some()
                // The pass pairs one name with one element; a hand-built value keeps
                // the old path.
                && self
                    .compound_children(value)
                    .is_some_and(|children| children.len() == arg_names.len())
            {
                let named = self.named_instantiate(e, type_pair, value, arg_names);
                structural = named.valid;
            }
            let types: Vec<Option<NodeId>> = self
                .compound_children(value)
                .unwrap_or_default()
                .iter()
                .map(|&a| self.state[a].ty)
                .collect();
            let fields: Vec<NodeId> = self
                .compound_children(value)
                .unwrap_or_default()
                .iter()
                .map(|&a| self.state[a].term.expect("an argument is compiled"))
                .collect();
            let fields = self.array_node(self.current_block, &fields);
            (fields, value_shape, structural, types)
        };
        if valid {
            let fields_match =
                self.instantiation_field_types(field_list, &argument_types, self.loc(e, 1));
            // The definition's field-type list: a shape element is a slot, so the
            // list is each slot's type position.
            let field_list = self.slot_types_of(field_list, arg_names.len());
            if fields_match {
                self.check_unify(
                    value_shape,
                    field_list,
                    self.loc(e, 1),
                    DiagKind::Annotation,
                );
            }
        }
        if !any_named {
            self.unify_instantiate_elements(type_expr, value);
        }
        let pair = self.pair_of(value_node, type_pair);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(type_pair);
        pair
    }

    /// Reconcile a **positional** instantiation's arguments against the
    /// requirements the fields state.
    ///
    /// # Invariant
    /// The field-type unify compares **types**; a field's `# n` and its `v : T`
    /// ride in the field slot, so this is the only place they are checked.
    ///
    /// The pairing is the call's order, which is the definition's for a
    /// positional call. An argument list that is **not** an element list — a
    /// bare literal in hand-built IR — answers no children, so the walk skips
    /// it.
    fn unify_instantiate_elements(&mut self, type_expr: ExprId, value: ExprId) {
        let Some(arguments) = self.compound_children(value) else {
            return;
        };
        // An unwritten type — frozen or applied — states its fields as slots, so
        // their attribute positions are what it requires.
        let Some(fields) = self.written_struct_fields(type_expr) else {
            let positions: Vec<Option<usize>> = (0..arguments.len()).map(Some).collect();
            self.unify_value_field_attributes(type_expr, &positions, &arguments);
            return;
        };
        let mut seen = ElementWalk::default();
        for (field, argument) in fields.into_iter().zip(arguments) {
            self.check_element_pair_attributes(field, argument, &mut seen);
            self.check_element_pair_values(field, argument, &mut seen);
        }
    }

    /// Reconcile a **named** instantiation's arguments against what the fields
    /// state, through the reorder's own map.
    ///
    /// # Invariant
    /// The pairing is the **definition's** order: the IR keeps `S(.b …, .a …)`
    /// in call order, so pairing by position would take the wrong field. An
    /// empty map is every early return's — a recorded mismatch or a deferred
    /// reorder — so the walk pairs nothing there.
    fn unify_named_instantiate_elements(
        &mut self,
        type_expr: ExprId,
        value: ExprId,
        assign: &[Option<usize>],
    ) {
        let Some(arguments) = self.compound_children(value) else {
            return;
        };
        // The same value-level reading the positional path takes, through the
        // reorder's own map.
        let written = self.written_struct_fields(type_expr);
        let Some(fields) = written else {
            self.unify_value_field_attributes(type_expr, assign, &arguments);
            return;
        };
        let mut seen = ElementWalk::default();
        for (field, supplied) in fields.into_iter().zip(assign.iter().copied()) {
            let Some(argument) = supplied.and_then(|index| arguments.get(index)) else {
                continue;
            };
            self.check_element_pair_attributes(field, *argument, &mut seen);
            self.check_element_pair_values(field, *argument, &mut seen);
        }
    }

    /// The declared type's field attributes, for an instantiation whose type is a
    /// value rather than a written struct.
    ///
    /// # Invariant
    /// A field slot is `[value, type, attrs…]` and an unspelled attribute is the
    /// wildcard: every other position is a requirement, and the argument's provided
    /// perspective must satisfy it (`# warp | # 0` holds, `# warp | # 1` does not).
    /// `positions` maps a field to the argument the call supplied for it: the call's
    /// order positionally, the reorder's map for a named call.
    fn unify_value_field_attributes(
        &mut self,
        type_expr: ExprId,
        positions: &[Option<usize>],
        arguments: &[ExprId],
    ) {
        let Some(type_pair) = self.state[type_expr].term else {
            return;
        };
        let Some(pair_items) = (unsafe { self.module.array_items(type_pair) }) else {
            return;
        };
        let Some(shape) = pair_items.first().map(|item| item.node) else {
            return;
        };
        let shape = self.module.as_dynamic(shape, self.current_block);
        let Some(field_items) = (unsafe { self.module.array_items(shape) }) else {
            return;
        };
        for (field, position) in field_items.iter().zip(positions) {
            let Some(argument) = position.and_then(|index| arguments.get(index)).copied() else {
                continue;
            };
            let slot = self.module.as_dynamic(field.node, self.current_block);
            for marker in P::Attr::ORDER {
                let Some(item) = self.slot_attribute(slot, marker.order_index() as u32) else {
                    continue;
                };
                let declared = self.module.as_dynamic(item, self.current_block);
                self.module.evaluate_node_deep(declared, None);
                let value = self
                    .node_value(declared)
                    .and_then(|value| AsEnum::<LowValue>::as_enum(&value));
                if value == Some(LowValue::Any) {
                    continue;
                }
                // The requirement is the slot's **value**: its `[value, type]` pair's
                // first element may itself be a frozen reference.
                let Some(LowValue::Array(items)) = value else {
                    continue;
                };
                // SAFETY: the array is a value of a live node of this module.
                let Some(element) = (unsafe { items.items() }).first().map(|item| item.node) else {
                    continue;
                };
                let declared = self.module.as_dynamic(element, self.current_block);
                self.module.evaluate_node_deep(declared, None);
                if self.node_value(declared).is_none() {
                    continue;
                }
                let loc = self.loc(argument, shape::PAIR_ATTR_BASE);
                self.unify_declared_value(argument, marker, declared, loc);
            }
        }
    }

    /// One guard per field whose type a supplied argument's type does not match.
    ///
    /// # Invariant
    /// The field's type read is forced before the unify: an undecided `Index` op
    /// takes the argument's type instead of refusing it.
    fn instantiation_field_types(
        &mut self,
        fields: NodeId,
        argument_types: &[Option<NodeId>],
        loc: crate::ir::Loc,
    ) -> bool {
        // SAFETY: `fields` is a live node of this module; nothing in this crate
        // calls `Module::drop_block`.
        let Some(fields) = (unsafe { self.module.array_items(fields) }) else {
            return true;
        };
        let nodes: Vec<AnyNodeId> = fields.iter().map(|field| field.node).collect();
        for (field, argument) in nodes.into_iter().zip(argument_types) {
            let Some(argument) = argument else { continue };
            let field = self.module.as_dynamic(field, self.current_block);
            let r#type = self.index(field, self.one());
            self.module.evaluate_node_deep(r#type, None);
            let before = self.module.unify_errors.len();
            self.check_unify(*argument, r#type, loc.clone(), DiagKind::Annotation);
            if self.module.unify_errors.len() > before {
                // One diagnostic per instantiation: the first field at fault.
                return false;
            }
        }
        true
    }

    /// A written struct type's field expressions, or `None` for any other callee.
    ///
    /// # Invariant
    /// The reconciliation is **syntactic**: only a source-level `struct{…}`
    /// spells the requirements, so a callee that does not has nothing to walk
    /// and the walk skips it by construction.
    fn written_struct_fields(&self, type_expr: ExprId) -> Option<Vec<ExprId>> {
        matches!(self.ir[type_expr].kind, ExprKind::TypeStruct { .. })
            .then(|| self.range_children(type_expr))
    }

    /// A named instantiation: validates the `.name` arguments against the
    /// name table and reorders into definition order.
    ///
    /// # Invariant
    /// `valid` is `false` once a structural mismatch is recorded, and the caller
    /// then skips the field-list unify. A callee the caller could not determine
    /// was refused before this point, so the name table here is the struct's own.
    fn named_instantiate(
        &mut self,
        e: ExprId,
        type_pair: NodeId,
        value: ExprId,
        arg_names: &[Option<&'static str>],
    ) -> NamedInstance {
        // The value's elements are the arguments in call order.
        let elem_ids = self.range_children(value);
        let vals: Vec<NodeId> = elem_ids
            .iter()
            .map(|&a| self.state[a].term.expect("an argument is compiled"))
            .collect();
        let tys: Vec<NodeId> = elem_ids
            .iter()
            .map(|&a| self.state[a].ty.unwrap())
            .collect();
        // No name table: a struct without one records each `.name`.
        if shape::struct_names_any(
            &mut self.module,
            self.type_expr,
            AnyNodeId::Dynamic(type_pair),
        )
        .is_none()
        {
            for (i, name) in arg_names.iter().enumerate() {
                if name.is_some() {
                    self.record_guard(
                        type_pair,
                        type_pair,
                        self.loc(elem_ids[i], 0),
                        DiagKind::StructAnonymousField,
                        *name,
                    );
                }
            }
            return NamedInstance {
                value: self.value_of(value),
                value_shape: self.state[value].ty.unwrap(),
                valid: false,
                assign: Vec::new(),
            };
        }
        // The definition's field count; `None` mid-recursion, which defers the
        // missing/excess checks to the probe unify.

        // SAFETY: `type_pair` is a live node of this module; nothing in this
        // crate calls `Module::drop_block`.
        let def_len = unsafe { self.module.array_items(type_pair) }
            .and_then(|items| items.first())
            // SAFETY: the field item's node is a live node of this module;
            // nothing in this crate calls `Module::drop_block`.
            .and_then(|item| unsafe { shape::array_items(&self.module, item.node) })
            .map(|items| items.len());
        // `assign[pos]` = the argument index supplying position `pos`.
        let mut assign: Vec<Option<usize>> = Vec::new();
        let mut valid = true;
        // An unknown name explains a gap, so it is the one diagnostic there.
        let mut unknown_field = false;
        // Named arguments claim their name's positional index.
        for (i, name) in arg_names.iter().enumerate() {
            if let Some(name) = name {
                match self.named_field_index_any(AnyNodeId::Dynamic(type_pair), name) {
                    Some(pos) => {
                        if pos >= assign.len() {
                            assign.resize(pos + 1, None);
                        }
                        if assign[pos].is_some() {
                            self.record_guard(
                                type_pair,
                                type_pair,
                                self.loc(elem_ids[i], 0),
                                DiagKind::StructDuplicateField,
                                Some(name),
                            );
                            valid = false;
                        } else {
                            assign[pos] = Some(i);
                        }
                    }
                    None => {
                        self.record_guard(
                            type_pair,
                            type_pair,
                            self.loc(elem_ids[i], 0),
                            DiagKind::StructUnknownField,
                            Some(name),
                        );
                        valid = false;
                        unknown_field = true;
                    }
                }
            }
        }
        // Positional arguments fill the lowest-numbered unclaimed position.
        let mut next = 0usize;
        for (i, name) in arg_names.iter().enumerate() {
            if name.is_none() {
                while next < assign.len() && assign[next].is_some() {
                    next += 1;
                }
                if next < assign.len() {
                    assign[next] = Some(i);
                    next += 1;
                } else if assign.len() >= def_len.unwrap_or(assign.len() + 1) {
                    // Every definition position is claimed; this is an excess.
                    self.record_guard(
                        type_pair,
                        type_pair,
                        self.loc(elem_ids[i], 0),
                        DiagKind::StructExcessField,
                        None,
                    );
                    valid = false;
                } else if next == assign.len() {
                    // Append a fresh position past the currently-claimed ones.
                    assign.push(Some(i));
                    next += 1;
                }
            }
        }
        // Missing fields — only when the definition's field count is known.
        if let Some(def_len) = def_len.filter(|_| !unknown_field) {
            if assign.len() < def_len {
                assign.resize(def_len, None);
            }
            for (pos, slot) in assign.iter().enumerate().take(def_len) {
                if slot.is_none() {
                    let name = self.struct_field_name(type_pair, pos);
                    self.record_guard(
                        type_pair,
                        type_pair,
                        self.loc(e, 0),
                        DiagKind::StructMissingField,
                        name,
                    );
                    valid = false;
                }
            }
        }
        if !valid {
            // The mismatch is the recorded diagnostic; return the call-order
            // value so the descent stays total.
            return NamedInstance {
                value: self.value_of(value),
                value_shape: self.state[value].ty.unwrap(),
                valid: false,
                assign: Vec::new(),
            };
        }
        // Reorder the values and their types into definition order, so the
        // instance reads positionally against the field list.
        let order: Vec<NodeId> = assign.iter().map(|&a| vals[a.unwrap()]).collect();
        let order_ty: Vec<NodeId> = assign.iter().map(|&a| tys[a.unwrap()]).collect();
        let value_node = self.array_node(self.current_block, &order);
        let value_shape = self.array_node(self.current_block, &order_ty);
        NamedInstance {
            value: value_node,
            value_shape,
            valid: true,
            assign,
        }
    }

    /// The lazy structural read `Index(base, at)`.
    fn index(&mut self, base: NodeId, at: NodeId) -> NodeId {
        let ops = self.array_node(self.current_block, &[base, at]);
        self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ops),
        )
    }

    /// The field name at a definition position, from the name table (`None` for
    /// an anonymous struct).
    fn struct_field_name(&mut self, type_pair: NodeId, pos: usize) -> Option<&'static str> {
        let table = shape::struct_names_any(
            &mut self.module,
            self.type_expr,
            AnyNodeId::Dynamic(type_pair),
        )?;
        // SAFETY: `table` is read from a live node of this module; nothing in
        // this crate calls `Module::drop_block`.
        for item in unsafe { table.items() } {
            if self
                .module
                .node_value(item.value)
                .and_then(|v| v.as_enum())
                .is_some_and(|v| v == LowValue::USize(pos))
                && let Some(LowValue::Str(name)) =
                    self.module.node_value(item.key).and_then(|v| v.as_enum())
            {
                return Some(name);
            }
        }
        None
    }

    /// A struct type expression. See `docs/notes/function-type-merge.md`.
    ///
    /// # Invariant
    /// A struct is polymorphic: the nominal id belongs in the **kind** slot (with
    /// `TypeStruct` as an inner tag), never the value shape, which is just the
    /// field-type list. The marker sits in the kind's marker slot like every
    /// other kind's, so a struct type's kind is a standard `[marker, K]` pair.
    pub(super) fn check_type_struct(&mut self, e: ExprId) -> NodeId {
        let (fields_range, names_range) = match self.ir[e].kind {
            ExprKind::TypeStruct { fields, names } => (fields, names),
            _ => unreachable!("check_type_struct on a non-struct expression"),
        };
        let elements: Vec<ExprId> =
            self.ir.children[fields_range.start as usize..fields_range.end as usize].to_vec();
        let names: Vec<Option<&'static str>> =
            self.ir.struct_names[names_range.start as usize..names_range.end as usize].to_vec();
        let mut slots = Vec::new();
        for &el in &elements {
            slots.push(self.element_slot_of(el));
        }
        let id = self.fresh_nominal_id(e);
        let (shape, kind, pair) = self.struct_type_type(id, &slots, &names);
        self.require_struct_field_names(&names, &elements, pair);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(shape);
        self.state[e].ty = Some(kind);
        pair
    }

    /// Refuse a struct **definition** with an unnamed field, at that field's
    /// own location (language-spec.md §Structs).
    ///
    /// # Invariant
    /// Every struct field carries a name: an instance reads by name, and the
    /// positional form is the *tuple* read. A block's fields need no check — they
    /// are its bindings.
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

    /// The struct's field names **in definition order**, mirroring
    /// [`Self::build_struct_names`].
    ///
    /// # Invariant
    /// This is the marker's `names_in_order` slot — the inverse of the
    /// name→index table, the one thing a lazy read of that table cannot derive.
    pub(super) fn build_struct_names_in_order(&mut self, names: &[Option<&'static str>]) -> NodeId {
        if !names.is_empty() && names.iter().all(|n| n.is_none()) {
            return self.alloc_node(
                self.current_block,
                None,
                Some(P::Value::from(LowValue::Error)),
            );
        }
        let mut items = Vec::with_capacity(names.len());
        for name in names {
            items.push(match name {
                Some(name) => self.name_node(name),
                // An unnamed field has no name to state: a refused build's hole,
                // not a case a read resolves through.
                None => self.alloc_node(
                    self.current_block,
                    None,
                    Some(P::Value::from(LowValue::Error)),
                ),
            });
        }
        self.array_node(self.current_block, &items)
    }
}
