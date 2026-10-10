//! Arrays, tables, and indexing: the array instance and type, the element
//! read with its bounds, and the raw reads.

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, NodeId};

use crate::diagnostic::DiagKind;
use crate::ir::{BinOp, ExprId};
use crate::program::{HighProgram, TypeOperator, ValueType};

use super::Checker;
use crate::diagnostic::AssertSpelling;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// An array-element read `a[i]`. See `docs/notes/raw-index.md`.
    ///
    /// # Invariant
    /// The container's type is pinned to a fresh array type, so a non-array
    /// fails here and an undecided one resolves at the call site's argument
    /// unify. Its type is the pinned shape's element cell. It registers the
    /// bounds assert `length <= i == 0`: a concrete index out of range fails
    /// the assert pass, a lazy length keeps it pending for the apply clone to
    /// re-check per argument.
    pub(super) fn check_index(&mut self, e: ExprId, array: ExprId, index: ExprId) -> NodeId {
        self.check_expr(array);
        self.check_expr(index);
        let elem_cell = self.fresh_cell();
        let len_cell = self.fresh_cell();
        let shape = self.array_node(self.current_block, &[elem_cell, len_cell]);
        let kind = self.kind_expr(self.current_block, self.markers.array_type_marker);
        let array_ty = self.array_node(self.current_block, &[shape, kind]);
        self.check_unify(
            self.state[array].ty.unwrap(),
            array_ty,
            self.loc(array, 1),
            DiagKind::Guard,
        );
        let array_value = self.value_of(array);
        let index_value = self.value_of(index);
        self.node_edges.insert(index_value, self.loc(index, 0));
        let value_ops = self.array_node(self.current_block, &[array_value, index_value]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        let beyond_ops = self.array_node(self.current_block, &[len_cell, index_value]);
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
        let pair = self.pair_of(value_node, elem_cell);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(elem_cell);
        pair
    }

    /// A **raw** positional read `X<e>` — element `index` of the container's
    /// value. See `docs/notes/raw-index.md`.
    ///
    /// # Invariant
    /// The container must be a tuple type value, stated once as a unify against
    /// the tuple kind: pinned when undecided, refused where it is decided and
    /// different. Beyond that the read validates nothing. Its term is the
    /// element's own `[value, type]` pair, never the bare element.
    pub(super) fn check_raw_index(
        &mut self,
        e: ExprId,
        container: ExprId,
        index: ExprId,
    ) -> NodeId {
        self.check_expr(container);
        self.check_expr(index);
        // The tuple-kind requirement, a unify on the container's kind: decided
        // here, deferred to the apply otherwise.
        let container_ty = self.state[container].ty.unwrap();
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        // A decided container of another kind is refused and not read:
        // its element's type slot would be the container's type.
        if !self.check_unify(container_ty, kind, self.loc(container, 1), DiagKind::Guard) {
            return self.refused_pair(e);
        }
        let container_value = self.value_of(container);
        let index_value = self.value_of(index);
        self.node_edges.insert(index_value, self.loc(index, 0));
        let (pair, element, ty_node) = self.element_read(container_value, index_value);
        // Both slot reads target the element, so this edge gives a failed one
        // a span and names it the element.
        self.node_edges.insert(element, self.loc(e, 1));
        self.state[e].term = Some(pair);
        // Left to `value_of`: it reads the same node `element_read` built.
        self.state[e].val = None;
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// The element read shared by `X<e>` and `X::a`; see
    /// `docs/notes/raw-index.md`. Returns `(pair, element, type)`.
    ///
    /// # Invariant
    /// Both callers require a **type value** container, whose components hold
    /// `[type value, kind]` pairs — so the read's type is the component's
    /// *kind*, the only handle on it. The slot read is computed here, so the
    /// type is decided before any check asks; an undecided container's read
    /// stays lazy, which is what the per-apply re-check relies on.
    pub(super) fn element_read(
        &mut self,
        container_value: NodeId,
        subscript: NodeId,
    ) -> (NodeId, NodeId, NodeId) {
        let element_ops = self.array_node(self.current_block, &[container_value, subscript]);
        let element = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(element_ops),
        );
        let value_ops = self.array_node(self.current_block, &[element, self.zero()]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        let ty_ops = self.array_node(self.current_block, &[element, self.one()]);
        let ty_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ty_ops),
        );
        // Run the slot read now, so its type is decided before any check asks.
        // `evaluate_node` follows the edges it needs.
        self.module.evaluate_node(AnyNodeId::Dynamic(ty_node), None);
        (self.pair_of(value_node, ty_node), element, ty_node)
    }

    /// A table lookup `t{k}`: the lowlevel `TableGet` reads the entry whose
    /// stored key is deep-content-equal to `k`.
    ///
    /// # Invariant
    /// The container's type is pinned to a fresh table type, so a non-table
    /// fails here and an undecided one resolves at the call site's argument
    /// unify. The read's type is the pinned shape's value-type cell.
    pub(super) fn check_table_find(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let key_cell = self.fresh_cell();
        let value_cell = self.fresh_cell();
        let shape = self.array_node(self.current_block, &[key_cell, value_cell]);
        let kind = self.kind_expr(self.current_block, self.markers.table_type_marker);
        let table_ty = self.array_node(self.current_block, &[shape, kind]);
        self.check_unify(
            self.state[container].ty.unwrap(),
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
        let pair = self.pair_of(value_node, value_cell);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(value_cell);
        pair
    }

    /// An array instance `[v1, …, vn]`:
    /// `[values, [[element type, length], [ArrayType, Type]]]`.
    ///
    /// # Invariant
    /// The element type is a fresh cell unified with each element's type, so a
    /// heterogeneous literal is an error rather than a type claiming one class
    /// for elements that differ. The length slot holds the element count.
    pub(super) fn check_array_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut vals = Vec::new();
        let element_ty = self.fresh_cell();
        for &el in &elements {
            self.check_expr(el);
            vals.push(self.value_of(el));
            // Found = the element's type, expected = the shared cell: the first
            // binds it, a later one that differs conflicts.
            self.check_unify(
                self.state[el].ty.unwrap(),
                element_ty,
                self.loc(el, 1),
                DiagKind::ArrayElement,
            );
        }
        let value = self.array_node(self.current_block, &vals);
        let length = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(vals.len()))),
        );
        let shape = self.array_node(self.current_block, &[element_ty, length]);
        let kind = self.kind_expr(self.current_block, self.markers.array_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// A set `set{a, b, …}` typed by the **set kind**, not by `array<T, n>`.
    ///
    /// # Invariant
    /// The shape is the element type alone, so `set{a}` and `set{a, b}` are one
    /// type; the separate kind keeps a set from flowing into an `array<T, n>`
    /// and makes `s[i]` a refusal. See [`crate::set`].
    pub(super) fn check_set_term(&mut self, e: ExprId) -> NodeId {
        let members = self.range_children(e);
        let mut vals = Vec::new();
        let element_ty = self.fresh_cell();
        for &member in &members {
            self.check_expr(member);
            vals.push(self.value_of(member));
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
        let shape = self.array_node(self.current_block, &[element_ty]);
        let kind = self.kind_expr(self.current_block, self.markers.set_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// A constant table literal `table { k1 ==> v1, … }`, typed by
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
        let mut pairs = Vec::with_capacity(entries.len() / 2);
        for chunk in entries.chunks(2) {
            let (key, value) = (chunk[0], chunk[1]);
            self.check_expr(key);
            self.check_expr(value);
            let key_node = self.value_of(key);
            self.node_edges.insert(key_node, self.loc(key, 0));
            pairs.push((
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
        let table = self.module.build_table(&pairs, self.current_block);
        let value = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Table(table))),
        );
        let shape = self.array_node(self.current_block, &[key_ty, value_ty]);
        let kind = self.kind_expr(self.current_block, self.markers.table_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// A shallow array `[v1, ~ v2, ~2 v3]`: per-element type slots, as a
    /// tuple's.
    ///
    /// # Invariant
    /// The value array carries a per-position mask: a bare `~` position's whole
    /// subtree stays lazy in the deep pass, and a `~n` position is wrapped so
    /// the value slot at each of the first `n` levels of its type spine stays
    /// shallow.
    pub(super) fn check_shallow_array_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let depths = self.range_depths(e);
        let mut vals = Vec::new();
        let mut tys = Vec::new();
        let mut mask = Vec::new();
        for (i, &el) in elements.iter().enumerate() {
            self.check_expr(el);
            match depths[i] {
                0 => {
                    vals.push(self.value_of(el));
                    tys.push(self.state[el].ty.unwrap());
                }
                usize::MAX => {
                    vals.push(self.value_of(el));
                    tys.push(self.state[el].ty.unwrap());
                }
                n => {
                    // The wrapped term is a lazy region: its pair chain does not match
                    // the element's type, so its reads get a fresh cell.
                    vals.push(self.wrap_shallow(el, n));
                    tys.push(self.fresh_cell());
                }
            }
            mask.push(depths[i] == usize::MAX);
        }
        // `[values, [[element types], [TupleType, Type]]]` — a tuple's shape, so
        // a read selects the per-element type slot.
        let value = self.array_node_masked(self.current_block, &vals, &mask);
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value);
        self.state[e].ty = Some(ty_node);
        pair
    }

    /// Wrap `e`'s checked term so the value slot at each of the first `depth`
    /// levels of its type spine stays shallow.
    ///
    /// # Invariant
    /// Each level is a fresh pair carrying the `shallow=[true, false]` mask, so
    /// a shared subexpression is never itself marked. The descent follows slot
    /// 1 only while the spine is a concrete pair at check time; an undecided
    /// slot ends it, and the wraps above the stop still apply.
    fn wrap_shallow(&mut self, e: ExprId, depth: usize) -> NodeId {
        // Level 1 is the element's own pair; a call's is built from the
        // extracted value and type, the apply pair being virtual.

        // Each deeper level is the previous level's type slot.
        let mut levels: Vec<(NodeId, NodeId)> = Vec::new();
        let mut current = self.state[e].term.unwrap();
        if self.module.node_operation(current).is_some() {
            levels.push((self.value_of(e), self.state[e].ty.unwrap()));
        } else {
            while levels.len() < depth {
                // SAFETY: `current` is a live node of this module (a checked
                // expression's term).
                let Some(items) = (unsafe { self.module.array_items(current) }) else {
                    break;
                };
                if items.len() != 2 {
                    break;
                }
                let slot0 = self.module.as_dynamic(items[0].node, self.current_block);
                let slot1 = self.module.as_dynamic(items[1].node, self.current_block);
                // SAFETY: `slot1` was just materialized into the current
                // block, whose arena is alive.
                let descend =
                    unsafe { self.module.array_items(slot1) }.is_some_and(|next| next.len() == 2);
                levels.push((slot0, slot1));
                if !descend {
                    break;
                }
                current = slot1;
            }
        }
        // Rebuild from the innermost level out, each a fresh masked pair.
        let mut wrapped = None;
        for &(slot0, slot1) in levels.iter().rev() {
            let next = wrapped.unwrap_or(slot1);
            wrapped =
                Some(self.array_node_masked(self.current_block, &[slot0, next], &[true, false]));
        }
        wrapped.unwrap_or(self.state[e].term.unwrap())
    }

    /// The array type `{ element_type, length }`, kinded as an array.
    ///
    /// # Invariant
    /// The element type is checked in type position and the length in term
    /// position; both compile identically, because a type expression is a
    /// first-class value and the array type *is* its pair.
    pub(super) fn check_array_type(
        &mut self,
        e: ExprId,
        element_type: ExprId,
        length: ExprId,
    ) -> NodeId {
        self.check_expr(element_type);
        self.check_expr(length);
        let length_value = self.value_of(length);
        // The element's denotation (operator-polymorphism.md §3): an element
        // with attributes names the annotated value's term.
        let element = self.type_denotation(element_type, None);
        let shape = self.array_node(self.current_block, &[element, length_value]);
        let kind = self.kind_expr(self.current_block, self.markers.array_type_marker);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(shape);
        self.state[e].ty = Some(kind);
        pair
    }
}
