//! Structs, fields, instantiation: the nominal id, the field reads, and
//! the instantiation checked against a field list.

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, NodeId};

use lichen_utils::extend::AsEnum;

use crate::diagnostic::{AssertSpelling, DiagKind};
use crate::ir::{ExprId, ExprKind};
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A positional slot read `a(k)` — a **tuple** element. See
    /// `docs/notes/eval-before-unify.md` §2.2.
    ///
    /// # Invariant
    /// A decided container is refused outright; an undecided one is pinned to a
    /// fresh tuple type, so the refusal is the application's argument unify, per
    /// call. A struct instance reads by name, so a struct type reaching this
    /// form is refused like any other non-tuple.
    pub(super) fn check_field(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let container_ty = self.state[container].ty.unwrap();
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let shape_cell = self.fresh_cell();
        let tuple_ty = self.array_node(self.current_block, &[shape_cell, kind]);
        // `kind_of` answers only for a decided term; a cell has none, which is
        // the case the pin exists for.
        let read_ty = match shape::kind_of(&self.module, AnyNodeId::Dynamic(container_ty)) {
            Some(container_kind) => {
                if !shape::kind_marker_is_any(
                    &mut self.module,
                    self.type_expr,
                    container_kind,
                    P::Value::tuple_type_marker(),
                ) {
                    // Recorded, not unified: stating the requirement beside it keeps
                    // the found side the container's own type.
                    self.record_guard(
                        container_ty,
                        tuple_ty,
                        self.loc(container, 1),
                        DiagKind::Guard,
                        None,
                    );
                }
                container_ty
            }
            None => {
                self.check_unify(
                    container_ty,
                    tuple_ty,
                    self.loc(container, 1),
                    DiagKind::Guard,
                );
                tuple_ty
            }
        };
        let container_value = self.value_of(container);
        let key_value = self.value_of(key);
        self.node_edges.insert(key_value, self.loc(key, 0));
        // The field's position, when the key is a literal: `slot_read` reads
        // the field's type out of the field list.
        let position = self.constant_position(key);
        let (value_node, ty_node) = self.slot_read(read_ty, container_value, key_value, position);
        let pair = self.pair_of(value_node, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// The structural slot read shared by `a(k)` and `a.name`.
    /// See `docs/notes/eval-before-unify.md`.
    ///
    /// # Invariant
    /// A known `position` makes the read's type decided rather than lazy: it is
    /// the field's own node out of the field list, which a class question can
    /// see through where an unevaluated `Index` it cannot. An undecided
    /// container keeps the lazy form, resolved at the apply.
    fn slot_read(
        &mut self,
        container_ty: NodeId,
        container_value: NodeId,
        key: NodeId,
        position: Option<usize>,
    ) -> (NodeId, NodeId) {
        let value_ops = self.array_node(self.current_block, &[container_value, key]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        // Only a **dynamic** field type is taken directly; a frozen one
        // is a different node, so it keeps the lazy form.
        let resolved = position
            .and_then(|at| {
                shape::field_type(
                    &self.module,
                    shape::TypeRef::Term(AnyNodeId::Dynamic(container_ty)),
                    at,
                )
            })
            .and_then(|ty| match ty {
                AnyNodeId::Dynamic(node) => Some(node),
                AnyNodeId::Static(_) => None,
            });
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
                self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::Index),
                    Some(ty_ops),
                )
            }
        };
        (value_node, ty_node)
    }

    /// A **raw** named component read `X::a` — see `docs/notes/raw-field.md`.
    ///
    /// # Invariant
    /// The container's **type** must be a TypeStruct kind, stated as a unify:
    /// a decided container is refused where it stands, an undecided one is
    /// pinned. The read is the element's own pair, so its type is the field's
    /// type *value* — `struct<.a Int, .b string>::a` is `Int : Type`.
    pub(super) fn check_raw_named_field(
        &mut self,
        e: ExprId,
        container: ExprId,
        name: &'static str,
    ) -> NodeId {
        self.check_expr(container);
        let container_ty = self.state[container].ty.unwrap();
        // The requirement, as a unify: the container's type must be a struct
        // kind — its `TypeStruct` tag, not a payload shape.
        let kind = self.struct_kind_requirement();
        // A decided container of another kind is refused and not read.
        if !self.check_unify(container_ty, kind, self.loc(container, 1), DiagKind::Guard) {
            return self.refused_pair(e);
        }
        // The struct marker's name table, read from the container's *type*.
        let names_node = self.lazy_index_path(container_ty, &shape::STRUCT_KIND_NAMES_PATH);
        // key = TableGet(names, name) — the field's positional index.
        let name_node = self.name_node(name);
        let key_ops = self.array_node(self.current_block, &[names_node, name_node]);
        let key = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::TableGet),
            Some(key_ops),
        );
        self.node_edges.insert(key, self.loc(e, 0));
        // The element's own pair, as the positional raw read builds it.
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
        let concrete = self.type_is_concrete(container_ty);
        // The field's position, resolved once for the guard below and the
        // read's type.

        // The lookup is the **structural** reader, asked whether the cell is
        // decided: a type through a class is readable.
        let position = self.named_field_index_any(AnyNodeId::Dynamic(container_ty), name);
        // The requirement is the container's **kind**, so the refusal names both
        // kinds (eval-before-unify.md §6.3).
        if concrete {
            let kind = self.struct_kind_requirement();
            let container_kind = self.lazy_index_path(container_ty, &[shape::TYPE_KIND_SLOT]);
            // The kind read runs first: an unevaluated `Index` holds no value,
            // so a unify writes into its class and asks nothing.
            self.module
                .evaluate_node(AnyNodeId::Dynamic(container_kind), None);
            self.check_unify(
                container_kind,
                kind,
                self.loc(container, 1),
                DiagKind::Guard,
            );
        } else {
            // An undecided container is registered as an assert condition, re-checked
            // per apply clone (eval-before-unify.md §6.2).
            let operands = self.array_node(self.current_block, &[container_ty, self.type_expr]);
            let condition = self.op_node(
                self.current_block,
                P::Operator::from(TypeOperator::IsStructType),
                Some(operands),
            );
            self.register_assert(
                condition,
                self.loc(container, 1),
                true,
                AssertSpelling::StructKind {
                    container: container_ty,
                },
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
                self.loc(container, 1),
                DiagKind::NamedField,
                Some(name),
            );
        }
        // The field's **subscript**: a constant position when the container
        // states it, else the lazy `TableGet(names, name)`.

        // A constant is not an optimisation: a kernel body is walked by
        // operands, and a `TableGet` is no value to resolve.
        let key = match position {
            Some(at) => self.alloc_node(
                self.current_block,
                None,
                Some(P::Value::from(LowValue::USize(at))),
            ),
            None => {
                // The struct marker's name table, read through the container
                // type's kind.
                let names_node = self.lazy_index_path(container_ty, &shape::STRUCT_TYPE_NAMES_PATH);
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
        let (value_node, ty_node) = self.slot_read(container_ty, container_value, key, position);
        let pair = self.pair_of(value_node, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
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
    fn named_field_index_any(&mut self, ty: AnyNodeId, name: &'static str) -> Option<usize> {
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
            vals.push(self.value_of(el));
            tys.push(self.state[el].ty.unwrap());
            names.push(Some(name));
        }
        let id = self.fresh_nominal_id();
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
            // Defer the check: pin the callee to a struct kind with its payload
            // written out — the deferred reorder reads it lazily.
            let id = self.fresh_cell();
            let names = self.fresh_cell();
            // The definition-order names, read by a named instantiation's
            // deferred reorder.
            let names_in_order = self.fresh_cell();
            let marker = self.struct_marker_node(id, names, names_in_order);
            let kind = self.kind_expr(self.current_block, marker);
            self.check_unify(
                callee_ty,
                kind,
                self.loc(type_expr, 1),
                DiagKind::InstantiateCallee,
            );
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
        let (value_node, value_shape, valid) = if any_named {
            self.named_instantiate(e, type_pair, callee_ty, value, arg_names, concrete)
        } else {
            // A positional instantiation keeps the value's own shape: a tuple's
            // element-type list, or the type itself.
            let value_ty = self.state[value].ty.unwrap();
            // SAFETY: `value_ty` is a live node of this module; nothing in
            // this crate calls `Module::drop_block`.
            let value_shape = match unsafe { self.module.array_items(value_ty) } {
                Some(items) if items.len() == 2 => {
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
                        shape
                    } else {
                        value_ty
                    }
                }
                _ => value_ty,
            };
            (self.value_of(value), value_shape, true)
        };
        if valid {
            self.check_unify(
                value_shape,
                field_list,
                self.loc(e, 1),
                DiagKind::Annotation,
            );
        }
        let pair = self.pair_of(value_node, type_pair);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(type_pair);
        pair
    }

    /// A named instantiation: validates the `.name` arguments against the
    /// name table and reorders into definition order.
    ///
    /// # Invariant
    /// `valid` is `false` once a structural mismatch is recorded, and the caller
    /// then skips the field-list unify. `callee_concrete` tells an anonymous
    /// struct (refused) from an unresolved callee, whose reorder is deferred
    /// ([`Self::lazy_named_instantiate`]).
    fn named_instantiate(
        &mut self,
        e: ExprId,
        type_pair: NodeId,
        callee_ty: NodeId,
        value: ExprId,
        arg_names: &[Option<&'static str>],
        callee_concrete: bool,
    ) -> (NodeId, NodeId, bool) {
        // The value's elements are the arguments in call order.
        let elem_ids = self.range_children(value);
        let vals: Vec<NodeId> = elem_ids.iter().map(|&a| self.value_of(a)).collect();
        let tys: Vec<NodeId> = elem_ids
            .iter()
            .map(|&a| self.state[a].ty.unwrap())
            .collect();
        // No name table: a concrete struct without one records each
        // `.name`; an unresolved callee defers the reorder.
        if shape::struct_names_any(
            &mut self.module,
            self.type_expr,
            AnyNodeId::Dynamic(type_pair),
        )
        .is_none()
        {
            if !callee_concrete {
                return self.lazy_named_instantiate(
                    type_pair, callee_ty, &elem_ids, arg_names, &vals, &tys,
                );
            }
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
            return (self.value_of(value), self.state[value].ty.unwrap(), false);
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
        if let Some(def_len) = def_len {
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
            return (self.value_of(value), self.state[value].ty.unwrap(), false);
        }
        // Reorder the values and their types into definition order, so the
        // instance reads positionally against the field list.
        let order: Vec<NodeId> = assign.iter().map(|&a| vals[a.unwrap()]).collect();
        let order_ty: Vec<NodeId> = assign.iter().map(|&a| tys[a.unwrap()]).collect();
        let value_node = self.array_node(self.current_block, &order);
        let value_shape = self.array_node(self.current_block, &order_ty);
        (value_node, value_shape, true)
    }

    /// The deferred named instantiation: an unresolved struct type, so the
    /// reorder is a lazy read (eval-before-unify.md).
    ///
    /// # Invariant
    /// A **duplicate** name is refused here — one name cannot supply two
    /// positions. A **missing** argument is not: the returned field-type list is
    /// a probe with one cell per argument, so the caller's field-list unify is
    /// an arity check that fires when the type resolves.
    fn lazy_named_instantiate(
        &mut self,
        type_pair: NodeId,
        callee_ty: NodeId,
        elem_ids: &[ExprId],
        arg_names: &[Option<&'static str>],
        vals: &[NodeId],
        tys: &[NodeId],
    ) -> (NodeId, NodeId, bool) {
        let mut valid = true;
        let mut supplied: Vec<&'static str> = Vec::new();
        for (i, name) in arg_names.iter().enumerate() {
            let Some(name) = *name else { continue };
            if supplied.contains(&name) {
                self.record_guard(
                    type_pair,
                    type_pair,
                    self.loc(elem_ids[i], 0),
                    DiagKind::StructDuplicateField,
                    Some(name),
                );
                valid = false;
            } else {
                supplied.push(name);
            }
        }
        if !valid {
            // The call-order value stands so the descent stays total.
            let value_node = self.array_node(self.current_block, vals);
            let value_shape = self.array_node(self.current_block, tys);
            return (value_node, value_shape, false);
        }
        let has_positional = arg_names.iter().any(|name| name.is_none());
        // Whether every argument's type is decided: then it joins the key, so
        // the lookup's equality is the field-type check.
        let typed = tys.iter().all(|&ty| self.type_is_concrete(ty));
        // The callee's field-type list, read lazily: `[shape, kind]`'s shape.
        let shape = self.index(type_pair, self.zero());
        // One entry per argument, keyed by name or positional rank, and by
        // type when every argument's type is decided.
        let mut entries = Vec::new();
        let mut positional_rank = 0usize;
        for (i, name) in arg_names.iter().enumerate() {
            let tag = match name {
                Some(name) => self.name_node(name),
                None => {
                    let tag = self.usize_node(positional_rank);
                    positional_rank += 1;
                    tag
                }
            };
            let key = if typed {
                self.array_node(self.current_block, &[tag, tys[i]])
            } else {
                tag
            };
            let value = self.usize_node(i);
            entries.push((AnyNodeId::Dynamic(key), AnyNodeId::Dynamic(value)));
        }
        let handle = self.module.build_table(&entries, self.current_block);
        let supply = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Table(handle))),
        );
        // The supplied names as a set value: what tells a named position from
        // one that takes the next positional argument.
        let mut supplied_nodes = Vec::with_capacity(supplied.len());
        for &name in &supplied {
            let node = self.name_node(name);
            supplied_nodes.push(node);
        }
        let supplied_names = self.array_node(self.current_block, &supplied_nodes);
        // The definition-order names, read lazily through the callee's kind.
        let names_in_order = self.lazy_index_path(callee_ty, &shape::STRUCT_KIND_NAMES_ORDER_PATH);
        let call_values = self.array_node(self.current_block, vals);
        // The field-type list is a **probe**: one fresh cell per argument, so
        // the caller's unify is an arity check.
        let mut order_ty = Vec::with_capacity(tys.len());
        for _ in 0..tys.len() {
            order_ty.push(self.fresh_cell());
        }
        let mut order = Vec::with_capacity(vals.len());
        // Definition positions already claimed by a named argument.
        let mut supplied_before = self.zero();
        for i in 0..vals.len() {
            let name_at = self.index_const(names_in_order, i);
            // The position's tag: its field name, or its positional rank.
            let tag = if has_positional {
                let is_named = self.in_domain(name_at, supplied_names);
                let rank = if i == 0 {
                    self.zero()
                } else {
                    let position = self.usize_node(i);
                    self.sub(position, supplied_before)
                };
                supplied_before = self.add(supplied_before, is_named);
                self.select(rank, name_at, is_named)
            } else {
                name_at
            };
            // The supplying argument's call slot. A tag no argument supplies is
            // a genuine miss, so the read is the refusal.
            let key = if typed {
                let field_type = self.index_const(shape, i);
                self.array_node(self.current_block, &[tag, field_type])
            } else {
                tag
            };
            self.node_edges.insert(key, self.loc(elem_ids[i], 0));
            let slot = self.table_get(supply, key);
            order.push(self.index(call_values, slot));
        }
        (
            self.array_node(self.current_block, &order),
            self.array_node(self.current_block, &order_ty),
            true,
        )
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

    /// [`Self::index`] with a constant subscript — the `Index(base, k)` of the
    /// encoding's positional offsets.
    fn index_const(&mut self, base: NodeId, k: usize) -> NodeId {
        let at = self.usize_node(k);
        self.index(base, at)
    }

    /// The lazy table read `TableGet(table, key)`.
    fn table_get(&mut self, table: NodeId, key: NodeId) -> NodeId {
        let ops = self.array_node(self.current_block, &[table, key]);
        self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::TableGet),
            Some(ops),
        )
    }

    /// Whether `value` is a member of the set `members` — the
    /// `InDomain` test, reading "an argument supplied it".
    fn in_domain(&mut self, value: NodeId, members: NodeId) -> NodeId {
        let ops = self.array_node(self.current_block, &[value, members]);
        self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::InDomain),
            Some(ops),
        )
    }

    /// The encoding's conditional `Index([first, second], cond)`: `second` when
    /// the condition is 1, `first` when it is 0.
    fn select(&mut self, first: NodeId, second: NodeId, condition: NodeId) -> NodeId {
        let arms = self.array_node(self.current_block, &[first, second]);
        let ops = self.array_node(self.current_block, &[arms, condition]);
        self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ops),
        )
    }

    /// The lazy `Add(left, right)` over machine scalars.
    fn add(&mut self, left: NodeId, right: NodeId) -> NodeId {
        let ops = self.array_node(self.current_block, &[left, right]);
        self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Add),
            Some(ops),
        )
    }

    /// The lazy `Sub(left, right)` over machine scalars.
    fn sub(&mut self, left: NodeId, right: NodeId) -> NodeId {
        let ops = self.array_node(self.current_block, &[left, right]);
        self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Sub),
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
        let mut tys = Vec::new();
        for &el in &elements {
            tys.push(self.check_type_element(el));
        }
        let id = self.fresh_nominal_id();
        let (shape, kind, pair) = self.struct_type_type(id, &tys, &names);
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
