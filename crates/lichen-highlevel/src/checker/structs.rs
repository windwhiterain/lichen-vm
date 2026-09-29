//! Structs, fields, and instantiation: the rules for a struct *type* — the
//! nominal id in the kind slot, the positional field types in the shape, the
//! name→index table a named read resolves through — for the positional and
//! named reads of a field, and for the positional and named instantiations that
//! check a value against a struct's field list.

use std::panic::{AssertUnwindSafe, catch_unwind};

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, NodeId, UnifyError};

use lichen_utils::extend::AsEnum;

use crate::diagnostic::{DiagKind, DiaryEntry};
use crate::ir::{ExprId, ExprKind};
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A positional slot read `a(k)` — a tuple element or a struct field
    /// (both type shapes are positional lists; the nominal struct id lives
    /// in the kind slot, so the extraction is the same for both).  The
    /// frontend emits this form for the adjacent single-expression paren —
    /// `a(1)` — the syntactic distinction from struct instantiation
    /// (`a(1,)`, `a(1,1)`, and the two zero-field spellings `a()` / `a(,)`,
    /// mirroring the tuple grammar's `()` unit vs `(,)` empty tuple) and
    /// from function application (a spaced paren), so no runtime kind
    /// dispatch decides the read.
    ///
    /// The value is the structural `Index` over the container's value; the
    /// type is `Index(shape, k)` over the container type's shape — read
    /// structurally from the type pair itself, so a concrete tuple/struct
    /// resolves at check time and an unbound container (a parameter, a call
    /// result) resolves when the call binds it.  A *concretely*
    /// non-positional container — an array (`a[i]` is its read), a table
    /// (`t{k}`), a function, an atomic type — is the guard's error below,
    /// not a runtime panic (mirroring the apply guard).
    pub(super) fn check_field(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let container_ty = self.ty[container].unwrap();
        let concrete = self
            .module
            .node_value(AnyNodeId::Dynamic(container_ty))
            .is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    None | Some(LowValue::USize(_)) | Some(LowValue::Array(_))
                )
            });
        if concrete && !shape::is_positional_type(&mut self.module, self.type_expr, container_ty) {
            let error_index = self.module.unify_errors.len();
            self.module.unify_errors.push(UnifyError {
                root_a: container_ty,
                root_b: container_ty,
                steps: Vec::new(),
                a: container_ty,
                b: container_ty,
                value_a: self.module.node_value(AnyNodeId::Dynamic(container_ty)),
                value_b: self.module.node_value(AnyNodeId::Dynamic(container_ty)),
            });
            self.diary.push(DiaryEntry {
                error_index,
                a: container_ty,
                b: container_ty,
                loc: self.loc(container, 1),
                kind: DiagKind::IndexTarget,
                field: None,
            });
        }
        let zero = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(0))),
        );
        let container_value = self.value_of(container);
        let key_value = self.value_of(key);
        self.node_edges.insert(key_value, self.loc(key, 0));
        let value_ops = self.array_node(self.current_block, &[container_value, key_value]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        let shape_ops = self.array_node(self.current_block, &[container_ty, zero]);
        let shape = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(shape_ops),
        );
        let ty_ops = self.array_node(self.current_block, &[shape, key_value]);
        let ty_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ty_ops),
        );
        let pair = self.pair_of(value_node, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// A **raw** named component read `X::a` (the glued `::` postfix).  It is
    /// *not* raw in the no-validation sense of [`Self::check_raw_index`]: the
    /// container's **type** must be a TypeStruct **kind**
    /// (`[TypeStruct{id, names}, K]` — the name→index table lies directly at
    /// `container_ty[0][1]`), a check-time requirement (a concretely non-struct
    /// container is a diagnostic; an unbound one stays lazy and resolves at the
    /// apply).  This is the sibling of [`Self::check_named_field`]'s `.a`,
    /// which instead requires the container's **kind** to be TypeStruct (its
    /// table at `container_ty[1][0][1]`).
    ///
    /// The value is the structural `Index` over the container's value at the
    /// name table's resolved index (`TableGet(names, name)`); the type is
    /// element 1 of the read — so `struct<.a Int, .b string>::a` is
    /// `Int : Type`, the field's *type* as a value (where `X<e>` reads a
    /// positional component, `X::a` reads a named one).
    pub(super) fn check_raw_named_field(
        &mut self,
        e: ExprId,
        container: ExprId,
        name: &'static str,
    ) -> NodeId {
        self.check_expr(container);
        let container_ty = self.ty[container].unwrap();
        let concrete = self
            .module
            .node_value(AnyNodeId::Dynamic(container_ty))
            .is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    None | Some(LowValue::USize(_)) | Some(LowValue::Array(_))
                )
            });
        if concrete
            && !shape::is_type_struct_kind_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(container_ty),
            )
        {
            self.record_index_target_error(container_ty, container, 1);
        }
        let zero = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(0))),
        );
        let one = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(1))),
        );
        // names — the struct marker's name table, read directly from the
        // container's *type* (a TypeStruct kind: marker at [0], names at [1]).
        let names_node =
            self.lazy_index_path(container_ty, &shape::STRUCT_KIND_NAMES_PATH, zero, one);
        // key = TableGet(names, name) — the field's positional index.
        let name_node = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Str(name))),
        );
        let key_ops = self.array_node(self.current_block, &[names_node, name_node]);
        let key = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::TableGet),
            Some(key_ops),
        );
        self.node_edges.insert(key, self.loc(e, 0));
        // value = Index(container_value, key); type = Index(value, 1).
        let container_value = self.value_of(container);
        let value_ops = self.array_node(self.current_block, &[container_value, key]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        let ty_ops = self.array_node(self.current_block, &[value_node, one]);
        let ty_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ty_ops),
        );
        self.term[e] = Some(value_node);
        self.val[e] = None;
        self.ty[e] = Some(ty_node);
        value_node
    }

    /// A named field read `a.name`.  The field name is resolved against the
    /// container type's struct name table (the `struct<.a T, …>` names) to
    /// the positional index, then read like a positional slot read
    /// ([`Self::check_field`]): the value is the structural `Index` over the
    /// container's value, the type is the shape slot at the resolved index.
    ///
    /// The index is derived **lazily** from the container type's names slot
    /// (`Index(Index(ty,1),2)` — the name table — then a `TableGet`), so an
    /// unbound container (a parameter, a call result) resolves when the call
    /// binds it, the same laziness as the positional form.  A *concretely*
    /// non-struct container, or a concrete struct whose name table has no such
    /// field, is the guard's error below — never a runtime panic.
    pub(super) fn check_named_field(
        &mut self,
        e: ExprId,
        container: ExprId,
        name: &'static str,
    ) -> NodeId {
        self.check_expr(container);
        let container_ty = self.ty[container].unwrap();
        let concrete = self
            .module
            .node_value(AnyNodeId::Dynamic(container_ty))
            .is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    None | Some(LowValue::USize(_)) | Some(LowValue::Array(_))
                )
            });
        if concrete {
            if !shape::is_struct_type_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(container_ty),
            ) {
                self.record_index_target_error(container_ty, container, 1);
            } else if self
                .named_field_index_any(AnyNodeId::Dynamic(container_ty), name)
                .is_none()
            {
                self.record_named_field_error(container_ty, container, 1, name);
            }
        }
        let zero = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(0))),
        );
        let one = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(1))),
        );
        // names — the struct marker's name table, read through the container
        // type's kind (`[shape, kind]`: kind at [1], marker at [0], names
        // at [1]).
        let names_node =
            self.lazy_index_path(container_ty, &shape::STRUCT_TYPE_NAMES_PATH, zero, one);
        // key = TableGet(names, name) — the field index.
        let name_node = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Str(name))),
        );
        let key_ops = self.array_node(self.current_block, &[names_node, name_node]);
        let key = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::TableGet),
            Some(key_ops),
        );
        self.node_edges.insert(key, self.loc(e, 0));
        let container_value = self.value_of(container);
        let value_ops = self.array_node(self.current_block, &[container_value, key]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        let shape_ops = self.array_node(self.current_block, &[container_ty, zero]);
        let shape = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(shape_ops),
        );
        let ty_ops = self.array_node(self.current_block, &[shape, key]);
        let ty_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ty_ops),
        );
        let pair = self.pair_of(value_node, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// The positional index of a named struct field, read from the struct
    /// type's name table at check time.  `None` when the type is not a
    /// struct, is an anonymous struct, or has no such named field.
    fn named_field_index_any(&mut self, ty: AnyNodeId, name: &'static str) -> Option<usize> {
        let table = shape::struct_names_any(&mut self.module, self.type_expr, ty)?;
        for item in table.items() {
            if self
                .module
                .node_value(item.key)
                .and_then(|v| v.as_enum())
                .is_some_and(|v| v == LowValue::Str(name))
            {
                if let Some(LowValue::USize(n)) =
                    self.module.node_value(item.value).and_then(|v| v.as_enum())
                {
                    return Some(n);
                }
            }
        }
        None
    }

    /// Whether a type cell's value is statically inspectable — a concrete
    /// type/kind expression (an array) or marker — as opposed to an unbound
    /// cell (a parameter, a deferred read), whose checks defer to the apply.
    /// The same predicate gates the field-read and function-ness guards.
    fn type_is_concrete(&self, ty: NodeId) -> bool {
        self.module
            .node_value(AnyNodeId::Dynamic(ty))
            .is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    None | Some(LowValue::USize(_)) | Some(LowValue::Array(_))
                )
            })
    }

    /// Struct instantiation: `s(1, 2)` — the positional tuple `value` is
    /// wrapped in the struct type `type_expr`.  The tuple's element-type
    /// list is checked against the struct's field list, and the
    /// expression's type is the struct type itself (the instance carries
    /// the nominal id; the tuple's own kind marker is discarded).  A
    /// non-tuple value fails the list check — a literal is not a struct
    /// value.
    pub(super) fn check_record(
        &mut self,
        e: ExprId,
        value: ExprId,
        field_names: &[Option<&'static str>],
    ) -> NodeId {
        // A struct-returning block: the value is a positional tuple of the
        // emitted field values.  The checker builds an anonymous struct type
        // whose shape is the value's element-type list and whose name table
        // maps each field name to its position, then wraps the value in it.
        // The instance is a fresh nominal type per occurrence, exactly like
        // an in-place `struct<…>` expression.
        self.check_expr(value);
        let elements = self.range_children(value);
        let mut vals = Vec::with_capacity(elements.len());
        let mut tys = Vec::with_capacity(elements.len());
        for &el in &elements {
            vals.push(self.value_of(el));
            tys.push(self.ty[el].unwrap());
        }
        let id = self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Fresh),
            None,
        );
        let shape = self.array_node(self.current_block, &tys);
        let names_node = self.build_struct_names(field_names);
        let marker = self.struct_marker_node(id, names_node);
        let kind = self.kind_expr(self.current_block, marker);
        let struct_ty = self.array_node(self.current_block, &[shape, kind]);
        let value_node = self.array_node(self.current_block, &vals);
        let pair = self.pair_of(value_node, struct_ty);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(struct_ty);
        pair
    }

    /// A struct instantiation `C(f1, …, fn)` — recognition is syntactic (the
    /// frontend lowers every glued comma-disciplined paren to
    /// [`ExprKind::Instantiate`]); the callee's struct-ness is validated here,
    /// type-directed: the callee's **type** must be a TypeStruct kind
    /// (structs are nominal — a tuple or function type cannot instantiate).
    /// A concretely non-struct callee is a diagnostic at the callee
    /// ([`DiagKind::InstantiateCallee`]); an unbound callee (a parameter, a
    /// deferred read) is *pinned* to a struct kind so a non-struct actual
    /// callee fails the apply's argument unify per call — the same pinning
    /// [`Self::check_binop`] applies to its operands.
    pub(super) fn check_instantiate(
        &mut self,
        e: ExprId,
        type_expr: ExprId,
        value: ExprId,
        arg_names: &[Option<&'static str>],
    ) -> NodeId {
        self.check_expr(type_expr);
        self.check_expr(value);
        let type_pair = self.term[type_expr].unwrap();
        // An unevaluated callee (a call result, `(mk (Int))(1, 2)`) has no
        // statically readable pair — it is an apply node, not an array.  Force
        // its evaluation so the nominality check and the field-list read see
        // the concrete struct type.  A callee that depends on an unbound
        // parameter stays lazy (the checks below defer to the apply), and a
        // non-terminating one trips the VM's guard: leave it lazy — the
        // build's statement pass evaluates the statement again and reports
        // the `NonTerminating` diagnostic.  After a caught guard the module's
        // counters are inflated, so never force twice.
        if self.module.array_items(type_pair).is_none() && !self.force_failed {
            let forced = catch_unwind(AssertUnwindSafe(|| {
                self.module.evaluate_node_deep(type_pair, None);
            }));
            self.force_failed = forced.is_err();
        }
        let callee_ty = self.ty[type_expr].unwrap();
        let concrete = self.type_is_concrete(callee_ty);
        let any_named = arg_names.iter().any(|n| n.is_some());
        if concrete
            && !shape::is_type_struct_kind_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(callee_ty),
            )
        {
            // Nominality: only a struct type instantiates.  The field checks
            // are skipped — against a non-struct callee they would only
            // cascade.  The instance still gets a term (the call-order value
            // under the callee's pair), so the descent stays total.
            self.record_instantiate_callee_error(type_pair, type_expr);
            let value_node = self.value_of(value);
            let pair = self.pair_of(value_node, type_pair);
            self.term[e] = Some(pair);
            self.val[e] = Some(value_node);
            self.ty[e] = Some(type_pair);
            return pair;
        }
        if !concrete {
            // Defer the nominality check: pin the callee's type to a struct
            // kind `[[id, names], K]` (a 2-element marker, per
            // [`shape::is_struct_marker_any`]'s structural guess).  The pin
            // binds an unbound cell now and is re-checked by the apply's
            // argument unify per call, so a tuple/function/atomic actual
            // callee is rejected there.
            let id = self.fresh_cell();
            let names = self.fresh_cell();
            let marker = self.struct_marker_node(id, names);
            let kind = self.kind_expr(self.current_block, marker);
            self.check_unify(
                callee_ty,
                kind,
                self.loc(type_expr, 1),
                DiagKind::InstantiateCallee,
            );
        }
        // The struct pair's shape *is* the positional field-type list (the
        // nominal id lives in the kind slot).  During a recursive struct's
        // own descent the shape is still an unbound cell (the bindings are
        // mutually recursive), so defer the field-list check through a probe
        // cell; a callee whose pair stayed unreadable after the force (it
        // depends on an unbound parameter) reads the shape through a lazy
        // `Index` that resolves when the call binds it.
        let field_list = match self
            .module
            .array_items(type_pair)
            .and_then(|items| items.first())
        {
            Some(item) => {
                let shape = self.module.as_dynamic(item.node, self.current_block);
                match self.module.array_items(shape) {
                    Some(_) => shape,
                    // The struct's shape cell is not resolved yet
                    // (mid-recursion): bind it to a [field-list] probe and
                    // check the value against the probe slot.  When the
                    // descent completes the cell unifies with the real shape,
                    // closing the deferred check.
                    _ => {
                        let fields_cell = self.fresh_cell();
                        self.module.unify(shape, fields_cell);
                        fields_cell
                    }
                }
            }
            // Lazy shape read: `Index(type_pair, 0)`.  Known limitation: the
            // deferred unify below resolves through the lowlevel's
            // pending-`Index` deferral, which accepts only a 2-element
            // concrete other side (`class_holds_type`) — a param-dependent
            // call-result callee whose struct has ≠2 fields reports the
            // field-list mismatch at check time instead of at the apply.
            // Phase 2's unification-hook extraction (D1) subsumes that rule.
            _ => {
                let zero = self.alloc_node(
                    self.current_block,
                    None,
                    Some(P::Value::from(LowValue::USize(0))),
                );
                let ops = self.array_node(self.current_block, &[type_pair, zero]);
                self.op_node(
                    self.current_block,
                    P::Operator::from(LowOperator::Index),
                    Some(ops),
                )
            }
        };
        let (value_node, value_shape, valid) = if any_named {
            self.named_instantiate(e, type_pair, value, arg_names, concrete)
        } else {
            // A positional instantiation keeps the value's own tuple shape.
            // The value's shape: the element-type list of a tuple type, or
            // the type itself for anything else (which then fails the list
            // check).
            let value_ty = self.ty[value].unwrap();
            let value_shape = match self.module.array_items(value_ty) {
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
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(type_pair);
        pair
    }

    /// A named struct instantiation: validates the `.name` arguments against
    /// the struct type's name table, reorders the argument values into the
    /// definition's positional order, and returns `(value_node, value_shape,
    /// valid)`.  `valid` is `false` when a structural mismatch was recorded
    /// (an unknown, duplicate, missing, or excess field, or a `.name`
    /// argument against an anonymous struct) — the caller then skips the
    /// ordinary field-list unify, since the mismatch is the structural one.
    ///
    /// `callee_concrete` distinguishes the two ways the name table can be
    /// unavailable: a concrete but anonymous struct (a `.name` argument is a
    /// [`DiagKind::StructAnonymousField`]) and a callee whose type is not
    /// statically known (an unbound parameter — the definition-order reorder
    /// cannot be computed at check time, a
    /// [`DiagKind::InstantiateNamesNotStatic`]).
    fn named_instantiate(
        &mut self,
        e: ExprId,
        type_pair: NodeId,
        value: ExprId,
        arg_names: &[Option<&'static str>],
        callee_concrete: bool,
    ) -> (NodeId, NodeId, bool) {
        // The value's tuple elements are the argument value expressions (call
        // order); their values and types are reordered into definition order.
        let elem_ids = self.range_children(value);
        let vals: Vec<NodeId> = elem_ids.iter().map(|&a| self.value_of(a)).collect();
        let tys: Vec<NodeId> = elem_ids.iter().map(|&a| self.ty[a].unwrap()).collect();
        // The name table is unavailable: record each `.name` argument and
        // fall back to the call-order value (the caller skips the field-list
        // unify).
        if shape::struct_names_any(
            &mut self.module,
            self.type_expr,
            AnyNodeId::Dynamic(type_pair),
        )
        .is_none()
        {
            let kind = if callee_concrete {
                // A concrete struct without a name table genuinely has no
                // named fields.
                DiagKind::StructAnonymousField
            } else {
                DiagKind::InstantiateNamesNotStatic
            };
            for (i, name) in arg_names.iter().enumerate() {
                if name.is_some() {
                    self.record_struct_error(elem_ids[i], type_pair, *name, kind);
                }
            }
            return (self.value_of(value), self.ty[value].unwrap(), false);
        }
        // The definition's field count (the shape's length) — `None` when the
        // shape is still an unbound cell mid-recursion, in which case the
        // missing/excess checks are deferred to the probe unify.
        let def_len = self
            .module
            .array_items(type_pair)
            .and_then(|items| items.get(0))
            .and_then(|item| shape::array_items(&self.module, item.node))
            .map(|items| items.len());
        // `assign[pos]` = the argument index supplying definition position
        // `pos`.  `valid` flips when a structural mismatch is recorded.
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
                            self.record_struct_error(
                                elem_ids[i],
                                type_pair,
                                Some(name),
                                DiagKind::StructDuplicateField,
                            );
                            valid = false;
                        } else {
                            assign[pos] = Some(i);
                        }
                    }
                    None => {
                        self.record_struct_error(
                            elem_ids[i],
                            type_pair,
                            Some(name),
                            DiagKind::StructUnknownField,
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
                    self.record_struct_error(
                        elem_ids[i],
                        type_pair,
                        None,
                        DiagKind::StructExcessField,
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
            for pos in 0..def_len {
                if assign[pos].is_none() {
                    let name = self.struct_field_name(type_pair, pos);
                    self.record_struct_error(e, type_pair, name, DiagKind::StructMissingField);
                    valid = false;
                }
            }
        }
        if !valid {
            // The structural mismatch is the recorded diagnostic; return the
            // call-order value so the checker still produces a term, but the
            // caller skips the field-list unify.
            return (self.value_of(value), self.ty[value].unwrap(), false);
        }
        // Reorder the argument values and their element types into definition
        // order, so the instance's value reads positionally against the
        // field list and a `.b` / `a(0)` read sees the definition's order.
        let order: Vec<NodeId> = assign.iter().map(|&a| vals[a.unwrap()]).collect();
        let order_ty: Vec<NodeId> = assign.iter().map(|&a| tys[a.unwrap()]).collect();
        let value_node = self.array_node(self.current_block, &order);
        let value_shape = self.array_node(self.current_block, &order_ty);
        (value_node, value_shape, true)
    }

    /// The field name at a definition position, from the struct's name table
    /// (`None` for a positional field or an anonymous struct).
    fn struct_field_name(&mut self, type_pair: NodeId, pos: usize) -> Option<&'static str> {
        let table = shape::struct_names_any(
            &mut self.module,
            self.type_expr,
            AnyNodeId::Dynamic(type_pair),
        )?;
        for item in table.items() {
            if self
                .module
                .node_value(item.value)
                .and_then(|v| v.as_enum())
                .is_some_and(|v| v == LowValue::USize(pos))
            {
                if let Some(LowValue::Str(name)) =
                    self.module.node_value(item.key).and_then(|v| v.as_enum())
                {
                    return Some(name);
                }
            }
        }
        None
    }

    /// A struct type expression.  A struct is *polymorphic*: the nominal
    /// type id identifies its constructor, and instantiating it with field
    /// types produces a concrete type.  So the id belongs in the kind slot
    /// (with `TypeStruct` as an inner tag), not in the value shape, which is
    /// just the field-type list:
    ///
    /// ```text
    /// pair = [ shape, kind ]
    /// shape = [ field types… ]
    /// kind  = [ TypeStruct{id, names}, K ]
    /// ```
    ///
    /// The `TypeStruct` marker is a **two-field value** `[id, names]` — the
    /// nominal id (a per-compilation
    /// [`P::Operator::from(TypeOperator::Fresh)`] call, so two occurrences keep
    /// distinct ids) plus the optional name→index table.  It sits in the kind's
    /// marker slot, exactly like a function/tuple/array/table kind's marker, so
    /// a struct type's kind is a standard `[marker, K]` pair.  Fields are
    /// positional unless a field carries a `.name Ty` prefix, in which case
    /// `names` maps each field name to its positional index (the map an
    /// `a.name` read resolves through).
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
        let id = self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Fresh),
            None,
        );
        let shape = self.array_node(self.current_block, &tys);
        let names_node = self.build_struct_names(&names);
        // TypeStruct{id, names} — the two-field struct marker.
        let marker = self.struct_marker_node(id, names_node);
        let kind = self.kind_expr(self.current_block, marker);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.term[e] = Some(pair);
        self.val[e] = Some(shape);
        self.ty[e] = Some(kind);
        pair
    }

    /// The struct name→index table value for a field-name list: the
    /// [`LowValue::Void`] marker when every field is unnamed (an anonymous
    /// positional struct has no name table — a *computed nothing*, not the
    /// unit value), otherwise a constant `Table` mapping each field name to
    /// its positional index.  The table's keys are the field names (string
    /// values), its values the field indices — the map an `a.name` read
    /// resolves through.  A named read over the marker misses with a
    /// recorded [`EvalError::TableMiss`], never a panic.
    fn build_struct_names(&mut self, names: &[Option<&'static str>]) -> NodeId {
        if names.iter().all(|n| n.is_none()) {
            return self.alloc_node(
                self.current_block,
                None,
                Some(P::Value::from(LowValue::Void)),
            );
        }
        let mut entries = Vec::new();
        for (i, name) in names.iter().enumerate() {
            if let Some(name) = name {
                let key = self.alloc_node(
                    self.current_block,
                    None,
                    Some(P::Value::from(LowValue::Str(name))),
                );
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
