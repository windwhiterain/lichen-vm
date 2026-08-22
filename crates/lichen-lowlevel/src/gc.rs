use stacksafe::stacksafe;

use crate::{
    lowlevel::{BlockId, Module, NodeId, Program, Value},
    utils::disjoint::{self, Node as _},
};

impl<P: Program> Module<P> {
    /// move the `block_root`'s reachable subtree into its `block.parent`.
    #[stacksafe]
    pub fn garbage_collect(&mut self, block_root: NodeId) -> Option<Value<P>> {
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
    ) -> Option<Value<P>> {
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
        let value = current.map(|value| match value {
            Value::Array(array) => {
                let nodes = unsafe { &*array };
                for &node in nodes {
                    self.garbage_collect_node(node, source, target);
                }
                Value::Array(self.copy_nodes(nodes, target))
            }
            Value::Function(function) => {
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
                Value::Function(function)
            }
            Value::Ext(ext) => Self::copy_ext(self, ext, target),
            value => value,
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
            self.nodes[member].meta_mut().parent = (member != representative).then_some(representative);
            prev = Some(member);
            tail = Some(member);
            size += 1;
        }
        let Some(representative) = new_rep else { return };
        let last = prev.expect("a surviving member was elected representative");
        self.nodes[last].meta_mut().next = None;
        let meta = self.nodes[representative].meta_mut();
        meta.tail = tail;
        meta.size = size;
    }

    #[stacksafe]
    fn drop_block(&mut self, block: BlockId) {
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
        self.blocks.remove(block);
    }
}
