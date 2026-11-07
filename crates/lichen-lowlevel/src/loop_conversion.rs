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
    pub function: FunctionId,
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
    pub condition: NodeId,
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
    pub apply: NodeId,
    /// The next state, in slot order — the call's argument's elements, each a
    /// computation over the state or a state read.
    pub next: Vec<NodeId>,
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
    pub value: NodeId,
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
                let items = frozen.static_array_items(sref.index).ok_or_else(|| {
                    "a frozen value's body is not an array, and a tuple-bodied return is a                      tuple of element slots"
                        .to_string()
                })?;
                let mut out = Vec::with_capacity(items.len());
                for index in items {
                    let element = AnyNodeId::Static(crate::StaticNodeId {
                        module: sref.module,
                        index,
                    });
                    out.push(self.expr_value_half(element).unwrap_or(element));
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
                .static_read(sref)
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

    /// The marked functions `g`'s template subtree applies to — the edges of
    /// the marked call graph, live and frozen.
    fn marked_callees_of(&self, g: crate::AnyFunctionId) -> Vec<crate::AnyFunctionId> {
        let mut out = Vec::new();
        for h in self.module.functions.keys() {
            let h = crate::AnyFunctionId::Dynamic(h);
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
    /// The live function a frozen `@loop` callee names; `None` when it carries no
    /// mark.
    ///
    /// # Invariant
    /// The mark is read from the live function, not the frozen copy: a freeze may
    /// run before `mark_looping`, so the static function's `looping` is stale.
    pub fn static_dynamic_function(&self, sref: crate::StaticFunctionRef) -> Option<FunctionId> {
        let module = self.static_module(sref.module);
        let source = module
            .dynamic_functions
            .get(sref.index.0)
            .copied()
            .flatten();
        self.functions
            .iter()
            .find(|(_, function)| {
                function.looping.is_some()
                    && (function.static_origin == Some(sref)
                        || (source.is_some() && function.template == source))
            })
            .map(|(id, _)| id)
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
        let component = self.loop_component(function);
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
            if self.class_root(current) == value_class {
                // The chain reached the state's value: what was collected reads
                // outermost-first.
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
                .static_read(sref)
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

    /// The marked component `function` belongs to: the `@loop` functions
    /// that reach it and reach back.
    fn loop_component(&self, function: FunctionId) -> Vec<FunctionId> {
        let graph = self.graph();
        let reachable = |from: crate::AnyFunctionId| {
            let mut seen = vec![];
            let mut stack = vec![from];
            while let Some(g) = stack.pop() {
                if seen.contains(&g) {
                    continue;
                }
                seen.push(g);
                for callee in graph.marked_callees_of(g) {
                    if !seen.contains(&callee) {
                        stack.push(callee);
                    }
                }
            }
            seen
        };
        let function = crate::AnyFunctionId::Dynamic(function);
        reachable(function)
            .into_iter()
            .filter(|&g| reachable(g).contains(&function))
            .filter_map(|g| match g {
                crate::AnyFunctionId::Dynamic(function) => Some(function),
                crate::AnyFunctionId::Static(_) => None,
            })
            .collect()
    }
}

/// The spine walk: classify the return value's branches, then validate the
/// state and the tail-position rule.
struct Spine<'m, P: Program> {
    module: &'m Module<P>,
    function: FunctionId,
    tests: Vec<LoopTest>,
    steps: Vec<LoopStep>,
    exits: Vec<LoopExit>,
}

impl<P: Program> Spine<'_, P> {
    fn new<'m>(module: &'m Module<P>, function: FunctionId) -> Spine<'m, P> {
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
        let function = crate::AnyFunctionId::Dynamic(self.function);
        let ret = graph
            .function_return(function)
            .ok_or(LoopRefusal::NotRecursive)?;
        let value = graph.expr_value_half(ret).unwrap_or(ret);
        let roots: Vec<NodeId> = graph
            .value_leaves(value)
            .map_err(|_| LoopRefusal::StateShape)?
            .into_iter()
            .filter_map(AnyNodeId::dynamic)
            .collect();
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
                if self.module.subtree_applies(root, self.function) {
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
    fn walk_test(&mut self, selection: NodeId) -> Result<usize, LoopRefusal> {
        // A view is not a conditional: the test is what the view names
        // (`docs/notes/loop-conversion.md`).
        let mut selection = selection;
        let mut seen = HashSet::new();
        let graph = LoopGraph::new(self.module);
        while seen.insert(selection) {
            match graph.selection_of(AnyNodeId::Dynamic(selection)) {
                Some(Selection::Views(view)) => selection = view,
                _ => break,
            }
        }
        let operands = self
            .module
            .operands_of(selection)
            .map_err(|_| LoopRefusal::StateShape)?;
        let (arms, condition) = (operands[0], operands[1]);
        // SAFETY: `arms` is a live node of the module.
        let arms =
            unsafe { self.module.array_items(arms) }.ok_or_else(|| LoopRefusal::StateShape)?;
        if arms.len() != 2 {
            return Err(LoopRefusal::StateShape);
        }
        let arm = |index: usize| {
            // An element is a complete slot: the arm is its value position.
            match self.module.element_value_node(arms[index].node) {
                Some(AnyNodeId::Dynamic(node)) => Ok(node),
                _ => Err(LoopRefusal::StateShape),
            }
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
    fn walk_arm(&mut self, arm: NodeId) -> Result<LoopArm, LoopRefusal> {
        match self.classify(arm)? {
            Classified::Test(selection) => Ok(LoopArm::Test(self.walk_test(selection)?)),
            Classified::Step(apply) => Ok(LoopArm::Step(self.record_step(apply)?)),
            Classified::Base(value) => Ok(LoopArm::Exit(self.record_exit(value)?)),
        }
    }

    /// What a spine node is. Views resolve first, but the recorded value
    /// stays the arm's node, so a base keeps its read path.
    fn classify(&self, arm: NodeId) -> Result<Classified, LoopRefusal> {
        let graph = LoopGraph::new(self.module);
        let function = crate::AnyFunctionId::Dynamic(self.function);
        let mut resolved = arm;
        let mut seen = HashSet::new();
        while seen.insert(resolved) {
            match graph.selection_of(AnyNodeId::Dynamic(resolved)) {
                Some(Selection::Views(view)) => resolved = view,
                Some(Selection::Computed) => return Ok(Classified::Test(resolved)),
                None => break,
            }
        }
        if graph.callee_function(AnyNodeId::Dynamic(resolved)) == Some(function) {
            return Ok(Classified::Step(resolved));
        }
        if graph.subtree_applies(AnyNodeId::Dynamic(resolved), function) {
            return Err(LoopRefusal::NonTailCall);
        }
        Ok(Classified::Base(arm))
    }

    /// A step's next state: the call's argument's elements, each a scalar
    /// cell; an aggregate or a function is not carryable.
    fn record_step(&mut self, apply: NodeId) -> Result<usize, LoopRefusal> {
        let operands = self
            .module
            .operands_of(apply)
            .map_err(|_| LoopRefusal::StateShape)?;
        let argument = operands[1];
        let value = self.module.expr_value_half(argument).unwrap_or(argument);
        // SAFETY: `value` is a live node of the module.
        let next: Vec<NodeId> = match unsafe { self.module.array_items(value) } {
            Some(items) => items
                .iter()
                .map(|item| {
                    // An element is a complete slot: the carried value is its first
                    // position.
                    let item = self
                        .module
                        .element_value_node(item.node)
                        .unwrap_or(item.node);
                    match item {
                        AnyNodeId::Dynamic(node) => Ok(node),
                        AnyNodeId::Static(_) => Err(LoopRefusal::StateShape),
                    }
                })
                .collect::<Result<_, _>>()?,
            None => vec![value],
        };
        for element in &next {
            if matches!(
                self.module.structural_value(*element),
                Some(LowValue::Array(_) | LowValue::Function(_) | LowValue::Table(_))
            ) && !self.carries_buffer(*element)
            {
                return Err(LoopRefusal::StateShape);
            }
        }
        let index = self.steps.len();
        self.steps.push(LoopStep { apply, next });
        Ok(index)
    }

    /// Whether `value` is a `Buf`: the struct whose `.native` payload is the `$buf` operator.
    ///
    /// # Invariant
    /// The lowlevel cannot name the compute's operator type (the dependency runs the other
    /// way), so the payload's debug label is the witness: `Buf` is the `$buf` op and nothing
    /// else. A plain array's items carry no such operator.
    fn carries_buffer(&self, value: NodeId) -> bool {
        // SAFETY: `value` is a live node of the module.
        let Some(items) = (unsafe { self.module.array_items(value) }) else {
            return false;
        };
        items
            .iter()
            .filter_map(|item| match item.node {
                AnyNodeId::Dynamic(node) => Some(node),
                AnyNodeId::Static(_) => None,
            })
            .filter_map(|node| self.module.node_operation(node))
            .any(|operation| format!("{:?}", operation.operator) == "Buf")
    }

    /// A base case: the branch's value half.
    fn record_exit(&mut self, arm: NodeId) -> Result<usize, LoopRefusal> {
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
        let steps: HashSet<NodeId> = self.steps.iter().map(|step| step.apply).collect();
        for h in self.module.functions.keys() {
            let mut owner = Some(h);
            let mut nested = false;
            while let Some(current) = owner {
                if current == self.function {
                    nested = true;
                    break;
                }
                owner = self.module.functions[current].parent;
            }
            if !nested {
                continue;
            }
            for &node in &self.module.functions[h].nodes {
                if self.module.callee_function(node) == Some(self.function)
                    && !steps.contains(&node)
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
    Test(NodeId),
    /// A tail self-application.
    Step(NodeId),
    /// A base value (the arm's own node, path intact).
    Base(NodeId),
}

impl<P: Program> Module<P> {
    /// Whether a self-application of `function` is reachable from `node`
    /// through operands and array values.
    ///
    /// # Invariant
    ///
    /// The walk stays inside values: a closure's *body* is not an operand of
    /// its value node, so a recursion hidden in a nested closure is
    /// [`Spine::complete`]'s catch, not this one's.
    fn subtree_applies(&self, node: NodeId, function: FunctionId) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![node];
        while let Some(current) = stack.pop() {
            if !seen.insert(current) {
                continue;
            }
            if self.callee_function(current) == Some(function) {
                return true;
            }
            if let Ok(operands) = self.operands_of(current) {
                stack.extend(operands);
            }
            // SAFETY: `current` is a live node of `self`.
            if let Some(items) = unsafe { self.array_items(current) } {
                stack.extend(items.iter().filter_map(|item| item.node.dynamic()));
            }
        }
        false
    }

    /// Validate every parameter read reachable from `root` against the
    /// state; refuse a read the flat state cannot name.
    fn check_reads(
        &self,
        function: FunctionId,
        root: NodeId,
        arity: usize,
        tuple: &mut bool,
    ) -> Result<(), LoopRefusal> {
        let mut seen = HashSet::new();
        let mut stack = vec![(root, false)];
        while let Some((node, structural)) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            if let Some(path) = self.parameter_value_path(function, node) {
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
            if let Ok(operands) = self.operands_of(node) {
                stack.extend(operands.into_iter().map(|operand| (operand, false)));
            }
            // A value's own items are its encoding, not a use of the state.
            // SAFETY: `node` is a live node of `self`.
            if let Some(items) = unsafe { self.array_items(node) } {
                stack.extend(
                    items
                        .iter()
                        .filter_map(|item| item.node.dynamic())
                        .map(|item| (item, true)),
                );
            }
        }
        Ok(())
    }
}
