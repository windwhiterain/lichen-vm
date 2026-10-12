//! Inspecting a built program: a value by name, and a node's origin.
//! See docs/notes/test-harness.md.

use std::collections::HashMap;

use lichen_highlevel::checker::{Build, Checker};
use lichen_highlevel::diagnostic::{Diag, DiagKind};
use lichen_highlevel::ir::ExprId;
use lichen_highlevel::lowering;
use lichen_highlevel::program::{HighProgram, TypeOperator, ValueType};
use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, NodeId};
use lichen_utils::extend::AsEnum;

use crate::writer::Program;

/// A built program under inspection: its nodes, its values, and the name every
/// expression was written with.
pub struct Probe<P>
where
    P: HighProgram + 'static,
    P::Value: ValueType,
{
    build: Build<P>,
    names: HashMap<ExprId, String>,
}

impl<P> Probe<P>
where
    P: HighProgram + 'static,
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// Check and evaluate `program`, refusing a diagnostic and a runtime refusal
    /// alike — a probe that ran a broken program answers questions about a
    /// program the test never meant to build.
    pub fn build(program: Program<P::Attr, P::Literal>) -> Self {
        let Program { ir, names, .. } = program;
        let build = Checker::<P>::build(ir);
        let mut probe = Probe { build, names };
        probe.finish_run();
        probe
    }

    /// Check `program` **without** refusing a failed build — the refusal is the
    /// fact under test.
    pub fn build_refusing(program: Program<P::Attr, P::Literal>) -> Self {
        let Program { ir, names, .. } = program;
        Probe {
            build: Checker::<P>::build(ir),
            names,
        }
    }

    /// The build itself, for a test that reads a `Build` field directly.
    pub fn checked(&self) -> &Build<P> {
        &self.build
    }

    /// The build mutably, for a test that evaluates a node itself.
    pub fn checked_mut(&mut self) -> &mut Build<P> {
        &mut self.build
    }

    pub fn ok(&self) -> bool {
        self.build.ok
    }

    // --- values, by the name they were written with ------------------------

    /// The expression written under `name`, or `None` when none is.
    ///
    /// # Invariant
    /// Names are **unique per program**: a builder that reuses one is a test
    /// bug, and resolving to one of the two would silently pick an arbitrary
    /// node. [`Self::expr_of`] panics on that case; this answers `None`.
    pub fn expr_named(&self, name: &str) -> Option<ExprId> {
        let mut found = self
            .names
            .iter()
            .filter(|(_, written)| written.as_str() == name)
            .map(|(&expr, _)| expr);
        let first = found.next()?;
        if found.next().is_some() {
            return None;
        }
        Some(first)
    }

    /// The expression written under `name`.
    ///
    /// # Invariant
    /// A name is unique in a program, so this is the one expression it names; a
    /// duplicate is a test bug and the panic says so rather than picking one.
    fn expr_of(&self, name: &str) -> ExprId {
        self.expr_named(name).unwrap_or_else(|| {
            panic!(
                "no expression named {name:?}; the program names {}",
                self.spellings().join(", ")
            )
        })
    }

    /// Every name in the program, sorted — the text of a "no such name" panic.
    pub fn spellings(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.names.values().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// The pair node `[value, type]` the named expression compiled to.
    pub fn pair_of(&self, name: &str) -> NodeId {
        self.expect_slot(name, "pair", |state| state.term)
    }

    /// The **value** node the named expression lowered to.
    pub fn node_of(&self, name: &str) -> NodeId {
        self.expect_slot(name, "value", |state| state.val)
    }

    /// The **type** node the named expression lowered to.
    pub fn type_of(&self, name: &str) -> NodeId {
        self.expect_slot(name, "type", |state| state.ty)
    }

    /// The `usize` the named expression evaluated to.
    pub fn usize_of(&mut self, name: &str) -> usize {
        match self.value_of(name).as_enum() {
            Some(LowValue::USize(value)) => value,
            other => panic!("{name:?} evaluated to {other:?}, not a usize"),
        }
    }

    /// The `f32` the named expression evaluated to.
    pub fn float_of(&mut self, name: &str) -> f32 {
        match self.value_of(name).as_enum() {
            Some(LowValue::Float(value)) => value,
            other => panic!("{name:?} evaluated to {other:?}, not a float"),
        }
    }

    /// The elements of the array the named expression evaluated to, in order.
    pub fn array_of(&mut self, name: &str) -> Vec<P::Value> {
        let value = self.value_of(name);
        let Some(LowValue::Array(array)) = value.as_enum() else {
            panic!("{name:?} evaluated to {value:?}, not an array");
        };
        // SAFETY: the value was produced by the module under test, whose block
        // has not been dropped.
        unsafe { array.items() }
            .iter()
            .map(|item| {
                self.build
                    .module
                    .node_value(item.node)
                    .unwrap_or_else(|| panic!("{name:?} has an element with no value"))
            })
            .collect()
    }

    /// The named expression's evaluated value, forcing it if it is still lazy.
    pub fn value_of(&mut self, name: &str) -> P::Value {
        let node = self.node_of(name);
        self.build
            .module
            .evaluate_node_deep(node, None)
            .unwrap_or_else(|| panic!("{name:?} has no value: the deep pass left it undecided"))
    }

    // --- a node's origin ---------------------------------------------------

    /// The expression `node` lowered from, or `None` when this build attributed
    /// it to nothing.
    pub fn expr_at(&self, node: NodeId) -> Option<ExprId> {
        self.build.lowering_of(node).map(|loc| loc.expr)
    }

    /// The pair slot `node` is: `value`, `type`, `an attribute slot`, `the
    /// expression itself`, or `a position inside the expression`.
    pub fn slot_at(&self, node: NodeId) -> &'static str {
        slot_at(&self.build, node)
    }

    /// Where `node` came from, in the words a test failure should read:
    /// ``node #3 ← `call` (value)``.
    pub fn explain(&self, node: NodeId) -> String {
        match self.build.lowering_of(node) {
            Some(loc) => {
                let name = self
                    .names
                    .get(&loc.expr)
                    .map(String::as_str)
                    .unwrap_or("<unnamed>");
                format!("node {node:?} ← `{name}` ({})", slot_at(&self.build, node))
            }
            None => format!("node {node:?} ← no expression in this build"),
        }
    }

    // --- diagnostics -------------------------------------------------------

    /// The build's diagnostics, rendered with names in place of expression ids.
    pub fn diagnostics(&self) -> Vec<String> {
        self.build
            .diagnostics()
            .into_iter()
            .map(|diag| self.render(&diag))
            .collect()
    }

    /// The kinds of the build's diagnostics, in order — what most tests assert.
    pub fn diagnostic_kinds(&self) -> Vec<DiagKind> {
        self.build
            .diagnostics()
            .into_iter()
            .map(|d| d.kind)
            .collect()
    }

    /// The first diagnostic of `kind`, rendered.
    pub fn diagnostic_of(&self, kind: DiagKind) -> Option<String> {
        self.build
            .diagnostics()
            .into_iter()
            .find(|diag| diag.kind == kind)
            .map(|diag| self.render(&diag))
    }

    fn render(&self, diag: &Diag<P>) -> String {
        let where_ = diag
            .loc()
            .map(|loc| {
                let name = self
                    .names
                    .get(&loc.expr)
                    .map(String::as_str)
                    .unwrap_or("<unnamed>");
                format!("`{name}`")
            })
            .unwrap_or_else(|| "nowhere".to_string());
        format!("{:?} at {where_}", diag.kind)
    }

    // --- internals ---------------------------------------------------------

    fn expect_slot(
        &self,
        name: &str,
        what: &str,
        read: impl Fn(&lichen_highlevel::checker::ExprState) -> Option<NodeId>,
    ) -> NodeId {
        let expr = self.expr_of(name);
        read(&self.build.state[expr]).unwrap_or_else(|| {
            panic!(
                "`{name}` has no compiled {what}; a `@gc` scope released it, or the check stopped short"
            )
        })
    }

    /// The root pass a probe runs before any question is asked of it.
    fn finish_run(&mut self) {
        let root_val = self.build.root_val;
        let root_ty = self.build.root_ty;
        let value = self.build.module.evaluate_node_deep(root_val, None);
        let _ = self.build.module.evaluate_node_deep(root_ty, None);
        let produced_nothing = match &value {
            None => true,
            Some(value) => matches!(value.as_enum(), Some(LowValue::Error)),
        };
        assert!(
            !(produced_nothing && !self.build.module.extension_diagnostics.is_empty()),
            "a runtime refusal was recorded: {:?}",
            self.build.module.extension_diagnostics
        );
    }
}

/// The slot `node` is, as the phrase an explanation reads.
fn slot_at<P>(build: &Build<P>, node: NodeId) -> &'static str
where
    P: HighProgram,
    P::Value: ValueType,
{
    let Some(loc) = build.lowering_of(node) else {
        return "no slot in this build";
    };
    match lowering::slot_of_path(&loc.path) {
        Some(0) => "value",
        Some(1) => "type",
        Some(_) => "an attribute slot",
        None if loc.path.is_empty() => "the expression itself",
        None => "a position inside the expression",
    }
}

/// The dynamic node behind a value read — the checker builds only dynamic
/// graphs, so this is the one conversion a probe needs.
pub fn dyn_node(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => unreachable!("checker graphs are dynamic"),
    }
}
