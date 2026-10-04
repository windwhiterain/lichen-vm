//! Structs, fields, and instantiation: the rules for a struct *type* — the
//! nominal id in the kind slot, the positional field types in the shape, the
//! name→index table a named read resolves through — for the positional and
//! named reads of a field, and for the positional and named instantiations that
//! check a value against a struct's field list.

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, NodeId};

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
    /// A positional slot read `a(k)` — a **tuple** element.  The frontend
    /// emits this form for the adjacent single-expression paren — `a(1)` — the
    /// syntactic distinction from struct instantiation (`a(1,)`, `a(1,1)`, and
    /// the two zero-field spellings `a()` / `a(,)`, mirroring the tuple
    /// grammar's `()` unit vs `(,)` empty tuple) and from function application
    /// (a spaced paren), so no runtime kind dispatch decides the read.
    ///
    /// **A struct instance reads by name** (`s.x`, `X::a`), so a struct type
    /// reaching this form is refused like any other non-tuple.  That is the
    /// same principle the array read follows ([`Self::check_index`]): the syntax
    /// picks the operator, and the refusal is the type's, never a runtime
    /// dispatch on the container's kind.
    ///
    /// The value is the structural `Index` over the container's value; the type
    /// is `Index(shape, k)` over the container type's shape.
    ///
    /// **The check is a unify, in one of two forms the container's state picks.**
    /// A *decided* container is refused outright — its kind is readable, so the
    /// term is judged where it is (`kind_marker_is_any`) and the refusal is
    /// recorded as a fact, with the same stated requirement the pin states.  An
    /// *undecided* container (a parameter, a call result) is **pinned** to a
    /// fresh tuple type `[?shape, [TypeTuple, K]]` — the mirror of
    /// `check_index`'s array pin — so the refusal is the application's argument
    /// unify, per call.  Both name the requirement as that open tuple type,
    /// which prints `<?a, …>`: a tuple whose arity is not decided.
    ///
    /// That per-call tier is the whole reason this is a unify and not a
    /// skip-when-undecided guard: a guard asked once, while a parameter's type
    /// is still a cell, is never asked again, so `x(0)` over an array used to be
    /// **accepted** — and a struct was accepted by the same hole
    /// (`docs/notes/eval-before-unify.md` §2.2/§2.4).
    ///
    /// An out-of-range slot is not a check: the runtime `Index` read records it
    /// (`IndexOutOfBounds`), reporting the container's actual arity.
    pub(super) fn check_field(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let container_ty = self.state[container].ty.unwrap();
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let shape_cell = self.fresh_cell();
        let tuple_ty = self.array_node(self.current_block, &[shape_cell, kind]);
        // `kind_of` answers only for a term the graph has already decided; a
        // cell (an unbound parameter, a call result) has none, and that is
        // exactly the case the pin exists for.
        let read_ty = match shape::kind_of(&self.module, AnyNodeId::Dynamic(container_ty)) {
            Some(container_kind) => {
                if !shape::kind_marker_is_any(
                    &mut self.module,
                    self.type_expr,
                    container_kind,
                    P::Value::tuple_type_marker(),
                ) {
                    // Recorded, not unified: the term is decided, so there is
                    // nothing to defer, and stating the requirement beside it
                    // keeps the found side the *container's* type rather than a
                    // shape a failed unify would have bound.
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
        // The field's position, when the key is a literal: the pinned shape is
        // a cell whose class holds the container's field list once the pin is
        // decided, so `slot_read` reads the field's type out of it instead of
        // leaving an `Index` no class question can see through.
        let position = self.constant_position(key);
        let (value_node, ty_node) = self.slot_read(read_ty, container_value, key_value, position);
        let pair = self.pair_of(value_node, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// The structural slot read shared by the positional form `a(k)` and the
    /// named form `a.name`: `value = Index(container_value, key)`, and the
    /// **type** is the field's own type node out of the container type's field
    /// list.  `key` is already the resolved slot: the argument's own value, or a
    /// struct name table's read.
    ///
    /// **`position` is the field's position, and it is what makes the type
    /// decided rather than lazy.**  A concrete container type states it — the
    /// named form resolves the name through the type's own name table for its
    /// guard ([`Self::named_field_index_any`]), and the positional form's key is
    /// a literal — so the type is read straight out of the field list
    /// ([`shape::field_type`]), which is the same node an
    /// `Index(Index(container_ty, 0), key)` would evaluate to, one unify
    /// earlier.
    ///
    /// That earliness is **observable**, and it is the reason this is not a
    /// cosmetic change: a class question asked of an operand reads the type
    /// *cell* with [`shape::low_type_of_slot`], which cannot see through an
    /// unevaluated `Index` — so `(x : struct<.a Float>) => x.a + x.a` used to
    /// find neither operand concretely `Float`, pin the operation to the `Int`
    /// default (`check_binop`), and then refuse both operands against it.
    ///
    /// An **undecided** container (`position` is `None`) keeps the lazy form:
    /// its type tree is not known here, and the `Index` is what resolves when the
    /// apply binds it.
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
        // Only a **dynamic** field type is taken directly: a frozen field type
        // (one an imported package's type is built from) belongs to its own
        // module's arena, and materializing a copy of it here would be a
        // different node than the one the lazy `Index` evaluates to.  Such a
        // field keeps the lazy form — the pre-existing behaviour — rather than
        // getting a copy that unifies as something else.
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

    /// A **raw** named component read `X::a` (the glued `::` postfix).  It is
    /// *not* raw in the no-validation sense of [`Self::check_raw_index`]: the
    /// container's **type** must be a TypeStruct **kind**
    /// (`[[TypeId, names, names_in_order], TypeStruct]` — the name→index
    /// table lies directly in the marker payload at
    /// `container_ty[0][0][1]`), a check-time requirement (a concretely non-struct
    /// container is a diagnostic; an unbound one stays lazy and resolves at the
    /// apply).  This is the sibling of [`Self::check_named_field`]'s `.a`,
    /// which instead requires the container's **kind** to be TypeStruct (its
    /// table at `container_ty[1][0][0][1]`).
    ///
    /// The read is the element's own pair at the name table's resolved index
    /// (`TableGet(names, name)`) — its slot 0 is the value and its slot 1 the
    /// type — so `struct<.a Int, .b string>::a` is
    /// `Int : Type`, the field's *type* as a value (where `X<e>` reads a
    /// positional component, `X::a` reads a named one).
    pub(super) fn check_raw_named_field(
        &mut self,
        e: ExprId,
        container: ExprId,
        name: &'static str,
    ) -> NodeId {
        self.check_expr(container);
        let container_ty = self.state[container].ty.unwrap();
        // The requirement, stated as a unify for **both** tiers: the container's
        // type must be a struct kind.  A decided container is judged where it is,
        // an undecided one is pinned, and the diagnostic names the same
        // requirement either way (`expected TypeStruct, found …`) — the
        // struct-kind sibling of the positional read's tuple pin, and the same
        // statement [`Super::check_instantiate`] pins.  The marker's *type* slot
        // is the `TypeStruct` atom, so this is a check of the tag (a
        // `[?payload, TypeStruct]` marker pair), not of a payload shape.
        let kind = self.struct_kind_requirement();
        self.check_unify(container_ty, kind, self.loc(container, 1), DiagKind::Guard);
        // names — the struct marker's name table, read directly from the
        // container's *type* (a TypeStruct kind: marker at [0], its payload at
        // [0], the names at [1] of the payload).
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
        // The element's own pair, exactly as the positional raw read builds it
        // (see [`Self::check_raw_index`] for why the *term* is that pair and
        // not the bare element).
        let container_value = self.value_of(container);
        let (pair, element, ty_node) = self.element_read(container_value, key);
        // As there: the element is the target of both slot reads.
        self.node_edges.insert(element, self.loc(e, 1));
        self.state[e].term = Some(pair);
        self.state[e].val = None;
        self.state[e].ty = Some(ty_node);
        pair
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
        let container_ty = self.state[container].ty.unwrap();
        let concrete = self.type_is_concrete(container_ty);
        // The field's position, resolved **once** for both the guard below and
        // the read's type: a concrete struct that has this field states where it
        // is, and `slot_read` uses that to read the field's type out of the field
        // list rather than leaving an `Index` no class question can see through.
        let position = concrete
            .then(|| self.named_field_index_any(AnyNodeId::Dynamic(container_ty), name))
            .flatten();
        // The requirement is the container's **kind** — the corresponding slot of
        // the `[shape, kind]` term — stated as a unify, so the refusal names the
        // two kinds (`expected TypeStruct, found TypeArray`) instead of the
        // shared "tuple, array, or struct" wording this read does not accept
        // (`docs/notes/eval-before-unify.md` §6.3).  It is the same statement the
        // raw named read makes: the marker is the `[?payload, TypeStruct]` pair,
        // so the `TypeStruct` tag is what the unify checks.
        if concrete {
            let kind = self.struct_kind_requirement();
            let container_kind = self.lazy_index_path(container_ty, &[shape::TYPE_KIND_SLOT]);
            self.check_unify(
                container_kind,
                kind,
                self.loc(container, 1),
                DiagKind::Guard,
            );
        }
        // An *undecided* container keeps the lazy name-table read and its
        // requirement stays unstated until the apply decides it — the open half
        // recorded in `docs/notes/eval-before-unify.md` §2.4/§6.2.  A term-shaped
        // pin here would bind the container's own type cell, and a consumer that
        // reads a type *structurally* through the class (the compute extension
        // forces a template's parameter type before any apply) would then see the
        // pin's open cells instead of the deferred real type: measured, the
        // extension's suite goes from its 58/2 baseline to 8/52.
        if concrete
            && shape::is_struct_type_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(container_ty),
            )
            && position.is_none()
            // ... and its **name table is readable**.  A table that is readable
            // and lacks this name is a genuine miss; one that is still unbound is
            // only undecided, and the read stays lazy.  Without this distinction
            // a *second* `k.name` on the same undecided container is refused
            // falsely: the first read's pin is an array value, so the container
            // now looks decided while the pin's name table is a fresh cell.
            && shape::struct_names_any(
                &mut self.module,
                self.type_expr,
                AnyNodeId::Dynamic(container_ty),
            )
            .is_some()
        {
            // The container is a struct but has no field of this name: the
            // offending name rides in the entry, so the language layer can
            // append a did-you-mean clause.  Only reached for a struct — a
            // non-struct already failed the unify above, and a second
            // diagnostic would only cascade.
            self.record_guard(
                container_ty,
                container_ty,
                self.loc(container, 1),
                DiagKind::NamedField,
                Some(name),
            );
        }
        // The field's **subscript**: a constant position when the container type
        // is concrete and states it, and otherwise the lazy
        // `TableGet(names, name)` a name table resolves through — which is what
        // keeps an *unbound* container's named read resolvable at the apply.
        //
        // **A constant is not an optimisation here; it is the whole difference
        // for the lowering.**  A kernel body is walked by its operands, and a
        // name table's `TableGet` is not a value anything can resolve without
        // re-deriving the struct's field order — so a concrete read's field
        // position is written where it is decided (`slot_read`'s type read is
        // the other half of the same decision).  The lazy form stays for the one
        // case that needs it: a container whose type is not known yet.
        let key = match position {
            Some(at) => self.alloc_node(
                self.current_block,
                None,
                Some(P::Value::from(LowValue::USize(at))),
            ),
            None => {
                // names — the struct marker's name table, read through the
                // container type's kind (`[shape, kind]`: kind at [1], marker at
                // [0], names at [1]).
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

    /// The positional index of a named struct field, read from the struct
    /// type's name table at check time.  `None` when the type is not a
    /// struct, is an anonymous struct, or has no such named field.
    ///
    /// The **universe gate stays here** (`struct_names_any` compares against the
    /// checker's canonical universe node); the fold over the table's entries is
    /// [`shape::name_table_index`], shared with the lowering's reader so the two
    /// cannot decode an entry differently.
    fn named_field_index_any(&mut self, ty: AnyNodeId, name: &'static str) -> Option<usize> {
        let table = shape::struct_names_any(&mut self.module, self.type_expr, ty)?;
        shape::name_table_index(&self.module, table, name)
    }

    /// The position a positional key expression states, when it states a
    /// constant one: the `USize` its value node holds.  `None` for a key that
    /// is computed or still undecided — the case where the field's position,
    /// and so its type, is only known once the apply binds the container, which
    /// is why [`Self::slot_read`] keeps its lazy form there.
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

    /// Whether a type cell's value is statically inspectable — a concrete
    /// type/kind expression (an array) or marker — as opposed to an unbound
    /// cell (a parameter, a deferred read), whose checks defer to the apply.
    /// The same predicate gates the field-read and function-ness guards.
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

    /// A struct-returning block (`{ x = 1; y = Int }`): the checker builds an
    /// anonymous struct type whose shape is the **bindings'** value-type list
    /// and whose name table maps each binding's name to its position, then wraps
    /// those values in it.  The instance is a fresh nominal type per occurrence,
    /// exactly like an in-place `struct<…>` expression.
    ///
    /// **An expression statement is not a field.**  A block's record is its
    /// *bindings*: a bare expression is an ordinary statement, checked like any
    /// other (so its diagnostics fire) but discarded, which is why the frontend
    /// hands the statements' value tuple and their name list index-aligned and
    /// this keeps the named positions alone.  A block's fields are therefore
    /// always named, like a `struct<…>` declaration's
    /// (`docs/language-spec.md` §Blocks).
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
                // A bare expression: no field, its value discarded.  It was
                // checked with the rest of the block, and nothing references its
                // value node, so it is not evaluated either — the same laziness a
                // statement nothing reads has anywhere else.
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
        let type_pair = self.state[type_expr].term.unwrap();
        // An unevaluated callee (a call result, `(mk (Int))(1, 2)`) has no
        // statically readable pair — it is an apply node, not an array.  Force
        // its evaluation so the nominality check and the field-list read see
        // the concrete struct type.  A callee that depends on an unbound
        // parameter stays lazy (the checks below defer to the apply), and a
        // non-terminating one trips the VM's guard: leave it lazy — the
        // build's statement pass evaluates the statement again and reports
        // the `NonTerminating` diagnostic.  A refused callee leaves a partly
        // walked graph, so never force twice.
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
            // Nominality: only a struct type instantiates.  The field checks
            // are skipped — against a non-struct callee they would only
            // cascade.  The instance still gets a term (the call-order value
            // under the callee's pair), so the descent stays total.
            let value_node = self.value_of(value);
            let pair = self.pair_of(value_node, type_pair);
            self.state[e].term = Some(pair);
            self.state[e].val = Some(value_node);
            self.state[e].ty = Some(type_pair);
            return pair;
        }
        if !concrete {
            // Defer the nominality check: pin the callee's type to a struct
            // kind whose marker is the `[payload, TypeStruct]` pair — the
            // payload `[?id, ?names, ?order]` left open, the `TypeStruct` tag
            // decided ([`shape::is_struct_marker_any`]).  The payload's shape is
            // written here (rather than left to one cell) because the deferred
            // named reorder reads its slots lazily
            // ([`shape::STRUCT_KIND_NAMES_ORDER_PATH`]).  The pin
            // binds an unbound cell now and is re-checked by the apply's
            // argument unify per call, so a tuple/function/atomic actual
            // callee is rejected there.
            let id = self.fresh_cell();
            let names = self.fresh_cell();
            // The definition-order names are what a named instantiation's
            // deferred reorder reads; an all-positional one never does.
            let names_in_order = self.fresh_cell();
            // Kind only, so `Self::struct_type_type` does not apply: the pin
            // has no field-type shape to wrap, and its marker's payload fields
            // are unbound cells rather than a `Fresh` id and the two name forms.
            let marker = self.struct_marker_node(id, names, names_in_order);
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
                // Lazy shape read: `Index(type_pair, 0)`.  The deferred unify
                // below resolves through the lowlevel's pending-`Index`
                // deferral: a pending read against a class that holds a type
                // value merges and commits the type value
                // ([`shape::defer_pending`]), so a param-dependent call-result
                // callee checks against the real fields at the apply.
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
            // A positional instantiation keeps the value's own tuple shape.
            // The value's shape: the element-type list of a tuple type, or
            // the type itself for anything else (which then fails the list
            // check).
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
    /// statically known — the reorder is then *deferred* rather than refused,
    /// because an unresolved struct type leaves the instantiation unresolved
    /// too ([`Self::lazy_named_instantiate`]).
    fn named_instantiate(
        &mut self,
        e: ExprId,
        type_pair: NodeId,
        callee_ty: NodeId,
        value: ExprId,
        arg_names: &[Option<&'static str>],
        callee_concrete: bool,
    ) -> (NodeId, NodeId, bool) {
        // The value's tuple elements are the argument value expressions (call
        // order); their values and types are reordered into definition order.
        let elem_ids = self.range_children(value);
        let vals: Vec<NodeId> = elem_ids.iter().map(|&a| self.value_of(a)).collect();
        let tys: Vec<NodeId> = elem_ids
            .iter()
            .map(|&a| self.state[a].ty.unwrap())
            .collect();
        // The name table is unavailable.  A *concrete* struct without one
        // genuinely has no named fields, so each `.name` argument is recorded
        // and the call-order value stands (the caller skips the field-list
        // unify).  An **unresolved** callee is a different fact: the struct
        // type is not decided yet, so the instantiation is not decided either —
        // nothing is refused, and the reorder resolves when the type does.
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
        // The definition's field count (the shape's length) — `None` when the
        // shape is still an unbound cell mid-recursion, in which case the
        // missing/excess checks are deferred to the probe unify.
        // SAFETY: `type_pair` is a live node of this module; nothing in this
        // crate calls `Module::drop_block`.
        let def_len = unsafe { self.module.array_items(type_pair) }
            .and_then(|items| items.first())
            // SAFETY: the field item's node is a live node of this module;
            // nothing in this crate calls `Module::drop_block`.
            .and_then(|item| unsafe { shape::array_items(&self.module, item.node) })
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
            // The structural mismatch is the recorded diagnostic; return the
            // call-order value so the checker still produces a term, but the
            // caller skips the field-list unify.
            return (self.value_of(value), self.state[value].ty.unwrap(), false);
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

    /// The deferred half of a named instantiation: the struct type — and so its
    /// name table — is not resolved here, so the definition-order reorder is not
    /// computed.  It is **built as a lazy read** instead and resolves at the
    /// unification that binds the callee's type, which is the same principle the
    /// positional instantiation already follows (its field-list unify wakes at
    /// the apply): when the struct type is unresolved, the instantiation is
    /// unresolved too.
    ///
    /// The instance's definition position `i` is the argument that supplies the
    /// field whose name sits at `names_in_order[i]` — the marker's
    /// definition-order names ([`shape::STRUCT_KIND_NAMES_ORDER_PATH`]), the
    /// inverse of the name→index table a lazy read cannot derive:
    ///
    /// ```text
    /// value[i] = Index(call_values, TableGet(supply, key(i)))
    /// ```
    ///
    /// `supply` is a constant table the checker builds from the source.  An
    /// argument's key is its **name**, or — for a positional argument — its
    /// **rank** among the positional ones.  A definition position whose name no
    /// argument supplies is the positional `rank`-th unclaimed position, so its
    /// key is that rank: `rank` counts the definition positions before it whose
    /// names are unsupplied, which the checker computes as the running sum of
    /// the membership test `InDomain(names_in_order[i], supplied_names)`.  With
    /// no positional argument there is no fallback to select, and the key is the
    /// name itself.
    ///
    /// **The key also carries the type** whenever every argument's type is
    /// decided (`[tag, argument type]` against `[tag, field type]`), so the
    /// lookup's own content comparison *is* the per-field type check: a field
    /// whose declared type no supplying argument matches is a miss.  That is the
    /// only form the check can take here — a `unify` between the gathered type
    /// and the field list is deferred and pinned before the marker binds (the
    /// shape half of the callee's pair is unified before its kind half, so no
    /// name-dependent read can resolve yet), and a pinned read is masked from
    /// then on.  The lookup, by contrast, is *evaluated* when the instance is,
    /// and the lowlevel's table read force-evaluates its key.
    ///
    /// Two facts stay check-time, because neither depends on the struct type.
    /// A **duplicate** name is refused here — whatever the field list is, one
    /// name supplying two positions is a structural mismatch, and a table keyed
    /// by name would silently keep only one.  A **missing** argument is not
    /// checked here: the returned field-type list is a probe with one cell per
    /// argument, so the caller's field-list unify is an arity check that fires
    /// the moment the type resolves.
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
            // The mismatch is the recorded diagnostic; the call-order value
            // stands so the descent stays total, and the caller skips the
            // field-list unify.
            let value_node = self.array_node(self.current_block, vals);
            let value_shape = self.array_node(self.current_block, tys);
            return (value_node, value_shape, false);
        }
        let has_positional = arg_names.iter().any(|name| name.is_none());
        // Whether every argument's type is decided here.  When it is, the type
        // joins the supplying key (below), which is what makes the field-type
        // check real: the lookup compares the *field's* type against the
        // supplying argument's by the table's own content equality, and a
        // mismatch is a miss.  An argument whose type is still open (a `_`, a
        // parameter-dependent value) carries no type in its key, so the lookup
        // stays a name-only hit.
        let typed = tys.iter().all(|&ty| self.type_is_concrete(ty));
        // The callee's field-type list, read lazily: `[shape, kind]`'s shape.
        // Once the callee binds, its element `i` is the definition position's
        // own field type.
        let shape = self.index(type_pair, self.zero());
        // The supplying table: one entry per argument, keyed by its name — or,
        // for a positional argument, by its rank among the positional ones —
        // and, when every argument's type is decided, by that type too.  The
        // value is the argument's call-order slot.
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
        // The supplied names as a set value: the domain the membership test
        // reads, and the only reader that distinguishes "this definition
        // position's name is supplied by name" from "it takes the next
        // positional argument".
        let mut supplied_nodes = Vec::with_capacity(supplied.len());
        for &name in &supplied {
            let node = self.name_node(name);
            supplied_nodes.push(node);
        }
        let supplied_names = self.array_node(self.current_block, &supplied_nodes);
        // The definition-order names, read lazily through the callee's struct
        // *kind* — `kind[0][2]`, the marker's names-in-order slot.
        let names_in_order = self.lazy_index_path(callee_ty, &shape::STRUCT_KIND_NAMES_ORDER_PATH);
        let call_values = self.array_node(self.current_block, vals);
        // The instance's field-type list is a **probe**: one fresh cell per
        // argument.  The caller unifies it with the callee's field list, so it
        // compares the *arity* and absorbs the definition's field types — which
        // is what a per-position read of the type resolves to, and so what the
        // supplying lookup's key carries.  The probe holds no computation of its
        // own, which is deliberate: a read of a real gathered type would reach
        // back into the lookup that reads it.
        let mut order_ty = Vec::with_capacity(tys.len());
        for _ in 0..tys.len() {
            order_ty.push(self.fresh_cell());
        }
        let mut order = Vec::with_capacity(vals.len());
        // The running count of definition positions already claimed by a named
        // argument — how many positions before `i` are not the positional
        // `rank`-th unclaimed one.
        let mut supplied_before = self.zero();
        for i in 0..vals.len() {
            let name_at = self.index_const(names_in_order, i);
            // The definition position's tag, read from the table: the field's
            // own name, or the positional rank when the position's name is one
            // no argument carries.
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
            // The supplying argument's call slot, read from the table.  A tag
            // no argument supplies — an unknown field, or a field type no
            // argument's type matches — is a genuine miss, so the read is the
            // refusal.
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

    /// Whether `value` is a member of the set whose value is `members` — the
    /// [`TypeOperator::InDomain`] membership test, used here to read "this field
    /// name is one an argument supplied" off a set of the supplied names.
    fn in_domain(&mut self, value: NodeId, members: NodeId) -> NodeId {
        let ops = self.array_node(self.current_block, &[value, members]);
        self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::InDomain),
            Some(ops),
        )
    }

    /// The two-armed lazy selection `Index([first, second], condition)` — the
    /// encoding's conditional: `second` when the condition is `USize(1)`,
    /// `first` when it is `0`.  Both arms are values the read picks between; the
    /// unselected one is never evaluated.
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

    /// The field name at a definition position, from the struct's name table
    /// (`None` for a positional field or an anonymous struct).
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

    /// A struct type expression.  A struct is *polymorphic*: the nominal
    /// type id identifies its constructor, and instantiating it with field
    /// types produces a concrete type.  So the id belongs in the kind slot
    /// (with `TypeStruct` as an inner tag), not in the value shape, which is
    /// just the field-type list:
    ///
    /// ```text
    /// pair    = [ shape, kind ]
    /// shape   = [ field types… ]
    /// payload = [ id, names, names_in_order ]
    /// marker  = [ payload, TypeStruct ]
    /// kind    = [ marker, K ]
    /// ```
    ///
    /// The marker is the ordinary `[value, type]` pair `[payload, TypeStruct]` —
    /// its *type* slot is the `TypeStruct` tag, its value a payload holding the
    /// nominal id ([`Checker::fresh_nominal_id`]: one id per written
    /// occurrence, so two occurrences keep distinct ids and an applied type
    /// lambda does not mint one per application) plus the optional name→index
    /// table.  It sits in the kind's
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
        let id = self.fresh_nominal_id();
        let (shape, kind, pair) = self.struct_type_type(id, &tys, &names);
        self.require_struct_field_names(&names, &elements, pair);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(shape);
        self.state[e].ty = Some(kind);
        pair
    }

    /// Refuse a struct **definition** with an unnamed field, at that field's own
    /// location.
    ///
    /// **Every struct field carries a name.**  A struct instance reads by name
    /// (`s.x`, `X::a`), so a field with none would be unreachable: the
    /// positional form `a(k)` is the *tuple* read, and the raw read `X<e>` reads
    /// an element's `[value, type]` pair, which a struct instance's field is not
    /// (`docs/language-spec.md` §Structs).  A *block*'s fields need no check
    /// here: they are its bindings, and a bare expression statement is simply not
    /// a field ([`Self::check_record`]).  `struct_node` is the struct type term
    /// the check is about — the hole the caller carries on with, since a
    /// reported definition is a rejected build either way.
    fn require_struct_field_names(
        &mut self,
        names: &[Option<&'static str>],
        fields: &[ExprId],
        struct_node: NodeId,
    ) {
        let Some(at) = names.iter().position(|name| name.is_none()) else {
            return;
        };
        // `fields` is index-aligned with `names` (the frontend's contract, and
        // the same pairing `struct_type_type` reads), so an unnamed field always
        // has the expression that states it, which is where the caret goes.
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

    /// The struct name→index table value for a field-name list: the
    /// [`LowValue::Void`] marker when every field is unnamed (an anonymous
    /// positional struct has no name table — a *computed nothing*, not the
    /// unit value), otherwise a constant `Table` mapping each field name to
    /// its positional index.  The table's keys are the field names (string
    /// values), its values the field indices — the map an `a.name` read
    /// resolves through.  A named read over the marker misses with a
    /// recorded [`EvalError::TableMiss`], never a panic.
    pub(super) fn build_struct_names(&mut self, names: &[Option<&'static str>]) -> NodeId {
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

    /// The struct's field names **in definition order** for a field-name list:
    /// one array element per definition position, the [`LowValue::Void`] marker
    /// when every field is unnamed (the anonymous positional struct, mirroring
    /// [`Self::build_struct_names`]), a [`LowValue::Void`] element for an
    /// unnamed field otherwise.
    ///
    /// This is the marker's `names_in_order` slot — the *inverse* of the
    /// name→index table, which is the one thing a lazy read of that table cannot
    /// derive.  It exists for the named instantiation whose struct type is still
    /// unresolved: the reorder runs when the type resolves, and at definition
    /// position `i` it needs the field's *name* to find the argument that
    /// supplies it ([`Self::lazy_named_instantiate`]).
    pub(super) fn build_struct_names_in_order(&mut self, names: &[Option<&'static str>]) -> NodeId {
        if names.iter().all(|n| n.is_none()) {
            return self.alloc_node(
                self.current_block,
                None,
                Some(P::Value::from(LowValue::Void)),
            );
        }
        let mut items = Vec::with_capacity(names.len());
        for name in names {
            items.push(match name {
                Some(name) => self.name_node(name),
                // An unnamed field has no name for a position to state.  Every
                // struct field must be named (the definition is refused
                // otherwise), so this is a refused build's hole, not a case a
                // read resolves through — and a `Void` element reads as the
                // computed nothing, never as a name.
                None => self.alloc_node(
                    self.current_block,
                    None,
                    Some(P::Value::from(LowValue::Void)),
                ),
            });
        }
        self.array_node(self.current_block, &items)
    }
}
