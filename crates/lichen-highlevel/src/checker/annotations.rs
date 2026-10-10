//! The `# p` / `? doc` annotation rules and the slot algebra — the merge,
//! a value's slot, and the *missing* slot.

use lichen_lowlevel::{AnyNodeId, LowOperator, NodeId};

use crate::attr::AttrSet;
use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind, Loc};
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;
use crate::diagnostic::AssertSpelling;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// The [`AttrExt`](crate::attr::AttrExt) for `marker`, or `None` when this
    /// build has no attribute extension at all.
    ///
    /// # Invariant
    /// A marker this build cannot lower is a check-time refusal, not a broken
    /// invariant: the sites that read it report [`DiagKind::NoAttributeExtension`]
    /// through [`Self::no_attr_ext_guard`] and carry on over the hole.
    pub(super) fn attribute_extension(
        &self,
        marker: &P::Attr,
    ) -> Option<&'static dyn crate::attr::AttrExt<P>> {
        self.attr_ext.as_ref().map(|registry| registry(marker))
    }

    /// Records the [`DiagKind::NoAttributeExtension`] guard at `loc` and
    /// returns the hole the slot falls back to.
    ///
    /// # Invariant
    /// The hole is a fresh undecided `[value, type]` pair — the shape every
    /// attribute slot has. Nothing unifies against it: the guard has already
    /// failed the build and `check_failed` skips the definition pass. The guard
    /// is recorded once per build, at the first reader.
    pub(super) fn no_attr_ext_guard(&mut self, loc: Loc) -> NodeId {
        if !self.no_attr_ext_reported {
            self.no_attr_ext_reported = true;
            self.record_guard(
                self.type_expr,
                self.type_expr,
                loc,
                DiagKind::NoAttributeExtension,
                None,
            );
        }
        let value = self.fresh_cell();
        let ty = self.fresh_cell();
        self.pair_of(value, ty)
    }

    /// The sub-expressions of a compound whose perspectives meet in a
    /// `# p` annotation — the reference. A leaf has none.
    ///
    /// # Invariant
    /// An annotated value contributes none: its attribute is its own slot, so a
    /// doc-only annotation reads as a leaf.
    fn persp_combine_children(&self, e: ExprId) -> Vec<ExprId> {
        match self.ir[e].kind {
            ExprKind::BinOp { left, right, .. } => vec![left, right],
            ExprKind::Apply { function, argument } => vec![function, argument],
            ExprKind::Instantiate {
                type_expr, value, ..
            } => vec![type_expr, value],
            ExprKind::Record { value, .. } => vec![value],
            ExprKind::Index { array, index } => vec![array, index],
            ExprKind::RawIndex { container, index } => vec![container, index],
            ExprKind::Field { container, key } => vec![container, key],
            ExprKind::NamedField { container, .. } => vec![container],
            ExprKind::RawNamedField { container, .. } => vec![container],
            ExprKind::Find { container, key } => vec![container, key],
            ExprKind::TypeArray {
                element_type,
                length,
            } => vec![element_type, length],
            ExprKind::TypeFunction {
                parameter,
                r#return,
            } => vec![parameter, r#return],
            ExprKind::Tuple(_)
            | ExprKind::TypeTuple(_)
            | ExprKind::Array(_)
            | ExprKind::Set(_)
            | ExprKind::Table(_)
            | ExprKind::ShallowArray { .. } => self.range_children(e),
            ExprKind::TypeStruct { fields, .. } => {
                self.ir.children[fields.start as usize..fields.end as usize].to_vec()
            }
            // An annotated value contributes no sub-expression: its attribute
            // is its own slot, never the value beneath it.
            ExprKind::Annotation { .. } => Vec::new(),
            // Leaf kinds; an `ErrorBlock` carries no children to combine.
            ExprKind::Literal(_)
            | ExprKind::Parameter
            | ExprKind::Placeholder
            | ExprKind::ErrorBlock
            | ExprKind::Static { .. } => Vec::new(),
            // A lambda is a leaf for stage 1 (its own `# p` binds a slot).
            ExprKind::Function { .. } => Vec::new(),
            ExprKind::Assert { .. } => Vec::new(),
            // The converted operand's perspective meets as a `BinOp` operand's.
            ExprKind::Convert { value, .. } => vec![value],
            ExprKind::NativeCall { .. } => self.range_children(e),
        }
    }

    /// Merge an annotation's *spelled* slots into a value's existing set,
    /// producing the expression's full schema tail.
    ///
    /// # Invariant
    /// The annotation **replaces** the slots it names and **preserves** every
    /// slot it does not, so `(x # 8 ? doc) # 4` keeps the doc. Ordered by the
    /// canonical attribute order, so the runtime pair stays consistent.
    fn merge_slots(&self, value_tail: Vec<P::Attr>, own_tail: Vec<P::Attr>) -> Vec<P::Attr> {
        let mut result = value_tail;
        for m in own_tail {
            let s = m.order_index();
            match result.iter().position(|x| x.order_index() == s) {
                Some(pos) => result[pos] = m,
                None => result.push(m),
            }
        }
        result.sort_by_key(|m| m.order_index());
        result
    }

    /// The value's existing attribute slot for `marker` — the node its runtime
    /// pair carries at that attribute's position.
    ///
    /// # Invariant
    /// It is always a `[value, type]` term pair, for a constraint and a label
    /// alike, and `None` when the value's schema has no such slot.
    fn value_attr_node(&self, value: ExprId, marker: &P::Attr) -> Option<NodeId> {
        let value_tail = self.schema_tail(value).to_vec();
        let pos = value_tail.iter().position(|m| m == marker)?;
        let pair = self.state[value].term?;
        // SAFETY: `pair` is a live node of this module; nothing in this crate
        // calls `Module::drop_block`.
        let items = unsafe { shape::array_items(&self.module, AnyNodeId::Dynamic(pair)) }?;
        items
            .get(shape::attr_slot(pos))
            .and_then(|item| match item.node {
                AnyNodeId::Dynamic(n) => Some(n),
                AnyNodeId::Static(_) => None,
            })
    }

    /// An expression's attribute slot for `marker` — its own slot, else the
    /// attribute's *missing* slot.
    ///
    /// # Invariant
    /// Both are `[value, type]` term pairs, so the apply-time check compares
    /// them directly. Only meaningful once the expression is compiled.
    pub(super) fn attr_or_missing(&mut self, e: ExprId, marker: &P::Attr) -> NodeId {
        if let Some(slot) = self.state[e].attr {
            return slot;
        }
        self.missing_slot_of(marker, self.loc(e, 2))
    }

    /// The attribute's *missing* slot node, shared when the attribute opted
    /// in, else built fresh by the extension.
    ///
    /// # Invariant
    /// The shared node is built on **first use**, not at install: the site is
    /// cold, so an eager install would spend two nodes on every build to save
    /// them only on the few that ask. Laziness is never worse — a slot first
    /// needed inside a lambda is cloned per apply, as the per-occurrence form
    /// did — only sometimes no better.
    pub(super) fn missing_slot_of(&mut self, marker: &P::Attr, loc: Loc) -> NodeId {
        // Invariant: keyed by the position in `P::Attr::ORDER`, which sizes
        // the cache; a plugin index is only debug-checked.
        let Some(index) = P::Attr::ORDER.iter().position(|attr| attr == marker) else {
            // Not an attribute of the set the cache is sized for: the same
            // guard and hole as an attribute this build cannot lower.
            return self.no_attr_ext_guard(loc);
        };
        if let Some(shared) = self.missing_slots[index] {
            return shared;
        }
        let Some(ext) = self.attribute_extension(marker) else {
            return self.no_attr_ext_guard(loc);
        };
        let slot = ext.missing_slot(self);
        if ext.share_missing_slot() {
            self.missing_slots[index] = Some(slot);
        }
        slot
    }

    /// The **type value** an expression in type position denotes. See
    /// `docs/notes/operator-polymorphism.md` §3.
    ///
    /// # Invariant
    /// A type expression's term is the type value itself, unless the expression
    /// carries attributes — then the type it denotes is the **annotated value's**
    /// own term, a placeholder's cell or a type constant's `[marker, kind]`
    /// pair. The pair is what keeps the class reachable from the parameter
    /// pair, so the apply clone re-instantiates the refinement per call.
    pub(super) fn type_denotation(&mut self, type_expr: ExprId, value: Option<ExprId>) -> NodeId {
        // Every attribute in play is reconciled here, the *missing* slot standing
        // in where a side carries none.
        if let Some(value) = value {
            self.unify_type_attributes(type_expr, value);
        }
        let mut expr = type_expr;
        while let ExprKind::Annotation { value, .. } = self.ir[expr].kind {
            expr = value;
        }
        self.state[expr]
            .term
            .expect("a type expression is compiled before its denotation is read")
    }

    /// Unify the attributes of the type expression with those of the annotated
    /// `value`, one marker at a time.
    ///
    /// # Invariant
    /// A side that does not carry a marker contributes the attribute's
    /// **missing** slot, so a one-sided spelling is reconciled against the other
    /// side's absence. The two schemas are dense over the attributes each
    /// carries, so the pairing is by marker, never by position.
    fn unify_type_attributes(&mut self, type_expr: ExprId, value: ExprId) {
        let type_tail = self.schema_tail(type_expr).to_vec();
        let value_tail = self.schema_tail(value).to_vec();
        let mut markers: Vec<P::Attr> = Vec::new();
        for marker in type_tail.iter().chain(value_tail.iter()) {
            if markers
                .iter()
                .any(|seen| seen.order_index() == marker.order_index())
            {
                continue;
            }
            markers.push(*marker);
        }
        let loc = self.loc(value, 2);
        let mut pairs: Vec<(P::Attr, NodeId, NodeId)> = Vec::new();
        for marker in markers {
            let type_slot = self.attr_or_missing(type_expr, &marker);
            let value_slot = self.attr_or_missing(value, &marker);
            pairs.push((marker, value_slot, type_slot));
        }
        for (marker, value_slot, type_slot) in pairs {
            let Some(ext) = self.attribute_extension(&marker) else {
                continue;
            };
            ext.unify_slots(self, value_slot, type_slot, loc.clone());
        }
    }

    pub(super) fn check_ann(&mut self, e: ExprId, value: ExprId, r#type: Option<ExprId>) -> NodeId {
        self.check_expr(value);
        // `: T` — the value's type unified with the type expression itself; both
        // sides are pairs in the recursive encoding.
        let type_pair = match r#type {
            Some(type_expr) => {
                self.check_expr(type_expr);
                // The type the annotation names (`type_denotation`); it is enforced
                // where written, so no slot is needed here.
                let denotation = self.type_denotation(type_expr, Some(value));
                let found = self.state[value].ty.unwrap();
                self.compute_operands(found, denotation);
                self.check_unify(found, denotation, self.loc(value, 1), DiagKind::Annotation);
                denotation
            }
            None => self.state[value].ty.unwrap(),
        };
        let value_node = self.value_of(value);
        // The annotated value's own term — the pair a refinement's predicate
        // is applied to.
        let value_term = self.state[value]
            .term
            .expect("an annotated value is compiled");
        // The annotation replaces the slots it spells and preserves the rest,
        // so the schema is the value's merged with its own.

        // A spelled constraint unifies the requirement against the value's
        // attribute, its provider; a label constrains nothing.
        let own_tail = self.ir.schema(e).clone().tail;
        let value_tail = self.schema_tail(value).to_vec();
        // Invariant: the frontend emits attribute values in canonical order,
        // so `attrs[i]` pairs with `tail[i]`.
        debug_assert!(
            own_tail.is_sorted_by_key(|m| m.order_index()),
            "an annotation's schema tail must be in the canonical attribute order"
        );
        let tail = self.merge_slots(value_tail, own_tail.clone());
        // The merged tail is a product of checking, so it lives in the checker's
        // own table, not in the frontend's IR.
        self.merged_tails.insert(e, tail.clone());
        let attrs = self.ir.annotation_attrs(e).to_vec();
        let mut slots: Vec<NodeId> = Vec::with_capacity(tail.len());
        let mut constraint_slot: Option<NodeId> = None;
        let mut attr_idx = 0;
        for marker in &tail {
            let Some(ext) = self.attribute_extension(marker) else {
                // An attribute this build cannot lower: report it here and keep
                // the arity with the hole; no constraint slot.
                slots.push(self.no_attr_ext_guard(self.loc(e, 2)));
                continue;
            };
            // Does this annotation spell this slot?
            let spelled = own_tail
                .iter()
                .any(|m| m.order_index() == marker.order_index());
            if spelled {
                let pe = attrs
                    .get(attr_idx)
                    .copied()
                    .expect("a spelled attribute slot has a value expression");
                attr_idx += 1;
                self.check_expr(pe);
                // The slot is the annotation value's `[value, type]` term pair, the
                // one shape every attribute shares.
                let slot = self.state[pe]
                    .term
                    .expect("an annotation value expr is compiled");
                if ext.is_label() {
                    // A label carries no constraint and no apply-time slot; its
                    // own `is_subtype` is what lets `? b` override `? a`.
                    slots.push(slot);
                } else {
                    // A *constraint* replaces the slot and validates it against the
                    // value's existing attribute, the *provider*.

                    // The provider is the value's own slot, or for a compound the
                    // combine of its children; with none, nothing to validate.
                    let provider = if self.state[value].attr.is_some() {
                        Some(self.attr_or_missing(value, marker))
                    } else {
                        let children = self.persp_combine_children(value);
                        if children.is_empty() {
                            None
                        } else {
                            let child_attrs: Vec<NodeId> = children
                                .iter()
                                .map(|&c| self.attr_or_missing(c, marker))
                                .collect();
                            Some(ext.combine(self, &child_attrs))
                        }
                    };
                    if let Some(p) = provider {
                        let loc2 = self.loc(e, 2);
                        ext.unify_slots(self, p, slot, loc2);
                    }
                    // The annotation value *is* the slot (it replaces).
                    constraint_slot = Some(slot);
                    slots.push(slot);
                    // A refinement is enforced: registered while the expression is
                    // lowered, so the apply clone re-checks it per call.
                    if let Some(condition) = ext.constraint(self, value_term, slot) {
                        self.register_assert(
                            condition,
                            self.loc(e, 0),
                            true,
                            AssertSpelling::Condition,
                        );
                    }
                }
            } else {
                // A slot the annotation does not spell is preserved: the value's
                // existing term pair carries over unchanged.
                let node = self
                    .value_attr_node(value, marker)
                    .unwrap_or_else(|| self.missing_slot_of(marker, self.loc(e, 2)));
                if !ext.is_label() {
                    constraint_slot = Some(node);
                }
                slots.push(node);
            }
        }
        // The constraint slot is what the apply-time check reads; a label is
        // metadata only.
        self.state[e].attr = constraint_slot;
        let mut pair = Vec::with_capacity(slots.len() + 2);
        pair.push(value_node);
        pair.push(type_pair);
        pair.extend(slots);
        let pair = self.array_node(self.current_block, &pair);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(type_pair);
        pair
    }
}
