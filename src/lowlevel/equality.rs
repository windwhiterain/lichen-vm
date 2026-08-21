use stacksafe::stacksafe;

use crate::{
    lowlevel::{Module, NodeId, Program, UnifyError, Value},
    utils::disjoint::{self, Node as _},
};

impl<P: Program> Module<P> {
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        disjoint::union(&mut self.nodes, a, b)
    }

    pub fn equality_representative(&mut self, node: NodeId) -> NodeId {
        disjoint::find(&mut self.nodes, node)
    }

    /// Structurally unify the classes of `a` and `b`.
    ///
    /// Unification is over values: an unbound class (`value: None` or
    /// [`Value::Parameterized`]) binds to the other side's value; two
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
        if is_unbound(va) || is_unbound(vb) {
            self.bind(ra, rb, va, vb);
            return true;
        }
        match (va, vb) {
            (Some(Value::Array(pa)), Some(Value::Array(pb))) => {
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
    fn bind(&mut self, ra: NodeId, rb: NodeId, va: Option<Value<P>>, vb: Option<Value<P>>) {
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

    fn record_error(&mut self, ra: NodeId, rb: NodeId) {
        self.unify_errors.push(UnifyError {
            a: ra,
            b: rb,
            value_a: self.nodes[ra].value,
            value_b: self.nodes[rb].value,
        });
    }
}

/// A class is unbound while it carries no value or only the lazy marker.
fn is_unbound<P: Program>(value: Option<Value<P>>) -> bool {
    matches!(value, None | Some(Value::Parameterized))
}
