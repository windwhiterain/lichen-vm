//! The `# p` / `? doc` annotation rules and the slot algebra — the merge,
//! a value's slot, and the *missing* slot.

use lichen_lowlevel::{AnyNodeId, LowOperator, NodeId};

use std::collections::HashSet;

use crate::attr::AttrSet;
use crate::diagnostic::DiagKind;
use crate::ir::{ExprId, ExprKind, Loc};
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;
use crate::diagnostic::AssertSpelling;

/// The element pairs one reconciliation walk has already visited — one set per
/// walk, so neither suppresses the other.
///
/// # Invariant
/// A compound type may be **recursive** (`A = struct{.g B}`, `B = struct{.g
/// A}`), and a pair's whole effect is the class merges it states, so visiting
/// it once is the whole of that effect. The two sets are separate because the
/// attribute and the pin walks are different effects over the same pair.
#[derive(Default)]
pub(super) struct ElementWalk {
    /// The pairs [`Checker::check_element_pair_attributes`] has reconciled.
    attributes: HashSet<(ExprId, ExprId)>,
    /// The pairs [`Checker::check_element_pair_values`] has reconciled.
    values: HashSet<(ExprId, ExprId)>,
}

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
            ExprKind::Field { container, key } => vec![container, key],
            ExprKind::NamedField { container, .. } => vec![container],
            ExprKind::Find { container, key } => vec![container, key],
            ExprKind::TypeArray {
                element_type,
                length,
            } => vec![element_type, length],
            ExprKind::TypeFunction {
                parameter,
                return_type,
            } => vec![parameter, return_type],
            ExprKind::Tuple(_)
            | ExprKind::TypeTuple(_)
            | ExprKind::TypeSet(_)
            | ExprKind::TypeTable(_)
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
            // The prefix assert is a leaf for stage 1: its own `# p` binds a slot.
            ExprKind::Assert { .. } => Vec::new(),
            // `@gc` adds no operator, so the value is the inner expression's.
            ExprKind::Gc { inner } => self.persp_combine_children(inner),
            // A prefix operator's operands' perspectives meet as a `BinOp`'s do.
            ExprKind::Prefix { .. } => self.range_children(e),
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
        if !value_tail.contains(marker) {
            return None;
        }
        let pair = self.state[value].term?;
        match self.slot_attribute(pair, marker.order_index() as u32)? {
            AnyNodeId::Dynamic(node) => Some(node),
            AnyNodeId::Static(_) => None,
        }
    }

    /// The node a value carries for `marker`: its term pair's slot, else its
    /// recorded constraint slot.
    fn value_attr_for(&mut self, value: ExprId, marker: &P::Attr) -> Option<NodeId> {
        if let Some(node) = self.value_attr_node(value, marker) {
            return Some(node);
        }
        let tail = self.schema_tail(value).to_vec();
        let constraint = tail
            .iter()
            .rev()
            .find(|m| {
                self.attribute_extension(m)
                    .is_some_and(|ext| !ext.is_label())
            })
            .copied();
        (constraint.as_ref() == Some(marker))
            .then(|| self.state[value].attr)
            .flatten()
    }

    /// The field of the enclosing parameter an attribute value follows, or [`None`]
    /// for a value that names no part of it.
    ///
    /// # Invariant

    /// The field of the enclosing parameter an attribute value follows, or [`None`]
    /// for a value that names no part of it.
    ///
    /// # Invariant
    /// Recognised by **shape** — a call whose argument is the parameter or one of its
    /// fields — never by the callee's name, so any `xxx_of` vocabulary function that
    /// takes the parameter qualifies. `None` inside the returned option is the whole
    /// parameter; `Some(name)` is that field.
    fn attribute_reference(&self, pe: ExprId) -> Option<Option<&'static str>> {
        let ExprKind::Apply { argument, .. } = self.ir[pe].kind else {
            return None;
        };
        let Some(parameter) = self.current_parameters.last().copied() else {
            return None;
        };
        if argument == parameter {
            return Some(None);
        }
        match self.ir[argument].kind {
            ExprKind::NamedField { container, name } if container == parameter => Some(Some(name)),
            _ => None,
        }
    }

    /// The node that holds the `[value, type]` pair an attribute value answers with.
    ///
    /// # Invariant
    /// A call's node holds no value of its own — its class does — so the walk is forced
    /// and the class's **representative** is the slot, because that is the node the
    /// merge wrote the pair onto. Every other expression's own term already is its pair.
    fn annotation_value_slot(&mut self, pe: ExprId) -> NodeId {
        let term = self.state[pe]
            .term
            .expect("an annotation value expr is compiled");
        if self.node_value(term).is_some() {
            return term;
        }
        if self.module.node_operation(term).is_some() {
            self.module.evaluate_node_deep(term, None);
            return self.module.equality_representative(term);
        }
        term
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

    /// The slot an argument provides for `marker`: its own, or for a compound the
    /// combine of what its components provide.
    ///
    /// # Invariant
    /// The reading is the annotation provider's, made recursive: an argument states the
    /// missing top only where it has no sub-expressions. A struct keeps the meet of its
    /// fields, so one lane-varying field makes the whole argument lane-varying to a
    /// requirement.
    pub(super) fn provided_slot(
        &mut self,
        argument: ExprId,
        marker: &P::Attr,
        ext: &dyn crate::attr::AttrExt<P>,
    ) -> NodeId {
        if self.state[argument].attr.is_some() {
            return self.attr_or_missing(argument, marker);
        }
        let children = self.persp_combine_children(argument);
        if children.is_empty() {
            return self.attr_or_missing(argument, marker);
        }
        let attrs: Vec<NodeId> = children
            .iter()
            .map(|&child| self.provided_slot(child, marker, ext))
            .collect();
        ext.combine(self, &attrs)
    }

    /// The provided slot against a declared value: one attribute requirement,
    /// through the extension's relaxation.
    ///
    /// # Invariant
    /// An undecided declared value states nothing (`#_`) and is skipped, so no call
    /// pins a template's cell; the relaxation is the extension's own (a perspective's
    /// `declared | provided`).
    pub(super) fn unify_declared_value(
        &mut self,
        argument: ExprId,
        marker: &P::Attr,
        declared: NodeId,
        loc: Loc,
    ) where
        P::Attr: PartialEq,
    {
        let Some(ext) = self.attribute_extension(marker) else {
            return;
        };
        if self.node_value(declared).is_none() {
            return;
        }
        // **A parameter's requirement is replayed at the apply**: the cell is a
        // hole (`docs/notes/array-attribute-view.md`).
        if matches!(self.ir[argument].kind, ExprKind::Parameter)
            && let Some(function) = self.current_function()
        {
            self.deferred_requirements
                .entry(function)
                .or_default()
                .push((*marker, declared));
        }
        let found = self.provided_slot(argument, marker, ext);
        // **A failure the order admits leaves no error**: the apply would stop
        // at it (`docs/notes/array-attribute-view.md`).
        let before = self.module.unify_errors.len();
        ext.unify_slots(self, found, declared, loc);
        if self.module.unify_errors.len() > before && ext.is_subtype(self, declared, found) {
            self.module.unify_errors.truncate(before);
        }
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
            self.unify_type_attributes(type_expr, value, self.loc(value, shape::PAIR_ATTR_BASE));
            self.unify_element_attributes(type_expr, value);
            self.unify_element_values(type_expr, value);
        }
        let mut expr = type_expr;
        while let ExprKind::Annotation { value, .. } = self.ir[expr].kind {
            expr = value;
        }
        self.state[expr]
            .term
            .expect("a type expression is compiled before its denotation is read")
    }

    /// Unify the attributes the **type expression carries** with the annotated
    /// `value`'s, one marker at a time.
    ///
    /// # Invariant
    /// A type that does not carry a marker states no requirement for it: the value's
    /// own attribute is its provider, and a plain type must not demand the top (a
    /// `# 1` value under `e : T` would be refused, which is the frozen signature's
    /// ordinary case). The two schemas are dense over the attributes each carries, so
    /// the pairing is by marker, never by position.
    fn unify_type_attributes(&mut self, type_expr: ExprId, value: ExprId, loc: Loc) {
        let type_tail = self.schema_tail(type_expr).to_vec();
        let mut pairs: Vec<(P::Attr, NodeId, NodeId)> = Vec::new();
        for marker in type_tail {
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

    /// Unify a **compound type's element requirements** with the elements of the
    /// value it types, one element at a time.
    ///
    /// # Invariant
    /// An element slot's attribute positions hold the requirement and a value's
    /// element slot positions are `@any`, so the lowlevel compare is vacuous by
    /// construction: the reconciliation is here, by marker, exactly as a whole
    /// annotation's is, and the same walk the function kind does for a declared
    /// parameter attribute. See docs/notes/compound-element-slots.md.
    fn unify_element_attributes(&mut self, type_expr: ExprId, value: ExprId) {
        let mut seen = ElementWalk::default();
        for (type_el, value_el) in self.element_pairs(type_expr, value) {
            self.check_element_pair_attributes(type_el, value_el, &mut seen);
        }
    }

    /// One element pair's attribute reconciliation, descending into the nested
    /// compounds beneath it.
    ///
    /// # Invariant
    /// This is [`Self::unify_element_attributes`]'s body, so a caller that pairs
    /// by its own rule — the struct instantiation's definition order — gets the
    /// same walk the positional pairing gives.
    ///
    /// `seen` terminates a **recursive** type: a pair's reconciliation is a
    /// fixed set of class merges, so visiting one once is all of its effect.
    pub(super) fn check_element_pair_attributes(
        &mut self,
        type_el: ExprId,
        value_el: ExprId,
        seen: &mut ElementWalk,
    ) {
        if !seen.attributes.insert((type_el, value_el)) {
            return;
        }
        if !self.schema_tail(type_el).is_empty() || !self.schema_tail(value_el).is_empty() {
            let loc = self.loc(value_el, shape::PAIR_ATTR_BASE);
            self.unify_type_attributes(type_el, value_el, loc);
        }
        for (nested_type, nested_value) in self.element_pairs(type_el, value_el) {
            self.check_element_pair_attributes(nested_type, nested_value, seen);
        }
    }

    /// The **pairs** an element walk compares: a compound type's element
    /// expressions against the value's.
    ///
    /// # Invariant
    /// An array's element position is *one* slot serving every element, so a
    /// single type element pairs with each value element — which pairs a
    /// one-element tuple with its own element the same way. A count mismatch of
    /// any other shape is the structural unify's to report.
    fn element_pairs(&self, type_expr: ExprId, value: ExprId) -> Vec<(ExprId, ExprId)> {
        let (Some(types), Some(values)) = (
            self.compound_children(type_expr),
            self.compound_children(value),
        ) else {
            return Vec::new();
        };
        match types.as_slice() {
            [element] => values.iter().map(|&value| (*element, value)).collect(),
            _ if types.len() == values.len() => types.into_iter().zip(values).collect(),
            _ => Vec::new(),
        }
    }

    /// Unify a **compound type's element values** with the elements of the value
    /// it types: a spelled value is a **pin**.
    ///
    /// # Invariant
    /// A value's element slots hold the wildcard, so the lowlevel compare cannot
    /// enforce a pin; the reconciliation is here, and a wildcard's own class makes
    /// its arm vacuous — which is exactly why `@any : T` is no pin.
    /// See docs/notes/compound-element-slots.md.
    fn unify_element_values(&mut self, type_expr: ExprId, value: ExprId) {
        let mut seen = ElementWalk::default();
        for (type_el, value_el) in self.element_pairs(type_expr, value) {
            self.check_element_pair_values(type_el, value_el, &mut seen);
        }
    }

    /// One element pair's pin: the `: ` its type element spells, against the
    /// value that fills the position.
    ///
    /// # Invariant
    /// This is [`Self::unify_element_values`]'s body, so a caller that pairs by
    /// its own rule gets the same walk the positional pairing gives. `seen`
    /// terminates a recursive type, as
    /// [`Self::check_element_pair_attributes`]'s does.
    pub(super) fn check_element_pair_values(
        &mut self,
        type_el: ExprId,
        value_el: ExprId,
        seen: &mut ElementWalk,
    ) {
        if !seen.values.insert((type_el, value_el)) {
            return;
        }
        if let ExprKind::Annotation {
            value: pin,
            r#type: Some(inner),
            ..
        } = self.ir[type_el].kind
        {
            let pin = self.value_of(pin);
            let actual = self.value_of(value_el);
            self.check_unify(
                actual,
                pin,
                self.loc(value_el, shape::PAIR_VALUE_SLOT),
                DiagKind::Annotation,
            );
            for (nested_type, nested_value) in self.element_pairs(inner, value_el) {
                self.check_element_pair_values(nested_type, nested_value, seen);
            }
            return;
        }
        for (nested_type, nested_value) in self.element_pairs(type_el, value_el) {
            self.check_element_pair_values(nested_type, nested_value, seen);
        }
    }

    /// The element expressions of a compound **type or value** expression: its
    /// children once any annotation is peeled off.
    ///
    /// # Invariant
    /// Only the compound kinds answer; an apply's operands are not elements. A
    /// set's and a table's type form write their elements directly, so a value
    /// pairs one for one; an array's is one element serving every value element.
    ///
    /// A **named** instantiation answers `None`: its IR keeps the call's order,
    /// so a positional pairing would take the wrong field (see
    /// `unify_instantiate_elements`).
    pub(super) fn compound_children(&self, e: ExprId) -> Option<Vec<ExprId>> {
        let mut expr = e;
        while let ExprKind::Annotation { value, .. } = self.ir[expr].kind {
            expr = value;
        }
        match self.ir[expr].kind {
            ExprKind::Tuple(_) | ExprKind::TypeTuple(_) => Some(self.range_children(expr)),
            // A value's elements: a set's members, a table's interleaved
            // key/value list, an array's element list.
            ExprKind::Array(_)
            | ExprKind::Set(_)
            | ExprKind::Table(_)
            | ExprKind::ShallowArray { .. } => Some(self.range_children(expr)),
            // The type forms: a set's one slot and a table's two, each of which
            // pairs with the value's elements.
            ExprKind::TypeSet(_) => Some(self.range_children(expr)),
            ExprKind::TypeTable(_) => Some(self.range_children(expr)),
            ExprKind::TypeArray { element_type, .. } => Some(vec![element_type]),
            // A struct's field types are element positions like a tuple's.
            ExprKind::TypeStruct { .. } => Some(self.range_children(expr)),
            // A struct instance's arguments, positional call only.
            ExprKind::Instantiate { value, names, .. } => {
                let named = self.ir.struct_names[names.start as usize..names.end as usize]
                    .iter()
                    .any(|name| name.is_some());
                if named {
                    None
                } else {
                    self.compound_children(value)
                }
            }
            _ => None,
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
                // A value computed from a part of the parameter is a **reference**.
                if let Some(field) = self.attribute_reference(pe) {
                    self.attr_refs.insert(e, (*marker, field));
                    // The reference's slot is a hole: the call site resolves it.
                    let cell = self.fresh_cell();
                    constraint_slot = Some(cell);
                    slots.push(cell);
                    continue;
                }
                // The slot is the annotation value's `[value, type]` term pair, the
                // one shape every attribute shares.
                let slot = self.annotation_value_slot(pe);
                if ext.is_label() {
                    // A label is metadata: it carries no apply-time slot, and its
                    // slot is the annotation value's pair like any other's.
                    slots.push(slot);
                } else {
                    // A *constraint* replaces the slot and validates it against the
                    // value's existing attribute, the *provider*.

                    // The provider is the value's own slot, else the combine of its
                    // sub-expressions' slots; none when neither provides one.
                    let provider = if self.state[value].attr.is_some() {
                        Some(self.attr_or_missing(value, marker))
                    } else {
                        let children = self.persp_combine_children(value);
                        if children.is_empty() {
                            None
                        } else {
                            let child_attrs: Vec<NodeId> = children
                                .iter()
                                .map(|&c| self.provided_slot(c, marker, ext))
                                .collect();
                            Some(ext.combine(self, &child_attrs))
                        }
                    };
                    // A computed value is not decidable here; its check belongs to
                    // the call site its certificate reaches.
                    let computed = matches!(
                        self.ir[pe].kind,
                        ExprKind::Apply { .. } | ExprKind::NativeCall { .. }
                    );
                    if let Some(p) = provider
                        && !computed
                    {
                        let loc2 = self.loc(e, 2);
                        ext.unify_slots(self, p, slot, loc2);
                    }
                    // The annotation value *is* the slot (it replaces).
                    constraint_slot = Some(slot);
                    slots.push(slot);
                    // A refinement is enforced: registered while the expression is
                    // lowered, so the apply clone re-checks it per call.
                    if let Some(condition) = ext.constraint(self, value_term, slot) {
                        let spelling = ext.assert_spelling().unwrap_or(AssertSpelling::Condition);
                        self.register_assert(condition, self.loc(e, 0), true, spelling);
                    }
                }
            } else {
                // A slot the annotation does not spell is preserved: the value's
                // existing term pair carries over unchanged.
                let node = self
                    .value_attr_for(value, marker)
                    .unwrap_or_else(|| self.missing_slot_of(marker, self.loc(e, 2)));
                if !ext.is_label() {
                    constraint_slot = Some(node);
                }
                slots.push(node);
            }
        }
        // The pair is **full width**: settled slots canonically, wildcards elsewhere
        // (`docs/notes/array-attribute-view.md`).
        let any = self.any();
        let mut width: Vec<NodeId> = vec![any; P::Attr::ORDER.len()];
        for (slot, marker) in slots.iter().zip(tail.iter()) {
            width[marker.order_index()] = *slot;
        }
        // Attributed at its canonical position, which `ExprState::attr` cannot say;
        // see `crate::lowering`.
        for (index, &node) in width.iter().enumerate() {
            self.node_edges
                .insert(node, self.loc(e, shape::attr_slot(index)));
        }
        // The constraint slot is what the apply-time check reads; a label is
        // metadata only.
        self.state[e].attr = constraint_slot;
        let mut pair = Vec::with_capacity(shape::slot_width::<P>());
        pair.push(value_node);
        pair.push(type_pair);
        pair.extend(width);
        let layout = self.full_width_layout();
        let pair = self.array_node_with_layout(self.current_block, &pair, layout);
        self.state[e].term = Some(pair);
        self.state[e].val = Some(value_node);
        self.state[e].ty = Some(type_pair);
        pair
    }
}
