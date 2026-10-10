//! The `@loop` conversion: whether a marked self-recursion is convertible, and
//! if it is, **what it converts to** — the recursion's own facts, named over
//! the function's template.
//!
//! # What this module is not
//!
//! It does **not** build a control-flow graph. This graph is a DAG of values —
//! every node is computed once — and a CFG says some of it runs many times,
//! which is a *program* fact: `resolve.rs` states the line, and the program
//! side (a kernel body) belongs to `lichen-kernel-ir` and `lichen-compute`.
//! What lives here is the half that *is* a graph fact: which node is the base
//! test, which node is the step, and what the loop carries. A JIT reads those
//! and builds whatever CFG its own IR wants.
//!
//! The design is `docs/notes/loop-conversion.md`: the marker is permission
//! ([`Function::looping`](crate::Function)), the scope is tail-recursive
//! cycles, and the conversion runs over the **templates** — the only place the
//! recursion is still a cycle, since every apply clones it away.
//!
//! # The shape a convertible recursion has
//!
//! One self-application, in **tail position** per iteration:
//!
//! ```lichen
//! @loop sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
//! ```
//!
//! The body's value is a selection ([`Selection::Computed`]) whose arms are
//! either a **base** (a value that does not recurse) or a **step** (the
//! recursive call itself, its argument the next state), and arms may nest
//! further selections. The carried state is the parameter, flattened to a
//! tuple of scalar cells: reads of it are constant-path peels (`s(0)`), and a
//! step's next state is its argument's elements. That shape is what
//! [`Module::loop_conversion`] checks, and what [`LoopConversion`] records.

use std::collections::HashSet;

use lichen_utils::extend::AsEnum;

use crate::resolve::Selection;
use crate::{AnyNodeId, FunctionId, LowOperator, LowValue, Module, NodeId, Program};

/// Why a marked recursion is not convertible — one variant per shape rule
/// (`docs/notes/loop-conversion.md` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopRefusal {
    /// The function never applies itself — through its own body or a nested
    /// closure's — so there is no cycle to convert. (A `@loop` on a
    /// non-recursive binding is an ordinary function; reaching this from the
    /// conversion entry means a caller asked about one anyway.)
    NotRecursive,
    /// The recursion is a **group**: this function's strongly connected
    /// component in the marked call graph has more than one member, and the
    /// conversion covers self-recursion only. §4's component cap, at one.
    MutualComponent {
        /// The component's size.
        size: usize,
    },
    /// A self-application is not in **tail position**: it sits inside a
    /// computation of the return spine — an operator's operand, a tuple
    /// element, a nested closure — rather than being a branch of it. §4 rule
    /// 1. Curried self-recursion lands here too: its nested lambdas hide the
    /// call from the spine.
    NonTailCall,
    /// Every branch of the return spine is a step — the recursion has no base
    /// case, so the loop would have no exit.
    NoBaseCase,
    /// The carried state is not a flat tuple of scalar cells: the steps
    /// disagree on its arity, a step hands an aggregate or a function, or the
    /// body reads the parameter at a path the flat state does not cover.
    /// §4 rule 2.
    StateShape,
}

impl LoopRefusal {
    /// The refusal's name — the detail a diagnostic carries beside its kind,
    /// so the renderer's wording and the rule cannot drift.
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
/// named as nodes of the function's template. See the module docs for what
/// this deliberately is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopConversion {
    /// The marked function.
    pub function: FunctionId,
    /// The carried state, in slot order: each slot is the path into the
    /// parameter's value that slot is — `[]` is the whole parameter (the
    /// scalar case), `[k]` is element `k` of a tuple state. A consumer maps a
    /// parameter read to its slot through
    /// [`Module::parameter_value_path`] and this list; how the slots become
    /// locals (or anything else) is the consumer's own convention.
    pub state: Vec<Vec<usize>>,
    /// The spine's decisions, the loop's base tests. The first is the
    /// outermost — the one each iteration re-enters.
    pub tests: Vec<LoopTest>,
    /// The steps: the tail self-applications, in spine order.
    pub steps: Vec<LoopStep>,
    /// The exits: the base cases, in spine order.
    pub exits: Vec<LoopExit>,
}

/// One decision of the spine: a selection whose selector is the base test.
/// `1` continues at `on_one`, `0` at `on_zero` (the arms of the underlying
/// `[else, then]` index).
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
    /// The next state, in slot order — the elements of the call's argument.
    /// Each is a computation over the current state (or a state read, for a
    /// slot carried unchanged).
    pub next: Vec<NodeId>,
}

/// One base case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopExit {
    /// The base's **value half** — the branch's result as a value: a scalar
    /// cell, or the array a tuple-valued codomain flattens to.
    ///
    /// **The value half, not the branch's leaves**: a consumer builds the
    /// apply's result pair `[value, type]` from it instead of evaluating the
    /// function's return, whose *untaken* step arm would otherwise stay in the
    /// graph for the next deep pass to walk (and unroll — see `loop_run.rs`).
    pub value: NodeId,
}

impl<P: Program> Module<P> {
    /// Check whether `function`'s `@loop` recursion is convertible, and if it
    /// is, return what it converts to. Read-only over the template: nothing
    /// is evaluated, cloned or rewritten.
    ///
    /// The checks, in the order they fire:
    ///
    /// 1. the marked **component** is this function alone (a mutual recursion
    ///    is refused at the cap);
    /// 2. the return spine's branches are all **base** or **step**, and every
    ///    self-application anywhere in the function's subtree is one of those
    ///    steps (tail position);
    /// 3. there is at least one base;
    /// 4. the carried state is a flat tuple of scalar cells, stable across
    ///    steps and covering every parameter read.
    ///
    /// The function is **stated** to be marked, like the domain a caller
    /// states to `resolve.rs`'s rules: the checker asks only about the
    /// components it found, and the mark is what makes a cycle convertible at
    /// all. Both the parameter and the return are read at the lowlevel's
    /// `[value, type]` pair convention ([`Module::pair_value_half`]) — the
    /// encoding every compiled function carries.
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
        let state = spine.state()?;
        Ok(LoopConversion {
            function,
            state,
            tests: spine.tests,
            steps: spine.steps,
            exits: spine.exits,
        })
    }

    /// The path into `function`'s parameter value that `node` reads, if it is
    /// a parameter read: `[]` for the whole value, `[k]` for element `k` of a
    /// tuple (`s(k)`), collected along a chain of constant-index peels
    /// (`s(0)(1)` is `[0, 1]`). `None` for anything else.
    ///
    /// This is the graph's own read of the chain — constant positions only,
    /// no type walk; a named (`.field`) step ends the match, since the field
    /// list is the encoding's fact, not the graph's.
    pub fn parameter_value_path(&self, function: FunctionId, node: NodeId) -> Option<Vec<usize>> {
        let parameter = self.functions[function].parameter;
        let value_cell = self.pair_value_half(parameter).unwrap_or(parameter);
        // A bare reference to the value cell is the whole-value read.
        if self.class_root(node) == self.class_root(value_cell) {
            return Some(Vec::new());
        }
        let mut path = Vec::new();
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
            if target == parameter && position == 0 {
                // The pair peel — the chain's root. The path collected so far
                // is innermost-first; it reads outermost-first.
                path.reverse();
                return Some(path);
            }
            path.push(position);
            current = target;
        }
        None
    }

    /// The function `node` applies, when `node` is an `Apply` whose callee's
    /// value is a function of this module. The callee sits in the operand
    /// array's first slot; its value may ride the node or its equality class
    /// (the checker substitutes through unification).
    ///
    /// **Public because "which function does this apply call" is the graph's own
    /// question**, and every reader of a recursion asks it: the conversion asks
    /// it per node, and a JIT reader asks it to tell a marked recursion from an
    /// ordinary call before it decides what to emit.
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
            .or_else(|| self.class_value(self.class_root(callee)))?;
        match AsEnum::<LowValue>::as_enum(&value) {
            Some(LowValue::Function(crate::AnyFunctionId::Dynamic(f))) => Some(f),
            _ => None,
        }
    }

    /// The marked component `function` belongs to: the `@loop` functions that
    /// reach it and that it reaches, through the applies in each one's
    /// template subtree (its own body and its nested closures').
    ///
    /// The marked call graph is tiny — one node per `@loop` binding — so the
    /// reach-and-reach-back test is run per member rather than by Tarjan.
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
                    && self.functions[callee].looping
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
/// state and the tail-position rule. The answers accumulate in `tests`,
/// `steps` and `exits`; the walk itself is the tree of [`LoopArm`]s.
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

    /// Walk the return value's spine. The body is one selection tree whose
    /// leaves are the bases and steps; anything else that recurses is not in
    /// tail position.
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
            // No step was classified. **Which refusal that is depends on
            // whether the recursion exists at all**: a self-application the
            // spine does not own — the curried shape, whose call hides inside
            // nested lambdas, or a call in an operand — is the tail-position
            // refusal, and only a function that never applies itself is not a
            // recursion.
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
        let operands = self
            .module
            .operands_of(selection)
            .map_err(|_| LoopRefusal::StateShape)?;
        let (arms, condition) = (operands[0], operands[1]);
        // SAFETY: `arms` is a live node of the module.
        let arms = unsafe { self.module.array_items(arms) }.ok_or(LoopRefusal::StateShape)?;
        if arms.len() != 2 {
            return Err(LoopRefusal::StateShape);
        }
        let arm = |index: usize| match arms[index].node {
            AnyNodeId::Dynamic(node) => Ok(node),
            AnyNodeId::Static(_) => Err(LoopRefusal::StateShape),
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

    /// What one spine node is. Views are resolved first — an arm is the value
    /// it peels to — but the *recorded* value stays the arm's own node, so a
    /// base that reads the state keeps its path.
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

    /// A step's next state: the elements of the recursive call's argument,
    /// each a scalar cell — an aggregate or a function cannot be carried.
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
                .map(|item| match item.node {
                    AnyNodeId::Dynamic(node) => Ok(node),
                    AnyNodeId::Static(_) => Err(LoopRefusal::StateShape),
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
    /// The steps fix the arity (they must agree); the parameter reads fix the
    /// addressing — a whole-value read means the scalar case, an element read
    /// `s(k)` means slot `k`. The two may not mix, and no read may escape the
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
                // A tuple-valued state that is never read by element: the
                // slots exist but nothing names them — refuse rather than
                // guess an addressing.
                return Err(LoopRefusal::StateShape);
            }
            Ok(vec![Vec::new()])
        }
    }

    /// The tail-position rule: every self-application in the function's
    /// subtree — its body and its nested closures' — is one of the spine's
    /// steps. One found anywhere else is the refusal.
    ///
    /// This is also what distinguishes a curried recursion from a
    /// non-recursion: a call inside a nested lambda is a self-application the
    /// spine does not own, and it is found here rather than being read as a
    /// function that never recurses.
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
    /// through operands and array values. The walk stays inside values — a
    /// closure's *body* is not an operand of its value node, so a recursion
    /// hidden in a nested closure is [`Spine::complete`]'s catch, not this
    /// one's.
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

    /// Validate every parameter read reachable from `root` against the state:
    /// record whether element reads happen (`tuple`), and refuse a read the
    /// flat state cannot name. Reads hide inside operand arrays and
    /// materialised values alike, so the walk enters both.
    fn check_reads(
        &self,
        function: FunctionId,
        root: NodeId,
        arity: usize,
        tuple: &mut bool,
    ) -> Result<(), LoopRefusal> {
        let mut seen = HashSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            if let Some(path) = self.parameter_value_path(function, node) {
                match path.as_slice() {
                    [] => {
                        if *tuple || arity != 1 {
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
                stack.extend(operands);
            }
            // SAFETY: `node` is a live node of `self`.
            if let Some(items) = unsafe { self.array_items(node) } {
                stack.extend(items.iter().filter_map(|item| item.node.dynamic()));
            }
        }
        Ok(())
    }
}
