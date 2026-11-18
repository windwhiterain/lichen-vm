//! The polymorphic flag's propagation: a boolean fixpoint over the recursion set (SPEC §Polymorphic).
//!
//! # Invariant
//! Reads structure and flags only, never evaluates a node, so it cannot reintroduce D18's
//! deep-evaluation hang (`docs/notes/code-audit.md` D18, `docs/notes/polymorphic-flag.md` §3).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{AnyFunctionId, FunctionId, LowValue, Module, NodeId, Program, Scope};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// The maximal mark scopes of every node reachable from `roots` over the recursion set, read-only.
    ///
    /// # Invariant
    /// The set is operation operands, value references (array items, table entries, a nested
    /// closure's template) and class members. A node's verdict is the antichain of maximal mark
    /// scopes it reaches, empty when none. Never evaluates a member (D18). A worklist bounded
    /// by `evaluate_depth_limit` terminates on a cyclic or lazy graph.
    pub(crate) fn compute_polymorphic(
        &self,
        roots: &[NodeId],
    ) -> HashMap<NodeId, Vec<Arc<Scope<FunctionId>>>> {
        let (seen, referrers) = self.polymorphic_referrers(roots);

        // Seeds: written marks plus every function's parameter; each carries its mark scope.
        let parameters: HashSet<NodeId> = self
            .functions
            .values()
            .map(|function| function.parameter)
            .collect();
        let mut chains: HashMap<NodeId, Vec<Arc<Scope<FunctionId>>>> =
            seen.iter().map(|&node| (node, Vec::new())).collect();
        let mut work: Vec<NodeId> = Vec::new();
        for &node in &seen {
            let seed = self.nodes[node].polymorphic == Some(true) || parameters.contains(&node);
            if seed && let Some(scope) = self.owner_scope(node) {
                antichain_insert(chains.get_mut(&node).expect("a seen node"), scope);
                work.push(node);
            }
            // A nested function value is a per-call closure over its defining (parent) function.
            if let Some(LowValue::Function(AnyFunctionId::Dynamic(function))) =
                self.nodes[node].value.and_then(|value| value.as_enum())
                && let Some(parent) = self.functions.get(function).and_then(|f| f.parent)
                && let Some(scope) = self.function_scope(parent)
            {
                antichain_insert(chains.get_mut(&node).expect("a seen node"), scope);
                work.push(node);
            }
        }
        // A node's antichain unions the marks it reaches, flowing to referrers; the set only grows.
        while let Some(node) = work.pop() {
            let incoming = chains[&node].clone();
            if incoming.is_empty() {
                continue;
            }
            if let Some(referrers) = referrers.get(&node) {
                for &referrer in referrers {
                    let Some(chain) = chains.get_mut(&referrer) else {
                        continue;
                    };
                    let mut changed = false;
                    for scope in &incoming {
                        changed |= antichain_insert(chain, scope.clone());
                    }
                    if changed {
                        work.push(referrer);
                    }
                }
            }
        }
        chains
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
        let chains = self.compute_polymorphic(&roots);
        for (node, chain) in chains {
            let poly = !chain.is_empty();
            if self.nodes[node].polymorphic != Some(true) {
                self.write_polymorphic(node, Some(poly));
            }
            self.nodes[node].polymorphic_scope = chain;
        }
        self.polymorphic_dirty = false;
    }
    /// The recursion set's reverse edges over `roots`: `referrers[v]` = every `u` recursing into `v` (D18).
    fn polymorphic_referrers(
        &self,
        roots: &[NodeId],
    ) -> (HashSet<NodeId>, HashMap<NodeId, Vec<NodeId>>) {
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut referrers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut stack: Vec<(NodeId, usize)> = roots.iter().map(|&root| (root, 0)).collect();
        let limit = self.evaluate_depth_limit;
        while let Some((node, depth)) = stack.pop() {
            if !self.nodes.contains_key(node) || !seen.insert(node) {
                continue;
            }
            if depth >= limit {
                continue;
            }
            let mut children: Vec<NodeId> = Vec::new();
            let cell = &self.nodes[node];
            if let Some(operation) = cell.operation {
                if let Some(operand) = operation.operand {
                    // A frozen operand's flag is persisted; propagation walks dynamic nodes only.
                    if let Some(node) = operand.dynamic() {
                        children.push(node);
                    }
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
        (seen, referrers)
    }

    /// Incremental class upgrade: spread the merged class's mark scopes along reverse edges (perf; same verdict).
    pub(crate) fn upgrade_polymorphic_class(
        &mut self,
        representative: NodeId,
        seed_scopes: Vec<Arc<Scope<FunctionId>>>,
    ) {
        let roots: Vec<NodeId> = self.nodes.keys().collect();
        let (_seen, referrers) = self.polymorphic_referrers(&roots);
        let mut work: Vec<NodeId> = Vec::new();
        let mut touched: HashSet<NodeId> = HashSet::new();
        // The merged class's members gain the polymorphic side's mark scopes.
        let mut member = Some(representative);
        while let Some(current) = member {
            member = self.nodes[current].equality.next();
            let mut changed = false;
            for scope in &seed_scopes {
                changed |=
                    antichain_insert(&mut self.nodes[current].polymorphic_scope, scope.clone());
            }
            if changed {
                work.push(current);
                touched.insert(current);
            }
        }
        // Pending creation marks / parameters / nested closures still seed (cheap when already present).
        let parameters: HashSet<NodeId> = self.functions.values().map(|f| f.parameter).collect();
        for &node in &roots {
            let seed = self.nodes[node].polymorphic == Some(true) || parameters.contains(&node);
            if seed
                && let Some(scope) = self.owner_scope(node)
                && antichain_insert(&mut self.nodes[node].polymorphic_scope, scope)
            {
                work.push(node);
                touched.insert(node);
            }
            if let Some(LowValue::Function(AnyFunctionId::Dynamic(function))) =
                self.nodes[node].value.and_then(|value| value.as_enum())
                && let Some(parent) = self.functions.get(function).and_then(|f| f.parent)
                && let Some(scope) = self.function_scope(parent)
                && antichain_insert(&mut self.nodes[node].polymorphic_scope, scope)
            {
                work.push(node);
                touched.insert(node);
            }
        }
        // Fixpoint: a node's mark scopes flow to its referrers; only changed nodes re-enter.
        while let Some(node) = work.pop() {
            let incoming = self.nodes[node].polymorphic_scope.clone();
            if incoming.is_empty() {
                continue;
            }
            if let Some(refs) = referrers.get(&node) {
                for &referrer in refs {
                    let mut changed = false;
                    for scope in &incoming {
                        changed |= antichain_insert(
                            &mut self.nodes[referrer].polymorphic_scope,
                            scope.clone(),
                        );
                    }
                    if changed {
                        work.push(referrer);
                        touched.insert(referrer);
                    }
                }
            }
        }
        // Refresh the bool erasure for the touched nodes only.
        for node in touched {
            let poly = !self.nodes[node].polymorphic_scope.is_empty();
            if self.nodes[node].polymorphic != Some(true) {
                self.write_polymorphic(node, Some(poly));
            }
        }
        self.polymorphic_dirty = false;
    }
}

/// Insert into a maximal-scope antichain: a present ancestor subsumes the newcomer; else it subsumes descendants.
fn antichain_insert(
    chain: &mut Vec<Arc<Scope<FunctionId>>>,
    scope: Arc<Scope<FunctionId>>,
) -> bool {
    if chain.iter().any(|a| a.is_ancestor_or_self(&scope)) {
        return false;
    }
    chain.retain(|d| !scope.is_ancestor_or_self(d));
    chain.push(scope);
    true
}
