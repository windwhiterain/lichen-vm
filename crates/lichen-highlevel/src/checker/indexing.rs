//! Arrays, tables, and indexing: the container rules — an array instance and
//! its type, a shallow array, a table literal and a table lookup, the array
//! element read with its bounds constraint, and the raw structural reads that
//! bypass type validation by design.

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, NodeId};

use crate::diagnostic::DiagKind;
use crate::ir::{BinOp, ExprId};
use crate::program::{HighProgram, TypeOperator, ValueType};

use super::Checker;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// An array-element read `a[i]`.  The container's type is *pinned* to a
    /// fresh array type — the same pin [`Self::check_binop`] applies to its
    /// operands and [`Self::check_table_find`] to its container — so a
    /// concretely non-array container (a tuple, a struct, a function, a
    /// table) fails here with a diagnostic, and an unbound container (a
    /// parameter, a call result) resolves at the call site's argument
    /// unify: only an array can flow in.  Tuple and struct slots are read
    /// with the dedicated positional form `a(k)` ([`Self::check_field`]),
    /// table entries with `t{k}` — the operator and the type extraction
    /// are chosen by syntax, never by a runtime kind dispatch.
    ///
    /// The value is the structural `Index` over the array's value; the type
    /// is the pinned shape's element cell, which lands in the concrete
    /// element type's class when the container type binds.  The read also
    /// registers a *bounds constraint* on the module's assert worklist:
    /// `i < length` as `length <= i == 0`, the comparison ops the value
    /// language already has.  A concrete out-of-range index fails the
    /// assert pass; a lazy length (an annotated parameter) keeps the assert
    /// pending, and the apply clone re-checks it against each argument's
    /// actual array.
    pub(super) fn check_index(&mut self, e: ExprId, array: ExprId, index: ExprId) -> NodeId {
        self.check_expr(array);
        self.check_expr(index);
        let elem_cell = self.fresh_cell();
        let len_cell = self.fresh_cell();
        let shape = self.array_node(self.current_block, &[elem_cell, len_cell]);
        let kind = self.kind_expr(self.current_block, self.markers.array_type_marker);
        let array_ty = self.array_node(self.current_block, &[shape, kind]);
        self.check_unify(
            self.ty[array].unwrap(),
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
        self.register_assert(in_range, self.loc(e, 0), false);
        let pair = self.pair_of(value_node, elem_cell);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(elem_cell);
        pair
    }

    /// A **raw** positional read `X<e>` (the glued `<` postfix) — element
    /// `index` of the container's **value**, read structurally through the
    /// lowlevel `Index` **without type validation**.  There is no array-type
    /// pinning (unlike [`Self::check_index`]), no `IndexTarget` guard and no
    /// shape-derived type (unlike [`Self::check_field`]): the container is
    /// read by value whatever its type, so it reads a component of a
    /// type-as-value (`<Int, string><0>`, `struct<Int, string><1>`) or of any
    /// expression's value.  An unbound container (a parameter, a call result)
    /// stays lazy — the lowlevel `Index` defers — and resolves at the apply,
    /// exactly the laziness the wrapper field reads rely on.
    ///
    /// The result is the element's own pair: `term[e]` is the raw read
    /// `Index(container_value, index)`; the value is element 0 of that pair
    /// and the type element 1, both read lazily.  An out-of-bounds index or a
    /// non-array container is a runtime `Index` evaluation error (never a
    /// static diagnostic), per the no-validation contract.
    pub(super) fn check_raw_index(
        &mut self,
        e: ExprId,
        container: ExprId,
        index: ExprId,
    ) -> NodeId {
        self.check_expr(container);
        self.check_expr(index);
        let container_value = self.value_of(container);
        let index_value = self.value_of(index);
        self.node_edges.insert(index_value, self.loc(index, 0));
        let ops = self.array_node(self.current_block, &[container_value, index_value]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ops),
        );
        let ty_ops = self.array_node(self.current_block, &[value_node, self.one()]);
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

    /// A table lookup `t{k}`: the lowlevel `TableGet` reads the entry whose
    /// stored key is deep-content-equal to `k`.  The frontend emits this
    /// form for the *adjacent* brace — the syntactic distinction from
    /// positional [`Self::check_index`] — so the operator is chosen by
    /// syntax, never by a runtime kind dispatch.
    ///
    /// The container's type is *pinned* to a fresh table type — the same
    /// pin [`Self::check_binop`] applies to its operands — so a concretely
    /// non-table container fails here with a diagnostic, and an unbound
    /// container (a parameter, a call result) resolves at the call site's
    /// argument unify: only a table can flow in.  The value is the
    /// `TableGet` op node itself; the type is the pinned shape's value-type
    /// cell, which lands in the concrete value type's class when the
    /// container type binds.
    pub(super) fn check_table_find(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let key_cell = self.fresh_cell();
        let value_cell = self.fresh_cell();
        let shape = self.array_node(self.current_block, &[key_cell, value_cell]);
        let kind = self.kind_expr(self.current_block, self.markers.table_type_marker);
        let table_ty = self.array_node(self.current_block, &[shape, kind]);
        self.check_unify(
            self.ty[container].unwrap(),
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
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(value_cell);
        pair
    }

    /// An array instance `[v1, ..., vn]` — every element shares one type:
    /// `[values, [[element type, length], [ArrayType, Type]]]`.  The element
    /// type is a fresh cell unified with each element's type (a
    /// heterogeneous literal is an error — the array's type would otherwise
    /// claim one type for elements that differ), and the length slot holds
    /// the element count.
    pub(super) fn check_array_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut vals = Vec::new();
        let element_ty = self.fresh_cell();
        for &el in &elements {
            self.check_expr(el);
            vals.push(self.value_of(el));
            // Found = this element's type, expected = the shared cell: the
            // first element binds the cell, a later one that differs
            // conflicts against it.
            self.check_unify(
                self.ty[el].unwrap(),
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
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// A constant table literal `table { k1 ==> v1, k2 ==> v2, … }` — the
    /// entries (interleaved key/value ids in the children arena) are checked
    /// like an array's elements, against a shared key-type cell and a shared
    /// value-type cell, and the value is built eagerly by the lowlevel
    /// [`Module::build_table`](lichen_lowlevel::Module::build_table): every key
    /// is force-evaluated and
    /// deep-content-hashed, an entry whose key is not concrete is dropped
    /// with a recorded [`EvalError::TableKeyUnbound`], and the survivors are
    /// stored sorted by hash.  The type is the kinded pair
    /// `[[key type, value type], [TypeTable, Type]]`.
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
                self.ty[key].unwrap(),
                key_ty,
                self.loc(key, 1),
                DiagKind::TableKey,
            );
            self.check_unify(
                self.ty[value].unwrap(),
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
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// A shallow array `[v1, ~ v2, ~2 v3]` — typed like a tuple (per-element
    /// type slots: a homogeneous `Array` type would reject `[x, ~ f(x+1)]`
    /// with an `Int` head and a `Stream` tail).  The value array carries the
    /// per-position mask: a bare-`~` position's whole subtree stays lazy in
    /// the deep pass (a read forces the single element on demand), and a
    /// `~n` position is wrapped so the value slot at each of the first `n`
    /// levels of its type spine stays shallow.
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
                    tys.push(self.ty[el].unwrap());
                }
                usize::MAX => {
                    vals.push(self.value_of(el));
                    tys.push(self.ty[el].unwrap());
                }
                n => {
                    // The wrapped term is a lazy region: its value is the
                    // pair chain `[s, [s, … [s, d]]]`, whose structure does
                    // not match the element's own type.  Its reads are
                    // therefore underdetermined — a fresh cell — never a
                    // concrete type that would silently mismatch the
                    // wrapped value.
                    vals.push(self.wrap_shallow(el, n));
                    tys.push(self.fresh_cell());
                }
            }
            mask.push(depths[i] == usize::MAX);
        }
        // `[values, [[element types], [TupleType, Type]]]` — the same shape
        // as a tuple, so reads dispatch on the tuple kind and select the
        // per-element type slot.
        let value = self.array_node_masked(self.current_block, &vals, &mask);
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// Wrap `e`'s checked term so the value slot at each of the first
    /// `depth` levels of its type spine stays shallow.  Each level is a
    /// fresh pair `[slot0, slot1]` carrying the `shallow=[true, false]`
    /// mask: slot 0 (the value slot) is marked, slot 1 is the next level
    /// down the type spine (the pair's own type slot).  The descent follows
    /// position 1 while the spine is a concrete pair at check time; an
    /// unbound slot ends the descent (the wraps above the stop still apply).
    /// The layers are fresh nodes, so a shared subexpression (a kind
    /// expression reused by every occurrence) is never itself marked.
    fn wrap_shallow(&mut self, e: ExprId, depth: usize) -> NodeId {
        // Level 1 is the element's own `[value, type]` pair.  For a static
        // element that is its stored term; for a call the term is the apply
        // operation node (its pair is virtual), so level 1 is built from
        // the extracted value and type.  Each deeper level is the previous
        // level's type slot's own `[shape, kind]` pair.
        let mut levels: Vec<(NodeId, NodeId)> = Vec::new();
        let mut current = self.term[e].unwrap();
        if self.module.nodes[current].operation.is_some() {
            levels.push((self.value_of(e), self.ty[e].unwrap()));
        } else {
            while levels.len() < depth {
                let Some(items) = self.module.array_items(current) else {
                    break;
                };
                if items.len() != 2 {
                    break;
                }
                let slot0 = self.module.as_dynamic(items[0].node, self.current_block);
                let slot1 = self.module.as_dynamic(items[1].node, self.current_block);
                let descend = self
                    .module
                    .array_items(slot1)
                    .is_some_and(|next| next.len() == 2);
                levels.push((slot0, slot1));
                if !descend {
                    break;
                }
                current = slot1;
            }
        }
        // Rebuild from the innermost level out: each level is a fresh pair
        // [value slot, next] with the shallow mask [true, false].
        let mut wrapped = None;
        for &(slot0, slot1) in levels.iter().rev() {
            let next = wrapped.unwrap_or(slot1);
            wrapped =
                Some(self.array_node_masked(self.current_block, &[slot0, next], &[true, false]));
        }
        wrapped.unwrap_or(self.term[e].unwrap())
    }

    /// The real array type `{ element_type, length }` — the instance is the
    /// 2-element shape `[element_type, length]` (element 0: the type shared
    /// by all elements, element 1: the length), kinded as an array.  The
    /// element type is checked in type position (it must be a type), the
    /// length in term position (it is a value — e.g. `3` or, dependently,
    /// a parameter holding the length).  Both roles compile identically: a
    /// type expression is a first-class value, so the array type *is* its
    /// pair.
    pub(super) fn check_array_type(
        &mut self,
        e: ExprId,
        element_type: ExprId,
        length: ExprId,
    ) -> NodeId {
        self.check_expr(element_type);
        self.check_expr(length);
        let length_value = self.value_of(length);
        let shape = self.array_node(
            self.current_block,
            &[self.term[element_type].unwrap(), length_value],
        );
        let kind = self.kind_expr(self.current_block, self.markers.array_type_marker);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.term[e] = Some(pair);
        self.val[e] = Some(shape);
        self.ty[e] = Some(kind);
        pair
    }
}
