use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId::Dynamic as Dyn, BlockId, LowValue, Module, NodeId,
    Program, ValueExt,
};
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
        // An unevaluated operation still depends on its operand, which may
        // be referenced later; a cached value means it is dead.
        if current.is_none()
            && let Some(operand) = operation.and_then(|operation| operation.operand)
        {
            self.garbage_collect_node(operand, source, target);
        }
        let value = current.map(|value| match value.as_enum() {
            Some(LowValue::Array(AnyHandle::Static(_))) => {
                // A static payload lives in the frozen module's arena: no
                // block to vacate, its item refs all static.
                value
            }
            Some(LowValue::Array(array)) => {
                // SAFETY: `array` is a live node's payload, and `source` is
                // released only after this walk returns.
                for item in unsafe { array.items() } {
                    // A static item lives in the static module — nothing to
                    // move (its value stays, referenced in place).
                    if let Dyn(node) = item.node {
                        self.garbage_collect_node(node, source, target);
                    }
                }
                // The whole payload moves into the target arena, so a
                // compacted array keeps its markers.
                P::Value::from(LowValue::Array(
                    self.alloc_array(unsafe { array.items() }, target),
                ))
            }
            Some(LowValue::Table(AnyHandle::Static(_))) => {
                // A static payload lives in the frozen module's arena: no
                // block to vacate, its entry refs all static.
                value
            }
            Some(LowValue::Table(table)) => {
                // SAFETY: `table` is a live node's payload, and `source` is
                // released only after this walk returns.
                for item in unsafe { table.items() } {
                    // A static entry lives in the static module — nothing
                    // to move (its value stays, referenced in place).
                    if let Dyn(node) = item.key {
                        self.garbage_collect_node(node, source, target);
                    }
                    if let Dyn(node) = item.value {
                        self.garbage_collect_node(node, source, target);
                    }
                }
                // The whole payload moves into the target arena, so a
                // compacted table keeps its order and hashes.
                P::Value::from(LowValue::Table(
                    self.alloc_table(unsafe { table.items() }, target),
                ))
            }
            // A static function value is frozen in the static module — no
            // scope to walk, no home block to re-point.
            Some(LowValue::Function(AnyFunctionId::Static(_))) => value,
            Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
                // The template's nodes and asserts must outlive the closing
                // block, so each is mapped like an array element.
                let ids = self.functions[function].nodes.clone();
                for &id in &ids {
                    self.garbage_collect_node(id, source, target);
                }
                let assert_ids = self.functions[function].asserts.clone();
                for &id in &assert_ids {
                    self.garbage_collect_node(id, source, target);
                }
                if self.descends_from(self.functions[function].block, source) {
                    self.functions[function].block = target;
                    self.blocks[target].functions.push(function);
                }
                P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(function)))
            }
            // A program-specific value may carry nodes the lowlevel cannot
            // see: the traced ones must move or die with the block.
            None => {
                let mut traced = Vec::new();
                value.traced(self, &mut traced);
                for node in traced {
                    // The walked value is discarded, like the array and
                    // table arms': only the node's block changes.
                    self.garbage_collect_node(node, source, target);
                }
                Self::copy_ext(self, value, target)
            }
            _ => value,
        });
        self.write_node_value(node, value);
        value
    }

    /// Splice every member homed in `dropped` out of the class rooted at
    /// `rep` and re-elect a representative.
    ///
    /// # Invariant
    ///
    /// No live member's chain passes through a removed node. The class value
    /// needs no migration: [`Module::bind`] already replicated it to every
    /// member, and a class whose members all die with the block is left
    /// alone. This runs before the block's nodes are removed, because the
    /// walk reads the `next` pointers of the members being removed.
    fn flatten_class(&mut self, rep: NodeId, dropped: BlockId) {
        let mut survivors = Vec::new();
        let mut current = Some(rep);
        while let Some(member) = current {
            current = self.nodes[member].meta().next();
            if self.nodes[member].block == dropped {
                continue; // removed below; keep walking past it
            }
            survivors.push(member);
        }
        disjoint::rebuild(&mut self.nodes, &survivors);
        // `rebuild` re-elected a representative, so the class's value slot
        // is its own; re-distribute the value from it.
        let Some((&representative, _)) = survivors.split_first() else {
            return;
        };
        if let Some(value) = survivors
            .iter()
            .find_map(|&member| self.nodes.get(member).and_then(|node| node.value))
        {
            self.propagate_class_value(representative, value);
        }
    }

    /// Drops `block` and everything homed in it — children, functions, nodes,
    /// arena — without moving anything.
    ///
    /// # Invariant
    ///
    /// No live node outside the block still references one inside it:
    /// evaluating a surviving reference to a released block panics.
    ///
    /// Public so a host can reap per-run frame blocks, which
    /// [`Self::garbage_collect`] would hoist into the parent and grow forever.
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
            // A function homed here is dropped with it: removing it
            // releases the scope. One re-pointed to the parent stays.
            if self
                .functions
                .get(function)
                .is_some_and(|function| function.block == block)
            {
                self.functions.remove(function);
            }
        }
        let nodes = std::mem::take(&mut self.blocks[block].nodes);
        // Splice this block's classes before removing any node, while
        // their `next` pointers are still readable.
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
        // Drop the assert points that died with this block: the check
        // pass walks the registry by id, so a dangling entry panics.
        self.asserts
            .retain(|entry| self.nodes.contains_key(entry.condition));
        self.blocks.remove(block);
    }
}
