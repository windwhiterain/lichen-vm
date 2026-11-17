//! The polymorphic flag's propagation: a boolean fixpoint over the recursion set (SPEC §Polymorphic).
//!
//! # Invariant
//! Reads structure and flags only, never evaluates a node, so it cannot reintroduce D18's
//! deep-evaluation hang (`docs/notes/code-audit.md` D18, `docs/notes/polymorphic-flag.md` §3).

use std::collections::{HashMap, HashSet};

use crate::{AnyFunctionId, LowValue, Module, NodeId, Program};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// The polymorphic flag of every node reachable from `roots` over the recursion set, read-only.
    ///
    /// # Invariant
    /// The set is operation operands, value references (array items — shallow included —
    /// table entries, a closure's whole template) and class members. A node is polymorphic
    /// when created marked or when any node it recurses into is; otherwise `false`. Boolean
    /// only: never evaluates a member (D18). An explicit worklist bounded by
    /// `evaluate_depth_limit` terminates on a cyclic or lazy graph.
    pub(crate) fn compute_polymorphic(&self, roots: &[NodeId]) -> HashMap<NodeId, bool> {
        // Reverse edges: `referrers[v]` holds every `u` recursing into `v`; a true flag flows back to them.
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut referrers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut stack: Vec<(NodeId, usize)> = roots.iter().map(|&root| (root, 0)).collect();
        let limit = self.evaluate_depth_limit;
        while let Some((node, depth)) = stack.pop() {
            if !self.nodes.contains_key(node) || !seen.insert(node) {
                continue;
            }
            if depth >= limit {
                // Reached, but the budget bounds the expansion: its subtree is not walked.
                continue;
            }
            let mut children: Vec<NodeId> = Vec::new();
            let cell = &self.nodes[node];
            if let Some(operation) = cell.operation {
                if let Some(operand) = operation.operand {
                    children.push(operand);
                }
            }
            if let Some(value) = cell.value {
                match value.as_enum() {
                    Some(LowValue::Array(array)) => {
                        // SAFETY: `array` is a live node's payload; this walk releases no block.
                        for item in unsafe { array.items() } {
                            if let Some(item) = item.node.dynamic() {
                                children.push(item);
                            }
                        }
                    }
                    Some(LowValue::Table(table)) => {
                        // SAFETY: `table` is a live node's payload; this walk releases no block.
                        for item in unsafe { table.items() } {
                            if let Some(key) = item.key.dynamic() {
                                children.push(key);
                            }
                            if let Some(value) = item.value.dynamic() {
                                children.push(value);
                            }
                        }
                    }
                    // A nested function value is a per-call closure; a top-level one is an identity (§9).
                    Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
                        if let Some(function) = self.functions.get(function)
                            && function.parent.is_some()
                        {
                            children.push(function.parameter);
                            children.push(function.r#return);
                            children.push(function.return_type);
                            children.extend(function.asserts.iter().copied());
                            children.extend(function.nodes.iter().copied());
                        }
                    }
                    _ => {}
                }
            }
            // A class is one unit: route members through the representative, expanded once.
            let representative = self.class_root(node);
            children.push(representative);
            if representative == node {
                let mut member = self.nodes[representative].equality.next();
                while let Some(current) = member {
                    children.push(current);
                    member = self.nodes[current].equality.next();
                }
            }
            for child in children {
                referrers.entry(child).or_default().push(node);
                stack.push((child, depth + 1));
            }
        }
        // Seeds: written flags plus every function's parameter (ruling 3a: every binder is primed).
        let parameters: HashSet<NodeId> = self
            .functions
            .values()
            .map(|function| function.parameter)
            .collect();
        let mut flags: HashMap<NodeId, bool> = seen
            .iter()
            .map(|&node| {
                (
                    node,
                    self.nodes[node].polymorphic == Some(true) || parameters.contains(&node),
                )
            })
            .collect();
        let mut work: Vec<NodeId> = flags
            .iter()
            .filter(|(_, poly)| **poly)
            .map(|(node, _)| *node)
            .collect();
        // Monotone upgrade: a node upgrades at most once, so the worklist terminates.
        while let Some(node) = work.pop() {
            if let Some(referrers) = referrers.get(&node) {
                for &referrer in referrers {
                    if !flags[&referrer] {
                        flags.insert(referrer, true);
                        work.push(referrer);
                    }
                }
            }
        }
        flags
    }

    /// Write the propagated flag over the whole module, upgrading only; skips in O(1) when none is owed.
    ///
    /// # Invariant
    /// The flag is a global fixpoint from the creation marks, so every node carries a
    /// verdict. Runs at a clone walk's entry before the walk reads it; a clone inherits a
    /// `Some` flag at birth, so an expansion adds no debt (`docs/notes/polymorphic-flag.md`,
    /// ruling 1).
    pub(crate) fn propagate_polymorphic(&mut self) {
        if self.polymorphic_unflagged == 0 && !self.polymorphic_dirty {
            return;
        }
        let roots: Vec<NodeId> = self.nodes.keys().collect();
        let flags = self.compute_polymorphic(&roots);
        for (node, poly) in flags {
            if self.nodes[node].polymorphic != Some(true) {
                self.write_polymorphic(node, Some(poly));
            }
        }
        self.polymorphic_dirty = false;
    }
}
