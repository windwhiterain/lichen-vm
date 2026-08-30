use stacksafe::stacksafe;

use crate::{BlockId, LowValue, Module, NodeId, Program};
use lichen_utils::disjoint::{self, Node as _};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// move the `block_root`'s reachable subtree into its `block.parent`.
    #[stacksafe]
    pub fn garbage_collect(&mut self, block_root: NodeId) -> Option<P::Value> {
        let source = self.nodes[block_root].block;
        let Some(target) = self.blocks[source].parent else {
            return self.nodes[block_root].value; // root block: nothing to move or release
        };
        let value = self.garbage_collect_node(block_root, source, target);
        self.drop_block(source);
        value
    }

    #[stacksafe]
    fn garbage_collect_node(
        &mut self,
        node: NodeId,
        source: BlockId,
        target: BlockId,
    ) -> Option<P::Value> {
        if !self.descends_from(self.nodes[node].block, source) {
            return self.nodes[node].value; // lives outside the vacated subtree — stays put
        }
        self.nodes[node].block = target;
        self.blocks[target].nodes.push(node);
        let current = self.nodes[node].value;
        let operation = self.nodes[node].operation;
        // An unevaluated operation still depends on its operand: the
        // subtree behind it may be referenced when the node is evaluated
        // later, so enter it through the operand edge — the same
        // unevaluated-op rule as the apply clone set.  A cached value
        // means the node is memoized and its operand is dead; it is not
        // followed, so evaluated intermediate chains still compact away.
        if current.is_none()
            && let Some(operand) = operation.and_then(|operation| operation.operand)
        {
            self.garbage_collect_node(operand, source, target);
        }
        // An assert point's condition is a graph edge like an unevaluated
        // operation's operand: the assert may be checked after the subtree
        // vacates, so the condition must move with the point.
        if let Some(condition) = self.nodes[node].assert {
            self.garbage_collect_node(condition, source, target);
        }
        let value = current.map(|value| match value.as_enum() {
            Some(LowValue::Array(array)) => {
                for item in array.items() {
                    self.garbage_collect_node(item.node, source, target);
                }
                // The whole payload — element nodes and their shallow flags
                // together — moves into the target arena, so a compacted
                // array keeps its markers.
                P::Value::from(LowValue::Array(self.alloc_array(array.items(), target)))
            }
            Some(LowValue::Function(function)) => {
                // The template's nodes must outlive the closing block, so
                // the scope is mapped like an array slice: each member
                // homed in the vacated subtree moves into the target.  The
                // function itself is homed like a node — if it lives in
                // the vacated subtree it is re-pointed to the target and
                // registered there, so release skips it and it stays
                // callable.
                let ids = self.functions[function].nodes.clone();
                for &id in &ids {
                    self.garbage_collect_node(id, source, target);
                }
                if self.descends_from(self.functions[function].block, source) {
                    self.functions[function].block = target;
                    self.blocks[target].functions.push(function);
                }
                P::Value::from(LowValue::Function(function))
            }
            // A program-specific value may carry a handle into an arena —
            // relocate it into the target block like any other payload.
            None => Self::copy_ext(self, value, target),
            _ => value,
        });
        self.nodes[node].value = value;
        value
    }

    /// Splice every member homed in `dropped` out of the class rooted at
    /// `rep`, re-elect a representative among the survivors, and re-point
    /// every survivor's parent at it, flattening the tree so no live
    /// member's chain passes through a removed node.  The class value needs
    /// no migration: [`Module::bind`] already replicated it to every member.
    /// A class whose members all die with the block is left alone.
    ///
    /// Must run before the block's nodes are removed — the walk reads the
    /// `next` pointers of the members being removed.
    fn flatten_class(&mut self, rep: NodeId, dropped: BlockId) {
        let mut current = Some(rep);
        let mut new_rep: Option<NodeId> = None;
        let mut prev: Option<NodeId> = None;
        let mut tail: Option<NodeId> = None;
        let mut size = 0u32;
        while let Some(member) = current {
            current = self.nodes[member].meta().next;
            if self.nodes[member].block == dropped {
                continue; // removed below; keep walking past it
            }
            let representative = match new_rep {
                Some(representative) => representative,
                None => {
                    new_rep = Some(member);
                    member
                }
            };
            if let Some(prev) = prev {
                self.nodes[prev].meta_mut().next = Some(member);
            }
            self.nodes[member].meta_mut().parent =
                (member != representative).then_some(representative);
            prev = Some(member);
            tail = Some(member);
            size += 1;
        }
        let Some(representative) = new_rep else {
            return;
        };
        let last = prev.expect("a surviving member was elected representative");
        self.nodes[last].meta_mut().next = None;
        let meta = self.nodes[representative].meta_mut();
        meta.tail = tail;
        meta.size = size;
    }

    /// Drops `block` and everything homed in it (children, functions,
    /// nodes, arena) without moving anything. The caller guarantees no
    /// live node outside the block still references one inside it —
    /// evaluating a surviving reference to a released block panics.
    ///
    /// Public so a host can reap per-run frame blocks (a kernel bridge
    /// spawns one block per call and drops it once the result is extracted;
    /// [`Self::garbage_collect`] would hoist the subtree into the parent
    /// instead, growing it forever).
    #[stacksafe]
    pub fn drop_block(&mut self, block: BlockId) {
        let children = std::mem::take(&mut self.blocks[block].children);
        for child in children {
            if self.blocks.contains_key(child) {
                self.drop_block(child);
            }
        }
        let functions = std::mem::take(&mut self.blocks[block].functions);
        for function in functions {
            // A function homed in this block is dropped with it: removing it
            // releases the function's scope.  Functions re-pointed to the
            // parent by compaction stay, still callable — the stale id in
            // this list is skipped, like a moved node's.
            if self
                .functions
                .get(function)
                .is_some_and(|function| function.block == block)
            {
                self.functions.remove(function);
            }
        }
        let nodes = std::mem::take(&mut self.blocks[block].nodes);
        // Splice classes touched by this block out of their member lists
        // and re-elect representatives among the survivors, so surviving
        // members keep working parent chains and member lists after the
        // removal below.  Runs before any node is removed, while the
        // dropped nodes' `next` pointers are still readable.
        let mut touched = std::collections::HashSet::new();
        for &node in &nodes {
            if self.nodes.get(node).is_some_and(|node| node.block == block) {
                touched.insert(disjoint::find(&mut self.nodes, node));
            }
        }
        for rep in touched {
            self.flatten_class(rep, block);
        }
        for node in nodes {
            if self.nodes.get(node).is_some_and(|node| node.block == block) {
                self.nodes.remove(node);
            }
        }
        // Drop the assert points that died with this block: the check pass
        // walks the registry by id, so a dangling entry would panic there.
        self.asserts.retain(|&id| self.nodes.contains_key(id));
        self.blocks.remove(block);
    }
}
