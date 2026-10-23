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
    /// A consumer builds the apply's result pair `[value, type]` from this
    /// node, never by evaluating the function's return: the *untaken* step
    /// arm would otherwise stay in the graph for the next deep pass to walk
    /// (and unroll — see `loop_run.rs`).
    pub value: NodeId,
}

impl<P: Program> Module<P> {
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

    /// The path into `function`'s parameter value that `node` reads: `[]` the
    /// whole value, `[k]` element `k` of a tuple.
    ///
    /// # Invariant
    /// Read from the **value side**: the chain alternates element positions with the
    /// slot's value position (`0`), so the pair peel and every slot peel are dropped
    /// and a pair's type or attribute position ends the match rather than naming a
    /// state element (`docs/notes/compound-element-slots.md`).
    ///
    /// **The position is read through [`Module::read_position`], so a frozen one
    /// answers.** The four order comparisons are routed through the prelude's
    /// bindings, and the residual that routing leaves behind holds the read's
    /// subscript as a reference into the **frozen** module — a decided node cannot
    /// carry the caller's (`static_module/apply.rs`, `static_remap_value`). A
    /// position is a position whichever module names it, and `read_position` already
    /// reads both; only the walk's **target** has to be this module's, so `operands_of`
    /// is kept for that alone. Reading the position through `operands_of` instead
    /// refused every routed comparison's state read, and a read that cannot be walked
    /// is a read the loop cannot place.
    ///
    /// **A clone of the read answers as the template's read does.** The walk above
    /// terminates on `class_root(current) == value_class`, and a clone's chain bottoms
    /// out at *this instantiation's* parameter — a fresh node in a singleton class —
    /// so the class test never matches there. The origin is therefore followed once
    /// when the walk meets a node with no operation, and the walk resumes from the
    /// template the clone instantiates. A clone of a state read reads that slot, so it
    /// must answer the same path; a clone that bottoms out somewhere else, or an
    /// origin that is not a read, still answers [`None`].
    ///
    /// **The two are one reading.** A routed comparison's read is frozen *and* a
    /// clone: reading the position alone leaves the walk at the clone's parameter,
    /// and following the origin alone leaves it at the frozen position. Either half
    /// alone answers [`None`] for every routed comparison, which is how the doubling
    /// survived the first of them.
    pub fn parameter_value_path(&self, function: FunctionId, node: NodeId) -> Option<Vec<usize>> {
        // The mark's slot matches a state read structurally
        // (`docs/notes/loop-conversion.md`).
        if let Some(slot) = self.functions[function].looping
            && let Some(path) = self.marked_state_path(function, node, slot)
        {
            return Some(path);
        }
        let parameter = self.functions[function].parameter;
        let value_cell = self.pair_value_half(parameter).unwrap_or(parameter);
        let value_class = self.class_root(value_cell);
        let mut path = Vec::new();
        let mut current = node;
        // **One origin hop**, so a chain that reaches a clone's own parameter resumes
        // at the template rather than walking the whole origin chain.
        let mut followed = false;
        for _ in 0..16 {
            if self.class_root(current) == value_class {
                // The chain reached the state's value: what was collected reads
                // outermost-first.
                return Self::state_path(path);
            }
            let Some(operation) = self.node_operation(current) else {
                // **The walk bottomed out**: a clone's chain ends at this
                // instantiation's parameter, which is a value node the class test
                // above never matches. The origin is the node the clone
                // instantiates, so the walk continues where the template's read is.
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
                // The argument **pair**'s value position is the state itself.
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
        let operation = self.node_operation(node)?;
        if !matches!(
            AsEnum::<LowOperator>::as_enum(&operation.operator),
            Some(LowOperator::Apply)
        ) {
            return None;
        }
        let callee = *self.operands_of(node).ok()?.first()?;
        let value = self
            .node_value(AnyNodeId::Dynamic(callee))
            .or_else(|| self.nodes[self.class_root(callee)].value)?;
        match AsEnum::<LowValue>::as_enum(&value) {
            Some(LowValue::Function(crate::AnyFunctionId::Dynamic(f))) => Some(f),
            _ => None,
        }
    }

    /// The marked component `function` belongs to: the `@loop` functions
    /// that reach it and reach back.
    fn loop_component(&self, function: FunctionId) -> Vec<FunctionId> {
        let reachable = |from: FunctionId| {
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
        let forward = reachable(function);
        forward
            .into_iter()
            .filter(|&g| reachable(g).contains(&function))
            .collect()
    }

    /// The marked functions `g`'s template subtree applies to — the edges of
    /// the marked call graph.
    fn marked_callees_of(&self, g: FunctionId) -> Vec<FunctionId> {
        let mut out = Vec::new();
        for h in self.functions.keys() {
            // `h` contributes when it is `g` itself or a closure nested in it.
            let mut owner = Some(h);
            let mut nested = false;
            while let Some(current) = owner {
                if current == g {
                    nested = true;
                    break;
                }
                owner = self.functions[current].parent;
            }
            if !nested {
                continue;
            }
            for &node in &self.functions[h].nodes {
                if let Some(callee) = self.callee_function(node)
                    && self.functions[callee].looping.is_some()
                    && !out.contains(&callee)
                {
                    out.push(callee);
                }
            }
        }
        out
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
        let ret = self.module.functions[self.function].r#return;
        let value = self.module.pair_value_half(ret).unwrap_or(ret);
        let roots = self
            .module
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
        while seen.insert(selection) {
            match self.module.selection_of(selection) {
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
        let mut resolved = arm;
        let mut seen = HashSet::new();
        while seen.insert(resolved) {
            match self.module.selection_of(resolved) {
                Some(Selection::Views(view)) => resolved = view,
                Some(Selection::Computed) => return Ok(Classified::Test(resolved)),
                None => break,
            }
        }
        if self.module.callee_function(resolved) == Some(self.function) {
            return Ok(Classified::Step(resolved));
        }
        if self.module.subtree_applies(resolved, self.function) {
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
        let value = self.module.pair_value_half(argument).unwrap_or(argument);
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
            ) {
                return Err(LoopRefusal::StateShape);
            }
        }
        let index = self.steps.len();
        self.steps.push(LoopStep { apply, next });
        Ok(index)
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
