use std::collections::HashSet;

use stacksafe::stacksafe;

use crate::{LowValue, Module, Node, NodeId, Operation, Operator, Program, is_unbound};
use lichen_utils::disjoint::{self, Node as _};
use lichen_utils::extend::AsEnum;

#[derive(Debug, Clone, Copy)]
pub struct UnifyError<P: Program> {
    pub a: NodeId,
    pub b: NodeId,
    pub value_a: Option<P::Value>,
    pub value_b: Option<P::Value>,
}

impl<P: Program> disjoint::Node for Node<P> {
    type Key = NodeId;
    fn meta(&self) -> &disjoint::Meta<NodeId> {
        &self.equality
    }
    fn meta_mut(&mut self) -> &mut disjoint::Meta<NodeId> {
        &mut self.equality
    }
}

impl<P: Program> Module<P> {
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        disjoint::union(&mut self.nodes, a, b)
    }

    pub fn equality_representative(&mut self, node: NodeId) -> NodeId {
        disjoint::find(&mut self.nodes, node)
    }

    /// Structurally unify the classes of `a` and `b`.
    ///
    /// Unification is over values: a class holding no value and no pending
    /// operation (a pure unbound cell) binds to the other side's value; an
    /// unevaluated operation is a *pending computation*, and a concrete
    /// value must never be bound over one — that would silently erase it
    /// (e.g. a dependent type branch that selects a different type per
    /// argument).  Such a computation is forced before comparing; if its
    /// operands are still unbound and it cannot resolve, the unify fails —
    /// except against an all-unbound skeleton (cells and arrays of cells),
    /// which merges with the computation: nothing is erased, and the
    /// computation's eventual value replicates onto the skeleton.  Two
    /// concrete values merge iff they are equal ([`PartialEq`]), except
    /// arrays, which unify elementwise (their structure is the value).  A
    /// conflict records a [`UnifyError`] in [`Self::unify_errors`] and
    /// leaves the two classes unmerged.
    ///
    /// Returns the representative of the merged class on success, or of
    /// `a`'s class when unification fails.
    pub fn unify(&mut self, a: NodeId, b: NodeId) -> NodeId {
        let mut path = Vec::new();
        self.unify_inner(a, b, &mut path);
        disjoint::find(&mut self.nodes, a)
    }

    /// Recursive core of [`Self::unify`]; `path` holds the class pairs on
    /// the current recursion, so a mutually recursive structure (an array
    /// unified with itself) records an error instead of looping.
    #[stacksafe]
    fn unify_inner(&mut self, a: NodeId, b: NodeId, path: &mut Vec<(NodeId, NodeId)>) -> bool {
        let ra = disjoint::find(&mut self.nodes, a);
        let rb = disjoint::find(&mut self.nodes, b);
        if ra == rb {
            return true;
        }
        if path.contains(&(ra, rb)) || path.contains(&(rb, ra)) {
            self.record_error(ra, rb);
            return false;
        }
        let va = self.nodes[ra].value;
        let vb = self.nodes[rb].value;
        // A class that is unbound and holds no unevaluated operation is a
        // pure cell: bind it to the other side.  A class with an unevaluated
        // operation is not bindable — it is a pending computation, and a
        // concrete value bound over it would erase the computation.  A read
        // of the class's own cell is not a pending computation — it is a
        // self-reference that resolves via replication when the class binds.
        let cell_a = is_unbound(va) && self.class_is_pure_cell(ra);
        let cell_b = is_unbound(vb) && self.class_is_pure_cell(rb);
        if cell_a || cell_b {
            self.bind(ra, rb, va, vb);
            return true;
        }
        // Neither side is a pure cell: force any pending computations so the
        // comparison sees their resolved values.  A computation whose
        // operands are still unbound cannot resolve; an `Index` over a
        // concrete array with a concrete index is instead resolved as a pure
        // reference to the selected element ([`Self::alias_index`]) — the
        // read then pins the element to whatever the other side unifies
        // with.  A computation that is neither forceable nor a resolvable
        // `Index` cannot be compared, so this fails rather than binding over
        // it.
        loop {
            let ra = disjoint::find(&mut self.nodes, ra);
            let rb = disjoint::find(&mut self.nodes, rb);
            let pending_a = self.class_has_pending_op(ra);
            let pending_b = self.class_has_pending_op(rb);
            if !pending_a && !pending_b {
                break;
            }
            let resolved_a = !pending_a || self.force_pending(ra).is_some() || {
                let other = self.nodes[rb].value;
                self.alias_index(ra, other)
            };
            let resolved_b = !pending_b || self.force_pending(rb).is_some() || {
                let other = self.nodes[ra].value;
                self.alias_index(rb, other)
            };
            if !resolved_a || !resolved_b {
                // A pending computation whose operands are still unbound is
                // compatible with an all-unbound skeleton on the other side:
                // the skeleton holds no concrete value and no computation, so
                // merging the classes erases nothing — the computation
                // resolves later and its value replicates onto the skeleton.
                // (The annotation `x : T => if …` hits this: the return type
                // is a pending computation at check time, the annotation's
                // `_` codomain is a skeleton, and they must simply join.)
                if (pending_a && self.class_is_skeleton(rb))
                    || (pending_b && self.class_is_skeleton(ra))
                {
                    self.add_equality(ra, rb);
                    return true;
                }
                self.record_error(ra, rb);
                return false;
            }
        }
        let ra = disjoint::find(&mut self.nodes, ra);
        let rb = disjoint::find(&mut self.nodes, rb);
        let va = self.nodes[ra].value;
        let vb = self.nodes[rb].value;
        match (
            va.map(|value| value.as_enum()),
            vb.map(|value| value.as_enum()),
        ) {
            (Some(Some(LowValue::Array(pa))), Some(Some(LowValue::Array(pb)))) => {
                let (left, right) = (unsafe { &*pa }, unsafe { &*pb });
                if left.len() != right.len() {
                    self.record_error(ra, rb);
                    return false;
                }
                path.push((ra, rb));
                let ok = left
                    .iter()
                    .zip(right.iter())
                    .all(|(&na, &nb)| self.unify_inner(na, nb, path));
                path.pop();
                if ok {
                    self.add_equality(ra, rb);
                }
                ok
            }
            _ if va == vb => {
                self.add_equality(ra, rb);
                true
            }
            _ => {
                self.record_error(ra, rb);
                false
            }
        }
    }

    /// Merge two classes; when the merged class holds a concrete value,
    /// replicate it to every member.  Each member's own `value` slot then
    /// stays locally correct — reads need no representative lookup, and the
    /// binding survives members being garbage-collected (the representative
    /// may die while another member is still live).
    fn bind(&mut self, ra: NodeId, rb: NodeId, va: Option<P::Value>, vb: Option<P::Value>) {
        let concrete = if is_unbound(va) { vb } else { va };
        let rep = self.add_equality(ra, rb);
        if let Some(value) = concrete
            && !is_unbound(Some(value))
        {
            let mut member = rep;
            loop {
                let next = self.nodes[member].meta().next;
                if is_unbound(self.nodes[member].value) {
                    self.nodes[member].value = Some(value);
                }
                let Some(next) = next else { break };
                member = next;
            }
        }
    }

    /// Whether `rep`'s class holds an unevaluated operation: a node with an
    /// operation whose value is still unbound.  Such nodes are pending
    /// computations, never bindable cells.
    fn class_has_pending_op(&self, rep: NodeId) -> bool {
        let mut member = rep;
        loop {
            if self.nodes[member].operation.is_some() && is_unbound(self.nodes[member].value) {
                return true;
            }
            let Some(next) = self.nodes[member].meta().next else {
                return false;
            };
            member = next;
        }
    }

    /// Whether `rep`'s class is a pure cell for binding purposes: its value
    /// is unbound and it holds no *independent* pending computation.  A read
    /// of the class's own cell ([`Self::is_self_read`]) is excluded — it is
    /// a reference to the class, resolved by replication when the class
    /// binds, not a computation that a bind would erase.
    fn class_is_pure_cell(&self, rep: NodeId) -> bool {
        let mut member = rep;
        loop {
            if self.nodes[member].operation.is_some()
                && is_unbound(self.nodes[member].value)
                && !self.is_self_read(member, rep)
            {
                return false;
            }
            let Some(next) = self.nodes[member].meta().next else {
                return true;
            };
            member = next;
        }
    }

    /// Whether `rep`'s class is an all-unbound skeleton: every member is a
    /// pure cell or an array whose elements are all skeletons — no
    /// computation, no concrete value.  A pending computation merges onto
    /// such a class without erasing anything; a class that holds any
    /// concrete value or operation is not a skeleton, and binding a
    /// computation onto it would corrupt it.
    fn class_is_skeleton(&self, rep: NodeId) -> bool {
        let mut member = rep;
        loop {
            if self.nodes[member].operation.is_some() {
                return false;
            }
            match self.nodes[member].value.and_then(|value| value.as_enum()) {
                None => {}
                Some(LowValue::Parameterized) => {}
                Some(LowValue::Array(ptr)) => {
                    let ids = unsafe { &*ptr };
                    let mut seen = HashSet::new();
                    if ids.iter().any(|&id| !self.value_is_skeleton(id, &mut seen)) {
                        return false;
                    }
                }
                _ => return false,
            }
            let Some(next) = self.nodes[member].meta().next else {
                return true;
            };
            member = next;
        }
    }

    /// Whether the subtree of array values rooted at `node` is all
    /// skeletons; `seen` cuts the cycle of a self-referential structure
    /// (which is a skeleton only if its own elements are).
    fn value_is_skeleton(&self, node: NodeId, seen: &mut HashSet<NodeId>) -> bool {
        if !seen.insert(node) {
            return true;
        }
        let ok = self.nodes[node].operation.is_none()
            && match self.nodes[node].value.and_then(|value| value.as_enum()) {
                None => true,
                Some(LowValue::Parameterized) => true,
                Some(LowValue::Array(ptr)) => {
                    let ids = unsafe { &*ptr };
                    ids.iter().all(|&id| self.value_is_skeleton(id, seen))
                }
                _ => false,
            };
        seen.remove(&node);
        ok
    }

    /// Whether `op`'s pending `Index` reads a cell of `rep`'s own class — a
    /// self-reference.  The read's target must be resolvable (a concrete
    /// operand, index, and container); a read whose target is not yet known
    /// counts as a pending computation, conservatively.
    fn is_self_read(&self, op: NodeId, rep: NodeId) -> bool {
        let Some(target) = self.index_target(op) else {
            return false;
        };
        let mut n = target;
        while let Some(parent) = self.nodes[n].equality.parent {
            n = parent;
        }
        n == rep
    }

    /// The element an `Index` operation reads, when the operand array, the
    /// index, and the container are all concrete.
    fn index_target(&self, op: NodeId) -> Option<NodeId> {
        let Operation { operator, operand } = self.nodes[op].operation?;
        if !matches!(operator, Operator::Index) {
            return None;
        }
        let operand = operand?;
        let operands = self.nodes[operand].value?;
        let Some(LowValue::Array(operands_ptr)) = operands.as_enum() else {
            return None;
        };
        let operands = unsafe { &*operands_ptr };
        if operands.len() != 2 {
            return None;
        }
        let index_value = self.nodes[operands[1]].value?;
        let Some(LowValue::USize(index)) = index_value.as_enum() else {
            return None;
        };
        let container_value = self.nodes[operands[0]].value?;
        let Some(LowValue::Array(container_ptr)) = container_value.as_enum() else {
            return None;
        };
        let container = unsafe { &*container_ptr };
        container.get(index).copied()
    }

    /// The first pending operation node in `rep`'s class, if any.
    fn pending_op(&self, rep: NodeId) -> Option<NodeId> {
        let mut member = rep;
        loop {
            if self.nodes[member].operation.is_some() && is_unbound(self.nodes[member].value) {
                return Some(member);
            }
            member = self.nodes[member].meta().next?;
        }
    }

    /// Resolve an unforceable `Index` as a pure reference.  An `Index` over a
    /// concrete array with a concrete index is just a read of that element —
    /// `operand[0][index]` — so the operator node is aliased to the element
    /// (the classes merge).  When the unify's other side holds a concrete
    /// `value`, it is written onto the read immediately — pinning the element
    /// (the "monomorphized" trade for dependent reads) — and the read keeps
    /// its operation, so its operand edge stays live for the apply's clone
    /// machinery to reach the parameter and enforce the pin.  With no value
    /// to pin, the read is a plain alias and the computation is dropped.
    /// Returns `false` when the pending computation is not such an `Index` —
    /// e.g. the index is itself a parameter, so the read genuinely cannot
    /// resolve until it is bound; the caller reports the unify failure.
    fn alias_index(&mut self, rep: NodeId, value: Option<P::Value>) -> bool {
        let Some(op) = self.pending_op(rep) else {
            return false;
        };
        let Some(indexed) = self.index_target(op) else {
            return false;
        };
        // Only alias onto a pure cell — the read must be a plain reference,
        // not itself a computation or a concrete value.  The reader's own
        // operation is a self-read of the target's class once the
        // evaluation-time alias joined them, so it does not make the target
        // a pending computation.
        let target = disjoint::find(&mut self.nodes, indexed);
        if !self.class_is_pure_cell(target) || !is_unbound(self.nodes[target].value) {
            return false;
        }
        if let Some(value) = value.filter(|v| !is_unbound(Some(*v))) {
            // Pin the read: merge with the element and replicate the value
            // over the class.  The read keeps its operation — the operand
            // edge must survive for the apply's clone to reach the parameter
            // and enforce the pin.
            self.bind(op, indexed, Some(value), None);
        } else {
            // A plain alias: the read *is* the element, no computation
            // remains.  The node must stay well-formed — every node is
            // either value-carrying or operation-carrying — so an aliased
            // read with no cached value takes the marker, reading as the
            // pure cell it now is.
            self.add_equality(op, indexed);
            self.nodes[op].operation = None;
            if self.nodes[op].value.is_none() {
                self.nodes[op].value = Some(P::Value::from(LowValue::Parameterized));
            }
        }
        true
    }

    /// Join `reader` into `target`'s class when the target is a pure cell —
    /// the evaluation-side counterpart of [`Self::alias_index`].  A read of
    /// an inference variable is a reference, so the reader unifies with the
    /// cell through the *standard* unify: both unbound → the classes merge,
    /// and a reader whose class already carries a value (an annotation over
    /// the read) replicates it onto the cell — a later conflicting bind then
    /// fails against it, exactly as if the read had been evaluated after the
    /// bind.  The guard is the precondition for the unify's bind path: a
    /// concrete or pending target would force this (mid-evaluation) reader
    /// and re-enter it.  The reader keeps its operation — the operand edge
    /// must stay live for the apply's clone machinery, and for the unify pin
    /// path to find the read.
    pub(crate) fn alias_read(&mut self, reader: NodeId, target: NodeId) -> bool {
        let rep = disjoint::find(&mut self.nodes, target);
        if self.class_has_pending_op(rep) || !is_unbound(self.nodes[rep].value) {
            return false;
        }
        self.unify(reader, target);
        true
    }

    /// Force the first unevaluated operation in `rep`'s class.  When the
    /// computation resolves, its value is replicated to the class's pure
    /// cells so reads stay locally correct — operation-bearing members keep
    /// their own computed value — and the value is returned.  `None` when
    /// the computation stays lazy, because its operands are still unbound.
    #[stacksafe]
    fn force_pending(&mut self, rep: NodeId) -> Option<P::Value> {
        let mut member = rep;
        loop {
            if self.nodes[member].operation.is_some() && is_unbound(self.nodes[member].value) {
                break;
            }
            member = self.nodes[member].meta().next?;
        }
        let block = self.nodes[member].block;
        let value = self.evaluate_node(member, Some(block));
        if is_unbound(Some(value)) {
            return None;
        }
        let mut m = rep;
        loop {
            if self.nodes[m].operation.is_none() && is_unbound(self.nodes[m].value) {
                self.nodes[m].value = Some(value);
            }
            let Some(next) = self.nodes[m].meta().next else {
                break;
            };
            m = next;
        }
        Some(value)
    }

    fn record_error(&mut self, ra: NodeId, rb: NodeId) {
        self.unify_errors.push(UnifyError {
            a: ra,
            b: rb,
            value_a: self.nodes[ra].value,
            value_b: self.nodes[rb].value,
        });
    }
}
