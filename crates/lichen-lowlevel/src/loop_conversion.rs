//! A marked `@loop` recursion, converted to its own facts over its template.
//! See docs/notes/loop-conversion.md §4, §8.6.

use std::collections::HashSet;

use lichen_utils::extend::AsEnum;

use crate::resolve::Selection;
use crate::{AnyNodeId, FunctionId, LowOperator, LowValue, Module, NodeId, Program};

/// Why a marked recursion is not convertible — one variant per shape rule
/// (`docs/notes/loop-conversion.md` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopRefusal {
    /// The function never applies itself, through its own body or a nested
    /// closure's, so there is no cycle to convert.
    NotRecursive,
    /// The marked call graph's component has more than one member; only
    /// self-recursion converts (§4's cap, at one).
    MutualComponent {
        /// The component's size.
        size: usize,
    },
    /// A self-application is not in tail position: inside a computation of
    /// the spine rather than a branch of it (§4 rule 1).
    NonTailCall,
    /// Every branch of the return spine is a step — the recursion has no base
    /// case, so the loop would have no exit.
    NoBaseCase,
    /// The state is not a flat tuple of scalar cells: arities disagree, a
    /// step carries an aggregate, or a read escapes it.
    StateShape,
}

impl LoopRefusal {
    /// The refusal's name — a diagnostic carries it beside its kind, so the
    /// renderer's wording and the rule cannot drift.
    pub fn name(&self) -> &'static str {
        match self {
            LoopRefusal::NotRecursive => "not recursive",
            LoopRefusal::MutualComponent { .. } => "a mutual recursion",
            LoopRefusal::NonTailCall => "a recursive call not in tail position",
            LoopRefusal::NoBaseCase => "no base case",
            LoopRefusal::StateShape => "a non-scalar carried state",
        }
    }
}

/// What a convertible `@loop` recursion converts to — the recursion's parts,
/// named as nodes of the function's template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopConversion {
    /// The marked function.
    pub function: crate::AnyFunctionId,
    /// The carried state, in slot order.
    ///
    /// # Invariant
    ///
    /// Each slot is the path into the parameter's value it names: `[]` the
    /// whole parameter, `[k]` element `k` of a tuple. A consumer maps a
    /// parameter read to its slot through this list and
    /// [`Module::parameter_value_path`]; how slots become locals is the
    /// consumer's own convention.
    pub state: Vec<Vec<usize>>,
    /// The spine's decisions, the loop's base tests. The first is the
    /// outermost — the one each iteration re-enters.
    pub tests: Vec<LoopTest>,
    /// The steps: the tail self-applications, in spine order.
    pub steps: Vec<LoopStep>,
    /// The exits: the base cases, in spine order.
    pub exits: Vec<LoopExit>,
}

/// One decision of the spine: a selection whose selector is the base test —
/// `1` continues at `on_one`, `0` at `on_zero`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopTest {
    /// The selector node — the condition the iteration evaluates.
    pub condition: AnyNodeId,
    pub on_one: LoopArm,
    pub on_zero: LoopArm,
}

/// Where one outcome of a [`LoopTest`] goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopArm {
    /// Another test (a nested conditional on the spine).
    Test(usize),
    /// A step: recurse — the loop's backedge.
    Step(usize),
    /// An exit: a base case — the loop's result.
    Exit(usize),
}

/// One tail self-application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopStep {
    /// The recursive call node itself.
    pub apply: AnyNodeId,
    /// The next state, in slot order — the call's argument's elements, each a
    /// computation over the state or a state read.
    pub next: Vec<AnyNodeId>,
}

/// One base case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopExit {
    /// The base's value half — the branch's result as a value: a scalar
    /// cell, or the array a tuple codomain flattens to.
    ///
    /// # Invariant
    ///
    /// A consumer builds the apply's result expr `[value, type]` from this
    /// node, never by evaluating the function's return: the *untaken* step
    /// arm would otherwise stay in the graph for the next deep pass to walk
    /// (and unroll — see `loop_run.rs`).
    pub value: AnyNodeId,
}

/// One view over the graphs the loop analysis reads, live and frozen.
///
/// # Invariant
/// A frozen function is read in its **own** module's graph (the [`StaticModule`]
/// its ref names), never through a dynamic `FunctionId`: an id is module-local,
/// and a frozen module has its own arena.
struct LoopGraph<'a, P: Program> {
    module: &'a Module<P>,
}

impl<'a, P: Program> LoopGraph<'a, P> {
    fn new(module: &'a Module<P>) -> Self {
        Self { module }
    }

    /// The function's body nodes, in its own graph.
    fn function_nodes(&self, function: crate::AnyFunctionId) -> Vec<AnyNodeId> {
        match function {
            crate::AnyFunctionId::Dynamic(function) => self.module.functions[function]
                .nodes
                .iter()
                .copied()
                .map(AnyNodeId::Dynamic)
                .collect(),
            crate::AnyFunctionId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                frozen.functions[sref.index.0]
                    .nodes
                    .iter()
                    .map(|&index| {
                        AnyNodeId::Static(crate::StaticNodeId {
                            module: sref.module,
                            index,
                        })
                    })
                    .collect()
            }
        }
    }

    /// The function that lexically encloses `function`.
    fn function_parent(&self, function: crate::AnyFunctionId) -> Option<crate::AnyFunctionId> {
        match function {
            crate::AnyFunctionId::Dynamic(function) => self.module.functions[function]
                .parent
                .map(crate::AnyFunctionId::Dynamic),
            crate::AnyFunctionId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                frozen.functions[sref.index.0].parent.map(|parent| {
                    crate::AnyFunctionId::Static(crate::StaticFunctionRef {
                        module: sref.module,
                        index: parent,
                    })
                })
            }
        }
    }

    /// Whether the function carries the `@loop` mark, and its state slot.
    fn function_looping(&self, function: crate::AnyFunctionId) -> Option<usize> {
        match function {
            crate::AnyFunctionId::Dynamic(function) => self.module.functions[function].looping,
            crate::AnyFunctionId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                frozen.functions[sref.index.0].looping
            }
        }
    }

    /// The node's operation and its operand node, in the node's own graph.
    fn node_operation(&self, node: AnyNodeId) -> Option<(P::Operator, Option<AnyNodeId>)> {
        match node {
            AnyNodeId::Dynamic(node) => self.module.node_operation(node).map(|operation| {
                (
                    operation.operator,
                    operation.operand.map(AnyNodeId::Dynamic),
                )
            }),
            AnyNodeId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                frozen.static_operation(sref.index).map(|operation| {
                    (
                        operation.operator,
                        operation.operand.map(|index| {
                            AnyNodeId::Static(crate::StaticNodeId {
                                module: sref.module,
                                index,
                            })
                        }),
                    )
                })
            }
        }
    }

    /// The array items of `node`, in the node's own graph.
    fn operand_items(&self, node: AnyNodeId) -> Option<Vec<AnyNodeId>> {
        match node {
            AnyNodeId::Dynamic(node) => {
                // SAFETY: the node is a live node of this module.
                let items = unsafe { self.module.array_items(node) }?;
                Some(items.iter().map(|item| item.node).collect())
            }
            AnyNodeId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                let items = frozen.static_array_items(sref.index)?;
                Some(
                    items
                        .into_iter()
                        .map(|index| {
                            AnyNodeId::Static(crate::StaticNodeId {
                                module: sref.module,
                                index,
                            })
                        })
                        .collect(),
                )
            }
        }
    }

    /// The value behind `node`, through the module's static facade.
    fn node_value(&self, node: AnyNodeId) -> Option<P::Value> {
        self.module.node_value(node)
    }

    /// The representative of `node`'s equality class, in the node's own graph.
    fn class_root(&self, node: AnyNodeId) -> AnyNodeId {
        match node {
            AnyNodeId::Dynamic(node) => AnyNodeId::Dynamic(self.module.class_root(node)),
            AnyNodeId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                AnyNodeId::Static(crate::StaticNodeId {
                    module: sref.module,
                    index: frozen.static_class_root(sref.index),
                })
            }
        }
    }

    /// The function's declared return node, in its own graph.
    fn function_return(&self, function: crate::AnyFunctionId) -> Option<AnyNodeId> {
        match function {
            crate::AnyFunctionId::Dynamic(function) => {
                Some(AnyNodeId::Dynamic(self.module.functions[function].r#return))
            }
            crate::AnyFunctionId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                Some(AnyNodeId::Static(crate::StaticNodeId {
                    module: sref.module,
                    index: frozen.functions[sref.index.0].r#return,
                }))
            }
        }
    }

    /// The value half of a `[value, type]` node, in the node's own graph.
    fn expr_value_half(&self, node: AnyNodeId) -> Option<AnyNodeId> {
        match node {
            AnyNodeId::Dynamic(node) => self.module.expr_value_half(node).map(AnyNodeId::Dynamic),
            AnyNodeId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                let items = frozen.static_array_items(sref.index)?;
                let first = *items.first()?;
                Some(AnyNodeId::Static(crate::StaticNodeId {
                    module: sref.module,
                    index: first,
                }))
            }
        }
    }

    /// The value's element value nodes, in the value's own graph.
    fn value_leaves(&self, value: AnyNodeId) -> Result<Vec<AnyNodeId>, String> {
        match value {
            AnyNodeId::Dynamic(node) => self
                .module
                .value_leaves(node)
                .map(|leaves| leaves.into_iter().map(AnyNodeId::Dynamic).collect()),
            AnyNodeId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                let Some(items) = frozen.static_array_items(sref.index) else {
                    return Ok(vec![value]);
                };
                let mut out = Vec::with_capacity(items.len());
                for index in items {
                    let element = AnyNodeId::Static(crate::StaticNodeId {
                        module: sref.module,
                        index,
                    });
                    out.push(self.element_value_node(element).unwrap_or(element));
                }
                Ok(out)
            }
        }
    }

    /// The selector position a node names, live or frozen.
    fn read_position(&self, item: AnyNodeId) -> Option<usize> {
        match item {
            AnyNodeId::Dynamic(node) => self.module.usize_value(node),
            AnyNodeId::Static(sref) => match self
                .module
                .node_value(AnyNodeId::Static(sref))
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
            {
                Some(LowValue::USize(value)) => Some(value),
                _ => None,
            },
        }
    }

    /// Element `k` of an array node, in the array's own graph.
    fn item_of(&self, array: AnyNodeId, k: usize) -> Option<AnyNodeId> {
        self.operand_items(array)?.get(k).copied()
    }

    /// The function's parameter node, in its own graph.
    fn function_parameter(&self, function: crate::AnyFunctionId) -> Option<AnyNodeId> {
        match function {
            crate::AnyFunctionId::Dynamic(function) => Some(AnyNodeId::Dynamic(
                self.module.functions[function].parameter,
            )),
            crate::AnyFunctionId::Static(sref) => {
                let frozen = self.module.static_module(sref.module);
                Some(AnyNodeId::Static(crate::StaticNodeId {
                    module: sref.module,
                    index: frozen.functions[sref.index.0].parameter,
                }))
            }
        }
    }

    /// The node a clone was made from, in the node's own graph.
    fn node_origin(&self, node: AnyNodeId) -> Option<AnyNodeId> {
        match node {
            AnyNodeId::Dynamic(node) => self.module.node_origin(node).map(AnyNodeId::Dynamic),
            AnyNodeId::Static(_) => None,
        }
    }

    /// The state path a read of the marked slot names, in either graph.
    fn marked_state_path(
        &self,
        function: crate::AnyFunctionId,
        node: AnyNodeId,
        slot: usize,
    ) -> Option<Vec<usize>> {
        let parameter = self.function_parameter(function)?;
        let mut collected = Vec::new();
        let mut current = node;
        for _ in 0..16 {
            let (operator, operand) = self.node_operation(current)?;
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&operator),
                Some(LowOperator::Index)
            ) {
                return None;
            }
            let operands = self.operand_items(operand?)?;
            let (target, selector) = (*operands.first()?, *operands.get(1)?);
            let position = self.read_position(selector)?;
            if target == parameter {
                if position != slot {
                    return None;
                }
                return Module::<P>::state_path(collected);
            }
            collected.push(position);
            current = target;
        }
        None
    }

    /// The path into `function`'s parameter value that `node` reads, in either graph.
    fn parameter_value_path(
        &self,
        function: crate::AnyFunctionId,
        node: AnyNodeId,
    ) -> Option<Vec<usize>> {
        if let Some(slot) = self.function_looping(function)
            && let Some(path) = self.marked_state_path(function, node, slot)
        {
            return Some(path);
        }
        let parameter = self.function_parameter(function)?;
        let value_cell = self.expr_value_half(parameter).unwrap_or(parameter);
        let value_class = self.class_root(value_cell);
        let mut path = Vec::new();
        let mut current = node;
        let mut followed = false;
        for _ in 0..16 {
            if self.class_root(current) == value_class {
                return Module::<P>::state_path(path);
            }
            let Some((operator, operand)) = self.node_operation(current) else {
                let origin = self.node_origin(current)?;
                if followed || origin == current {
                    return None;
                }
                followed = true;
                current = origin;
                continue;
            };
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&operator),
                Some(LowOperator::Index)
            ) {
                return None;
            }
            let items = self.operand_items(operand?)?;
            let target = *items.first()?;
            let position = self.read_position(*items.get(1)?)?;
            path.push(position);
            current = target;
        }
        None
    }

    /// The structural value behind `node`, in either graph.
    fn structural_value(&self, node: AnyNodeId) -> Option<LowValue> {
        let value = self.node_value(node)?;
        AsEnum::<LowValue>::as_enum(&value)
    }

    /// Whether `node` is a `Buf`: its `.native` payload is the `$buf` operator.
    fn carries_buffer(&self, node: AnyNodeId) -> bool {
        let Some(items) = self.operand_items(node) else {
            return false;
        };
        items
            .into_iter()
            .filter_map(|item| self.node_operation(item))
            .any(|(operator, _)| format!("{operator:?}") == "Buf")
    }

    /// The value node an element names, in either graph.
    fn element_value_node(&self, node: AnyNodeId) -> Option<AnyNodeId> {
        self.module.element_value_node(node)
    }

    /// Whether `node`'s subtree applies `function`, in either graph.
    fn subtree_applies(&self, node: AnyNodeId, function: crate::AnyFunctionId) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![node];
        while let Some(current) = stack.pop() {
            if !seen.insert(current) {
                continue;
            }
            if self.callee_function(current) == Some(function) {
                return true;
            }
            if let Some(items) = self.operands_of(current) {
                stack.extend(items);
            }
            if let Some(items) = self.operand_items(current) {
                stack.extend(items);
            }
        }
        false
    }

    /// Whether a node is the language's conditional or a view, in either graph.
    fn selection_of(&self, node: AnyNodeId) -> Option<Selection> {
        let (operator, operand) = self.node_operation(node)?;
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operator),
            Some(LowOperator::Index)
        ) {
            return None;
        }
        let items = self.operand_items(operand?)?;
        let (target, index) = (items.first()?, items.get(1)?);
        if let Some(position) = self.read_position(*index) {
            if let Some(element) = self.item_of(*target, position) {
                return match element {
                    AnyNodeId::Dynamic(element) => Some(Selection::Views(element)),
                    AnyNodeId::Static(_) => None,
                };
            }
            // A computed target at index 0 is a `value_of` peel: the result is the
            // expr's value, so it names the operator.
            if position == 0
                && let AnyNodeId::Dynamic(target) = target
                && self.node_operation(AnyNodeId::Dynamic(*target)).is_some()
            {
                return Some(Selection::Views(*target));
            }
            return None;
        }
        match self.operand_items(*target) {
            Some(items) if items.len() >= 2 => Some(Selection::Computed),
            _ => None,
        }
    }

    /// The items of a node's operation operand, in the node's own graph.
    fn operands_of(&self, node: AnyNodeId) -> Option<Vec<AnyNodeId>> {
        let (_, operand) = self.node_operation(node)?;
        self.operand_items(operand?)
    }

    /// The function a `node`'s apply names, in either graph.
    fn callee_function(&self, node: AnyNodeId) -> Option<crate::AnyFunctionId> {
        let (operator, operand) = self.node_operation(node)?;
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operator),
            Some(LowOperator::Apply)
        ) {
            return None;
        }
        let items = self.operand_items(operand?)?;
        let callee = *items.first()?;
        let value = self
            .node_value(callee)
            .or_else(|| self.node_value(self.class_root(callee)))?;
        match AsEnum::<LowValue>::as_enum(&value) {
            Some(LowValue::Function(function)) => Some(function),
            _ => None,
        }
    }

    /// Whether `h` is `g` or a closure nested in it.
    fn nested_in(&self, h: crate::AnyFunctionId, g: crate::AnyFunctionId) -> bool {
        let mut owner = Some(h);
        while let Some(current) = owner {
            if current == g {
                return true;
            }
            owner = self.function_parent(current);
        }
        false
    }

    /// The marked component `function` belongs to: the `@loop` functions that
    /// reach it and reach back, in either graph.
    fn loop_component(&self, function: crate::AnyFunctionId) -> Vec<crate::AnyFunctionId> {
        let reachable = |from: crate::AnyFunctionId| {
            let mut seen = vec![];
            let mut stack = vec![from];
            while let Some(g) = stack.pop() {
                if seen.contains(&g) {
                    continue;
                }
                seen.push(g);
                for callee in self.marked_callees_of(g) {
                    if !seen.contains(&callee) {
                        stack.push(callee);
                    }
                }
            }
            seen
        };
        reachable(function)
            .into_iter()
            .filter(|&g| reachable(g).contains(&function))
            .collect()
    }

    /// The marked functions `g`'s template subtree applies to — the edges of
    /// the marked call graph, live and frozen.
    fn marked_callees_of(&self, g: crate::AnyFunctionId) -> Vec<crate::AnyFunctionId> {
        let mut functions: Vec<crate::AnyFunctionId> = self
            .module
            .functions
            .keys()
            .map(crate::AnyFunctionId::Dynamic)
            .collect();
        // A frozen target's module functions join the scan: its recursion is
        // static.
        if let crate::AnyFunctionId::Static(sref) = g {
            let frozen = self.module.static_module(sref.module);
            functions.extend((0..frozen.functions.len()).map(|index| {
                crate::AnyFunctionId::Static(crate::StaticFunctionRef {
                    module: sref.module,
                    index: crate::StaticFunctionId(index),
                })
            }));
        }
        let mut out = Vec::new();
        for h in functions {
            if !self.nested_in(h, g) {
                continue;
            }
            for node in self.function_nodes(h) {
                if let Some(callee) = self.callee_function(node)
                    && self.function_looping(callee).is_some()
                    && !out.contains(&callee)
                {
                    out.push(callee);
                }
            }
        }
        out
    }
}

impl<P: Program> Module<P> {
    /// Whether the function carries the `@loop` mark, live or frozen, and its
    /// state slot.
    ///
    /// # Invariant
    /// The mark lives on the function's own graph: a frozen function states the
    /// slot its freeze copied, never a live function's.
    pub fn function_looping_any(&self, function: crate::AnyFunctionId) -> Option<usize> {
        LoopGraph::new(self).function_looping(function)
    }

    /// Whether `function`'s `@loop` recursion is convertible, and what it
    /// converts to. Read-only over the template.
    ///
    /// # Invariant
    ///
    /// `function` is stated to be marked, the mark being what makes a cycle
    /// convertible. The checks fire in order: the marked component is this
    /// function alone; the return spine's branches are all base or step and
    /// every self-application in the subtree is one of those steps; there is a
    /// base; the state is a flat tuple of scalar cells covering every read.
    pub fn loop_conversion(&self, function: FunctionId) -> Result<LoopConversion, LoopRefusal> {
        self.loop_conversion_any(crate::AnyFunctionId::Dynamic(function))
    }

    /// The conversion of a live or a frozen `@loop` function, read through one
    /// graph (see [`LoopGraph`]).
    pub fn loop_conversion_any(
        &self,
        function: crate::AnyFunctionId,
    ) -> Result<LoopConversion, LoopRefusal> {
        let component = LoopGraph::new(self).loop_component(function);
        if !component.contains(&function) {
            return Err(LoopRefusal::NotRecursive);
        }
        if component.len() > 1 {
            return Err(LoopRefusal::MutualComponent {
                size: component.len(),
            });
        }
        let mut spine = Spine::new(self, function);
        spine.walk()?;
        let state = spine.state();
        let state = state?;
        Ok(LoopConversion {
            function,
            state,
            tests: spine.tests,
            steps: spine.steps,
            exits: spine.exits,
        })
    }

    /// Every state read that names `path` — the inverse of [`Self::parameter_value_path`].
    ///
    /// # Invariant
    /// A node is a state read when the marked slot's own chain collects to `path`. The
    /// same state has one read in the loop's body and another at the entry, so **all**
    /// of them are answered, not the first.
    pub fn state_nodes(&self, function: FunctionId, path: &[usize]) -> Vec<NodeId> {
        let Some(slot) = self.functions[function].looping else {
            return Vec::new();
        };
        self.blocks
            .iter()
            .flat_map(|(_, block)| block.nodes.iter().copied())
            .filter(|node| self.marked_state_path(function, *node, slot).as_deref() == Some(path))
            .collect()
    }

    /// The path into `function`'s parameter value that `node` reads: `[]` the
    /// whole value, `[k]` element `k` of a tuple.
    ///
    /// # Invariant
    /// Read from the **value side**: the chain alternates element positions with the
    /// slot's value position (`0`), so the expr peel and every slot peel are dropped
    /// and an expr's type or attribute position ends the match rather than naming a
    /// state element (`docs/notes/compound-element-slots.md`). The frozen position
    /// and the clone are one reading: `docs/notes/loop-conversion.md` §8.2.
    pub fn parameter_value_path(&self, function: FunctionId, node: NodeId) -> Option<Vec<usize>> {
        // The mark's slot matches a state read structurally
        // (`docs/notes/loop-conversion.md`).
        if let Some(slot) = self.functions[function].looping
            && let Some(path) = self.marked_state_path(function, node, slot)
        {
            return Some(path);
        }
        let parameter = self.functions[function].parameter;
        let value_cell = self.expr_value_half(parameter).unwrap_or(parameter);
        let value_class = self.class_root(value_cell);
        let mut path = Vec::new();
        let mut current = node;
        // **One origin hop**, so a chain that reaches a clone's own parameter
        // resumes at the template.
        let mut followed = false;
        for _ in 0..16 {
            let class_matched = self.class_root(current) == value_class;
            if class_matched && !path.is_empty() {
                // The chain reached a state element: what was collected reads
                // outermost-first.
                return Self::state_path(path);
            }
            // **The state's reader names the element**: the value's expression is
            // the state's `Index`.
            for expr in self.value_exprs(current) {
                let Some(operation) = self.node_operation(*expr) else {
                    continue;
                };
                if !matches!(
                    AsEnum::<LowOperator>::as_enum(&operation.operator),
                    Some(LowOperator::Index)
                ) {
                    continue;
                }
                let Some(operand) = operation.operand else {
                    continue;
                };
                let Ok(items) = self.operand_items(operand) else {
                    continue;
                };
                let Some(target) = items.first().and_then(|item| item.node.dynamic()) else {
                    continue;
                };
                let state_type = self.element_types.get(&value_cell).copied().or_else(|| {
                    self.value_exprs(current).iter().find_map(|expr| {
                        let operation = self.node_operation(*expr)?;
                        if !matches!(
                            AsEnum::<LowOperator>::as_enum(&operation.operator),
                            Some(LowOperator::Index)
                        ) {
                            return None;
                        }
                        let items = self.operand_items(operation.operand?).ok()?;
                        let target = items.first().and_then(|item| item.node.dynamic())?;
                        self.element_types.get(&target).copied()
                    })
                });
                let Some(state_type) = state_type else {
                    continue;
                };
                if self.element_types.get(&target).copied() != Some(state_type) {
                    continue;
                }
                let Some(position) = items.get(1).and_then(|item| self.read_position(item.node))
                else {
                    continue;
                };
                let mut slot = path.clone();
                slot.push(position);
                return Self::state_path(slot);
            }
            if class_matched {
                // The whole state is the value: a scalar state's empty path.
                return Self::state_path(path);
            }
            let Some(operation) = self.node_operation(current) else {
                // **The walk bottomed out**: a clone's chain ends at this
                // instantiation's parameter, a node the class test misses.
                let origin = self.node_origin(current)?;
                if followed || origin == current {
                    return None;
                }
                followed = true;
                current = origin;
                continue;
            };
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&operation.operator),
                Some(LowOperator::Index)
            ) {
                return None;
            }
            // **The target must be this module's, the position need not be.**
            let operand = operation.operand?;
            let items = self.operand_items(operand).ok()?;
            let target = items.first()?.node.dynamic()?;
            let position = self.read_position(items.get(1)?.node)?;
            path.push(position);
            current = target;
        }
        None
    }

    /// A read's selector position: a dynamic `USize` node or a frozen one.
    pub fn read_position(&self, item: AnyNodeId) -> Option<usize> {
        match item {
            AnyNodeId::Dynamic(node) => self.usize_value(node),
            AnyNodeId::Static(sref) => match self
                .node_value(AnyNodeId::Static(sref))
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
            {
                Some(LowValue::USize(n)) => Some(n),
                _ => None,
            },
        }
    }

    /// The state path a read of the entering call's element names
    /// (`docs/notes/loop-conversion.md`).
    pub fn entry_state_path(&self, state: NodeId, node: NodeId) -> Option<Vec<usize>> {
        let mut collected = Vec::new();
        let mut current = node;
        for _ in 0..16 {
            let operation = self.node_operation(current)?;
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&operation.operator),
                Some(LowOperator::Index)
            ) {
                return None;
            }
            let operands = self.operand_items(operation.operand?).ok()?;
            let position = self.read_position(operands.get(1)?.node)?;
            let target = operands.first()?.node.dynamic()?;
            if target == state {
                // The argument **expr**'s value position is the state itself.
                return if position == 0 {
                    Self::state_path(collected)
                } else {
                    None
                };
            }
            collected.push(position);
            current = target;
        }
        None
    }

    /// The state path a read of the marked slot names.
    fn marked_state_path(
        &self,
        function: FunctionId,
        node: NodeId,
        slot: usize,
    ) -> Option<Vec<usize>> {
        let parameter = self.functions[function].parameter;
        let mut collected = Vec::new();
        let mut current = node;
        for _ in 0..16 {
            let operation = self.node_operation(current)?;
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&operation.operator),
                Some(LowOperator::Index)
            ) {
                return None;
            }
            let operands = self.operands_of(current).ok()?;
            let (target, selector) = (*operands.first()?, *operands.get(1)?);
            let position = self.usize_value(selector)?;
            if target == parameter {
                if position != slot {
                    return None;
                }
                return Self::state_path(collected);
            }
            collected.push(position);
            current = target;
        }
        None
    }

    /// The state positions a collected peel chain names, or [`None`] for a slot's
    /// type or attribute read.
    ///
    /// # Invariant
    /// The collected positions are outermost-first: an element, then the element
    /// slot's value position (`0`), then the next element. A non-zero position where
    /// the slot peel belongs is a type (`1`) or an attribute (`2+`).
    fn state_path(mut collected: Vec<usize>) -> Option<Vec<usize>> {
        collected.reverse();
        let mut path = Vec::with_capacity(collected.len());
        for (index, position) in collected.into_iter().enumerate() {
            if index % 2 == 1 {
                if position != 0 {
                    return None;
                }
            } else {
                path.push(position);
            }
        }
        Some(path)
    }

    /// The function `node` applies, when it is an `Apply` whose callee's
    /// value is a function of this module.
    ///
    /// # Invariant
    ///
    /// The callee sits in the operand array's first slot, and its value may
    /// ride the node or its equality class: the checker substitutes through
    /// unification.
    pub fn callee_function(&self, node: NodeId) -> Option<FunctionId> {
        match self.graph().callee_function(AnyNodeId::Dynamic(node))? {
            crate::AnyFunctionId::Dynamic(function) => Some(function),
            crate::AnyFunctionId::Static(_) => None,
        }
    }

    /// The graph view this module reads, live and frozen functions alike.
    fn graph(&self) -> LoopGraph<'_, P> {
        LoopGraph::new(self)
    }
}

/// The spine walk: classify the return value's branches, then validate the
/// state and the tail-position rule.
struct Spine<'m, P: Program> {
    module: &'m Module<P>,
    function: crate::AnyFunctionId,
    tests: Vec<LoopTest>,
    steps: Vec<LoopStep>,
    exits: Vec<LoopExit>,
}

impl<P: Program> Spine<'_, P> {
    fn new<'m>(module: &'m Module<P>, function: crate::AnyFunctionId) -> Spine<'m, P> {
        Spine {
            module,
            function,
            tests: Vec::new(),
            steps: Vec::new(),
            exits: Vec::new(),
        }
    }

    /// Walk the return value's spine. Its leaves are the bases and steps;
    /// anything else that recurses is not a tail call.
    fn walk(&mut self) -> Result<(), LoopRefusal> {
        let graph = LoopGraph::new(self.module);
        let ret = graph
            .function_return(self.function)
            .ok_or(LoopRefusal::NotRecursive)?;
        let value = graph.expr_value_half(ret).unwrap_or(ret);
        let roots = graph
            .value_leaves(value)
            .map_err(|_| LoopRefusal::StateShape)?;
        if roots.len() == 1 {
            match self.classify(roots[0])? {
                Classified::Test(selection) => {
                    self.walk_test(selection)?;
                }
                Classified::Step(apply) => {
                    self.record_step(apply)?;
                }
                Classified::Base(value) => {
                    self.record_exit(value)?;
                }
            }
        } else {
            // A tuple-bodied return: a recursion hiding in an element is a
            // call the spine does not own.
            for &root in &roots {
                if graph.subtree_applies(root, self.function) {
                    return Err(LoopRefusal::NonTailCall);
                }
            }
        }
        if self.steps.is_empty() {
            // No step: `NotRecursive` only when nothing applies itself;
            // otherwise a self-application exists the spine does not own.
            self.complete()?;
            return Err(LoopRefusal::NotRecursive);
        }
        if self.exits.is_empty() {
            return Err(LoopRefusal::NoBaseCase);
        }
        self.complete()?;
        Ok(())
    }

    /// One selection of the spine: record its test, then walk both arms.
    fn walk_test(&mut self, selection: AnyNodeId) -> Result<usize, LoopRefusal> {
        // A view is not a conditional: the test is what the view names
        // (`docs/notes/loop-conversion.md`).
        let mut selection = selection;
        let mut seen = HashSet::new();
        let graph = LoopGraph::new(self.module);
        while seen.insert(selection) {
            match graph.selection_of(selection) {
                Some(Selection::Views(view)) => selection = AnyNodeId::Dynamic(view),
                _ => break,
            }
        }
        let operands = graph
            .operands_of(selection)
            .ok_or(LoopRefusal::StateShape)?;
        let (Some(arms), Some(condition)) = (operands.first().copied(), operands.get(1).copied())
        else {
            return Err(LoopRefusal::StateShape);
        };
        let arms = graph.operand_items(arms).ok_or(LoopRefusal::StateShape)?;
        if arms.len() != 2 {
            return Err(LoopRefusal::StateShape);
        }
        let arm = |index: usize| {
            // An element is a complete slot: the arm is its value position.
            graph
                .element_value_node(arms[index])
                .ok_or(LoopRefusal::StateShape)
        };
        // `[b, a][c]` is `if c then a else b`: element 1 answers the `1` edge.
        let on_zero = self.walk_arm(arm(0)?)?;
        let on_one = self.walk_arm(arm(1)?)?;
        let index = self.tests.len();
        self.tests.push(LoopTest {
            condition,
            on_one,
            on_zero,
        });
        Ok(index)
    }

    /// One branch of the spine: a base, a step, or a nested test.
    fn walk_arm(&mut self, arm: AnyNodeId) -> Result<LoopArm, LoopRefusal> {
        match self.classify(arm)? {
            Classified::Test(selection) => Ok(LoopArm::Test(self.walk_test(selection)?)),
            Classified::Step(apply) => Ok(LoopArm::Step(self.record_step(apply)?)),
            Classified::Base(value) => Ok(LoopArm::Exit(self.record_exit(value)?)),
        }
    }

    /// What a spine node is. Views resolve first, but the recorded value
    /// stays the arm's node, so a base keeps its read path.
    fn classify(&self, arm: AnyNodeId) -> Result<Classified, LoopRefusal> {
        let graph = LoopGraph::new(self.module);
        let function = self.function;
        let mut resolved = arm;
        let mut seen = HashSet::new();
        while seen.insert(resolved) {
            match graph.selection_of(resolved) {
                Some(Selection::Views(view)) => resolved = AnyNodeId::Dynamic(view),
                Some(Selection::Computed) => return Ok(Classified::Test(resolved)),
                None => break,
            }
        }
        if graph.callee_function(resolved) == Some(function) {
            return Ok(Classified::Step(resolved));
        }
        if graph.subtree_applies(resolved, function) {
            return Err(LoopRefusal::NonTailCall);
        }
        Ok(Classified::Base(arm))
    }

    /// A step's next state: the call's argument's elements, each a scalar
    /// cell; an aggregate or a function is not carryable.
    fn record_step(&mut self, apply: AnyNodeId) -> Result<usize, LoopRefusal> {
        let graph = LoopGraph::new(self.module);
        let operands = graph.operands_of(apply).ok_or(LoopRefusal::StateShape)?;
        let argument = *operands.get(1).ok_or(LoopRefusal::StateShape)?;
        let value = graph.expr_value_half(argument).unwrap_or(argument);
        let next: Vec<AnyNodeId> = match graph.operand_items(value) {
            Some(items) => items
                .into_iter()
                .map(|item| {
                    // An element is a complete slot: the carried value is its first
                    // position.
                    graph.element_value_node(item).unwrap_or(item)
                })
                .collect(),
            None => vec![value],
        };
        for element in &next {
            if matches!(
                graph.structural_value(*element),
                Some(LowValue::Array(_) | LowValue::Function(_) | LowValue::Table(_))
            ) && !graph.carries_buffer(*element)
            {
                return Err(LoopRefusal::StateShape);
            }
        }
        let index = self.steps.len();
        self.steps.push(LoopStep { apply, next });
        Ok(index)
    }

    /// A base case: the branch's value half.
    fn record_exit(&mut self, arm: AnyNodeId) -> Result<usize, LoopRefusal> {
        let index = self.exits.len();
        self.exits.push(LoopExit { value: arm });
        Ok(index)
    }

    /// The carried state, from how the steps and the reads see it.
    ///
    /// # Invariant
    ///
    /// The steps fix the arity and must agree; the parameter reads fix the
    /// addressing — a whole-value read is the scalar case, an element read
    /// `s(k)` is slot `k`. The two may not mix, and no read may escape the
    /// arity.
    fn state(&self) -> Result<Vec<Vec<usize>>, LoopRefusal> {
        let arity = self.steps[0].next.len();
        if self.steps.iter().any(|step| step.next.len() != arity) {
            return Err(LoopRefusal::StateShape);
        }
        let mut tuple = false;
        let roots = self
            .tests
            .iter()
            .map(|test| test.condition)
            .chain(self.steps.iter().flat_map(|step| step.next.iter().copied()))
            .chain(self.exits.iter().map(|exit| exit.value));
        for root in roots {
            self.module
                .check_reads(self.function, root, arity, &mut tuple)?;
        }
        if tuple {
            Ok((0..arity).map(|k| vec![k]).collect())
        } else {
            if arity != 1 {
                // A tuple-valued state never read by element: the slots
                // exist but nothing names them — refuse rather than guess.
                return Err(LoopRefusal::StateShape);
            }
            Ok(vec![Vec::new()])
        }
    }

    /// The tail-position rule: every self-application in the subtree is
    /// one of the spine's steps.
    ///
    /// # Invariant
    ///
    /// This also separates a curried recursion from a non-recursion: a call
    /// inside a nested lambda is a self-application the spine does not own,
    /// found here rather than read as a function that never recurses.
    fn complete(&self) -> Result<(), LoopRefusal> {
        let graph = LoopGraph::new(self.module);
        let steps: HashSet<AnyNodeId> = self.steps.iter().map(|step| step.apply).collect();
        let function = self.function;
        for h in self.module.functions.keys() {
            let h = crate::AnyFunctionId::Dynamic(h);
            if !graph.nested_in(h, function) {
                continue;
            }
            for node in graph.function_nodes(h) {
                let Some(node) = node.dynamic() else {
                    continue;
                };
                if graph.callee_function(AnyNodeId::Dynamic(node)) == Some(function)
                    && !steps.contains(&AnyNodeId::Dynamic(node))
                {
                    return Err(LoopRefusal::NonTailCall);
                }
            }
        }
        Ok(())
    }
}

/// What [`Spine::classify`] resolved one branch to.
enum Classified {
    /// A nested selection — the walk continues into it.
    Test(AnyNodeId),
    /// A tail self-application.
    Step(AnyNodeId),
    /// A base value (the arm's own node, path intact).
    Base(AnyNodeId),
}

impl<P: Program> Module<P> {
    /// Validate every parameter read reachable from `root` against the
    /// state; refuse a read the flat state cannot name.
    fn check_reads(
        &self,
        function: crate::AnyFunctionId,
        root: AnyNodeId,
        arity: usize,
        tuple: &mut bool,
    ) -> Result<(), LoopRefusal> {
        let mut seen = HashSet::new();
        let mut stack = vec![(root, false)];
        while let Some((node, structural)) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            if let Some(path) = LoopGraph::new(self).parameter_value_path(function, node) {
                match path.as_slice() {
                    [] => {
                        if !structural && (*tuple || arity != 1) {
                            // The whole tuple as a value cannot be carried
                            // as flat scalars — and the two addressings may
                            // not mix in one loop.
                            return Err(LoopRefusal::StateShape);
                        }
                    }
                    [k] => {
                        if *k >= arity {
                            return Err(LoopRefusal::StateShape);
                        }
                        *tuple = true;
                    }
                    _ => return Err(LoopRefusal::StateShape),
                }
                // A read is a leaf of this walk: what it names is the
                // parameter, not its own operands.
                continue;
            }
            let graph = LoopGraph::new(self);
            if let Some(operands) = graph.operands_of(node) {
                stack.extend(operands.into_iter().map(|operand| (operand, false)));
            }
            // A value's own items are its encoding, not a use of the state.
            if let Some(items) = graph.operand_items(node) {
                stack.extend(items.into_iter().map(|item| (item, true)));
            }
        }
        Ok(())
    }
}
