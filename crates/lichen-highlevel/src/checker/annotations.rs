//! Annotations and attributes: the `# p` / `? doc` annotation rules and the
//! attribute-slot algebra they are built on — the slot merge, the slot a value
//! already carries, and the *missing* slot an expression without the attribute
//! is checked against.  The checker never names a concrete attribute: it asks
//! the registry ([`crate::attr::AttrExt`]) for the behaviour.

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
    /// build has no attribute extension at all ([`Checker::build`] and
    /// [`Checker::build_in`] install none).  A marker this build cannot lower
    /// is a check-time refusal, not a broken invariant: the sites that read it
    /// report [`DiagKind::NoAttributeExtension`] through
    /// [`Self::no_attr_ext_guard`] and carry on over the hole it returns.
    ///
    /// [`Checker::build`]: super::Checker::build
    /// [`Checker::build_in`]: super::Checker::build_in
    pub(super) fn attribute_extension(
        &self,
        marker: &P::Attr,
    ) -> Option<&'static dyn crate::attr::AttrExt<P>> {
        self.attr_ext.as_ref().map(|registry| registry(marker))
    }

    /// Records the [`DiagKind::NoAttributeExtension`] guard at `loc` — the
    /// site that read the attribute — and returns the well-formed hole the
    /// slot falls back to: a fresh unbound `[value, type]` pair, the shape
    /// every attribute slot has (see [`crate::attr`]).  Nothing unifies
    /// against it — the guard has already failed the build, and
    /// `check_failed` skips the definition pass — so it binds nothing, while
    /// the runtime pair keeps the arity its schema declared.
    ///
    /// Recorded **once per build**, at the first reader: a program may read an
    /// attribute at every expression it annotates, and the fact being reported
    /// is about the build, not about any one of them.
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

    /// The direct sub-expressions of a compound whose perspectives participate
    /// in a `# p` annotation's meet (gcd) — the reference.  A leaf has none.
    /// Per the plan's combine table: the named sub-expressions of a `BinOp`/
    /// `Apply`/`Instantiate`/`Index`/`Field`/`Find`/`TypeArray`/`TypeFunction`
    /// and a `TypeFunction`, the `children` range of a variadic, transparent
    /// through an `Annotation`, and empty for a leaf.
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
            // An annotated value contributes no sub-expression to a parent's
            // combine: its attribute is its OWN slot (read via
            // `self.state[value].attr` in `check_ann`), never the value beneath it
            // (`a # q` re-annotated keeps `q`; a `? doc` on a value writes the
            // new doc).  A doc-only annotation (no perspective) therefore
            // reads as a leaf.
            ExprKind::Annotation { .. } => Vec::new(),
            // Leaf kinds — the annotation binds the slot directly.  An
            // `ErrorBlock` is a leaf too: a masked region carries no children
            // to combine.
            ExprKind::Literal(_)
            | ExprKind::Parameter
            | ExprKind::Placeholder
            | ExprKind::ErrorBlock
            | ExprKind::Static { .. } => Vec::new(),
            // A lambda is a leaf for stage 1 (its own `# p` binds a slot).
            ExprKind::Function { .. } => Vec::new(),
            ExprKind::Assert { .. } => Vec::new(),
            // The converted operand is the expression's subject, so its
            // perspective participates in the meet exactly as a `BinOp`
            // operand's does.
            ExprKind::Convert { value, .. } => vec![value],
            ExprKind::NativeCall { .. } => self.range_children(e),
        }
    }

    /// Merge an annotation's *spelled* attribute slots into a value's existing
    /// slot set, producing the resulting expression's full schema tail.  The
    /// annotation **replaces** the slots it names (the same
    /// [`AttrSet::order_index`]) and **preserves** every slot it does not — so
    /// `(x # 8 ? doc) # 4` keeps the doc while re-checking the perspective.
    /// Ordered by the canonical attribute order, so the runtime pair stays
    /// positionally consistent.
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

    /// The value expression's existing attribute slot for `marker` — the node
    /// the value's runtime pair carries at that attribute's position (always a
    /// `[value, type]` term pair, for a constraint and a label alike).  Used
    /// to *preserve* a slot an annotation does not spell (`(x # 8 ? doc) # 4`
    /// keeps the doc).  `None` when the value's schema has no such slot.
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

    /// The attribute slot of an expression for `marker` — its own slot when it
    /// carries the attribute, else the attribute's *missing* slot (built as a
    /// `[missing_value, int]` term pair, the uniform slot shape).  Only
    /// meaningful after the expression has been compiled.  The marker names
    /// which attribute the caller is asking about; the missing slot is the
    /// attribute's own (`[0, int]` for a perspective).
    pub(super) fn attr_or_missing(&mut self, e: ExprId, marker: &P::Attr) -> NodeId {
        if let Some(slot) = self.state[e].attr {
            return slot;
        }
        self.missing_slot_of(marker, self.loc(e, 2))
    }

    /// The attribute's *missing* slot node — one shared node for the whole
    /// build when the attribute opted in (see
    /// [`AttrExt::share_missing_slot`]), otherwise built fresh through the
    /// extension's `AttrExt` (a perspective reads `[0, int]`).  Used where a
    /// slot is needed for an expression that does not carry the attribute, or
    /// where the *declared* side of a check is the absent value.
    ///
    /// The shared node is built on **first use**, not in the install pass, and
    /// the measurement is why: this site is cold — a program that never has an
    /// absent perspective never reaches it — so an eager install would spend two
    /// nodes on every build to save them only on the few that ask.  The cost of
    /// laziness is that a slot first needed *inside* a lambda is tagged into
    /// that function's template and so is cloned per apply, which is exactly
    /// what the per-occurrence form did; it is never worse, only sometimes no
    /// better.  `loc` is the attribute slot of the expression the slot is
    /// needed for — where the missing attribute is read.
    pub(super) fn missing_slot_of(&mut self, marker: &P::Attr, loc: Loc) -> NodeId {
        // The slot cache is keyed by the marker's **position in the set's own
        // order list**, not by `AttrSet::order_index`: the list is what sizes
        // [`Checker::missing_slots`], so the position is in range by
        // construction, while an index the plugin returns is only checked in
        // debug builds by `order_is_canonical` — a hand-written set that
        // disagrees with its own list would index out of bounds, or alias
        // another attribute's cached slot, in a release build.  For a set whose
        // order *is* canonical the two are the same number, so nothing moves.
        let Some(index) = P::Attr::ORDER.iter().position(|attr| attr == marker) else {
            // Not an attribute of the set the cache is sized for: the same
            // recorded guard and well-formed hole as an attribute this build
            // cannot lower.
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

    /// The **type value** an expression in type position denotes.
    ///
    /// A type expression's term *is* the type value — unless the expression
    /// carries attributes.  A refinement written on a type (`x : (_ ! in_num)`)
    /// makes the term the `[type, …, attribute]` pair the attribute lives in, and
    /// the type it denotes is the **annotated value's** own term: a placeholder's
    /// cell (what will hold the class), or a type constant's `[marker, kind]`
    /// pair.  Taking the pair's first slot instead would answer a *shape* for a
    /// type constant, and unifying a shape with a parameter's type slot makes
    /// `g 7` fail against `Int` while printing `expected Int, found Int`
    /// (measured).
    ///
    /// The attributes are not lost by taking the denotation: the annotation
    /// registered its own assert where it was written, so it rides the enclosing
    /// function and is re-checked per call
    /// (`docs/notes/operator-polymorphism.md` §3).
    ///
    /// One consequence is visible in a printed signature, and it is honest
    /// rather than a defect: an **open** class refinement's annotated type is the
    /// placeholder's `[shape, kind]` pair of cells — the pair is what makes the
    /// class reachable from the parameter pair, so the apply clone re-instantiates
    /// the refinement's condition per call (taking the placeholder's *value cell*
    /// alone loses that, measured: `f "a"` was then accepted) — and a type the
    /// printer cannot read as a form is marked raw
    /// ([raw-rendering-mark](raw-rendering-mark.md)).  So `x : (_ ! in_num) => e`
    /// prints `raw[?a, ?b] -> …` where `x : (Int ! in_num) => e` prints
    /// `Int -> …`.
    pub(super) fn type_denotation(&self, type_expr: ExprId) -> NodeId {
        let mut expr = type_expr;
        while let ExprKind::Annotation { value, .. } = self.ir[expr].kind {
            expr = value;
        }
        self.state[expr]
            .term
            .expect("a type expression is compiled before its denotation is read")
    }

    pub(super) fn check_ann(&mut self, e: ExprId, value: ExprId, r#type: Option<ExprId>) -> NodeId {
        self.check_expr(value);
        // `: T` — the value expression's type must unify with the type
        // expression itself; both sides are pairs in the recursive encoding.
        // The type slot is the annotation's own type expression (shared), or
        // the value's own type when only an attribute is present.  (Struct
        // instantiation is not an annotation — it is the dedicated
        // [`ExprKind::Instantiate`].)
        let type_pair = match r#type {
            Some(type_expr) => {
                self.check_expr(type_expr);
                // The type the annotation **names**.  A type expression's term
                // is the type value itself — unless the expression carries
                // attributes, which is how a refinement is written *on a type*:
                // `x : (_ ! in_num)` makes the type expression's term the
                // `[type, …, attribute]` pair the attribute lives in, and the
                // type it denotes is the annotated value's own term
                // ([`Checker::type_denotation`]).  The attribute is enforced
                // where it was written — that annotation registered its own
                // assert on the type value — so the outer annotation needs no
                // slot of its own.
                let denotation = self.type_denotation(type_expr);
                self.check_unify(
                    self.state[value].ty.unwrap(),
                    denotation,
                    self.loc(value, 1),
                    DiagKind::Annotation,
                );
                denotation
            }
            None => self.state[value].ty.unwrap(),
        };
        let value_node = self.value_of(value);
        // The annotated value's own term — the `[value, type]` pair a
        // refinement's predicate is applied to (the *value being checked*, not
        // the annotation's pair, which carries the refinement slot itself).
        let value_term = self.state[value]
            .term
            .expect("an annotated value is compiled");
        // The annotation *replaces* the attribute slots it spells and
        // *preserves* every slot it does not — it is not a fresh, isolated
        // attribute set.  So the resulting schema is the value's slots merged
        // with the annotation's own: `(x # 8 ? doc) # 4` re-checks the
        // perspective (unifying the requirement `4` against the provider `8`)
        // and keeps the doc.  A spelled constraint unifies the annotation
        // (the *requirement*) against the value's existing attribute (the
        // *provider*; it must be a subtype of it); a spelled label carries no
        // constraint.  The checker asks the registry for each attribute's
        // `AttrExt` — it never names a concrete attribute, so the mechanism is
        // generic over the attribute set.
        let own_tail = self.ir.schema(e).clone().tail;
        let value_tail = self.schema_tail(value).to_vec();
        // Contract with the frontend: an annotation's attribute value
        // expressions are emitted in the canonical attribute order, the same
        // order the merged tail is sorted into below — that is what makes the
        // positional `attrs[i]` ↔ `tail[i]` pairing well defined.
        debug_assert!(
            own_tail.is_sorted_by_key(|m| m.order_index()),
            "an annotation's schema tail must be in the canonical attribute order"
        );
        let tail = self.merge_slots(value_tail, own_tail.clone());
        // Record the merged tail in the checker's own table — the IR is the
        // frontend's, and this tail is a product of checking it, so later
        // readers (and the renderer) read it from
        // [`Checker::schema_tail`](super::Checker::schema_tail) instead.
        self.merged_tails.insert(e, tail.clone());
        let attrs = self.ir.annotation_attrs(e).to_vec();
        let mut slots: Vec<NodeId> = Vec::with_capacity(tail.len());
        let mut constraint_slot: Option<NodeId> = None;
        let mut attr_idx = 0;
        for marker in &tail {
            let Some(ext) = self.attribute_extension(marker) else {
                // The schema carries an attribute this build cannot lower:
                // report it at the annotation and keep the pair's arity with
                // the hole.  No constraint slot is recorded — without an
                // extension there is nothing to constrain with — so nothing
                // downstream consults the extension for this expression a
                // second time.
                slots.push(self.no_attr_ext_guard(self.loc(e, 2)));
                continue;
            };
            // Does this annotation spell this slot?  (its own schema lists
            // exactly the slots it replaces; everything else is preserved.)
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
                // The slot is the annotation value's `[value, type]` term pair
                // — the ONE slot shape every attribute shares.  A constraint
                // (e.g. `Perspective`) reads its lattice value from element 0;
                // a label (e.g. `Doc`) uses the whole pair (its renderer walks
                // the value's type chain).  The checker never special-cases a
                // label's slot representation — the distinction below is the
                // *semantic* one (does the attribute constrain at apply time?)
                // that lives in [`AttrExt::is_label`].
                let slot = self.state[pe]
                    .term
                    .expect("an annotation value expr is compiled");
                if ext.is_label() {
                    // A label (metadata, e.g. `Doc`) carries no constraint: it
                    // contributes no apply-time slot.  The attribute's own
                    // `is_subtype` (doc → always `true`) is what permits
                    // `? b` to override `? a` without conflict.
                    slots.push(slot);
                } else {
                    // A *constraint* **replaces** the slot with the annotation
                    // value, and validates it against the value's EXISTING
                    // attribute (the *provider*).  The annotation (`expr2`) is
                    // the *requirement* and must be a subtype of the provider —
                    // `(x # 8) # 4` is legal (uniform-8 entails uniform-4), so
                    // the slot becomes `4`; `(x # 4) # 8` is not (uniform-4
                    // does not entail uniform-8).  The provider is the value's
                    // own attribute slot (a value that is itself annotated, or
                    // a bound name carrying an attribute) or, for a compound,
                    // the combine of its sub-expressions' slots.  A value with
                    // no attribute of its own (a plain leaf, or a doc-only
                    // annotation) has no provider, so there is nothing to
                    // validate against and the annotation is the slot.
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
                    // A refinement is **enforced**, not merely reconciled: the
                    // attribute applies its predicate to the annotated value and
                    // the ordinary assert channel requires the result to be `1`.
                    // Registered here, while the expression is lowered, so it
                    // rides the enclosing function and the apply clone re-checks
                    // the instantiated condition per call — which is what turns a
                    // refinement written on a parameter into a per-application
                    // check.  An attribute that constrains nothing returns `None`
                    // and the checker names no concrete attribute.
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
                // A slot the annotation does not spell is *preserved*: carry the
                // value's existing attribute for this marker over unchanged —
                // a term pair in both cases (a constraint's lattice value sits
                // at element 0, a label's metadata is the whole pair).
                let node = self
                    .value_attr_node(value, marker)
                    .unwrap_or_else(|| self.missing_slot_of(marker, self.loc(e, 2)));
                if !ext.is_label() {
                    constraint_slot = Some(node);
                }
                slots.push(node);
            }
        }
        // The constraint slot (e.g. the perspective) is what the apply-time
        // attribute check reads; a label slot is metadata only.
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
