use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, BlockId, EvalError, LowValue,
    Module, NodeId, Program, ValueExt,
};
use lichen_utils::disjoint::{self, Node as _};
use lichen_utils::extend::AsEnum;

impl<P: Program> Module<P> {
    /// Whether `node` still names a live node; a static ref is in the frozen
    /// module's arena, which no block owns.
    fn ref_is_live(&self, node: AnyNodeId) -> bool {
        match node {
            Dyn(node) => self.nodes.contains_key(node),
            AnyNodeId::Static(_) => true,
        }
    }

    /// Move the `block_root`'s reachable subtree into its `block.parent`.
    ///
    /// # Invariant
    ///
    /// Nothing reachable from `block_root` may name a node this releases, so a
    /// surviving node's every edge into `source` has to be one the walk follows:
    /// the equality classes it aliases into, its value's items, its traced nodes,
    /// or the operand of an operation it still owes an answer for. A pending
    /// assert's condition is a second root: the pass drains later. See
    /// [`Self::drop_block`].
    #[stacksafe]
    pub fn garbage_collect(&mut self, block_root: NodeId) -> Option<P::Value> {
        let source = self.nodes[block_root].block;
        let Some(target) = self.blocks[source].parent else {
            return self.nodes[block_root].value; // root block: nothing to move or release
        };
        self.gc_classes.clear();
        let value = self.garbage_collect_node(block_root, source, target);
        // A pending assert's condition is a root: the assert pass drains later.
        let pending: Vec<NodeId> = self.asserts.iter().map(|entry| entry.condition).collect();
        for condition in pending {
            if self
                .nodes
                .get(condition)
                .is_some_and(|node| self.descends_from(node.block, source))
            {
                self.garbage_collect_node(condition, source, target);
            }
        }
        self.drop_block(source);
        value
    }

    /// The equality-class members of `node` that this walk has not entered yet.
    ///
    /// # Invariant
    /// A class is entered once per collect, so the walk visits each member once;
    /// the representative is inserted before the members are read, so a class
    /// cycle terminates. The list is headed by the representative, so the walk
    /// starts there: entered at a member behind the head, it would reach only
    /// the members behind *it* and release the ones ahead.
    fn class_walk(&mut self, node: NodeId) -> Vec<NodeId> {
        let representative = disjoint::find(&mut self.nodes, node);
        if !self.gc_classes.insert(representative) {
            return Vec::new();
        }
        let mut members = Vec::new();
        let mut current = Some(representative);
        while let Some(member) = current {
            current = self.nodes[member].meta().next();
            if member != node {
                members.push(member);
            }
        }
        members
    }

    #[stacksafe]
    fn garbage_collect_node(
        &mut self,
        node: NodeId,
        source: BlockId,
        target: BlockId,
    ) -> Option<P::Value> {
        // A reader's answer lives in the class it read into, so every
        // member the node aliases is as live as the node is.
        let members = self.class_walk(node);
        if !self.descends_from(self.nodes[node].block, source) {
            for member in members {
                self.garbage_collect_node(member, source, target);
            }
            return self.nodes[node].value; // lives outside the vacated subtree — stays put
        }
        self.nodes[node].block = target;
        self.blocks[target].nodes.push(node);
        for member in members {
            self.garbage_collect_node(member, source, target);
        }
        let current = self.nodes[node].value;
        let operation = self.nodes[node].operation;
        // The operator still owes its answer when the run stamp says
        // so, and it reads the operand for one.
        if self.has_no_result_yet(node)
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
        // The slot is rewritten by a value it already holds: the one licensed rewrite.
        if let Some(value) = value {
            self.relocate_node_value(node, value);
        }
        value
    }

    /// Splice every member homed in `dropped` out of the class rooted at
    /// `rep` and re-elect a representative.
    ///
    /// # Invariant
    ///
    /// No live member's chain passes through a removed node. The value needs
    /// no migration: [`Module::propagate_class_value`] replicated it. The low
    /// type is class-routed through the representative alone, so the released
    /// one's is joined into the re-elected one. This runs before the removal
    /// reads the members' `next` pointers.
    fn flatten_class(&mut self, rep: NodeId, dropped: BlockId) {
        // Class-routed and stored nowhere else: `docs/notes/lowlevel-low-types.md`.
        let low_shape = self.nodes[rep].low_shape.clone();
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
            self.fill_class_holes(representative, value);
        }
        if let Some(shape) = low_shape {
            self.refine_class_low_type(representative, shape);
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
        // A failure inside a released block goes with it, as the
        // assert worklist above already does.
        self.prune_released_failures();
        // A mark anchor is **not** retained: its payloads are text, and a
        // key is never reused, so a stale entry names nothing.
        self.blocks.remove(block);
    }

    /// Drop every recorded failure whose context no longer names a live node.
    fn prune_released_failures(&mut self) {
        // Each `retain` borrows one field mutably and the
        // liveness test reads the module, so they are taken out first.
        let mut failures = (
            std::mem::take(&mut self.unify_errors),
            std::mem::take(&mut self.eval_errors),
            std::mem::take(&mut self.assert_errors),
            std::mem::take(&mut self.apply_errors),
        );
        let live = |node: NodeId| self.nodes.contains_key(node);
        failures.0.retain(|error| {
            live(error.root_a)
                && live(error.root_b)
                && live(error.a)
                && live(error.b)
                && error.steps.iter().all(|step| live(step.a) && live(step.b))
        });
        failures.1.retain(|error| match error {
            EvalError::Index { index, .. } => self.ref_is_live(*index),
            EvalError::TableMiss { table, key } => {
                self.ref_is_live(*table) && self.ref_is_live(*key)
            }
            EvalError::TableKeyUndecided { key } => self.ref_is_live(*key),
            EvalError::IndexTarget { target } => self.ref_is_live(*target),
            EvalError::IndexSubscript { subscript } => self.ref_is_live(*subscript),
            EvalError::ApplyTarget { function } => self.ref_is_live(*function),
        });
        failures
            .2
            .retain(|error| live(error.condition) && self.ref_is_live(error.template));
        failures.3.retain(|error| {
            live(error.parameter_type)
                && live(error.argument_type)
                && live(error.argument)
                && live(error.apply_node)
        });
        (
            self.unify_errors,
            self.eval_errors,
            self.assert_errors,
            self.apply_errors,
        ) = failures;
    }
}
