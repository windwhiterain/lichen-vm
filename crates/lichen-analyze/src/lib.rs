//! Programmatic inspection of a compiled lichen program.
//!
//! The debugger of last resort for a graph question that neither a rendered
//! value nor a diagnostic answers: *which node holds what, and through which
//! class*.  A reader here asks the same questions the printers and the backends
//! ask — a node's own value, the class's committed value, the width of its item
//! list, the class it shares — so a conclusion drawn from this crate is a
//! conclusion about the compiled program, not about a second implementation of
//! it.
//!
//! ```no_run
//! use lichen_analyze::Analysis;
//!
//! let mut analysis = Analysis::compile("1 + 1").expect("it checks");
//! let expressions: Vec<_> = analysis.expressions().collect();
//! for expression in expressions {
//!     println!("{}", analysis.describe_expression(expression));
//! }
//! ```
//!
//! Every read goes through the lowlevel's public API, so this crate adds no
//! behaviour to the compiler and changes no graph.

use lichen_highlevel::checker::Build;
use lichen_highlevel::ir::ExprId;
use lichen_language::package::PackageStore;
use lichen_language::program::{LangProgram, LangValue};
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};

/// One node as this crate reports it: the two value axes side by side, which is
/// what separates a node that merely holds no value from one whose value is the
/// undecided marker.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeReport {
    pub node: NodeId,
    /// The node's own slot ([`Module::node_value`]).
    pub own: Option<LangValue>,
    /// The class's committed value ([`Module::class_value`]).
    pub class: Option<LangValue>,
    /// The number of items when the node holds an array, `None` otherwise.
    pub width: Option<usize>,
    /// The node this one was cloned from, when it is an apply's clone.
    pub origin: Option<NodeId>,
    /// The deep pass's verdict for this node: `None` when it never ran.
    pub evaluated_deep: Option<bool>,
    /// Whether this node's own value is the undecided marker.
    pub unbound: bool,
}

/// A compiled program, kept whole so its graph can be walked.
pub struct Analysis {
    build: Build<LangProgram>,
    diagnostics: Vec<String>,
}

impl Analysis {
    /// Compile `source` as the language program the CLI compiles, returning the
    /// compiled graph even when checking reported diagnostics — a graph that
    /// failed to check still has a shape worth reading.
    pub fn compile(source: &str) -> Result<Self, Vec<String>> {
        let mut store = PackageStore::<LangProgram>::new();
        let (preprocessed, _) = lichen_language::preprocess::preprocess(source, None, &mut store);
        let line_starts = lichen_language::lex::line_starts(source);
        let report = lichen_language::compile_with_imports_at::<LangProgram>(
            preprocessed.code,
            &preprocessed.imports,
            Some(store.registry()),
            preprocessed.code_base,
            &line_starts,
            lichen_highlevel::no_native_ops(),
        );
        let diagnostics: Vec<String> = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        match report.build {
            Some(build) => Ok(Self { build, diagnostics }),
            None => Err(diagnostics),
        }
    }

    /// The messages checking produced, in order.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// The compiled graph.
    pub fn module(&mut self) -> &mut Module<LangProgram> {
        &mut self.build.module
    }

    /// The root expression's pair, value and type.
    pub fn root(&self) -> (NodeId, NodeId, NodeId) {
        (
            self.build.root_term,
            self.build.root_val,
            self.build.root_ty,
        )
    }

    /// Evaluate the program, so the graph holds this run's apply clones and not
    /// only the templates the checker built.  Clones are what an apply's
    /// behaviour is a fact about, so a reader that asks about an applied program
    /// asks after this.
    pub fn evaluate(&mut self) {
        let root = self.build.root_val;
        self.build.module.evaluate_node_deep(root, None);
    }

    /// Every expression the checker attributed, in index order.
    pub fn expressions(&self) -> impl Iterator<Item = ExprId> + '_ {
        (0..self.build.state.len() as u32).map(ExprId)
    }

    /// The source-ish form of an expression: its IR kind, abbreviated.
    pub fn kind_of(&self, expression: ExprId) -> String {
        let kind = format!("{:?}", self.build.ir[expression].kind);
        let end = kind.find('{').unwrap_or(kind.len());
        kind[..end].trim().to_string()
    }

    /// An expression's compiled slots: the pair, and its value and type halves.
    pub fn slots(
        &self,
        expression: ExprId,
    ) -> (
        Option<NodeId>,
        Option<NodeId>,
        Option<NodeId>,
        Option<NodeId>,
    ) {
        let state = self.build.state[expression];
        (state.term, state.val, state.ty, state.attr)
    }

    /// One node, with the two value axes and the structural facts.
    pub fn node(&mut self, node: NodeId) -> NodeReport {
        let own = self.build.module.node_value(AnyNodeId::Dynamic(node));
        let class = self.build.module.class_value(node);
        let width = unsafe { self.build.module.array_items(node) }.map(|items| items.len());
        NodeReport {
            node,
            unbound: lichen_lowlevel::is_unbound(own),
            own,
            class,
            width,
            origin: self.build.module.node_origin(node),
            evaluated_deep: self
                .build
                .module
                .node_evaluated_deep(node)
                .map(|deep| deep.parameterized),
        }
    }

    /// The node at `slot` of `node`'s item list, when `node` holds one.
    pub fn slot(&mut self, node: NodeId, slot: usize) -> Option<NodeId> {
        unsafe { self.build.module.array_items(node) }
            .and_then(|items| items.get(slot).map(|item| item.node))
            .and_then(|node| match node {
                AnyNodeId::Dynamic(node) => Some(node),
                AnyNodeId::Static(_) => None,
            })
    }

    /// Every member of `node`'s equality class, in class order.
    pub fn class_members(&mut self, node: NodeId) -> Vec<NodeId> {
        let Some(representative) = self.representative(node) else {
            return Vec::new();
        };
        lichen_utils::disjoint::members(&self.build.module.nodes, representative).collect()
    }

    /// Every node in the graph that was cloned from `template`: one run's clones
    /// of a single template node.
    ///
    /// A question about what an *apply* did is a question about the clones it
    /// made — the template is not written (that is what keeps a function
    /// reusable) — so a reader that asks about the result of an applied program
    /// asks here, and [`NodeReport::origin`] is the inverse map.  Linear in the
    /// node count, so this answers one question once rather than sitting on a
    /// path the compiler takes.
    pub fn clones_of(&mut self, template: NodeId) -> Vec<NodeId> {
        let nodes: Vec<NodeId> = self.build.module.nodes.keys().collect();
        nodes
            .into_iter()
            .filter(|node| self.build.module.node_origin(*node) == Some(template))
            .collect()
    }

    /// The template node `node` was cloned from, resolved to the end of the
    /// origin chain: an apply's clone names its template, and a clone of a clone
    /// names the previous clone, so a reader that wants the source node follows
    /// the chain rather than one step.
    pub fn source_of(&mut self, node: NodeId) -> NodeId {
        let mut current = node;
        for _ in 0..64 {
            match self.build.module.node_origin(current) {
                Some(origin) if origin != current => current = origin,
                _ => break,
            }
        }
        current
    }

    /// The representative of `node`'s equality class.
    pub fn representative(&mut self, node: NodeId) -> Option<NodeId> {
        if !self.build.module.nodes.contains_key(node) {
            return None;
        }
        Some(lichen_utils::disjoint::find(
            &mut self.build.module.nodes,
            node,
        ))
    }

    /// The value a node carries, in one line: its own slot, its class's, and
    /// whether it is undecided.
    pub fn describe(&mut self, node: NodeId) -> String {
        let report = self.node(node);
        let own = short(report.own.as_ref());
        let class = short(report.class.as_ref());
        let deep = match report.evaluated_deep {
            Some(true) => "parameterized",
            Some(false) => "concrete",
            None => "unevaluated",
        };
        format!(
            "{node:?} width={:?} own={own} class={class} deep={deep} unbound={} origin={:?}",
            report.width, report.unbound, report.origin
        )
    }

    /// One expression and its slots, each slot described.
    pub fn describe_expression(&mut self, expression: ExprId) -> String {
        let (term, val, ty, attr) = self.slots(expression);
        let mut out = format!(
            "EXPR {} {}: term={term:?}",
            expression.0,
            self.kind_of(expression)
        );
        for (label, node) in [("val", val), ("ty", ty), ("attr", attr)] {
            if let Some(node) = node {
                out.push_str(&format!("\n    {label:<5} {}", self.describe(node)));
            }
        }
        out
    }

    /// Walk a node's array tree breadth-first, describing at most `limit` nodes,
    /// so a structure's shape can be read without naming every node by hand.
    pub fn dump_tree(&mut self, root: NodeId, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut queue = vec![(root, 0usize)];
        let mut seen = Vec::new();
        while let Some((node, depth)) = queue.pop() {
            if out.len() >= limit || seen.contains(&node) {
                continue;
            }
            seen.push(node);
            out.push(format!("{}{}", "  ".repeat(depth), self.describe(node)));
            if let Some(width) = self.node(node).width {
                for slot in (0..width).rev() {
                    if let Some(child) = self.slot(node, slot) {
                        queue.push((child, depth + 1));
                    }
                }
            }
        }
        out
    }
}

/// A value in a few characters: the marker names a cell that is undecided.
fn short(value: Option<&LangValue>) -> String {
    match value {
        None => "None".to_string(),
        Some(LangValue::TypeValue(kind)) => format!("TypeValue({kind:?})"),
        Some(LangValue::LowValue(LowValue::Parameterized)) => "Parameterized".to_string(),
        Some(LangValue::LowValue(LowValue::Array(_))) => "Array(..)".to_string(),
        Some(LangValue::LowValue(LowValue::Table(_))) => "Table(..)".to_string(),
        Some(other) => format!("{other:?}").chars().take(24).collect(),
    }
}
