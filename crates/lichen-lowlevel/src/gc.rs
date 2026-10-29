use stacksafe::stacksafe;

use crate::{
    AnyFunctionId, AnyHandle, AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayValue, BlockId, EvalError,
    LowValue, Module, NodeId, Program, ValueExt,
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
    /// surviving node's every edge into `source` must be one the walk follows: its
    /// class, its value's items, its traced nodes, or the operand of an owed
    /// operation. Three roots ride along: a pending assert's condition, and the
    /// template of a function whose home outlives the block.
    /// See [`Self::drop_block`].
    #[stacksafe]
    pub fn garbage_collect(&mut self, block_root: NodeId) -> Option<P::Value> {
        let source = self.nodes[block_root].block;
        let Some(target) = self.blocks[source].parent else {
            return self.nodes[block_root].value; // root block: nothing to move or release
        };
        self.gc_classes.clear();
        self.gc_visited.clear();
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
        // A surviving function's template is a root set too.
        for node in self.owned_by_surviving_function(source) {
            self.garbage_collect_node(node, source, target);
        }
        self.drop_block(source);
        // Records whose cell went with the block name nothing any more.
        self.requirements
            .retain(|node, _| self.nodes.contains_key(*node));
        self.replay_applied.retain(|node, applied| {
            self.nodes.contains_key(*node) && self.nodes.contains_key(*applied)
        });
        self.replay_parameters.retain(|node, parameter| {
            self.nodes.contains_key(*node) && self.nodes.contains_key(*parameter)
        });
        self.field_sources.retain(|required, supplied| {
            self.nodes.contains_key(*required) && self.nodes.contains_key(*supplied)
        });
        self.pending_projections.retain(|formal, operations| {
            operations.retain(|operation| self.nodes.contains_key(*operation));
            self.nodes.contains_key(*formal) && !operations.is_empty()
        });
        self.clone_parameters.retain(|clone, (parameter, image)| {
            self.nodes.contains_key(*clone)
                && self.nodes.contains_key(*parameter)
                && self.nodes.contains_key(*image)
        });
        self.parameter_images.retain(|formal, image| {
            self.nodes.contains_key(*formal) && self.nodes.contains_key(*image)
        });
        value
    }

    /// The nodes in `source`'s block subtree that a function outliving `source` names.
    ///
    /// # Invariant
    ///
    /// A function's home is not always the block its template nodes are homed in, so
    /// the walk cannot find them from the root; the node's owner is the one record that
    /// says which function a node belongs to, and a function whose home descends from
    /// `source` dies with it — its nodes are then released with it and are not kept.
    fn owned_by_surviving_function(&mut self, source: BlockId) -> Vec<NodeId> {
        let mut owned = Vec::new();
        let mut pending = vec![source];
        while let Some(block) = pending.pop() {
            let Some(entry) = self.blocks.get(block) else {
                continue;
            };
            pending.extend(entry.children.iter().copied());
            for &node in &entry.nodes {
                let lives_here = self
                    .nodes
                    .get(node)
                    .is_some_and(|node_entry| node_entry.block == block);
                if lives_here && self.owns_surviving_function(node, source) {
                    owned.push(node);
                }
            }
        }
        owned
    }

    /// Whether `node` belongs to a function whose home is outside `source`.
    fn owns_surviving_function(&self, node: NodeId, source: BlockId) -> bool {
        self.nodes[node].function.is_some_and(|owner| {
            self.functions
                .get(owner)
                .is_some_and(|entry| !self.descends_from(entry.block, source))
        })
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

    /// One node's part of the walk, and every edge it owns.
    ///
    /// # Invariant
    ///
    /// A node outside the vacated subtree is walked too: a live node may name one inside,
    /// or owe an answer whose operand is inside, and neither is reachable from the root.
    /// Its block stays put; only an inside node moves, into `target`. A node is walked once
    /// per collect, so a value cycle terminates.
    #[stacksafe]
    fn garbage_collect_node(
        &mut self,
        node: NodeId,
        source: BlockId,
        target: BlockId,
    ) -> Option<P::Value> {
        if !self.gc_visited.insert(node) {
            return self.nodes[node].value;
        }
        // A reader's answer lives in the class it read into, so every
        // member the node aliases is as live as the node is.
        let members = self.class_walk(node);
        let inside = self.descends_from(self.nodes[node].block, source);
        if inside {
            self.nodes[node].block = target;
            self.blocks[target].nodes.push(node);
        }
        for member in members {
            let member_inside = self.descends_from(self.nodes[member].block, source);
            self.garbage_collect_node(member, source, target);
            if !member_inside {
                // The class's value may live in the arena this collect releases: a class
                // write hands every member the same payload.
                self.relocate_class_value(member);
            }
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
        if let Some(supplied) = self.field_sources.get(&node).copied() {
            self.garbage_collect_node(supplied, source, target);
        }
        let value = current.map(|value| match value.as_enum() {
            Some(LowValue::Array(array)) if matches!(*array, AnyHandle::Static(_)) => {
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
                // compacted array keeps its markers — and its layout.
                if inside {
                    P::Value::from(LowValue::Array(ArrayValue::with_layout(
                        self.alloc_array(unsafe { array.items() }, target),
                        array.layout(),
                    )))
                } else {
                    value
                }
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
                if inside {
                    P::Value::from(LowValue::Table(
                        self.alloc_table(unsafe { table.items() }, target),
                    ))
                } else {
                    value
                }
            }
            // A static function value is frozen in the static module — no
            // scope to walk, no home block to re-point.
            Some(LowValue::Function(AnyFunctionId::Static(_))) => value,
            Some(LowValue::Function(AnyFunctionId::Dynamic(function))) => {
                // Only a function reached from inside the subtree is walked.
                if inside {
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
                if inside {
                    Self::copy_ext(self, value, target)
                } else {
                    value
                }
            }
            _ => value,
        });
        // The slot is rewritten by a value it already holds: the one licensed rewrite.
        if inside && let Some(value) = value {
            self.relocate_node_value(node, value);
        }
        value
    }

    /// Re-allocate `node`'s payload into **its own** block, keeping the value itself.
    ///
    /// # Invariant
    ///
    /// A class write gives every member the same payload, so a member outside the vacated
    /// subtree can hold a payload the collect is about to release; its own block is the one
    /// arena that certainly outlives it. The slot is rewritten by a value it already holds —
    /// GC's one licensed rewrite — and the payload is moved, not replaced: it is
    /// re-allocated from the same items.
    fn relocate_class_value(&mut self, node: NodeId) {
        let Some(value) = self.nodes[node].value else {
            return;
        };
        let home = self.nodes[node].block;
        let relocated = match value.as_enum() {
            Some(LowValue::Array(array)) if matches!(*array, AnyHandle::Static(_)) => return,
            // SAFETY: `array` is a live node's payload, and `home` outlives the node.
            Some(LowValue::Array(array)) => {
                P::Value::from(LowValue::Array(ArrayValue::with_layout(
                    self.alloc_array(unsafe { array.items() }, home),
                    array.layout(),
                )))
            }
            Some(LowValue::Table(AnyHandle::Static(_))) => return,
            // SAFETY: `table` is a live node's payload, and `home` outlives the node.
            Some(LowValue::Table(table)) => P::Value::from(LowValue::Table(
                self.alloc_table(unsafe { table.items() }, home),
            )),
            // A scalar, a function or the wildcard owns no arena payload.
            Some(_) => return,
            None => Self::copy_ext(self, value, home),
        };
        self.relocate_node_value(node, relocated);
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
        // The summary moves with the class: the survivors' owners are the whole
        // class now, so the summary is rebuilt from them.
        self.refresh_class_summary(representative);
        if let Some(value) = survivors
            .iter()
            .find_map(|&member| self.nodes.get(member).and_then(|node| node.value))
        {
            self.fill_class_holes(representative, value);
        }
        if let Some(shape) = low_shape {
            self.refine_class_low_type(representative, (*shape).clone());
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
                // The nodes it owns lose their owner: the refresh runs
                // after the record is gone, so it reads as invalid.
                let owned: Vec<NodeId> = self
                    .functions
                    .get(function)
                    .map(|entry| entry.nodes.clone())
                    .unwrap_or_default();
                self.functions.remove(function);
                for node in owned {
                    if self.nodes.get(node).is_some_and(|node| node.block != block) {
                        self.refresh_class_summary(node);
                    }
                }
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
